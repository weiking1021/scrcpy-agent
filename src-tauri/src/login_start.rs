//! The start at the login on this system: on Windows the `Run` key of the current user, on Linux
//! the `autostart` folder of the user (`core::integration::autostart` says what is written).

use crate::core::error::CoreError;
use crate::core::integration::autostart::{self, Autostart};
#[cfg(not(windows))]
use crate::core::window::layout::Os;

#[cfg(windows)]
mod registry {
    use crate::core::integration::autostart::RunRegistry;
    use std::io;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    /// The current user's part of the registry: no administrator is needed.
    pub struct UserRegistry;

    impl RunRegistry for UserRegistry {
        fn set(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key)?;
            key.set_value(name, &value)
        }

        fn get(&self, key: &str, name: &str) -> Option<String> {
            let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_READ).ok()?;
            key.get_value::<String, _>(name).ok()
        }

        fn remove(&mut self, key: &str, name: &str) -> io::Result<()> {
            let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_SET_VALUE) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                other => other?,
            };
            match key.delete_value(name) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        }
    }
}

/// The entry of this system, or `None` where there is none (no `autostart` folder to find).
#[cfg(windows)]
fn entry() -> Option<impl Autostart> {
    Some(autostart::RunKey(registry::UserRegistry))
}

#[cfg(not(windows))]
fn entry() -> Option<impl Autostart> {
    if Os::current() != Os::Linux {
        return None;
    }
    let folder = autostart::autostart_folder(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    Some(autostart::AutostartFile { folder })
}

/// Whether this system has a start at the login to write.
pub fn available() -> bool {
    entry().is_some()
}

/// Writes the entry (`wanted`) for the program that is running, or removes it.
pub fn apply(wanted: bool) -> Result<(), CoreError> {
    let Some(mut entry) = entry() else {
        // Nothing to remove where there is nothing to write.
        return if wanted { Err(CoreError::Unavailable) } else { Ok(()) };
    };
    let exe = std::env::current_exe().map_err(|e| CoreError::Autostart(e.to_string()))?;
    autostart::apply(&mut entry, wanted, &exe)
}
