//! The entries of the Windows Explorer menu for files: a submenu "scrcpy-agent"
//! with four entries. They are registry values under the current user's
//! `Software\Classes\*\shell`, which Windows 10 shows in its menu and Windows 11
//! in "Show more options". This module is what to write, and the registry itself
//! is behind `Registry`, so the logic is the same on every system and a fake
//! registry stands in for the real one in tests.

use crate::core::requests::shell_request::ShellAction;
use std::io;
use std::path::Path;

/// The key of the submenu, below the current user's classes.
pub const ROOT: &str = r"Software\Classes\*\shell\scrcpy-agent";

/// The texts of the entries, in the user's language.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct MenuTexts {
    pub title: String,
    pub send_default: String,
    pub send_choose: String,
    pub open_default: String,
    pub open_choose: String,
}

impl Default for MenuTexts {
    fn default() -> Self {
        MenuTexts {
            title: crate::core::app_name::DEFAULT_APP_NAME.into(),
            send_default: "Send to the default device".into(),
            send_choose: "Send to a device…".into(),
            open_default: "Open on the default device".into(),
            open_choose: "Open on a device…".into(),
        }
    }
}

/// One value to write: a key, the name of the value (empty for the key's default
/// value) and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegEntry {
    pub key: String,
    pub name: String,
    pub value: String,
}

/// Everything that makes the menu, for the program at `exe`.
pub fn entries(exe: &Path, texts: &MenuTexts) -> Vec<RegEntry> {
    let program = exe.display().to_string();
    let entry = |key: &str, name: &str, value: &str| RegEntry { key: key.into(), name: name.into(), value: value.into() };
    let mut all = vec![
        entry(ROOT, "MUIVerb", &texts.title),
        // A key with this value (even an empty one) is a submenu of the keys of `shell` below it.
        entry(ROOT, "SubCommands", ""),
        entry(ROOT, "Icon", &format!("\"{program}\",0")),
    ];
    // Explorer lists the entries of a submenu by the names of their keys.
    for (key, action, text) in [
        ("1-send-default", ShellAction::SendDefault, &texts.send_default),
        ("2-send-choose", ShellAction::SendChoose, &texts.send_choose),
        ("3-open-default", ShellAction::OpenDefault, &texts.open_default),
        ("4-open-choose", ShellAction::OpenChoose, &texts.open_choose),
    ] {
        let key = format!(r"{ROOT}\shell\{key}");
        all.push(entry(&key, "MUIVerb", text));
        // The file is `%1`; Explorer starts the program once for each selected file.
        all.push(entry(&format!(r"{key}\command"), "", &format!("\"{program}\" --shell {} \"%1\"", action.word())));
    }
    all
}

/// A registry the entries are written to: values of keys, below one root.
pub trait Registry {
    fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()>;
    fn get(&self, key: &str, name: &str) -> Option<String>;
    /// Removes a key and all below it; a key that is not there is not an error.
    fn delete_tree(&mut self, key: &str) -> io::Result<()>;
}

/// Writes the menu, replacing what was there (another language, another place of
/// the program).
pub fn register(registry: &mut impl Registry, exe: &Path, texts: &MenuTexts) -> io::Result<()> {
    registry.delete_tree(ROOT)?;
    for e in entries(exe, texts) {
        registry.set(&e.key, &e.name, &e.value)?;
    }
    Ok(())
}

pub fn unregister(registry: &mut impl Registry) -> io::Result<()> {
    registry.delete_tree(ROOT)
}

/// Whether the menu is there for this program with these texts, exactly.
pub fn is_registered(registry: &impl Registry, exe: &Path, texts: &MenuTexts) -> bool {
    entries(exe, texts).iter().all(|e| registry.get(&e.key, &e.name).as_deref() == Some(e.value.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A registry in memory.
    #[derive(Default)]
    struct Fake {
        values: BTreeMap<(String, String), String>,
        fail_on: Option<String>,
    }

    impl Registry for Fake {
        fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
            if self.fail_on.as_deref() == Some(key) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.values.insert((key.to_string(), name.to_string()), value.to_string());
            Ok(())
        }
        fn get(&self, key: &str, name: &str) -> Option<String> {
            self.values.get(&(key.to_string(), name.to_string())).cloned()
        }
        fn delete_tree(&mut self, key: &str) -> io::Result<()> {
            let below = format!("{key}\\");
            self.values.retain(|(k, _), _| k != key && !k.starts_with(&below));
            Ok(())
        }
    }

    fn exe() -> &'static Path {
        Path::new(r"X:\app\scrcpy-agent.exe")
    }

    fn value(entries: &[RegEntry], key: &str, name: &str) -> Option<String> {
        entries.iter().find(|e| e.key == key && e.name == name).map(|e| e.value.clone())
    }

    #[test]
    fn the_menu_is_called_by_the_standard_name_of_the_program_until_it_is_given_another() {
        assert_eq!(MenuTexts::default().title, crate::core::app_name::DEFAULT_APP_NAME);
        let kitchen = MenuTexts { title: "Kitchen".into(), ..MenuTexts::default() };
        assert_eq!(value(&entries(exe(), &kitchen), ROOT, "MUIVerb").as_deref(), Some("Kitchen"));
        // What identifies the menu in the registry is not the name it is shown by.
        assert_eq!(ROOT, r"Software\Classes\*\shell\scrcpy-agent");
    }

    #[test]
    fn the_menu_is_a_submenu_with_four_entries_each_running_the_program_with_its_action_and_the_file() {
        let texts = MenuTexts::default();
        let all = entries(exe(), &texts);
        assert_eq!(value(&all, ROOT, "MUIVerb").as_deref(), Some("ScrcpyAgent"));
        // An empty value of this name is what makes the key a submenu.
        assert_eq!(value(&all, ROOT, "SubCommands").as_deref(), Some(""));
        assert_eq!(value(&all, ROOT, "Icon").as_deref(), Some(r#""X:\app\scrcpy-agent.exe",0"#));
        for (order, action, text) in [
            ("1-send-default", ShellAction::SendDefault, &texts.send_default),
            ("2-send-choose", ShellAction::SendChoose, &texts.send_choose),
            ("3-open-default", ShellAction::OpenDefault, &texts.open_default),
            ("4-open-choose", ShellAction::OpenChoose, &texts.open_choose),
        ] {
            let entry = format!(r"{ROOT}\shell\{order}");
            assert_eq!(value(&all, &entry, "MUIVerb").as_ref(), Some(text), "{order}");
            assert_eq!(
                value(&all, &format!(r"{entry}\command"), "").as_deref(),
                Some(format!(r#""X:\app\scrcpy-agent.exe" --shell {} "%1""#, action.word()).as_str()),
                "{order}"
            );
        }
        // Nothing else is written.
        assert_eq!(all.len(), 3 + 4 * 2);
    }

    #[test]
    fn registering_writes_every_entry_and_is_then_seen_as_registered_only_for_the_same_program_and_texts() {
        let mut registry = Fake::default();
        let texts = MenuTexts::default();
        assert!(!is_registered(&registry, exe(), &texts));
        register(&mut registry, exe(), &texts).unwrap();
        assert!(is_registered(&registry, exe(), &texts));
        assert_eq!(registry.values.len(), 3 + 4 * 2);
        // Another place of the program, or another language, is not the same menu.
        assert!(!is_registered(&registry, Path::new(r"Y:\other\scrcpy-agent.exe"), &texts));
        let other = MenuTexts { open_choose: "Open elsewhere".into(), ..MenuTexts::default() };
        assert!(!is_registered(&registry, exe(), &other));
    }

    #[test]
    fn registering_again_replaces_what_was_there_and_leaves_nothing_of_the_old() {
        let mut registry = Fake::default();
        register(&mut registry, exe(), &MenuTexts::default()).unwrap();
        // A key of an older version of the menu, below the root, must go.
        registry.set(&format!(r"{ROOT}\shell\9-old"), "MUIVerb", "old").unwrap();
        // (Example paths only: any path would do.)
        let moved = Path::new(r"Y:\moved\scrcpy-agent.exe");
        let other_words = MenuTexts { title: "scrcpy-agent".into(), send_default: "Send away".into(), ..MenuTexts::default() };
        register(&mut registry, moved, &other_words).unwrap();
        assert!(is_registered(&registry, moved, &other_words));
        assert_eq!(registry.get(&format!(r"{ROOT}\shell\9-old"), "MUIVerb"), None);
        assert_eq!(registry.values.len(), 3 + 4 * 2);
    }

    #[test]
    fn unregistering_removes_the_whole_menu_and_nothing_else_and_may_be_done_twice() {
        let mut registry = Fake::default();
        registry.set(r"Software\Classes\*\shell\other-program", "MUIVerb", "keep me").unwrap();
        register(&mut registry, exe(), &MenuTexts::default()).unwrap();
        assert!(is_registered(&registry, exe(), &MenuTexts::default()));
        unregister(&mut registry).unwrap();
        assert!(!is_registered(&registry, exe(), &MenuTexts::default()));
        assert_eq!(registry.values.len(), 1);
        assert_eq!(registry.get(r"Software\Classes\*\shell\other-program", "MUIVerb").as_deref(), Some("keep me"));
        unregister(&mut registry).unwrap();
    }

    #[test]
    fn a_registry_that_refuses_is_an_error_and_the_menu_is_not_taken_for_registered() {
        let mut registry = Fake { fail_on: Some(format!(r"{ROOT}\shell\3-open-default\command")), ..Fake::default() };
        let result = register(&mut registry, exe(), &MenuTexts::default());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(!is_registered(&registry, exe(), &MenuTexts::default()));
    }
}
