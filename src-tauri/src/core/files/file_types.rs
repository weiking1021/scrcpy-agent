//! The kinds of files that add-ons name: an extension and the media type it stands
//! for, used to open a file on a device when the system has no type for it (see
//! `openers::open_type`). An add-on that works with an app of its own on the device
//! registers the extension of its files here, so the device is asked for that app
//! instead of for every app that can show a file.

use super::openers::MimeType;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTypeError {
    /// The add-on's name is not a name (see `buttons::valid_name`).
    InvalidAddon,
    /// An extension is 1 to 16 letters and digits, without the dot.
    InvalidExtension,
    /// A media type such as `text/plain`, not a pattern (`*/*`) and not one with parameters.
    InvalidType,
    /// An add-on already named this extension.
    AlreadyRegistered,
}

/// The longest extension that is a plain one.
pub const MAX_EXTENSION: usize = 16;

/// An extension in lower case if it is a plain one: 1 to `MAX_EXTENSION` letters and
/// digits, without the dot.
pub fn plain(extension: &str) -> Option<String> {
    let ok = !extension.is_empty() && extension.len() <= MAX_EXTENSION && extension.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| extension.to_ascii_lowercase())
}

/// The extensions that add-ons have named, each with the add-on that did it.
#[derive(Debug, Default)]
pub struct FileTypes {
    entries: BTreeMap<String, (String, MimeType)>,
}

impl FileTypes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Names the type of the files with this extension (in any case).
    pub fn register(&mut self, addon: &str, extension: &str, mime: &str) -> Result<(), FileTypeError> {
        if !crate::core::addons::buttons::valid_name(addon) {
            return Err(FileTypeError::InvalidAddon);
        }
        let extension = plain(extension).ok_or(FileTypeError::InvalidExtension)?;
        let mime = MimeType::parse(mime).ok_or(FileTypeError::InvalidType)?;
        if self.entries.contains_key(&extension) {
            return Err(FileTypeError::AlreadyRegistered);
        }
        self.entries.insert(extension, (addon.to_string(), mime));
        Ok(())
    }

    /// Takes back everything an add-on named.
    pub fn remove_addon(&mut self, addon: &str) {
        self.entries.retain(|_, (owner, _)| owner != addon);
    }

    /// The type named for this extension (in any case), if one was.
    pub fn get(&self, extension: &str) -> Option<MimeType> {
        self.entries.get(&plain(extension)?).map(|(_, mime)| mime.clone())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mime(text: &str) -> Option<MimeType> {
        MimeType::parse(text)
    }

    #[test]
    fn a_type_that_was_named_is_found_for_the_extension_in_any_case() {
        let mut types = FileTypes::new();
        assert_eq!(types.get("log"), None);
        types.register("viewer", "Log", "text/plain").unwrap();
        for extension in ["log", "LOG", "Log"] {
            assert_eq!(types.get(extension), mime("text/plain"), "{extension}");
        }
        assert_eq!(types.get("logs"), None);
        assert_eq!(types.get("lo"), None);
        assert_eq!(types.len(), 1);
    }

    #[test]
    fn an_extension_that_is_not_plain_or_a_type_that_is_not_one_is_refused_and_nothing_is_kept() {
        let mut types = FileTypes::new();
        for extension in ["", ".log", "tx t", "a.b", "jäp", "abcdefghijklmnopq", "lo/g"] {
            assert_eq!(types.register("viewer", extension, "text/plain"), Err(FileTypeError::InvalidExtension), "{extension:?}");
        }
        for bad in ["", "text", "*/*", "text/*", "text/plain; charset=utf-8", "te xt/plain"] {
            assert_eq!(types.register("viewer", "log", bad), Err(FileTypeError::InvalidType), "{bad:?}");
        }
        for addon in ["", " ", "has space", "a/b"] {
            assert_eq!(types.register(addon, "log", "text/plain"), Err(FileTypeError::InvalidAddon), "{addon:?}");
        }
        assert!(types.is_empty());
        // The longest extension that is allowed.
        types.register("viewer", "abcdefghijklmnop", "text/plain").unwrap();
    }

    #[test]
    fn an_extension_is_named_once_whoever_asks_again() {
        let mut types = FileTypes::new();
        types.register("viewer", "log", "text/plain").unwrap();
        assert_eq!(types.register("other", "LOG", "text/x-log"), Err(FileTypeError::AlreadyRegistered));
        assert_eq!(types.register("viewer", "log", "text/plain"), Err(FileTypeError::AlreadyRegistered));
        // What was named first stays.
        assert_eq!(types.get("log"), mime("text/plain"));
    }

    #[test]
    fn taking_an_addon_back_removes_what_it_named_and_nothing_else() {
        let mut types = FileTypes::new();
        types.register("viewer", "log", "text/plain").unwrap();
        types.register("viewer", "nfo", "text/plain").unwrap();
        types.register("other", "dat", "application/x-own").unwrap();
        types.remove_addon("viewer");
        assert_eq!(types.get("log"), None);
        assert_eq!(types.get("nfo"), None);
        assert_eq!(types.get("dat"), mime("application/x-own"));
        // The extension can be named again.
        types.register("third", "log", "text/x-log").unwrap();
        types.remove_addon("nobody");
        assert_eq!(types.len(), 2);
    }
}
