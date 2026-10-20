//! Starting the program when the user logs in. What is written is the same idea on both systems
//! (an entry that runs this program with `--background`, so that it goes to the tray and shows no
//! window), and it is behind `Autostart`: on Windows a value of the `Run` key of the current user,
//! on Linux a `.desktop` file in the `autostart` folder of the user. Whether it is on is read from
//! the entry itself, not from the setting. Where the entry is and what it says is decided here, and
//! the system itself (the registry, the folder) is the smallest part, so the same tests run
//! everywhere.

use crate::core::error::CoreError;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

/// The argument that tells the program it was started by the login: no window, only the tray.
pub const BACKGROUND_FLAG: &str = "--background";

/// What a start of the program asks for about its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// The main window is shown.
    Window,
    /// Only the tray icon; the window comes when the user asks for it.
    Background,
}

/// The mode the arguments of a start ask for (the first is the program itself): exactly
/// `--background` and nothing else is the background; anything else is a start with a window.
pub fn start_mode(args: &[String]) -> StartMode {
    match args {
        [_program, flag] if flag == BACKGROUND_FLAG => StartMode::Background,
        _ => StartMode::Window,
    }
}

/// An entry that starts the program at the login.
pub trait Autostart {
    /// Writes the entry for the program at `exe`, replacing an older one.
    fn enable(&mut self, exe: &Path) -> Result<(), CoreError>;
    /// Removes the entry; one that is not there is not an error.
    fn disable(&mut self) -> Result<(), CoreError>;
    /// Whether the entry is there, exactly as `enable` writes it for the program at `exe`.
    fn is_enabled(&self, exe: &Path) -> bool;
}

/// Brings the entry to what is wanted: written if it is wanted and not there as it should be,
/// removed if it is not wanted. An entry that is already right is not written again.
pub fn apply(autostart: &mut impl Autostart, wanted: bool, exe: &Path) -> Result<(), CoreError> {
    if !wanted {
        // Also when the entry is for another place: it is the entry of this program all the same.
        return autostart.disable();
    }
    if autostart.is_enabled(exe) { Ok(()) } else { autostart.enable(exe) }
}

fn failed(error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Autostart(error.to_string())
}

// ------------------------------------------------------------------ Windows: the Run key

/// The key below the current user's classes root that Windows runs at the login.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The name of the value of this program in that key.
pub const RUN_VALUE: &str = "scrcpy-agent";

/// The command line the value holds: the program (quoted) and the flag.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {BACKGROUND_FLAG}", exe.display())
}

/// The values of the registry that the entry needs. Not the whole key: other programs have
/// values in the `Run` key, which are never touched.
pub trait RunRegistry {
    fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()>;
    fn get(&self, key: &str, name: &str) -> Option<String>;
    /// Removes one value; one that is not there is not an error.
    fn remove(&mut self, key: &str, name: &str) -> io::Result<()>;
}

/// The entry as a value of the `Run` key.
pub struct RunKey<R: RunRegistry>(pub R);

impl<R: RunRegistry> Autostart for RunKey<R> {
    fn enable(&mut self, exe: &Path) -> Result<(), CoreError> {
        self.0.set(RUN_KEY, RUN_VALUE, &run_command(exe)).map_err(|e| failed(&e))
    }

    fn disable(&mut self) -> Result<(), CoreError> {
        self.0.remove(RUN_KEY, RUN_VALUE).map_err(|e| failed(&e))
    }

    fn is_enabled(&self, exe: &Path) -> bool {
        self.0.get(RUN_KEY, RUN_VALUE).as_deref() == Some(run_command(exe).as_str())
    }
}

// ------------------------------------------------------------------ Linux: the autostart folder

/// The name of the file of this program in the `autostart` folder.
pub const FILE_NAME: &str = "scrcpy-agent.desktop";

/// The `autostart` folder of the user: below `XDG_CONFIG_HOME` if that is set to a path from the
/// root, else below `.config` in the home folder (which also has to be one). A value that is empty
/// or relative is not a place, and is as if it was not set.
pub fn autostart_folder(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    // "From the root": a path that starts at the root of the file system (on Linux, with a `/`).
    let rooted = |value: &&OsStr| Path::new(value).has_root();
    let config = match xdg_config_home.filter(rooted) {
        Some(xdg) => PathBuf::from(xdg),
        None => Path::new(home.filter(rooted)?).join(".config"),
    };
    Some(config.join("autostart"))
}

/// The text of the file for the program at `exe`. A path that cannot be written in the `Exec`
/// key is an error, not a file that would start something else.
pub fn desktop_text(exe: &Path) -> Result<String, CoreError> {
    let exec = super::desktop_entry::exec_line(exe, &[BACKGROUND_FLAG.to_string()])
        .ok_or_else(|| CoreError::Autostart(format!("a command that cannot be written: {exe:?}")))?;
    Ok(format!(
        "{}\n[Desktop Entry]\nType=Application\nName={}\nExec={exec}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        super::file_manager_menu::MARKER,
        crate::core::app_name::DEFAULT_APP_NAME,
    ))
}

/// The entry as a file in a folder (made if it is not there).
pub struct AutostartFile {
    pub folder: PathBuf,
}

impl Autostart for AutostartFile {
    fn enable(&mut self, exe: &Path) -> Result<(), CoreError> {
        let text = desktop_text(exe)?;
        std::fs::create_dir_all(&self.folder).map_err(|e| failed(&e))?;
        // A desktop that watches the folder must never see half a file: it is written whole under
        // another name and renamed, which also replaces an older one in one step.
        let part = self.folder.join(format!(".{FILE_NAME}.part"));
        let written = std::fs::write(&part, text).and_then(|()| std::fs::rename(&part, self.folder.join(FILE_NAME)));
        if written.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        written.map_err(|e| failed(&e))
    }

    fn disable(&mut self) -> Result<(), CoreError> {
        match std::fs::remove_file(self.folder.join(FILE_NAME)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(failed(&e)),
            _ => Ok(()),
        }
    }

    fn is_enabled(&self, exe: &Path) -> bool {
        let Ok(wanted) = desktop_text(exe) else { return false };
        std::fs::read_to_string(self.folder.join(FILE_NAME)).is_ok_and(|text| text == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // The format of both entries (a `Run` value is a command line; an autostart file is a Desktop
    // Entry whose `Exec` is written as in `desktop_entry`) is confirmed against the system's own
    // documentation; every path and name here is made up.

    /// A registry in memory, with the value of another program in the `Run` key.
    #[derive(Default)]
    struct FakeRegistry {
        values: BTreeMap<(String, String), String>,
        refuse: bool,
    }

    impl RunRegistry for FakeRegistry {
        fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
            if self.refuse {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.values.insert((key.into(), name.into()), value.into());
            Ok(())
        }
        fn get(&self, key: &str, name: &str) -> Option<String> {
            self.values.get(&(key.to_string(), name.to_string())).cloned()
        }
        fn remove(&mut self, key: &str, name: &str) -> io::Result<()> {
            if self.refuse {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.values.remove(&(key.to_string(), name.to_string()));
            Ok(())
        }
    }

    fn other_programs_value() -> ((String, String), String) {
        ((RUN_KEY.to_string(), "other-program".to_string()), r#""X:\other\other.exe""#.to_string())
    }

    fn registry_with_another_program() -> RunKey<FakeRegistry> {
        let mut registry = FakeRegistry::default();
        let (key, value) = other_programs_value();
        registry.values.insert(key, value);
        RunKey(registry)
    }

    /// A folder of its own in the temporary folder, gone when the test ends.
    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(name: &str) -> TempFolder {
            let path = std::env::temp_dir().join(format!("scrcpy-agent-autostart-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            TempFolder(path)
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn exe() -> &'static Path {
        Path::new("/opt/my apps/scrcpy-agent")
    }

    fn moved() -> &'static Path {
        Path::new("/srv/other place/scrcpy-agent")
    }

    /// What every kind of entry has to do, whatever it is written to.
    fn the_contract(autostart: &mut impl Autostart) {
        assert!(!autostart.is_enabled(exe()), "nothing is written yet");
        autostart.enable(exe()).unwrap();
        assert!(autostart.is_enabled(exe()));
        // An entry for the program somewhere else is not this program's entry.
        assert!(!autostart.is_enabled(moved()));
        // Writing again for another place replaces the entry.
        autostart.enable(moved()).unwrap();
        assert!(autostart.is_enabled(moved()));
        assert!(!autostart.is_enabled(exe()));
        autostart.disable().unwrap();
        assert!(!autostart.is_enabled(moved()));
        // Removing what is not there is not an error.
        autostart.disable().unwrap();
    }

    #[test]
    fn only_exactly_the_background_flag_is_a_start_in_the_tray() {
        assert_eq!(start_mode(&args(&["scrcpy-agent", "--background"])), StartMode::Background);
        for other in [
            vec![],
            vec!["scrcpy-agent"],
            vec!["scrcpy-agent", "--background", "extra"],
            vec!["scrcpy-agent", "--Background"],
            vec!["scrcpy-agent", "background"],
            vec!["scrcpy-agent", "--background=1"],
            vec!["scrcpy-agent", "--shell", "send-default", "--background"],
            vec!["scrcpy-agent", "--app", "device", "--background"],
        ] {
            assert_eq!(start_mode(&args(&other)), StartMode::Window, "{other:?}");
        }
    }

    #[test]
    fn the_run_value_is_the_quoted_program_and_the_flag() {
        assert_eq!(run_command(Path::new(r"X:\my apps\scrcpy-agent.exe")), r#""X:\my apps\scrcpy-agent.exe" --background"#);
    }

    #[test]
    fn the_run_key_follows_the_contract_of_an_entry() {
        the_contract(&mut registry_with_another_program());
    }

    #[test]
    fn the_run_key_gets_one_value_of_its_own_and_the_values_of_other_programs_stay() {
        let mut autostart = registry_with_another_program();
        autostart.enable(Path::new(r"X:\app\scrcpy-agent.exe")).unwrap();
        let (other_key, other_value) = other_programs_value();
        assert_eq!(autostart.0.values.len(), 2);
        assert_eq!(autostart.0.values.get(&other_key), Some(&other_value));
        assert_eq!(
            autostart.0.get(RUN_KEY, RUN_VALUE).as_deref(),
            Some(r#""X:\app\scrcpy-agent.exe" --background"#)
        );
        autostart.disable().unwrap();
        assert_eq!(autostart.0.values.len(), 1);
        assert_eq!(autostart.0.values.get(&other_key), Some(&other_value));
    }

    #[test]
    fn a_value_that_was_changed_by_hand_is_not_taken_for_the_entry() {
        let mut autostart = registry_with_another_program();
        autostart.enable(exe()).unwrap();
        assert!(autostart.is_enabled(exe()));
        autostart.0.set(RUN_KEY, RUN_VALUE, r#""/opt/my apps/scrcpy-agent""#).unwrap();
        assert!(!autostart.is_enabled(exe()), "without the flag it would open the window at the login");
    }

    #[test]
    fn a_registry_that_refuses_is_an_error_and_the_entry_is_not_taken_for_written() {
        let mut autostart = RunKey(FakeRegistry { refuse: true, ..FakeRegistry::default() });
        assert!(matches!(autostart.enable(exe()), Err(CoreError::Autostart(_))));
        assert!(!autostart.is_enabled(exe()));
        assert!(matches!(autostart.disable(), Err(CoreError::Autostart(_))));
    }

    #[test]
    fn the_folder_is_below_the_config_folder_of_the_user() {
        let some = |s: &str| Some(std::ffi::OsString::from(s));
        let folder = |xdg: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>| {
            autostart_folder(xdg.as_deref(), home.as_deref())
        };
        assert_eq!(folder(some("/xdg/config"), some("/home/user")), Some(PathBuf::from("/xdg/config/autostart")));
        assert_eq!(folder(None, some("/home/user")), Some(PathBuf::from("/home/user/.config/autostart")));
        // Empty or relative is not a place.
        assert_eq!(folder(some(""), some("/home/user")), Some(PathBuf::from("/home/user/.config/autostart")));
        assert_eq!(folder(some("relative/config"), some("/home/user")), Some(PathBuf::from("/home/user/.config/autostart")));
        assert_eq!(folder(None, some("relative/home")), None);
        assert_eq!(folder(None, None), None);
    }

    #[test]
    fn the_file_is_a_desktop_entry_that_runs_the_program_with_the_flag() {
        assert_eq!(
            desktop_text(exe()).unwrap(),
            format!(
                "{}\n[Desktop Entry]\nType=Application\nName=ScrcpyAgent\nExec=\"/opt/my apps/scrcpy-agent\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
                crate::core::integration::file_manager_menu::MARKER
            )
        );
        // A path that cannot be written in the file is an error, not a broken file.
        assert!(matches!(desktop_text(Path::new("/opt/bad\nname/scrcpy-agent")), Err(CoreError::Autostart(_))));
    }

    #[test]
    fn the_file_follows_the_contract_of_an_entry() {
        let temp = TempFolder::new("contract");
        the_contract(&mut AutostartFile { folder: temp.0.join("not").join("made").join("yet") });
    }

    #[test]
    fn the_file_is_one_file_of_its_own_and_other_files_of_the_folder_stay() {
        let temp = TempFolder::new("own-file");
        std::fs::create_dir_all(&temp.0).unwrap();
        std::fs::write(temp.0.join("other.desktop"), "keep me").unwrap();
        let mut autostart = AutostartFile { folder: temp.0.clone() };
        autostart.enable(exe()).unwrap();
        assert_eq!(std::fs::read_to_string(temp.0.join(FILE_NAME)).ok(), Some(desktop_text(exe()).unwrap()));
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2, "no half-written file is left");
        autostart.disable().unwrap();
        assert!(!temp.0.join(FILE_NAME).exists());
        assert_eq!(std::fs::read_to_string(temp.0.join("other.desktop")).unwrap(), "keep me");
    }

    #[test]
    fn a_file_that_was_changed_by_hand_is_not_taken_for_the_entry() {
        let temp = TempFolder::new("changed");
        let mut autostart = AutostartFile { folder: temp.0.clone() };
        autostart.enable(exe()).unwrap();
        assert!(autostart.is_enabled(exe()));
        std::fs::create_dir_all(&temp.0).unwrap();
        std::fs::write(temp.0.join(FILE_NAME), "[Desktop Entry]\nType=Application\nName=x\nExec=/opt/my apps/scrcpy-agent\n").unwrap();
        assert!(!autostart.is_enabled(exe()));
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error() {
        let temp = TempFolder::new("blocked");
        std::fs::write(&temp.0, "a file where the folder should be").unwrap();
        let mut autostart = AutostartFile { folder: temp.0.join("autostart") };
        assert!(matches!(autostart.enable(exe()), Err(CoreError::Autostart(_))));
        assert!(!autostart.is_enabled(exe()));
    }

    /// An entry that counts what is done to it.
    #[derive(Default)]
    struct Counting {
        written_for: Option<PathBuf>,
        writes: u32,
        removals: u32,
    }

    impl Autostart for Counting {
        fn enable(&mut self, exe: &Path) -> Result<(), CoreError> {
            self.written_for = Some(exe.to_path_buf());
            self.writes += 1;
            Ok(())
        }
        fn disable(&mut self) -> Result<(), CoreError> {
            self.written_for = None;
            self.removals += 1;
            Ok(())
        }
        fn is_enabled(&self, exe: &Path) -> bool {
            self.written_for.as_deref() == Some(exe)
        }
    }

    #[test]
    fn applying_writes_what_is_missing_or_for_another_place_and_leaves_what_is_right() {
        let mut entry = Counting::default();
        apply(&mut entry, true, exe()).unwrap();
        assert_eq!((entry.writes, entry.is_enabled(exe())), (1, true));
        // Right already: not written again.
        apply(&mut entry, true, exe()).unwrap();
        assert_eq!(entry.writes, 1);
        // The program was moved: the entry follows it.
        apply(&mut entry, true, moved()).unwrap();
        assert_eq!((entry.writes, entry.is_enabled(moved())), (2, true));
        // Not wanted: removed.
        apply(&mut entry, false, moved()).unwrap();
        assert_eq!((entry.removals, entry.is_enabled(moved())), (1, false));
    }
}
