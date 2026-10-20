//! The interface an add-on sees, described by Rust types: the one source of the TypeScript types in `addon-api/`.
//!
//! What can be said as a type is said here, and `ts-rs` writes it as TypeScript (`npm run gen:addon-types`, which runs the
//! tests that export): the permissions, what each event carries, and the codes of each error. The program uses these
//! same types to make what the script receives (the payload of an event, the `code` of an error), so the types an
//! author reads and what arrives cannot differ. What cannot be a Rust type (the functions of `host`, written in
//! `prelude.js`) is listed in `HOST_MEMBERS`; a test compares that list with the object a script really gets, and the
//! check of the TypeScript side compares it with `addon-api/index.d.ts`.

use crate::core::addons::menu::MenuRegisterError;
use crate::core::addons::process::SpawnError;
use crate::core::addons::screen_pipe::PipeError;
use crate::core::addons::settings::SettingValue;
use serde::Serialize;
use serde_json::Value;

/// The functions of `host`, by path (`deviceMenu.register`). The TypeScript side checks that `Host` in
/// `addon-api/index.d.ts` has exactly these, and a test checks that the object a script gets has exactly these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "HostMember.ts"))]
pub enum HostMember {
    #[serde(rename = "addon.folder")]
    AddonFolder,
    #[serde(rename = "appMenu.register")]
    AppMenuRegister,
    #[serde(rename = "deviceMenu.register")]
    DeviceMenuRegister,
    #[serde(rename = "deviceShell")]
    DeviceShell,
    #[serde(rename = "log")]
    Log,
    #[serde(rename = "on")]
    On,
    #[serde(rename = "process.spawn")]
    ProcessSpawn,
    #[serde(rename = "screen.pipeTo")]
    ScreenPipeTo,
    #[serde(rename = "settings.get")]
    SettingsGet,
    #[serde(rename = "toolbar.register")]
    ToolbarRegister,
}

impl HostMember {
    pub const ALL: [HostMember; 10] = [
        HostMember::AddonFolder,
        HostMember::AppMenuRegister,
        HostMember::DeviceMenuRegister,
        HostMember::DeviceShell,
        HostMember::Log,
        HostMember::On,
        HostMember::ProcessSpawn,
        HostMember::ScreenPipeTo,
        HostMember::SettingsGet,
        HostMember::ToolbarRegister,
    ];
}

/// The path of every function of `host`, sorted.
pub fn host_members() -> Vec<String> {
    let mut paths: Vec<String> = HostMember::ALL.iter().map(code_text).collect();
    paths.sort_unstable();
    paths
}

/// The code of a `MenuRegisterError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "MenuRegisterCode.ts"))]
pub enum MenuRegisterCode {
    InvalidId,
    InvalidLabel,
    AlreadyRegistered,
}

impl From<MenuRegisterError> for MenuRegisterCode {
    fn from(error: MenuRegisterError) -> Self {
        match error {
            MenuRegisterError::InvalidId => MenuRegisterCode::InvalidId,
            MenuRegisterError::InvalidLabel => MenuRegisterCode::InvalidLabel,
            MenuRegisterError::AlreadyRegistered => MenuRegisterCode::AlreadyRegistered,
        }
    }
}

/// The code of a `ProcessError`: why a program was not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "ProcessErrorCode.ts"))]
pub enum ProcessErrorCode {
    InvalidCommand,
    NotFound,
    NotAllowed,
    TooMany,
    Failed,
}

impl From<SpawnError> for ProcessErrorCode {
    fn from(error: SpawnError) -> Self {
        match error {
            SpawnError::InvalidCommand => ProcessErrorCode::InvalidCommand,
            SpawnError::NotFound => ProcessErrorCode::NotFound,
            SpawnError::NotAllowed => ProcessErrorCode::NotAllowed,
            SpawnError::TooMany => ProcessErrorCode::TooMany,
            SpawnError::Failed => ProcessErrorCode::Failed,
        }
    }
}

/// The code of a `ScreenError`: why a screen was not fed to a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS), ts(export, export_to = "ScreenErrorCode.ts"))]
pub enum ScreenErrorCode {
    NoScreen,
    NoProcess,
}

impl From<PipeError> for ScreenErrorCode {
    fn from(error: PipeError) -> Self {
        match error {
            PipeError::NoScreen => ScreenErrorCode::NoScreen,
            PipeError::NoProcess => ScreenErrorCode::NoProcess,
        }
    }
}

/// What `device.online`, `device.offline` and `device.added` carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS, Default), ts(export, export_to = "DeviceEventData.ts"))]
pub struct DeviceEventData {
    /// The id of the device (not its serial number).
    pub device: String,
    /// What the program calls the device: the user's own name for it, else its model.
    pub name: String,
}

/// What `screen.started` and `screen.ended` carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS, Default), ts(export, export_to = "ScreenEventData.ts"))]
pub struct ScreenEventData {
    pub device: String,
    /// The package for the window of an app, null for the screen of the device.
    pub app: Option<String>,
}

/// What `files.sent` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS, Default), ts(export, export_to = "FilesSentEventData.ts"))]
pub struct FilesSentEventData {
    pub device: String,
    pub name: String,
    pub ok: bool,
    /// The code of why it was not sent, else null.
    pub error: Option<String>,
}

/// What `files.opened` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS, Default), ts(export, export_to = "FilesOpenedEventData.ts"))]
pub struct FilesOpenedEventData {
    pub device: String,
    pub file: String,
    pub ok: bool,
    /// The code of why it was not opened, else null.
    pub error: Option<String>,
}

/// What `settings.changed` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS, Default), ts(export, export_to = "SettingChangedEventData.ts"))]
pub struct SettingChangedEventData {
    /// The id of the setting, as the add-on declared it.
    pub key: String,
    /// The value it has now.
    pub value: SettingValue,
}

#[cfg(test)]
impl Default for SettingValue {
    fn default() -> Self {
        SettingValue::Boolean(false)
    }
}

/// Which event carries what, by the name an add-on subscribes by (`host.on(name, ...)`). Written for the TypeScript
/// types only; a test keeps its names equal to `EVENT_NAMES`.
#[cfg(test)]
#[derive(Debug, Default, Serialize, ts_rs::TS)]
#[ts(export, export_to = "AddonEventMap.ts")]
pub struct AddonEventMap {
    #[serde(rename = "device.online")]
    pub device_online: DeviceEventData,
    #[serde(rename = "device.offline")]
    pub device_offline: DeviceEventData,
    #[serde(rename = "device.added")]
    pub device_added: DeviceEventData,
    #[serde(rename = "screen.started")]
    pub screen_started: ScreenEventData,
    #[serde(rename = "screen.ended")]
    pub screen_ended: ScreenEventData,
    #[serde(rename = "files.sent")]
    pub files_sent: FilesSentEventData,
    #[serde(rename = "files.opened")]
    pub files_opened: FilesOpenedEventData,
    #[serde(rename = "settings.changed")]
    pub settings_changed: SettingChangedEventData,
}

/// The text of a code, as the script sees it.
pub fn code_text(code: &impl Serialize) -> String {
    match serde_json::to_value(code) {
        Ok(Value::String(text)) => text,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_of_the_type_map_are_exactly_the_events_an_add_on_can_subscribe_to() {
        use crate::core::addons::events::EVENT_NAMES;
        let Value::Object(map) = serde_json::to_value(AddonEventMap::default()).unwrap() else {
            panic!("the map of the events is an object");
        };
        let mut named: Vec<&str> = map.keys().map(String::as_str).collect();
        let mut subscribed = EVENT_NAMES.to_vec();
        named.sort_unstable();
        subscribed.sort_unstable();
        assert_eq!(named, subscribed);
    }

    #[test]
    fn the_codes_are_the_words_the_scripts_switch_on() {
        assert_eq!(
            [
                code_text(&MenuRegisterCode::InvalidId),
                code_text(&MenuRegisterCode::InvalidLabel),
                code_text(&MenuRegisterCode::AlreadyRegistered),
            ],
            ["invalid_id", "invalid_label", "already_registered"]
        );
        assert_eq!(
            [
                code_text(&ProcessErrorCode::InvalidCommand),
                code_text(&ProcessErrorCode::NotFound),
                code_text(&ProcessErrorCode::NotAllowed),
                code_text(&ProcessErrorCode::TooMany),
                code_text(&ProcessErrorCode::Failed),
            ],
            ["invalid_command", "not_found", "not_allowed", "too_many", "failed"]
        );
        assert_eq!(
            [code_text(&ScreenErrorCode::NoScreen), code_text(&ScreenErrorCode::NoProcess)],
            ["no_screen", "no_process"]
        );
    }
}
