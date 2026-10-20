//! The version of a scrcpy release. The client and the server it starts must be
//! the same version, and the internal protocol differs between versions, so the
//! version decides which protocol adapter is used (`scrcpy_protocol`).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScrcpyVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ScrcpyVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        ScrcpyVersion { major, minor, patch }
    }

    /// Parses `5.0.1` or the release tag `v5.0.1`: three numbers of plain
    /// digits, nothing else.
    pub fn parse(text: &str) -> Option<Self> {
        let numbers = text.strip_prefix('v').unwrap_or(text);
        // `parse` also accepts a leading `+` and non-ASCII digits are not
        // digits here; only plain ASCII digits make a number.
        let mut parts = numbers.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse::<u32>().ok()).flatten()
        });
        let version = ScrcpyVersion::new(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for ScrcpyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_tag_and_a_plain_version_are_the_same_version() {
        assert_eq!(ScrcpyVersion::parse("v5.0.1"), Some(ScrcpyVersion::new(5, 0, 1)));
        assert_eq!(ScrcpyVersion::parse("5.0.1"), Some(ScrcpyVersion::new(5, 0, 1)));
        assert_eq!(ScrcpyVersion::parse("v10.20.30"), Some(ScrcpyVersion::new(10, 20, 30)));
    }

    #[test]
    fn anything_that_is_not_exactly_three_plain_numbers_is_not_a_version() {
        for bad in [
            "", "v", "5", "5.0", "v5.0", "5.0.1.2", "5.0.x", "5..1", "+5.0.1", "5.0.-1", " 5.0.1", "5.0.1 ", "V5.0.1",
            "vv5.0.1", "5.0.1-rc1", "٥.٠.١", "5.0.99999999999",
        ] {
            assert_eq!(ScrcpyVersion::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_are_ordered_by_their_numbers_not_their_text() {
        let v = |t: &str| ScrcpyVersion::parse(t).unwrap();
        assert!(v("5.0.1") < v("5.0.10"));
        assert!(v("5.9.9") < v("5.10.0"));
        assert!(v("4.99.99") < v("5.0.0"));
        assert_eq!(v("v5.0.1").to_string(), "5.0.1");
    }
}
