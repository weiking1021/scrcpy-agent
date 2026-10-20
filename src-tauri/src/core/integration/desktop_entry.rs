//! Desktop entries (`.desktop` files) of Linux desktops: what a launcher for an app
//! of a device is, and how a command is written in the `Exec` key. Pure functions of
//! the Desktop Entry specification, the same on every system and tested on all of
//! them; writing the file is `shell_menu`'s. The menus of the file managers
//! (`file_manager_menu`) write their commands the same way.

use crate::core::error::CoreError;
use super::file_manager_menu::MARKER;
use std::path::Path;

/// The characters the Desktop Entry specification reserves in the `Exec` key: a
/// word that has one is quoted.
const RESERVED: &[char] = &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];

/// A word of the `Exec` key (quoted when it has a character the format reserves, and
/// written the way the file needs it, which doubles the backslashes); `None` for a text
/// with a control character, which cannot be written.
pub fn exec_arg(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    // An empty word would vanish between its spaces: it is written as two quotes.
    let word = if text.is_empty() {
        r#""""#.to_string()
    } else if text.contains(RESERVED) {
        let mut quoted = String::from("\"");
        for c in text.chars() {
            // Inside quotes these four take a backslash (the rule of the quotes)...
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        // ...and the string escaping of the file, applied first, doubles every backslash.
        quoted.replace('\\', r"\\")
    } else {
        text.to_string()
    };
    // A percent sign is not a field code when it is written twice.
    Some(word.replace('%', "%%"))
}

/// The `Exec` value that starts the program at `exe` with these arguments; `None` if
/// any of them cannot be written.
pub fn exec_line(exe: &Path, args: &[String]) -> Option<String> {
    let mut words = vec![exec_arg(exe.to_str()?)?];
    for arg in args {
        words.push(exec_arg(arg)?);
    }
    Some(words.join(" "))
}

/// The name of the icon the package of the program installs (in the icon theme `hicolor`).
pub const ICON_NAME: &str = "scrcpy-agent";

/// The `Icon` of a launcher for the program at `exe`: the path of the icon file that is
/// installed with it (`<prefix>/share/icons/hicolor/<size>/apps/scrcpy-agent.png`, where
/// the prefix is the folder above the one of the program), the largest size first, as
/// `exists` finds it; else the name of the icon, which the desktop looks for in its theme.
pub fn icon_for(exe: &Path, exists: impl Fn(&Path) -> bool) -> String {
    let Some(prefix) = exe.parent().and_then(Path::parent) else { return ICON_NAME.to_string() };
    ["128x128", "32x32"]
        .iter()
        .map(|size| prefix.join("share/icons/hicolor").join(size).join("apps").join(format!("{ICON_NAME}.png")))
        .find(|path| exists(path))
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| ICON_NAME.to_string())
}

/// A value of a key of type string: no control character (a line break would end it),
/// and a backslash written twice.
fn value(what: &str, text: &str) -> Result<String, CoreError> {
    if text.trim().is_empty() || text.chars().any(char::is_control) {
        return Err(CoreError::Shortcut(format!("not usable as the {what} of a launcher: {text:?}")));
    }
    Ok(text.replace('\\', r"\\"))
}

/// The text of a launcher for the application menu or the desktop: `name` as the name,
/// `exec` (see `exec_line`) as the command, `icon` the name of an icon of the theme.
pub fn render_application(name: &str, exec: &str, icon: &str) -> Result<String, CoreError> {
    // The command is already written the way the file needs it (`exec_line`): it is only checked.
    if exec.trim().is_empty() || exec.chars().any(char::is_control) {
        return Err(CoreError::Shortcut(format!("not usable as the command of a launcher: {exec:?}")));
    }
    Ok(format!(
        "{MARKER}\n[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nExec={exec}\nIcon={}\nTerminal=false\nCategories=Utility;\n",
        value("name", name)?,
        value("icon", icon)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_command_is_the_program_and_each_argument_as_a_word_of_its_own() {
        assert_eq!(
            exec_line(Path::new("/usr/bin/scrcpy-agent"), &strings(&["--app", "device-1", "com.example.notes"])).as_deref(),
            Some("/usr/bin/scrcpy-agent --app device-1 com.example.notes")
        );
        assert_eq!(exec_line(Path::new("/usr/bin/scrcpy-agent"), &[]).as_deref(), Some("/usr/bin/scrcpy-agent"));
    }

    #[test]
    fn a_word_with_a_reserved_character_is_quoted_and_a_percent_sign_is_not_a_field_code() {
        // The id of a device may have a space, quotes, a dollar sign or a percent sign.
        let line = exec_line(
            Path::new("/opt/my apps/scrcpy-agent"),
            &strings(&["--app", "adb-A B._adb._tcp.", "it's 100%", "x$y"]),
        );
        assert_eq!(
            line.as_deref(),
            Some(r#""/opt/my apps/scrcpy-agent" --app "adb-A B._adb._tcp." "it's 100%%" "x\\$y""#)
        );
        // A device id that has none is written as it is.
        assert_eq!(
            exec_line(Path::new("/usr/bin/a"), &strings(&["--app", "adb-X._adb-tls-connect._tcp.", "a.b"])).as_deref(),
            Some("/usr/bin/a --app adb-X._adb-tls-connect._tcp. a.b")
        );
    }

    #[test]
    fn a_command_with_a_control_character_in_it_cannot_be_written() {
        assert_eq!(exec_line(Path::new("/usr/bin/a"), &strings(&["--app", "line\nbreak", "a.b"])), None);
        assert_eq!(exec_line(Path::new("/usr/bin/a\tb"), &strings(&["--app"])), None);
        // The same words without the control character are fine.
        assert!(exec_line(Path::new("/usr/bin/a"), &strings(&["--app", "line break", "a.b"])).is_some());
    }

    #[test]
    fn the_launcher_is_an_application_with_a_name_a_command_and_an_icon_and_no_terminal() {
        let text = render_application("Notes - Kitchen", "/usr/bin/scrcpy-agent --app dev com.example.notes", "scrcpy-agent").unwrap();
        assert_eq!(
            text,
            format!(
                "{MARKER}\n\
                 [Desktop Entry]\n\
                 Type=Application\n\
                 Version=1.0\n\
                 Name=Notes - Kitchen\n\
                 Exec=/usr/bin/scrcpy-agent --app dev com.example.notes\n\
                 Icon=scrcpy-agent\n\
                 Terminal=false\n\
                 Categories=Utility;\n"
            )
        );
    }

    #[test]
    fn the_icon_is_the_installed_file_by_its_path_when_there_is_one_else_its_name() {
        let all = |_: &Path| true;
        let none = |_: &Path| false;
        // Compared as paths, not as text: the separator is the system's.
        let path = |text: String| std::path::PathBuf::from(text);
        // The package puts the program in a `bin` folder and the icons in `share/icons` beside it.
        assert_eq!(
            path(icon_for(Path::new("/usr/bin/scrcpy-agent"), all)),
            Path::new("/usr/share/icons/hicolor/128x128/apps/scrcpy-agent.png")
        );
        assert_eq!(
            path(icon_for(Path::new("/opt/my apps/bin/scrcpy-agent"), all)),
            Path::new("/opt/my apps/share/icons/hicolor/128x128/apps/scrcpy-agent.png")
        );
        // The largest size that is there; one that is not there is skipped.
        let only_small = |p: &Path| p.ends_with("hicolor/32x32/apps/scrcpy-agent.png");
        assert_eq!(
            path(icon_for(Path::new("/usr/bin/scrcpy-agent"), only_small)),
            Path::new("/usr/share/icons/hicolor/32x32/apps/scrcpy-agent.png")
        );
        // Without a file the desktop is left to look for the name in its theme.
        assert_eq!(icon_for(Path::new("/usr/bin/scrcpy-agent"), none), "scrcpy-agent");
        assert_eq!(icon_for(Path::new("/scrcpy-agent"), all), "scrcpy-agent");
        assert_eq!(icon_for(Path::new("scrcpy-agent"), all), "scrcpy-agent");
    }

    #[test]
    fn a_name_is_written_the_way_the_file_needs_and_one_that_cannot_be_is_refused() {
        // A backslash is written twice (the escaping of the values of the format).
        let text = render_application(r"a\b Käfer", "/x", "i").unwrap();
        assert!(text.lines().any(|l| l == r"Name=a\\b Käfer"), "{text}");
        // A line break would end the value; a name is never empty.
        for bad in ["two\nlines", "tab\there", "cr\rhere", "", "   "] {
            assert!(matches!(render_application(bad, "/x", "i"), Err(CoreError::Shortcut(_))), "{bad:?}");
        }
        for bad in ["", "two\nlines"] {
            assert!(matches!(render_application("n", bad, "i"), Err(CoreError::Shortcut(_))), "{bad:?}");
            assert!(matches!(render_application("n", "/x", bad), Err(CoreError::Shortcut(_))), "{bad:?}");
        }
    }
}
