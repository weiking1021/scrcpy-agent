//! Every kind of data the program keeps, in one place: its scope, its value and the name
//! of its file. A new kind is one more line here; the name is checked by a test (valid,
//! and not used twice in its scope).

use super::scope::{Data, Device, Global};
use crate::core::addons::library::InstalledAddons;
use crate::core::apps::AppCache;
use crate::core::model::{AppUsage, DeviceSettings, Settings as GeneralSettings};
use crate::core::integration::shortcuts::ShortcutRecords;
use serde::{Deserialize, Serialize};

/// What is known about a device itself, as far as this computer keeps it. (What is set for it,
/// what the user marked and the lists of apps are kinds of their own: they change at other
/// times, and a broken one does not take the others away.)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    pub id: String,
    pub model: String,
    pub label: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<u32>,
    pub last_address: Option<String>,
    pub last_seen: u64,
    /// When this computer first saw the device (seconds since 1970); 0 for a device that was
    /// known before this was recorded.
    pub first_seen: u64,
}

macro_rules! kinds {
    ($($(#[$doc:meta])* $name:ident: $scope:ident, $file:literal => $value:ty;)*) => {
        $(
            $(#[$doc])*
            pub struct $name;
            impl Data for $name {
                type Scope = $scope;
                type Value = $value;
                const NAME: &'static str = $file;
            }
        )*
        /// Every kind as (scope, name of its file): for the test that checks the names.
        #[cfg(test)]
        pub(super) const ALL: &[(&str, &str)] = &[$((stringify!($scope), $file)),*];
    };
}

kinds! {
    /// The general settings.
    Settings: Global, "settings" => GeneralSettings;
    /// What is known about the device itself.
    Profile: Device, "profile" => DeviceProfile;
    /// What is set for the device alone.
    Options: Device, "settings" => DeviceSettings;
    /// The apps of the device the user marked and the ones opened lately.
    Usage: Device, "usage" => AppUsage;
    /// The list of the apps of the device, as it was last read.
    Apps: Device, "apps" => AppCache;
    /// The shortcuts that were made for the apps of the device.
    Shortcuts: Device, "shortcuts" => ShortcutRecords;
    /// The add-ons that are installed, and the folders they are linked from.
    Installed: Global, "installed-addons" => InstalledAddons;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_name_of_a_kind_is_a_plain_word_and_is_not_used_twice_in_its_scope() {
        let mut seen = HashSet::new();
        for (scope, name) in ALL {
            assert!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{name:?} is not a plain lower case name"
            );
            assert!(seen.insert((*scope, *name)), "{name:?} is used twice in {scope}");
        }
        assert!(ALL.len() >= 5);
    }
}
