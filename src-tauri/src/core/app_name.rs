//! The name the program calls itself in what the user sees: the right-click menu of files, the title
//! of its windows, the sidebar, the tray. The user may give it another (a setting). What the program
//! is called in the places that identify it (the executable, the installer, the folders of the data and of
//! the Start menu) does not follow it.

use serde::{Deserialize, Serialize};

/// The name the program has when the user did not choose one and the language pack has no other.
pub const DEFAULT_APP_NAME: &str = "ScrcpyAgent";

/// The most characters a name may have.
pub const MAX_APP_NAME_CHARS: usize = 40;

/// Characters that cannot be in the name of a file on one system or another; the name becomes part of
/// the names of files of the file managers of Linux, so it cannot have them.
const NOT_IN_A_FILE_NAME: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// A name the user gave the program: without the spaces round it, 1 to 40 characters, no control
/// character and none of `/ \ : * ? " < > |`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppName(String);

impl AppName {
    /// `None` for text that is not a name.
    pub fn parse(text: &str) -> Option<AppName> {
        let name = text.trim();
        let usable = !name.is_empty()
            && name.chars().count() <= MAX_APP_NAME_CHARS
            && !name.chars().any(|c| c.is_control() || NOT_IN_A_FILE_NAME.contains(&c));
        usable.then(|| AppName(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AppName {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        AppName::parse(&text).ok_or_else(|| format!("{text:?} is not usable as the name of the program"))
    }
}

impl From<AppName> for String {
    fn from(name: AppName) -> String {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_to_forty_characters_without_the_spaces_round_it() {
        for good in ["ScrcpyAgent", "My tool", "Küche", "a", "x y z", "Dev-Kit_2", "über (test)"] {
            assert_eq!(AppName::parse(good).as_ref().map(AppName::as_str), Some(good), "{good:?}");
        }
        assert_eq!(AppName::parse("  Kitchen  ").as_ref().map(AppName::as_str), Some("Kitchen"));
        let longest = "n".repeat(MAX_APP_NAME_CHARS);
        assert!(AppName::parse(&longest).is_some());
        // Characters, not bytes.
        assert!(AppName::parse(&"ü".repeat(MAX_APP_NAME_CHARS)).is_some());
    }

    #[test]
    fn a_text_that_cannot_be_a_name_is_not_one() {
        let too_long = "n".repeat(MAX_APP_NAME_CHARS + 1);
        let mut bad: Vec<String> = ["", " ", "   ", "a\nb", "a\tb", "a\0b", "\u{7f}"].iter().map(|s| s.to_string()).collect();
        bad.extend(NOT_IN_A_FILE_NAME.iter().map(|c| format!("a{c}b")));
        bad.push(too_long);
        bad.push("ü".repeat(MAX_APP_NAME_CHARS + 1));
        for text in &bad {
            assert_eq!(AppName::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_name_is_written_as_the_text_it_is_and_only_such_a_text_is_read_back() {
        let name = AppName::parse("Kitchen").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), r#""Kitchen""#);
        assert_eq!(serde_json::from_str::<AppName>(r#"" Kitchen ""#).unwrap(), name);
        assert!(serde_json::from_str::<AppName>(r#""a/b""#).is_err());
        assert!(serde_json::from_str::<AppName>(r#""""#).is_err());
        assert!(serde_json::from_str::<AppName>("5").is_err());
        assert_eq!(DEFAULT_APP_NAME, "ScrcpyAgent");
        assert!(AppName::parse(DEFAULT_APP_NAME).is_some());
    }
}
