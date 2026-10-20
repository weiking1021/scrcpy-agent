//! The right-click menu for files in the file managers of Linux desktops, the
//! counterpart of `explorer_menu`: the same four entries, each starting the
//! program with `--shell <action> <files>`. A file manager is told by files in
//! the user's data folder, and each has its own kind: a service menu for Dolphin
//! (KDE), actions for Nemo (Cinnamon), scripts for Nautilus (GNOME). What is
//! written is a pure function of the program, the texts and the manager, so it is
//! the same on every system and tested on all of them; only the folder it is
//! written below (the data folder) and the executable bit are the system's.
//!
//! Every file starts with `MARKER`: that is how the files of this program are told
//! from the user's own when the menu is replaced or removed.

use crate::core::error::CoreError;
use super::explorer_menu::MenuTexts;
use crate::core::requests::shell_request::ShellAction;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The line every file written here has: what these files are, for whoever opens one.
pub const MARKER: &str = "# Written by scrcpy-agent; changes are overwritten.";

/// A file manager that has a menu this program can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileManager {
    /// KDE.
    Dolphin,
    /// Cinnamon.
    Nemo,
    /// GNOME.
    Nautilus,
}

impl FileManager {
    pub const ALL: [FileManager; 3] = [FileManager::Dolphin, FileManager::Nemo, FileManager::Nautilus];

    /// The program that is the file manager: its presence is what tells it is installed.
    pub fn program(self) -> &'static str {
        match self {
            FileManager::Dolphin => "dolphin",
            FileManager::Nemo => "nemo",
            FileManager::Nautilus => "nautilus",
        }
    }

    /// The folder below the data folder (`~/.local/share`) whose files are its menu.
    pub fn folder(self) -> &'static str {
        match self {
            FileManager::Dolphin => "kio/servicemenus",
            FileManager::Nemo => "nemo/actions",
            FileManager::Nautilus => "nautilus/scripts",
        }
    }
}

/// One file of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuFile {
    /// Below the data folder.
    pub path: PathBuf,
    pub content: String,
    /// Dolphin and Nautilus only use a file that can be run.
    pub executable: bool,
}

fn refuse(what: impl std::fmt::Display) -> CoreError {
    CoreError::ExplorerMenu(what.to_string())
}

/// The four entries, in the order of the menu, with their texts.
fn entries(texts: &MenuTexts) -> [(ShellAction, &str); 4] {
    [
        (ShellAction::SendDefault, texts.send_default.as_str()),
        (ShellAction::SendChoose, texts.send_choose.as_str()),
        (ShellAction::OpenDefault, texts.open_default.as_str()),
        (ShellAction::OpenChoose, texts.open_choose.as_str()),
    ]
}

/// A text that becomes the name of an entry, and in two systems the name of a
/// file: not empty, no control character (a line break would end the value), no `/`.
fn check_text(text: &str) -> Result<&str, CoreError> {
    if text.trim().is_empty() || text.chars().any(|c| c.is_control() || c == '/') {
        return Err(refuse(format!("not usable as the name of a menu entry: {text:?}")));
    }
    Ok(text)
}

/// The files that make the menu of `manager` for the program at `exe`.
pub fn files(manager: FileManager, exe: &Path, texts: &MenuTexts) -> Result<Vec<MenuFile>, CoreError> {
    let title = check_text(&texts.title)?;
    for (_, text) in entries(texts) {
        check_text(text)?;
    }
    let program = exec_word(exe).ok_or_else(|| refuse(format!("the program cannot be written in a menu: {exe:?}")))?;
    Ok(match manager {
        FileManager::Dolphin => vec![dolphin(title, &program, texts)],
        FileManager::Nemo => nemo(title, &program, texts),
        FileManager::Nautilus => nautilus(title, exe, texts)?,
    })
}

fn dolphin(title: &str, program: &str, texts: &MenuTexts) -> MenuFile {
    let words: String = entries(texts).iter().map(|(action, _)| format!("{};", action.word())).collect();
    let mut content = format!(
        "{MARKER}\n[Desktop Entry]\nType=Service\nMimeType=application/octet-stream;\nActions={words}\nX-KDE-Submenu={title}\nIcon=scrcpy-agent\n"
    );
    for (action, text) in entries(texts) {
        let word = action.word();
        content.push_str(&format!("\n[Desktop Action {word}]\nName={text}\nExec={program} --shell {word} %F\n"));
    }
    MenuFile { path: PathBuf::from(FileManager::Dolphin.folder()).join("scrcpy-agent.desktop"), content, executable: true }
}

fn nemo(title: &str, program: &str, texts: &MenuTexts) -> Vec<MenuFile> {
    entries(texts)
        .iter()
        .map(|(action, text)| {
            let word = action.word();
            // Nemo has no submenus: the title is part of each name. `Quote=double` puts each
            // path in quotes, so a name with a space is one argument.
            let content = format!(
                "{MARKER}\n[Nemo Action]\nName={title}: {text}\nExec={program} --shell {word} %F\nIcon-Name=scrcpy-agent\nSelection=notnone\nExtensions=nodirs;\nQuote=double\n"
            );
            MenuFile {
                path: PathBuf::from(FileManager::Nemo.folder()).join(format!("scrcpy-agent-{word}.nemo_action")),
                content,
                executable: false,
            }
        })
        .collect()
}

fn nautilus(title: &str, exe: &Path, texts: &MenuTexts) -> Result<Vec<MenuFile>, CoreError> {
    let program = shell_word(exe.to_str().ok_or_else(|| refuse("the program path is not text"))?);
    Ok(entries(texts)
        .iter()
        .map(|(action, text)| {
            // Nautilus passes the selected paths in one variable, one per line. Globbing is off
            // (a name with `*` stays a name) and only a line break separates the paths.
            let content = format!(
                "#!/bin/sh\n{MARKER}\nset -f\nIFS='\n'\nset -- $NAUTILUS_SCRIPT_SELECTED_FILE_PATHS\n[ \"$#\" -gt 0 ] || exit 0\nexec {program} --shell {} \"$@\"\n",
                action.word()
            );
            MenuFile {
                // The name of the file is the name of the entry in the menu. (No colon: on
                // Windows it would make a data stream of another file instead of a file, and
                // the same files are written by the tests there.)
                path: PathBuf::from(FileManager::Nautilus.folder()).join(format!("{title} - {text}")),
                content,
                executable: true,
            }
        })
        .collect())
}

/// A word of a shell script: in single quotes, where a single quote is closed,
/// written escaped, and opened again.
fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A program path as a word of the `Exec` key of a desktop entry (quoted when it
/// has a character the format reserves, and written the way the file needs it,
/// which doubles the backslashes); `None` for a path that cannot be written.
pub fn exec_word(exe: &Path) -> Option<String> {
    super::desktop_entry::exec_arg(exe.to_str()?)
}

fn folder_of(data_home: &Path, manager: FileManager) -> PathBuf {
    data_home.join(manager.folder())
}

/// The files in the folder of `manager` that this program wrote: those that have the marker line.
fn owned(data_home: &Path, manager: FileManager) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder_of(data_home, manager)) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|path| path.is_file())
        // A file that is not text is not ours.
        .filter(|path| std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| line == MARKER)))
        .collect();
    found.sort();
    found
}

/// Writes the menu of `manager` below `data_home`, replacing what this program
/// wrote there before (another language, another place of the program) and
/// leaving every other file alone.
pub fn install(data_home: &Path, manager: FileManager, exe: &Path, texts: &MenuTexts) -> Result<(), CoreError> {
    // Made before anything is removed: texts that cannot be written leave the old menu as it was.
    let wanted = files(manager, exe, texts)?;
    uninstall(data_home, manager)?;
    for file in wanted {
        let path = data_home.join(&file.path);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(refuse)?;
        }
        std::fs::write(&path, &file.content).map_err(refuse)?;
        set_executable(&path, file.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(refuse)
}

/// Where there is no executable bit there is nothing to set.
#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<(), CoreError> {
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Removes what this program wrote for `manager`; there being nothing is not an error.
pub fn uninstall(data_home: &Path, manager: FileManager) -> Result<(), CoreError> {
    for path in owned(data_home, manager) {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(refuse(e)),
            _ => {}
        }
    }
    Ok(())
}

/// Whether the menu is there for this program with these texts, exactly: the same
/// files with the same content, and none left of an older menu.
pub fn is_installed(data_home: &Path, manager: FileManager, exe: &Path, texts: &MenuTexts) -> bool {
    let Ok(wanted) = files(manager, exe, texts) else { return false };
    let mut paths: Vec<PathBuf> = wanted.iter().map(|f| data_home.join(&f.path)).collect();
    paths.sort();
    owned(data_home, manager) == paths
        && wanted.iter().all(|f| {
            let path = data_home.join(&f.path);
            std::fs::read_to_string(&path).is_ok_and(|text| text == f.content) && (!f.executable || is_executable(&path))
        })
}

/// The data folder of the user: `XDG_DATA_HOME` if it is set to a path from the
/// root, else `.local/share` in the home folder (which also has to be one). A
/// value that is empty or relative is not a place, and is as if it was not set.
pub fn data_home(xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    // "From the root": a path that starts at the root of the file system (on Linux, with a `/`).
    let rooted = |value: &&OsStr| Path::new(value).has_root();
    if let Some(xdg) = xdg_data_home.filter(rooted) {
        return Some(PathBuf::from(xdg));
    }
    home.filter(rooted).map(|home| Path::new(home).join(".local").join("share"))
}

/// What is done for a file manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Install,
    Remove,
}

/// What to do for every file manager, in the order of `FileManager::ALL`: the menu
/// is written for those that are found and not turned off, while the menu is
/// wanted at all; everywhere else it is removed, so a file manager that was
/// uninstalled, or turned off, or a menu that is not wanted any more leaves nothing.
pub fn plan(enabled: bool, disabled: &[FileManager], detected: &[FileManager]) -> Vec<(FileManager, Change)> {
    FileManager::ALL
        .into_iter()
        .map(|manager| {
            let wanted = enabled && detected.contains(&manager) && !disabled.contains(&manager);
            (manager, if wanted { Change::Install } else { Change::Remove })
        })
        .collect()
}

/// Carries out a plan. Every change is tried, one that fails does not keep the
/// others from being made; the first error is what is returned.
pub fn apply(data_home: &Path, plan: &[(FileManager, Change)], exe: &Path, texts: &MenuTexts) -> Result<(), CoreError> {
    let mut first = None;
    for &(manager, change) in plan {
        let result = match change {
            Change::Install => install(data_home, manager, exe, texts),
            Change::Remove => uninstall(data_home, manager),
        };
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

/// The file managers whose program `has_program` finds.
pub fn detect(has_program: impl Fn(&str) -> bool) -> Vec<FileManager> {
    FileManager::ALL.into_iter().filter(|m| has_program(m.program())).collect()
}

/// Whether `program` is a file that can be run in one of the folders of `path`
/// (the value of `PATH`).
pub fn on_path(program: &str, path: &OsStr) -> bool {
    std::env::split_paths(path)
        // An empty entry is the folder the program runs in, not a place to look.
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| is_executable(&dir.join(program)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/usr/bin/scrcpy-agent";

    fn exe() -> &'static Path {
        Path::new(EXE)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-fm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file<'a>(all: &'a [MenuFile], path: &str) -> &'a MenuFile {
        all.iter().find(|f| f.path == Path::new(path)).unwrap_or_else(|| panic!("no file {path}: {:?}", all.iter().map(|f| &f.path).collect::<Vec<_>>()))
    }

    const ACTIONS: [(&str, &str); 4] = [
        ("send-default", "Send to the default device"),
        ("send-choose", "Send to a device…"),
        ("open-default", "Open on the default device"),
        ("open-choose", "Open on a device…"),
    ];

    // ------------------------------------------------ what is written

    #[test]
    fn dolphin_gets_one_service_menu_with_a_submenu_of_four_actions_for_all_files() {
        let all = files(FileManager::Dolphin, exe(), &MenuTexts::default()).unwrap();
        assert_eq!(all.len(), 1);
        let f = file(&all, "kio/servicemenus/scrcpy-agent.desktop");
        assert!(f.executable, "Dolphin only shows a service menu that can be run");
        assert_eq!(
            f.content,
            "# Written by scrcpy-agent; changes are overwritten.\n\
             [Desktop Entry]\n\
             Type=Service\n\
             MimeType=application/octet-stream;\n\
             Actions=send-default;send-choose;open-default;open-choose;\n\
             X-KDE-Submenu=ScrcpyAgent\n\
             Icon=scrcpy-agent\n\
             \n\
             [Desktop Action send-default]\n\
             Name=Send to the default device\n\
             Exec=/usr/bin/scrcpy-agent --shell send-default %F\n\
             \n\
             [Desktop Action send-choose]\n\
             Name=Send to a device…\n\
             Exec=/usr/bin/scrcpy-agent --shell send-choose %F\n\
             \n\
             [Desktop Action open-default]\n\
             Name=Open on the default device\n\
             Exec=/usr/bin/scrcpy-agent --shell open-default %F\n\
             \n\
             [Desktop Action open-choose]\n\
             Name=Open on a device…\n\
             Exec=/usr/bin/scrcpy-agent --shell open-choose %F\n"
        );
    }

    #[test]
    fn nemo_gets_one_action_file_per_entry_for_files_only() {
        let all = files(FileManager::Nemo, exe(), &MenuTexts::default()).unwrap();
        assert_eq!(all.len(), 4);
        for (word, text) in ACTIONS {
            let f = file(&all, &format!("nemo/actions/scrcpy-agent-{word}.nemo_action"));
            assert!(!f.executable, "{word}");
            assert_eq!(
                f.content,
                format!(
                    "# Written by scrcpy-agent; changes are overwritten.\n\
                     [Nemo Action]\n\
                     Name=ScrcpyAgent: {text}\n\
                     Exec=/usr/bin/scrcpy-agent --shell {word} %F\n\
                     Icon-Name=scrcpy-agent\n\
                     Selection=notnone\n\
                     Extensions=nodirs;\n\
                     Quote=double\n"
                ),
                "{word}"
            );
        }
    }

    #[test]
    fn nautilus_gets_one_script_per_entry_that_hands_over_the_selected_files() {
        let all = files(FileManager::Nautilus, exe(), &MenuTexts::default()).unwrap();
        assert_eq!(all.len(), 4);
        for (word, text) in ACTIONS {
            let f = file(&all, &format!("nautilus/scripts/ScrcpyAgent - {text}"));
            assert!(f.executable, "{word}");
            assert_eq!(
                f.content,
                format!(
                    "#!/bin/sh\n\
                     # Written by scrcpy-agent; changes are overwritten.\n\
                     set -f\n\
                     IFS='\n'\n\
                     set -- $NAUTILUS_SCRIPT_SELECTED_FILE_PATHS\n\
                     [ \"$#\" -gt 0 ] || exit 0\n\
                     exec '/usr/bin/scrcpy-agent' --shell {word} \"$@\"\n"
                ),
                "{word}"
            );
        }
    }

    #[test]
    fn every_file_has_the_marker_as_a_line_of_its_own() {
        for manager in FileManager::ALL {
            let all = files(manager, exe(), &MenuTexts::default()).unwrap();
            assert!(!all.is_empty(), "{manager:?}");
            for f in all {
                assert!(f.content.lines().any(|line| line == MARKER), "{manager:?} {:?}", f.path);
            }
        }
    }

    #[test]
    fn the_name_the_menu_is_given_is_the_name_of_the_submenu_and_part_of_the_names_of_the_entries() {
        let kitchen = MenuTexts { title: "My tool".into(), ..MenuTexts::default() };
        let dolphin = files(FileManager::Dolphin, exe(), &kitchen).unwrap();
        assert!(dolphin[0].content.lines().any(|line| line == "X-KDE-Submenu=My tool"));
        let nemo = files(FileManager::Nemo, exe(), &kitchen).unwrap();
        assert!(nemo.iter().all(|f| f.content.lines().any(|line| line.starts_with("Name=My tool: "))));
        let nautilus = files(FileManager::Nautilus, exe(), &kitchen).unwrap();
        for (_, text) in ACTIONS {
            assert!(file(&nautilus, &format!("nautilus/scripts/My tool - {text}")).executable, "{text}");
        }
        // The default name is the standard one (what the programs of other systems are written by is another matter).
        let standard = files(FileManager::Dolphin, exe(), &MenuTexts::default()).unwrap();
        assert!(standard[0].content.lines().any(|line| line == "X-KDE-Submenu=ScrcpyAgent"));
    }

    #[test]
    fn the_texts_of_the_language_are_the_names_of_the_entries() {
        let texts = MenuTexts { send_default: "Send away".into(), ..MenuTexts::default() };
        let all = files(FileManager::Nautilus, exe(), &texts).unwrap();
        assert!(file(&all, "nautilus/scripts/ScrcpyAgent - Send away").executable);
        let all = files(FileManager::Dolphin, exe(), &texts).unwrap();
        assert!(file(&all, "kio/servicemenus/scrcpy-agent.desktop").content.contains("Name=Send away\n"));
    }

    #[test]
    fn a_text_that_cannot_be_written_is_an_error_not_a_broken_file() {
        for bad in ["two\nlines", "tab\there", "a/b", ""] {
            let texts = MenuTexts { open_choose: bad.into(), ..MenuTexts::default() };
            for manager in FileManager::ALL {
                assert!(
                    matches!(files(manager, exe(), &texts), Err(CoreError::ExplorerMenu(_))),
                    "{manager:?} {bad:?}"
                );
            }
        }
    }

    // ------------------------------------------------ the program in the Exec key

    #[test]
    fn a_path_without_reserved_characters_is_written_as_it_is() {
        assert_eq!(exec_word(Path::new("/usr/bin/scrcpy-agent")).as_deref(), Some("/usr/bin/scrcpy-agent"));
    }

    #[test]
    fn a_path_with_a_reserved_character_is_quoted_and_what_the_quotes_need_is_escaped_twice_over() {
        // The rules of the Desktop Entry specification (the Exec key): inside quotes a
        // double quote, a backtick, a dollar sign and a backslash take a backslash, and the
        // file's own string escaping (applied first) doubles every backslash.
        for (path, written) in [
            ("/opt/scrcpy agent/scrcpy-agent", r#""/opt/scrcpy agent/scrcpy-agent""#),
            ("/opt/a\"b c", r#""/opt/a\\"b c""#),
            ("/opt/a$b c", r#""/opt/a\\$b c""#),
            ("/opt/a`b c", r#""/opt/a\\`b c""#),
            (r"/opt/a\b c", r#""/opt/a\\\\b c""#),
            ("/opt/(x)", r#""/opt/(x)""#),
        ] {
            assert_eq!(exec_word(Path::new(path)).as_deref(), Some(written), "{path}");
        }
    }

    #[test]
    fn a_percent_sign_is_written_twice_so_it_is_not_a_field_code() {
        assert_eq!(exec_word(Path::new("/opt/100%/agent")).as_deref(), Some("/opt/100%%/agent"));
    }

    #[test]
    fn a_path_with_a_control_character_cannot_be_written() {
        assert_eq!(exec_word(Path::new("/opt/ab")).as_deref(), Some("/opt/ab"));
        assert_eq!(exec_word(Path::new("/opt/a\nb")), None);
        assert_eq!(exec_word(Path::new("/opt/a\tb")), None);
    }

    #[test]
    fn a_path_with_a_space_is_quoted_in_the_files_and_in_the_script() {
        let exe = Path::new("/opt/it's here/scrcpy-agent");
        let dolphin = files(FileManager::Dolphin, exe, &MenuTexts::default()).unwrap();
        assert_eq!(dolphin.len(), 1);
        assert!(dolphin[0].content.contains(r#"Exec="/opt/it's here/scrcpy-agent" --shell send-default %F"#));
        // In a shell script a single quote is closed, written escaped, and opened again.
        let nautilus = files(FileManager::Nautilus, exe, &MenuTexts::default()).unwrap();
        assert_eq!(nautilus.len(), 4);
        assert!(nautilus[0].content.contains(r"exec '/opt/it'\''s here/scrcpy-agent' --shell"));
    }

    // ------------------------------------------------ in a folder

    #[test]
    fn installing_writes_the_files_below_the_data_folder_and_they_are_then_seen_as_installed() {
        let home = scratch("install");
        let texts = MenuTexts::default();
        for manager in FileManager::ALL {
            assert!(!is_installed(&home, manager, exe(), &texts), "{manager:?}");
            install(&home, manager, exe(), &texts).unwrap();
            assert!(is_installed(&home, manager, exe(), &texts), "{manager:?}");
            for f in files(manager, exe(), &texts).unwrap() {
                assert_eq!(std::fs::read_to_string(home.join(&f.path)).unwrap(), f.content, "{:?}", f.path);
            }
        }
        // Another place of the program, or another language, is not the same menu.
        assert!(!is_installed(&home, FileManager::Nemo, Path::new("/elsewhere/scrcpy-agent"), &texts));
        let other = MenuTexts { open_choose: "Open elsewhere".into(), ..MenuTexts::default() };
        assert!(!is_installed(&home, FileManager::Nautilus, exe(), &other));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn the_files_that_have_to_be_run_get_the_executable_bit_and_the_others_do_not() {
        use std::os::unix::fs::PermissionsExt;
        let home = scratch("mode");
        for manager in FileManager::ALL {
            install(&home, manager, exe(), &MenuTexts::default()).unwrap();
            for f in files(manager, exe(), &MenuTexts::default()).unwrap() {
                let mode = std::fs::metadata(home.join(&f.path)).unwrap().permissions().mode();
                assert_eq!(mode & 0o111 != 0, f.executable, "{:?} {mode:o}", f.path);
            }
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn installing_again_replaces_what_was_there_and_leaves_the_files_of_others_alone() {
        let home = scratch("replace");
        install(&home, FileManager::Nautilus, exe(), &MenuTexts::default()).unwrap();
        let scripts = home.join("nautilus/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("my own script"), "#!/bin/sh\necho mine\n").unwrap();
        // Another language: the scripts are named by their texts, so the old ones have to go.
        let other_words = MenuTexts { send_default: "Send away".into(), ..MenuTexts::default() };
        install(&home, FileManager::Nautilus, Path::new("/moved/scrcpy-agent"), &other_words).unwrap();
        assert!(is_installed(&home, FileManager::Nautilus, Path::new("/moved/scrcpy-agent"), &other_words));
        assert!(!scripts.join("ScrcpyAgent - Send to the default device").exists());
        assert_eq!(std::fs::read_to_string(scripts.join("my own script")).unwrap(), "#!/bin/sh\necho mine\n");
        assert_eq!(std::fs::read_dir(&scripts).unwrap().count(), 4 + 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstalling_removes_what_this_program_wrote_for_that_manager_only_and_may_be_done_twice() {
        let home = scratch("uninstall");
        let texts = MenuTexts::default();
        for manager in FileManager::ALL {
            install(&home, manager, exe(), &texts).unwrap();
        }
        let own = home.join("nemo/actions/my-own.nemo_action");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "[Nemo Action]\nName=mine\nExec=true\n").unwrap();
        uninstall(&home, FileManager::Nemo).unwrap();
        assert!(!is_installed(&home, FileManager::Nemo, exe(), &texts));
        assert!(own.exists(), "a file of the user's is not removed");
        assert_eq!(std::fs::read_dir(home.join("nemo/actions")).unwrap().count(), 1);
        // The other managers keep theirs.
        assert!(is_installed(&home, FileManager::Dolphin, exe(), &texts));
        assert!(is_installed(&home, FileManager::Nautilus, exe(), &texts));
        uninstall(&home, FileManager::Nemo).unwrap();
        uninstall(&home, FileManager::Dolphin).unwrap();
        uninstall(&home, FileManager::Nautilus).unwrap();
        assert!(!is_installed(&home, FileManager::Dolphin, exe(), &texts));
        // Nothing to remove in a data folder that has none of it.
        uninstall(&scratch("uninstall-empty"), FileManager::Dolphin).unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }

    // ------------------------------------------------ where, and which

    #[test]
    fn the_data_folder_is_xdg_data_home_or_local_share_of_the_home_folder_and_only_from_the_root() {
        let some = |s: &'static str| Some(OsStr::new(s));
        let at = |xdg, home| data_home(xdg, home);
        assert_eq!(at(some("/x/data"), some("/home/u")), Some(PathBuf::from("/x/data")));
        assert_eq!(at(None, some("/home/u")), Some(PathBuf::from("/home/u/.local/share")));
        // Empty or relative values are not places (the XDG specification says to ignore them).
        assert_eq!(at(some(""), some("/home/u")), Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(at(some("data"), some("/home/u")), Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(at(None, some("home/u")), None);
        assert_eq!(at(None, some("")), None);
        assert_eq!(at(None, None), None);
        assert_eq!(at(some("/x/data"), None), Some(PathBuf::from("/x/data")));
    }

    #[test]
    fn the_plan_writes_for_the_file_managers_found_and_not_turned_off_and_removes_everywhere_else() {
        use Change::{Install, Remove};
        use FileManager::{Dolphin, Nautilus, Nemo};
        let all = FileManager::ALL;
        assert_eq!(plan(true, &[], &all), [(Dolphin, Install), (Nemo, Install), (Nautilus, Install)]);
        assert_eq!(plan(true, &[], &[Nemo]), [(Dolphin, Remove), (Nemo, Install), (Nautilus, Remove)]);
        assert_eq!(plan(true, &[Nemo], &all), [(Dolphin, Install), (Nemo, Remove), (Nautilus, Install)]);
        assert_eq!(plan(true, &[], &[]), [(Dolphin, Remove), (Nemo, Remove), (Nautilus, Remove)]);
        // Not wanted at all: nothing is written, whatever is found, and what was written goes.
        assert_eq!(plan(false, &[], &all), [(Dolphin, Remove), (Nemo, Remove), (Nautilus, Remove)]);
    }

    #[test]
    fn a_plan_is_carried_out_for_each_file_manager_and_a_failure_does_not_stop_the_others() {
        use Change::{Install, Remove};
        let home = scratch("apply");
        let texts = MenuTexts::default();
        install(&home, FileManager::Nemo, exe(), &texts).unwrap();
        // A file where the folder of Nautilus has to be: its menu cannot be written.
        std::fs::write(home.join("nautilus"), "in the way").unwrap();
        let result = apply(
            &home,
            &[(FileManager::Dolphin, Install), (FileManager::Nemo, Remove), (FileManager::Nautilus, Install)],
            exe(),
            &texts,
        );
        assert!(matches!(result, Err(CoreError::ExplorerMenu(_))), "{result:?}");
        assert!(is_installed(&home, FileManager::Dolphin, exe(), &texts));
        assert!(!is_installed(&home, FileManager::Nemo, exe(), &texts));
        assert!(owned(&home, FileManager::Nemo).is_empty());
        // Without the failure it is Ok.
        std::fs::remove_file(home.join("nautilus")).unwrap();
        apply(&home, &[(FileManager::Nautilus, Install)], exe(), &texts).unwrap();
        assert!(is_installed(&home, FileManager::Nautilus, exe(), &texts));
        let _ = std::fs::remove_dir_all(&home);
    }

    // ------------------------------------------------ which file managers

    #[test]
    fn the_file_managers_found_are_those_whose_program_is_there_in_the_order_of_the_list() {
        assert_eq!(detect(|_| false), []);
        assert_eq!(detect(|p| p == "nemo"), [FileManager::Nemo]);
        assert_eq!(detect(|p| p == "nautilus" || p == "dolphin"), [FileManager::Dolphin, FileManager::Nautilus]);
        assert_eq!(detect(|_| true), FileManager::ALL);
        // A program of another name is not one: nothing is taken for a file manager by part of a name.
        assert_eq!(detect(|p| p == "dolphin-emu" || p == "nautilus-python"), []);
    }

    #[cfg(unix)]
    #[test]
    fn a_program_is_on_the_path_when_it_is_a_file_that_can_be_run_in_one_of_its_folders() {
        use std::os::unix::fs::PermissionsExt;
        let one = scratch("path-one");
        let two = scratch("path-two");
        let run = |dir: &Path, name: &str, mode: u32| {
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        run(&two, "nemo", 0o755);
        run(&one, "dolphin", 0o644);
        std::fs::create_dir(one.join("nautilus")).unwrap();
        let path = std::env::join_paths([&one, &two]).unwrap();
        assert!(on_path("nemo", &path), "in the second folder");
        assert!(!on_path("dolphin", &path), "a file that cannot be run is not a program");
        assert!(!on_path("nautilus", &path), "a folder is not a program");
        assert!(!on_path("missing", &path));
        assert!(!on_path("nemo", OsStr::new("")));
        let _ = std::fs::remove_dir_all(&one);
        let _ = std::fs::remove_dir_all(&two);
    }
}
