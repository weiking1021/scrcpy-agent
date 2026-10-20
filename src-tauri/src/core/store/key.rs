//! How the data folder names a device: its id in hexadecimal.
//!
//! An id is whatever the system calls the device (`TESTDEV…`, `192.168.1.5:40000`, an mDNS
//! name with dots and colons), so it cannot be a name of a folder as it is: it may have a
//! path separator, `..`, or be the same as another id on a system that ignores case.
//! The hexadecimal text of its bytes has none of that, and gives the id back.

/// The name of a device's place in the data folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceKey(String);

impl DeviceKey {
    /// The key of a device id; `None` for an empty id (it has no name to be put under).
    pub fn from_id(id: &str) -> Option<DeviceKey> {
        (!id.is_empty()).then(|| DeviceKey(id.bytes().map(|b| format!("{b:02x}")).collect()))
    }

    /// The key a folder name stands for: lower case hexadecimal digits in pairs that are the
    /// bytes of a text, nothing else. A name that is not exactly one made by `from_id` gives `None`.
    pub fn from_hex(name: &str) -> Option<DeviceKey> {
        let digits = name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        (!name.is_empty() && name.len().is_multiple_of(2) && digits && decode(name).is_some()).then(|| DeviceKey(name.to_string()))
    }

    /// The device id this is the key of.
    pub fn id(&self) -> String {
        decode(&self.0).expect("a key is the hexadecimal text of a text")
    }

    /// The text that is the name of the folder.
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

/// The text whose bytes are these hexadecimal digits (in pairs); `None` if they are not valid text.
fn decode(hex: &str) -> Option<String> {
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_the_id_in_hexadecimal_and_gives_the_id_back() {
        for id in ["TESTDEV0001", "192.168.1.5:40000", "adb-x._adb-tls-connect._tcp.", "../../x/y", "Gerät Ü", "A"] {
            let key = DeviceKey::from_id(id).unwrap();
            assert_eq!(key.id(), id);
            assert!(
                key.as_hex().bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{} has to be hexadecimal digits and nothing else",
                key.as_hex()
            );
            assert_eq!(DeviceKey::from_hex(key.as_hex()), Some(key));
        }
        assert_eq!(DeviceKey::from_id("AB").unwrap().as_hex(), "4142");
    }

    #[test]
    fn two_ids_never_have_the_same_key_even_when_they_differ_only_in_case() {
        let a = DeviceKey::from_id("abc").unwrap();
        let b = DeviceKey::from_id("ABC").unwrap();
        assert_ne!(a, b);
        assert_ne!(a.as_hex(), b.as_hex());
    }

    #[test]
    fn an_id_that_is_empty_has_no_key() {
        assert_eq!(DeviceKey::from_id(""), None);
    }

    #[test]
    fn a_name_is_a_key_only_if_a_key_made_it() {
        for bad in ["", "4", "4g", "4A42", "4142 ", " 4142", "41 42", "../4142", "4142/", "ff", "c3", "4142.json"] {
            assert_eq!(DeviceKey::from_hex(bad), None, "{bad:?}");
        }
        // Digits that are the bytes of a text are one.
        assert_eq!(DeviceKey::from_hex("4142").unwrap().id(), "AB");
    }
}
