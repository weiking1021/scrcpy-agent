//! What an add-on may ask to be allowed, and the check that is made each time it uses a part of the
//! interface that needs one.
//!
//! The rule for the interface: every kind of call that can do more than register something names the
//! permission it needs, and is refused without it (`Granted::check`). A permission is added with the
//! first call that needs it, not before.

use serde::{Deserialize, Serialize};

/// Something an add-on asks to be allowed. The user is told of each when the add-on is installed, and
/// can leave any of them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "AddonPermission.ts"))]
pub enum AddonPermission {
    /// Runs shell commands on a device.
    DeviceShell,
    /// Starts programs of its own on this computer, with everything the user may do there: it can read
    /// and write files, use the network, start other programs and open windows.
    RunProcess,
    /// Gets the live picture of the screen of a device (what is on it: messages, passwords, anything).
    ScreenCapture,
}

impl AddonPermission {
    pub const ALL: [AddonPermission; 3] =
        [AddonPermission::DeviceShell, AddonPermission::RunProcess, AddonPermission::ScreenCapture];
}

/// A call was refused because the add-on was not given this permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("the add-on was not given the permission {0:?}")]
pub struct PermissionDenied(pub AddonPermission);

/// The permissions an add-on has been given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Granted(Vec<AddonPermission>);

impl Granted {
    pub fn new(permissions: &[AddonPermission]) -> Granted {
        Granted(permissions.to_vec())
    }

    /// Whether a call that needs `permission` may be made.
    pub fn check(&self, permission: AddonPermission) -> Result<(), PermissionDenied> {
        if self.0.contains(&permission) { Ok(()) } else { Err(PermissionDenied(permission)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_allowed_only_with_the_permission_it_needs() {
        let shell_only = Granted::new(&[AddonPermission::DeviceShell]);
        assert_eq!(shell_only.check(AddonPermission::DeviceShell), Ok(()));
        assert_eq!(shell_only.check(AddonPermission::RunProcess), Err(PermissionDenied(AddonPermission::RunProcess)));
        let process_only = Granted::new(&[AddonPermission::RunProcess]);
        assert_eq!(process_only.check(AddonPermission::RunProcess), Ok(()));
        assert_eq!(process_only.check(AddonPermission::DeviceShell), Err(PermissionDenied(AddonPermission::DeviceShell)));
    }

    #[test]
    fn nothing_given_means_nothing_allowed_and_everything_given_means_everything_is() {
        for permission in AddonPermission::ALL {
            assert_eq!(Granted::default().check(permission), Err(PermissionDenied(permission)), "{permission:?}");
            assert_eq!(Granted::new(&AddonPermission::ALL).check(permission), Ok(()), "{permission:?}");
        }
    }

    #[test]
    fn the_names_in_a_file_are_exactly_these_and_nothing_else_is_a_permission() {
        assert_eq!(serde_json::to_string(&AddonPermission::DeviceShell).unwrap(), r#""device_shell""#);
        assert_eq!(serde_json::to_string(&AddonPermission::RunProcess).unwrap(), r#""run_process""#);
        for name in ["device_shell", "run_process"] {
            let parsed = serde_json::from_str::<AddonPermission>(&format!("\"{name}\""));
            assert!(parsed.is_ok(), "{name}");
        }
        for name in ["", "root", "Device_Shell", "device-shell", "run_process "] {
            let parsed = serde_json::from_str::<AddonPermission>(&format!("\"{name}\""));
            assert!(parsed.is_err(), "{name:?}");
        }
    }

    #[test]
    fn the_screen_is_a_permission_of_its_own_and_every_permission_is_in_the_list_of_all() {
        assert_eq!(serde_json::to_string(&AddonPermission::ScreenCapture).unwrap(), r#""screen_capture""#);
        assert_eq!(
            AddonPermission::ALL.to_vec(),
            vec![AddonPermission::DeviceShell, AddonPermission::RunProcess, AddonPermission::ScreenCapture]
        );
        let process_only = Granted::new(&[AddonPermission::RunProcess]);
        assert_eq!(
            process_only.check(AddonPermission::ScreenCapture),
            Err(PermissionDenied(AddonPermission::ScreenCapture))
        );
    }

    #[test]
    fn a_refusal_names_the_permission_that_was_missing() {
        assert_eq!(
            serde_json::to_string(&PermissionDenied(AddonPermission::RunProcess)).unwrap(),
            r#""run_process""#
        );
    }
}
