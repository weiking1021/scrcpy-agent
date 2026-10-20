//! Errors that cross the boundary to the UI. They are typed: the UI switches
//! on `code`, it never inspects message text. Text carried in `detail` is raw
//! output meant for display only.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum CoreError {
    #[error("scrcpy is not ready yet")]
    NotReady,
    #[error("invalid address, expected an IP address and port")]
    InvalidAddress,
    #[error("the pairing code must be 6 digits")]
    InvalidPairingCode,
    /// The device is alive but refused the connection: it is not paired.
    #[error("the device is not paired with this computer")]
    NotPaired,
    #[error("the device cannot be reached")]
    Unreachable,
    /// Something is connecting to this device already (the same device, whichever port it is at).
    #[error("a connection to this device is going on")]
    ConnectInProgress,
    #[error("unknown device")]
    UnknownDevice,
    #[error("the device is not ready")]
    NotOnline,
    #[error("the screen of this device is already open")]
    AlreadyOpen,
    /// The button exists but has no action of its own (yet).
    #[error("this button has no action")]
    NotAvailable,
    /// The device did not show up after pairing. Carries adb's own output so
    /// the user can read it; nothing branches on it.
    #[error("pairing could not be confirmed: {0}")]
    PairNotConfirmed(String),
    /// This device's screen is not open with a proxy: it is not open, or it was
    /// opened without the proxy (so there is no video to follow and no way to
    /// send it control messages).
    #[error("this device has no proxy")]
    NoProxy,
    /// The screen has a proxy, but not a control socket to send messages on.
    #[error("the control channel of this device is not available")]
    ControlUnavailable,
    /// The package name is not one (see `apps::PackageName`).
    #[error("not a package name")]
    InvalidPackage,
    /// This scrcpy version has not been checked, so what it needs to list and
    /// open apps is not known.
    #[error("this scrcpy version is not supported for this")]
    UnsupportedScrcpy,
    /// scrcpy could not list the apps. Carries what it printed, for showing.
    #[error("the apps could not be listed: {0}")]
    AppListFailed(String),
    /// The name is empty, longer than 64 characters or has control characters.
    #[error("not a usable name")]
    InvalidLabel,
    /// The folder files are sent to (a setting) is not a usable path on the device.
    #[error("not a usable folder on the device")]
    InvalidDirectory,
    /// What was to be sent is not a file that can be read (a folder, or gone).
    #[error("not a file that can be sent")]
    NotAFile,
    /// The entries of the Explorer menu could not be written or removed (the text
    /// is the system's, for showing only).
    #[error("the Explorer menu could not be changed: {0}")]
    ExplorerMenu(String),
    /// The entry that starts the program at the login could not be written or removed (the
    /// text is the system's, for showing only).
    #[error("the start at the login could not be changed: {0}")]
    Autostart(String),
    /// A release of scrcpy could not be listed, offered or downloaded.
    #[error("the release of scrcpy could not be fetched")]
    ScrcpyRelease(super::scrcpy::release::ReleaseFailure),
    /// The release of scrcpy that was asked for is not installed.
    #[error("that release of scrcpy is not installed")]
    ScrcpyNotInstalled,
    /// The release of scrcpy cannot be changed while screens are open.
    #[error("screens are open")]
    ScreensOpen,
    /// The adb of the new release could not bring its server up (the text is the system's, for showing only).
    #[error("adb could not be started: {0}")]
    AdbNotStarted(String),
    /// An add-on could not be installed or its permissions decided (see `InstallError`).
    #[error("the add-on could not be installed: {0:?}")]
    Addon(super::addons::library::InstallError),
    /// A shortcut could not be written (the text is the system's, for showing only).
    #[error("the shortcut could not be made: {0}")]
    Shortcut(String),
    /// Data could not be written to the data folder (the text is the system's, for showing only).
    #[error("the data could not be saved: {0}")]
    Storage(String),
    /// This system has no Explorer menu to register.
    #[error("not available on this system")]
    Unavailable,
    /// No app of the device can open this kind of file.
    #[error("no app of the device can open this kind of file")]
    NoApp,
    /// The name of the file cannot be a name on the device.
    #[error("the name of the file cannot be used on the device")]
    BadFileName,
    /// A command ran on the device and ended with a non-zero exit code.
    #[error("the command failed on the device (exit code {0})")]
    CommandFailed(u8),
    /// The add-on declares no such setting.
    #[error("the add-on has no such setting")]
    UnknownSetting,
    /// The value is not one the setting allows (and why).
    #[error("the value is not allowed for this setting: {0:?}")]
    InvalidSetting(super::addons::settings::ValueProblem),
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    /// The code the error is written out with (`not_online`): what the UI and the add-ons switch on.
    pub fn code(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.get("code").and_then(serde_json::Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "other".to_string())
    }
}

impl From<super::adb::api::AdbError> for CoreError {
    fn from(e: super::adb::api::AdbError) -> Self {
        CoreError::Other(e.to_string())
    }
}

impl From<anyhow::Error> for CoreError {
    fn from(e: anyhow::Error) -> Self {
        CoreError::Other(format!("{e:#}"))
    }
}
