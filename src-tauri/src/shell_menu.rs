//! The right-click menu for files on a system that has one: on Windows the
//! registry behind `core::integration::explorer_menu`, on Linux the files of the file managers
//! (`core::integration::file_manager_menu`); and when it is written (the setting, a change of
//! language, the start of the program).

use crate::core::error::CoreError;
use crate::core::integration::explorer_menu::MenuTexts;
use crate::core::integration::file_manager_menu::FileManager;
use crate::core::integration::shortcuts::ShortcutKind;
use crate::core::window::layout::Os;

/// Whether this system has a menu for files to write: Windows always, Linux when
/// it has a file manager this program knows.
pub fn available() -> bool {
    match Os::current() {
        Os::Windows => true,
        Os::Linux => !detected_file_managers().is_empty(),
        Os::Other => false,
    }
}

/// The kind of shortcut of apps this system has; `None` where there are none.
pub fn shortcut_kind() -> Option<ShortcutKind> {
    ShortcutKind::of(Os::current())
}

/// The file managers of this system that have a menu to write (none outside Linux).
pub fn detected_file_managers() -> Vec<FileManager> {
    if Os::current() != Os::Linux {
        return Vec::new();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    crate::core::integration::file_manager_menu::detect(|program| crate::core::integration::file_manager_menu::on_path(program, &path))
}

#[cfg(windows)]
mod registry {
    use crate::core::integration::explorer_menu::Registry;
    use std::io;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    /// The current user's part of the registry: no administrator is needed.
    pub struct UserRegistry;

    impl Registry for UserRegistry {
        fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key)?;
            key.set_value(name, &value)
        }

        fn get(&self, key: &str, name: &str) -> Option<String> {
            let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_READ).ok()?;
            key.get_value::<String, _>(name).ok()
        }

        fn delete_tree(&mut self, key: &str) -> io::Result<()> {
            match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(key) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        }
    }
}

/// Writes a shortcut of the `kind` that starts `exe` with these arguments, as
/// `file_name` in `folder`, which is made if it is not there; `display_name` is the
/// name it is shown with where the kind has one. An older one of that name is
/// replaced. Returns the path of the file.
pub fn create_shortcut(
    kind: ShortcutKind,
    folder: &std::path::Path,
    exe: &std::path::Path,
    file_name: &str,
    display_name: &str,
    arguments: &[String],
) -> Result<std::path::PathBuf, CoreError> {
    match kind {
        ShortcutKind::DesktopEntry => {
            let fail = |e: &dyn std::fmt::Display| CoreError::Shortcut(e.to_string());
            // Made before anything is written: a name or command that cannot be written leaves the old file.
            let command = crate::core::integration::desktop_entry::exec_line(exe, arguments)
                .ok_or_else(|| CoreError::Shortcut(format!("a command that cannot be written: {exe:?}")))?;
            let icon = crate::core::integration::desktop_entry::icon_for(exe, |path| path.is_file());
            let text = crate::core::integration::desktop_entry::render_application(display_name, &command, &icon)?;
            std::fs::create_dir_all(folder).map_err(|e| fail(&e))?;
            let path = folder.join(file_name);
            // A desktop that watches the folder reads a file as soon as it is made: one that is made
            // empty and filled afterwards is taken for a file that is not a launcher (it gets the icon of
            // a document and keeps it). So the file is written whole, with its permissions, under another
            // name, and then renamed: it appears in one step, and replaces an old one the same way.
            let part = folder.join(format!(".{file_name}.part"));
            let written = std::fs::write(&part, text)
                .map_err(|e| fail(&e))
                .and_then(|()| make_launchable(&part))
                .and_then(|()| std::fs::rename(&part, &path).map_err(|e| fail(&e)));
            if written.is_err() {
                let _ = std::fs::remove_file(&part);
            }
            written?;
            Ok(path)
        }
        #[cfg(windows)]
        ShortcutKind::WindowsLink => {
            let fail = |e: &dyn std::fmt::Display| CoreError::Shortcut(e.to_string());
            std::fs::create_dir_all(folder).map_err(|e| fail(&e))?;
            let path = folder.join(file_name);
            let mut link = mslnk::ShellLink::new(exe).map_err(|e| fail(&e))?;
            link.set_arguments(Some(crate::core::integration::shortcuts::command_line(arguments)));
            if let Some(dir) = exe.parent() {
                link.set_working_dir(Some(dir.display().to_string()));
            }
            link.create_lnk(&path).map_err(|e| fail(&e))?;
            Ok(path)
        }
        #[cfg(not(windows))]
        ShortcutKind::WindowsLink => {
            let _ = (folder, exe, file_name);
            Err(CoreError::Unavailable)
        }
    }
}

/// The desktops start a launcher on the desktop only if it can be run (otherwise they warn
/// that it is "untrusted"); the menu of applications does not care.
#[cfg(unix)]
fn make_launchable(path: &std::path::Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| CoreError::Shortcut(e.to_string()))
}

/// Where there is no executable bit there is nothing to set.
#[cfg(not(unix))]
fn make_launchable(_path: &std::path::Path) -> Result<(), CoreError> {
    Ok(())
}

/// Removes a shortcut (the file that it is); one that is not there is not an error.
pub fn remove_shortcut_file(path: &std::path::Path) -> Result<(), CoreError> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(CoreError::Shortcut(e.to_string())),
        _ => Ok(()),
    }
}

/// Removes a folder that this program made, if nothing is left in it. A folder
/// with anything in it, or one that is not there, is left as it is.
pub fn remove_folder_if_empty(folder: &std::path::Path) -> Result<(), CoreError> {
    // `remove_dir` refuses a folder with anything in it: that is the check, made by the system.
    match std::fs::remove_dir(folder) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) if std::fs::read_dir(folder).is_ok_and(|mut entries| entries.next().is_some()) => Ok(()),
        Err(e) => Err(CoreError::Shortcut(e.to_string())),
        Ok(()) => Ok(()),
    }
}

/// Writes the menu (`enabled`) or removes it, for the program that is running.
///
/// `disabled` are the file managers (Linux) the menu is not written for.
#[cfg(windows)]
pub fn apply(enabled: bool, _disabled: &[FileManager], texts: &MenuTexts) -> Result<(), CoreError> {
    use crate::core::integration::explorer_menu::{register, unregister};
    let exe = std::env::current_exe().map_err(|e| CoreError::ExplorerMenu(e.to_string()))?;
    let mut registry = registry::UserRegistry;
    let result = if enabled { register(&mut registry, &exe, texts) } else { unregister(&mut registry) };
    result.map_err(|e| CoreError::ExplorerMenu(e.to_string()))
}

#[cfg(not(windows))]
pub fn apply(enabled: bool, disabled: &[FileManager], texts: &MenuTexts) -> Result<(), CoreError> {
    use crate::core::integration::file_manager_menu::{apply as carry_out, data_home, plan};
    if Os::current() != Os::Linux {
        // Nothing to remove where there is nothing to write.
        return if enabled { Err(CoreError::Unavailable) } else { Ok(()) };
    }
    let Some(home) = data_home(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    ) else {
        return if enabled { Err(CoreError::Unavailable) } else { Ok(()) };
    };
    let exe = std::env::current_exe().map_err(|e| CoreError::ExplorerMenu(e.to_string()))?;
    carry_out(&home, &plan(enabled, disabled, &detected_file_managers()), &exe, texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_shortcut_is_a_link_file_in_the_folder_that_is_made_and_a_second_one_replaces_it() {
        let folder = std::env::temp_dir().join(format!("scrcpy-agent-lnk-{}", std::process::id())).join("nested");
        let exe = std::env::current_exe().unwrap();
        let args = |package: &str| vec!["--app".to_string(), "dev".to_string(), package.to_string()];
        let make = |package: &str| {
            create_shortcut(ShortcutKind::WindowsLink, &folder, &exe, "App - Tablet.lnk", "App - Tablet", &args(package))
        };
        let first = make("com.example.app").unwrap();
        assert_eq!(first, folder.join("App - Tablet.lnk"));
        let bytes = std::fs::read(&first).unwrap();
        // The header of a shell link: its size (0x4C) as a 32-bit number, then its class id.
        assert_eq!(bytes[..4], [0x4C, 0, 0, 0]);
        let second = make("com.example.other").unwrap();
        assert_eq!(second, first);
        assert_ne!(std::fs::read(&second).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removing_a_shortcut_takes_only_that_file_and_one_that_is_gone_is_not_an_error() {
        let dir = scratch("remove");
        std::fs::write(dir.join("A.lnk"), b"x").unwrap();
        std::fs::write(dir.join("B.lnk"), b"x").unwrap();
        remove_shortcut_file(&dir.join("A.lnk")).unwrap();
        assert!(!dir.join("A.lnk").exists());
        assert!(dir.join("B.lnk").exists());
        remove_shortcut_file(&dir.join("A.lnk")).unwrap();
        // The folder itself stays: it may be the desktop.
        assert!(dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_removed_only_when_it_is_empty() {
        let dir = scratch("folder");
        std::fs::write(dir.join("A.lnk"), b"x").unwrap();
        remove_folder_if_empty(&dir).unwrap();
        assert!(dir.exists(), "a folder with a file in it must stay");
        std::fs::remove_file(dir.join("A.lnk")).unwrap();
        remove_folder_if_empty(&dir).unwrap();
        assert!(!dir.exists());
        // One that is not there is not an error.
        remove_folder_if_empty(&dir).unwrap();
    }

    fn args(package: &str) -> Vec<String> {
        vec!["--app".to_string(), "dev-1".to_string(), package.to_string()]
    }

    #[test]
    fn a_desktop_entry_is_written_with_its_name_command_and_icon_in_the_folder_that_is_made_and_a_second_one_replaces_it() {
        let root = scratch("desktop-entry");
        let folder = root.join("nested").join("applications");
        let exe = std::path::Path::new("/usr/bin/scrcpy-agent");
        let make = |package: &str| {
            create_shortcut(ShortcutKind::DesktopEntry, &folder, exe, "Notes - Tablet.desktop", "Notes - Tablet", &args(package))
        };
        let first = make("com.example.notes").unwrap();
        assert_eq!(first, folder.join("Notes - Tablet.desktop"));
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "# Written by scrcpy-agent; changes are overwritten.\n\
             [Desktop Entry]\n\
             Type=Application\n\
             Version=1.0\n\
             Name=Notes - Tablet\n\
             Exec=/usr/bin/scrcpy-agent --app dev-1 com.example.notes\n\
             Icon=scrcpy-agent\n\
             Terminal=false\n\
             Categories=Utility;\n"
        );
        let second = make("com.example.other").unwrap();
        assert_eq!(second, first);
        assert!(std::fs::read_to_string(&second).unwrap().contains("Exec=/usr/bin/scrcpy-agent --app dev-1 com.example.other\n"));
        // A command or a name that cannot be written makes no file (and leaves the old one).
        let before = std::fs::read(&second).unwrap();
        let bad = create_shortcut(ShortcutKind::DesktopEntry, &folder, exe, "Notes - Tablet.desktop", "Notes - Tablet", &args("a\nb"));
        assert!(matches!(bad, Err(CoreError::Shortcut(_))), "{bad:?}");
        assert_eq!(std::fs::read(&second).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_desktop_entry_can_be_run_so_that_the_desktops_take_it_for_a_launcher_they_may_start() {
        use std::os::unix::fs::PermissionsExt;
        let folder = scratch("desktop-mode");
        let path = create_shortcut(
            ShortcutKind::DesktopEntry,
            &folder,
            std::path::Path::new("/usr/bin/scrcpy-agent"),
            "A.desktop",
            "A",
            &args("com.example.a"),
        )
        .unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn a_desktop_entry_appears_whole_in_one_step_and_a_new_one_takes_the_place_of_the_old_as_another_file() {
        use std::os::unix::fs::MetadataExt;
        // A desktop that watches the folder reads a file as soon as it is made: one that is made empty
        // and filled afterwards is taken for a file that is not a launcher (it gets the icon of a
        // document and keeps it). So the file is written under another name and renamed.
        let folder = scratch("desktop-whole");
        let make = |package: &str| {
            create_shortcut(
                ShortcutKind::DesktopEntry,
                &folder,
                std::path::Path::new("/usr/bin/scrcpy-agent"),
                "A.desktop",
                "A",
                &args(package),
            )
            .unwrap()
        };
        let path = make("com.example.a");
        let first = std::fs::metadata(&path).unwrap().ino();
        make("com.example.b");
        let second = std::fs::metadata(&path).unwrap().ino();
        assert_ne!(first, second, "a new one replaces the old by a rename, not by writing into it");
        // Nothing of the way it was made is left in the folder.
        let names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["A.desktop"]);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_windows_link_cannot_be_made_where_there_are_none() {
        let result = create_shortcut(
            ShortcutKind::WindowsLink,
            std::path::Path::new("x"),
            std::path::Path::new("y"),
            "z.lnk",
            "z",
            &[],
        );
        assert_eq!(result, Err(CoreError::Unavailable));
    }
}
