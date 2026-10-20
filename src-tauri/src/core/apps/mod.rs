//! The apps on a device, as scrcpy lists them, and what is needed to open one.

pub mod service;

use serde::Serialize;

/// An Android package name such as `com.android.chrome`: dot-separated parts,
/// at least two, each starting with a letter and made of letters, digits and
/// `_`, at most 255 characters in all. Only such a name is ever given to scrcpy
/// as the app to start: scrcpy reads a leading `+` or `?` in the same argument
/// as an instruction (stop the app first; find it by name), which must never
/// come from what a user or an add-on typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageName(String);

/// Android's limit for the length of a package name.
const MAX_PACKAGE_LEN: usize = 255;

impl PackageName {
    pub fn parse(text: &str) -> Option<Self> {
        let part_ok = |part: &str| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        let valid = text.len() <= MAX_PACKAGE_LEN && text.split('.').count() >= 2 && text.split('.').all(part_ok);
        valid.then(|| PackageName(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a running scrcpy window is called inside the core: the id of the device
/// for its own screen, and the id followed by `#app:` and the package for the
/// window of an app. The toolbar of a window and the control of its screen go by
/// this key; the device a key belongs to is read from it exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKey {
    pub device: String,
    pub app: Option<PackageName>,
}

const APP_MARK: &str = "#app:";

impl SessionKey {
    /// A key is an app's if it ends with `#app:` and a package name after a
    /// device id that is not empty; anything else is the device's own.
    pub fn parse(key: &str) -> Self {
        if let Some((device, package)) = key.rsplit_once(APP_MARK)
            && !device.is_empty()
            && let Some(app) = PackageName::parse(package)
        {
            return SessionKey { device: device.to_string(), app: Some(app) };
        }
        SessionKey { device: key.to_string(), app: None }
    }

    /// The key of the window of an app on a device.
    pub fn of_app(device: &str, package: &PackageName) -> String {
        format!("{device}{APP_MARK}{}", package.as_str())
    }
}

/// One app as listed by scrcpy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AppEntry {
    pub package: String,
    /// The name the device shows for it.
    pub name: String,
    /// An app that came with the system (scrcpy marks it with `*`; the others
    /// with `-`). The meaning of the marks is read from the lists of a real
    /// device: the marked ones are the apps of the system.
    pub system: bool,
}

/// The apps of one device as they were when last listed; kept in the data
/// folder so the list can be shown at once while a new one is being read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AppCache {
    /// Seconds since 1970; 0 if it has never been listed.
    pub updated_at: u64,
    pub apps: Vec<AppEntry>,
}

/// Where an app is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDisplay {
    /// On a display of its own that scrcpy creates, so the device's own screen
    /// is left alone.
    VirtualDisplay,
    /// On the device's screen, which scrcpy mirrors.
    MainDisplay,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_package_names_of_all_the_shapes_seen_are_accepted() {
        for name in [
            "com.android.chrome",
            "de.szalkowski.activitylauncher",
            // A web app installed from the browser: a long hexadecimal part and an underscore.
            "org.chromium.webapk.a66bd31f0361b04a5_v2",
            "easynotes.notes.notepad.notebook.privatenotes.note",
            "jp.co.celsys.clipstudiopaint.googleplay",
            "com.Reflektone.AstroDX",
            "a.b",
        ] {
            assert_eq!(PackageName::parse(name).map(|p| p.as_str().to_string()), Some(name.to_string()), "{name}");
        }
    }

    #[test]
    fn anything_else_is_not_a_package_name_and_never_reaches_scrcpy() {
        let too_long = format!("a.{}", "b".repeat(254));
        for bad in [
            "", "com", ".com.a", "com..a", "com.a.", "1com.a", "com.1a", "com.a-b", "com.a b", "com.a\n", " com.a",
            // scrcpy's own instructions in the same argument.
            "+com.android.chrome", "?Chrome", "?com.a.b", "+", "?",
            // Not ASCII.
            "com.é.a", "com.ß.a", "com.a.\u{ff21}",
            // One character too many.
            too_long.as_str(),
        ] {
            assert_eq!(PackageName::parse(bad), None, "{bad:?}");
        }
        // 255 characters are still allowed.
        let longest = format!("a.{}", "b".repeat(253));
        assert!(PackageName::parse(&longest).is_some());
    }

    #[test]
    fn a_session_key_is_a_device_or_a_device_and_an_app_and_gives_back_exactly_that() {
        let notes = PackageName::parse("com.example.notes").unwrap();
        let own = SessionKey::parse("TESTDEV0001");
        assert_eq!((own.device.as_str(), own.app.as_ref()), ("TESTDEV0001", None));
        let key = SessionKey::of_app("TESTDEV0001", &notes);
        assert_eq!(key, "TESTDEV0001#app:com.example.notes");
        let app = SessionKey::parse(&key);
        assert_eq!((app.device.as_str(), app.app), ("TESTDEV0001", Some(notes.clone())));
        // The last mark counts, so a device id that has one in it still works.
        let odd = SessionKey::parse("a#app:b.c#app:d.e");
        assert_eq!((odd.device.as_str(), odd.app.map(|p| p.as_str().to_string())), ("a#app:b.c", Some("d.e".into())));
    }

    #[test]
    fn what_does_not_end_in_a_mark_and_a_package_is_the_devices_own_key() {
        for key in ["A1#app:", "A1#app:bad", "A1#app:com.example.", "A1#app:+com.example.a", "#app:com.example.notes", "A1#APP:com.example.notes", "A1 #app: com.example.notes", ""] {
            let parsed = SessionKey::parse(key);
            assert_eq!((parsed.device.as_str(), parsed.app), (key, None), "{key:?}");
        }
    }

    #[test]
    fn where_an_app_opens_is_written_by_name() {
        assert_eq!(serde_json::to_value(AppDisplay::VirtualDisplay).unwrap(), "virtual_display");
        assert_eq!(serde_json::from_str::<AppDisplay>("\"main_display\"").unwrap(), AppDisplay::MainDisplay);
        assert!(serde_json::from_str::<AppDisplay>("\"elsewhere\"").is_err());
    }
}
