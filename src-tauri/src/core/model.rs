//! Data shared between the core, the persistence layer and the UI.

use super::app_name::AppName;
use super::scrcpy::versions::ReleaseTag;
use super::integration::file_manager_menu::FileManager;
use super::theme::ThemeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Connected through adb and ready to use.
    Online,
    /// Connected but the device has not authorised this computer yet.
    Unauthorized,
    /// Listed by adb but not responding (typically a stale wireless entry).
    Offline,
    /// Seen through mDNS but not connected through adb.
    Discovered,
    /// Paired before, but not currently visible.
    Disconnected,
}

impl DeviceState {
    pub fn rank(self) -> u8 {
        match self {
            DeviceState::Online => 0,
            DeviceState::Unauthorized => 1,
            DeviceState::Offline => 2,
            DeviceState::Discovered => 3,
            DeviceState::Disconnected => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Usb,
    Wifi,
}

/// One device as shown in the UI. `id` is the hardware serial, which stays the
/// same when the IP address or the wireless-debugging port changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub label: Option<String>,
    pub state: DeviceState,
    pub transport: Option<Transport>,
    /// The string to pass to `adb -s` / `scrcpy --serial`. Only set while connected.
    pub adb_serial: Option<String>,
    pub address: Option<String>,
    pub android: Option<String>,
    pub sdk: Option<u32>,
    pub known: bool,
}

/// A device that has been connected at least once; persisted in `devices.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnownDevice {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub android: Option<String>,
    #[serde(default)]
    pub sdk: Option<u32>,
    #[serde(default)]
    pub last_address: Option<String>,
    #[serde(default)]
    pub last_seen: u64,
    /// When this computer first saw the device (seconds since 1970); 0 for a
    /// device that was known before this was recorded.
    #[serde(default)]
    pub first_seen: u64,
    /// What is set for this device alone.
    #[serde(default)]
    pub settings: DeviceSettings,
    /// The apps of this device the user marked, and the ones opened lately.
    #[serde(default)]
    pub apps: AppUsage,
}

/// How many apps are kept as the ones opened lately.
pub const MAX_RECENT_APPS: usize = 20;

/// The apps of one device that matter to the user: the favourites (in the order
/// they were marked) and the ones opened from this program lately (the newest
/// first, each once, at most `MAX_RECENT_APPS`). They are package names.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppUsage {
    pub favorites: Vec<String>,
    pub recent: Vec<String>,
}

/// The longest side of the video in pixels: 0 (the device's own size) or 100 to
/// 16384. A number outside that is not accepted, so a hand-edited file with one
/// gets the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct MaxSize(u32);

impl MaxSize {
    pub const MIN: u32 = 100;
    pub const MAX: u32 = 16384;

    /// `None` for a number that is not allowed.
    pub fn new(pixels: u32) -> Option<Self> {
        (pixels == 0 || (Self::MIN..=Self::MAX).contains(&pixels)).then_some(MaxSize(pixels))
    }

    /// The pixels, or `None` for the device's own size.
    pub fn pixels(self) -> Option<u32> {
        (self.0 > 0).then_some(self.0)
    }
}

impl TryFrom<u32> for MaxSize {
    type Error = String;

    fn try_from(pixels: u32) -> Result<Self, String> {
        Self::new(pixels).ok_or_else(|| format!("{pixels} is not 0 or from {} to {}", Self::MIN, Self::MAX))
    }
}

impl From<MaxSize> for u32 {
    fn from(value: MaxSize) -> u32 {
        value.0
    }
}

/// What is set for one device. A setting that is `None` follows the general one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    /// How the keyboard is sent to this device.
    #[serde(deserialize_with = "lenient")]
    pub keyboard_mode: Option<KeyboardMode>,
    /// The bit rate of this device's video.
    #[serde(deserialize_with = "lenient")]
    pub video_bit_rate_mbps: Option<BitRateMbps>,
    /// The longest side of this device's video (the picture is made smaller to
    /// fit); 0 is the device's own size.
    #[serde(deserialize_with = "lenient")]
    pub max_size: MaxSize,
    /// An app is opened in a display of its own, so the device's screen is left
    /// alone (otherwise on the device's screen, which scrcpy mirrors).
    pub apps_in_virtual_display: bool,
    /// The device is kept from going to sleep while a window of an app is open.
    pub keep_awake: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        DeviceSettings {
            keyboard_mode: None,
            video_bit_rate_mbps: None,
            max_size: MaxSize::default(),
            apps_in_virtual_display: true,
            keep_awake: false,
        }
    }
}

/// Everything the device window shows about one device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceDetails {
    pub device: Device,
    /// The name the device reports for itself; `device.label` is the user's own.
    pub reported_name: String,
    /// Seconds since 1970; 0 if unknown.
    pub first_seen: u64,
    pub last_seen: u64,
    /// Read from the device; only for one that is online and answered.
    pub info: Option<super::device::info::DeviceInfo>,
}

/// A wireless-debugging pairing endpoint advertised through mDNS.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairingService {
    pub name: String,
    pub address: String,
}

/// The floating toolbar next to the scrcpy window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarSettings {
    pub enabled: bool,
    /// Which side of the scrcpy window it sits on, when it can be attached.
    pub side: super::window::layout::Side,
    /// Float above every window. Off by default: the toolbar then only goes
    /// with its own screen window (it is owned by it), in front of that window
    /// and behind other programs' windows when those are in front.
    pub always_on_top: bool,
    /// The buttons the user arranged: the order, and which are hidden. A button that is not in the list (one an
    /// add-on added later) is shown after the others.
    #[serde(deserialize_with = "known_button_prefs")]
    pub buttons: Vec<super::addons::buttons::ButtonPref>,
}

impl Default for ToolbarSettings {
    fn default() -> Self {
        ToolbarSettings {
            enabled: true,
            side: super::window::layout::Side::Right,
            always_on_top: false,
            buttons: Vec::new(),
        }
    }
}

/// The layout of the toolbar from a file: the entries that can be read are kept, in order, once each button; the
/// rest (a newer version's button, a typo, another kind of value) is left out. Not a list at all is an empty one.
fn known_button_prefs<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<super::addons::buttons::ButtonPref>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let mut kept: Vec<super::addons::buttons::ButtonPref> = Vec::new();
    for item in value.as_array().into_iter().flatten() {
        if let Ok(pref) = serde_json::from_value::<super::addons::buttons::ButtonPref>(item.clone())
            && !kept.iter().any(|k| k.id == pref.id)
        {
            kept.push(pref);
        }
    }
    Ok(kept)
}

/// How the application talks to adb. Both give the same answers; the choice
/// is a setting so a user who runs into trouble with one can try the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdbBackend {
    /// The adb server's own protocol (`adb::wire`).
    #[default]
    Server,
    /// Running the adb program and reading what it prints (`adb::cli`).
    CommandLine,
}

/// How the scrcpy window is run. The user always sees scrcpy's own window; the
/// proxy only exists so add-ons can have what needs the protocol (the video
/// stream), and so built-in buttons can act on the device in better ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrcpyMode {
    /// scrcpy talks to the device by itself.
    #[default]
    Official,
    /// scrcpy talks to the device through a proxy of ours (`proxy`).
    Proxy,
}

/// How often a device's window asks the device again, in seconds: 0 (never) or
/// 1 to 3600. A number outside that is not accepted, so a hand-edited file with
/// one gets the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RefreshSeconds(u32);

impl RefreshSeconds {
    pub const MAX: u32 = 3600;

    /// `None` for a number that is not allowed.
    pub fn new(seconds: u32) -> Option<Self> {
        (seconds <= Self::MAX).then_some(RefreshSeconds(seconds))
    }

    pub fn seconds(self) -> u32 {
        self.0
    }
}

impl Default for RefreshSeconds {
    fn default() -> Self {
        RefreshSeconds(5)
    }
}

impl TryFrom<u32> for RefreshSeconds {
    type Error = String;

    fn try_from(seconds: u32) -> Result<Self, String> {
        Self::new(seconds).ok_or_else(|| format!("{seconds} is more than {} seconds", Self::MAX))
    }
}

impl From<RefreshSeconds> for u32 {
    fn from(value: RefreshSeconds) -> u32 {
        value.0
    }
}

/// How long the small window of a request from a file manager stays after
/// everything went well, in seconds: 0 (it closes at once) to 60. A number outside
/// that is not accepted, so a hand-edited file with one gets the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RequestCloseSeconds(u32);

impl RequestCloseSeconds {
    pub const MAX: u32 = 60;

    /// `None` for a number that is not allowed.
    pub fn new(seconds: u32) -> Option<Self> {
        (seconds <= Self::MAX).then_some(RequestCloseSeconds(seconds))
    }

    pub fn seconds(self) -> u32 {
        self.0
    }
}

impl Default for RequestCloseSeconds {
    fn default() -> Self {
        RequestCloseSeconds(2)
    }
}

impl TryFrom<u32> for RequestCloseSeconds {
    type Error = String;

    fn try_from(seconds: u32) -> Result<Self, String> {
        Self::new(seconds).ok_or_else(|| format!("{seconds} is more than {} seconds", Self::MAX))
    }
}

impl From<RequestCloseSeconds> for u32 {
    fn from(value: RequestCloseSeconds) -> u32 {
        value.0
    }
}

/// How the computer's keyboard is sent to the device. The two give different
/// results, so it is a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardMode {
    /// Key events through Android's own interface. Only ASCII and a few other
    /// characters get through; the device's input method is not used.
    Sdk,
    /// The device is made to see a physical USB keyboard: every character and
    /// the device's own input methods (such as Chinese ones) work. The default:
    /// it was tried on a tablet and typing Chinese worked.
    #[default]
    Uhid,
}

/// Where files are sent to unless the setting says otherwise.
pub const DEFAULT_SEND_DIRECTORY: &str = "/sdcard/Download/scrcpy-agent";

/// The bit rate of the video in megabits per second: 0 (what scrcpy chooses, which
/// is 8) or 1 to 200. A number outside that is not accepted, so a hand-edited
/// file with one gets the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BitRateMbps(u32);

impl BitRateMbps {
    pub const MAX: u32 = 200;

    /// `None` for a number that is not allowed.
    pub fn new(mbps: u32) -> Option<Self> {
        (mbps <= Self::MAX).then_some(BitRateMbps(mbps))
    }

    /// The megabits per second, or `None` for scrcpy's own choice.
    pub fn mbps(self) -> Option<u32> {
        (self.0 > 0).then_some(self.0)
    }
}

impl TryFrom<u32> for BitRateMbps {
    type Error = String;

    fn try_from(mbps: u32) -> Result<Self, String> {
        Self::new(mbps).ok_or_else(|| format!("{mbps} is more than {} Mbps", Self::MAX))
    }
}

impl From<BitRateMbps> for u32 {
    fn from(value: BitRateMbps) -> u32 {
        value.0
    }
}

/// A value of a setting that is not one of the known names (a typo in a
/// hand-edited file) is the default of that setting; it must not discard the
/// other settings.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// A list of file managers from a file: the names that are known are kept, in
/// order, once each; the rest (a newer version's name, a typo, another kind of
/// value) is left out. Not a list at all is an empty one.
fn known_file_managers<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<FileManager>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let mut kept: Vec<FileManager> = Vec::new();
    for item in value.as_array().into_iter().flatten() {
        if let Ok(manager) = serde_json::from_value::<FileManager>(item.clone())
            && !kept.contains(&manager)
        {
            kept.push(manager);
        }
    }
    Ok(kept)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub toolbar: ToolbarSettings,
    /// How adb is talked to.
    #[serde(deserialize_with = "lenient")]
    pub adb_backend: AdbBackend,
    /// Whether scrcpy runs through the proxy.
    #[serde(deserialize_with = "lenient")]
    pub scrcpy_mode: ScrcpyMode,
    /// How the computer's keyboard is sent to the device.
    #[serde(deserialize_with = "lenient")]
    pub keyboard_mode: KeyboardMode,
    /// The bit rate of the video (the picture quality); 0 is scrcpy's own choice.
    #[serde(deserialize_with = "lenient")]
    pub video_bit_rate_mbps: BitRateMbps,
    /// The folder on a device that files are sent to.
    pub send_directory: String,
    /// The device that "send to the default device" and "open on the default
    /// device" use (its id); `None` if none was chosen.
    pub default_device: Option<String>,
    /// The program starts when the user logs in, in the tray.
    #[serde(deserialize_with = "lenient")]
    pub launch_at_login: bool,
    /// The entries of the Windows Explorer menu are registered.
    pub explorer_menu: bool,
    /// The file managers of Linux that the menu is not written for, though they are
    /// there. A name that is not one of the known ones is left out, not an error.
    #[serde(deserialize_with = "known_file_managers")]
    pub disabled_file_managers: Vec<FileManager>,
    /// How long the small window of a request stays after everything went well.
    #[serde(deserialize_with = "lenient")]
    pub request_close_seconds: RequestCloseSeconds,
    /// How often a device's window reads the device again while it is usable;
    /// 0 turns it off.
    #[serde(deserialize_with = "lenient")]
    pub device_refresh_seconds: RefreshSeconds,
    /// UI language code; `None` follows the system language.
    pub language: Option<String>,
    /// What the program calls itself in what the user sees (the right-click menu, the titles of
    /// the windows, the sidebar, the tray); `None` is the standard name of the language.
    #[serde(deserialize_with = "lenient")]
    pub app_name: Option<AppName>,
    /// The theme the windows are drawn in (its id); a theme that is not there any more is
    /// the program's own look.
    #[serde(deserialize_with = "lenient")]
    pub theme: ThemeId,
    /// The scrcpy release the user chose; `None` is the one the program is checked against.
    #[serde(deserialize_with = "lenient")]
    pub scrcpy_version: Option<ReleaseTag>,
    /// Closing the main window hides it in the system tray (the application
    /// keeps running) instead of quitting.
    pub close_to_tray: bool,
    /// Extra arguments appended to the scrcpy command line.
    pub scrcpy_extra_args: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            toolbar: ToolbarSettings::default(),
            adb_backend: AdbBackend::default(),
            scrcpy_mode: ScrcpyMode::default(),
            keyboard_mode: KeyboardMode::default(),
            video_bit_rate_mbps: BitRateMbps::default(),
            send_directory: DEFAULT_SEND_DIRECTORY.into(),
            default_device: None,
            launch_at_login: false,
            explorer_menu: false,
            disabled_file_managers: Vec::new(),
            request_close_seconds: RequestCloseSeconds::default(),
            device_refresh_seconds: RefreshSeconds::default(),
            language: None,
            app_name: None,
            scrcpy_version: None,
            theme: ThemeId::default(),
            close_to_tray: true,
            scrcpy_extra_args: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::addons::buttons::{ButtonId, ButtonPref};
    use crate::core::window::layout::Side;

    #[test]
    fn the_toolbar_is_on_by_default_on_the_right_and_not_above_all_other_windows() {
        // Above everything is an option; by default the toolbar only goes
        // with its own screen window.
        let toolbar = Settings::default().toolbar;
        assert_eq!(toolbar, ToolbarSettings { enabled: true, side: Side::Right, always_on_top: false, buttons: vec![] });
    }

    fn pref(id: ButtonId, visible: bool) -> ButtonPref {
        ButtonPref { id, visible }
    }

    #[test]
    fn the_layout_of_the_toolbar_is_kept_in_a_file_in_the_order_it_was_arranged() {
        let layout = vec![
            pref(ButtonId::Home, true),
            pref(ButtonId::Back, false),
            pref(ButtonId::Addon { addon: "tools".into(), id: "tap".into() }, true),
        ];
        let settings = Settings { toolbar: ToolbarSettings { buttons: layout.clone(), ..Default::default() }, ..Default::default() };
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            json["toolbar"]["buttons"],
            serde_json::json!([
                { "id": "home", "visible": true },
                { "id": "back", "visible": false },
                { "id": { "addon": { "addon": "tools", "id": "tap" } }, "visible": true },
            ])
        );
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap().toolbar.buttons, layout);
    }

    #[test]
    fn what_cannot_be_read_of_the_layout_is_left_out_and_the_rest_is_kept() {
        let text = r#"{"toolbar":{"buttons":[
            {"id":"home","visible":false},
            {"id":"from_a_newer_version","visible":true},
            {"id":"back"},
            "not an entry",
            {"id":"home","visible":true},
            {"id":"rotate","visible":true}
        ]}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        // The first entry of a button counts; the ones that are not entries of this version are not kept.
        assert_eq!(s.toolbar.buttons, vec![pref(ButtonId::Home, false), pref(ButtonId::Rotate, true)]);
        // Something that is not a list is no layout, and does not lose the other settings.
        let not_a_list: Settings = serde_json::from_str(r#"{"language":"de","toolbar":{"buttons":"x","side":"left"}}"#).unwrap();
        assert_eq!((not_a_list.toolbar.buttons, not_a_list.toolbar.side), (vec![], Side::Left));
        assert_eq!(not_a_list.language.as_deref(), Some("de"));
    }

    #[test]
    fn a_settings_file_naming_only_one_toolbar_option_keeps_the_other_defaults() {
        let s: Settings = serde_json::from_str(r#"{"toolbar":{"side":"left"}}"#).unwrap();
        assert_eq!(s.toolbar, ToolbarSettings { enabled: true, side: Side::Left, always_on_top: false, buttons: vec![] });
        // An older file without any toolbar section still loads.
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.toolbar, ToolbarSettings::default());
    }

    #[test]
    fn adb_is_talked_to_through_the_server_unless_the_user_chooses_otherwise() {
        assert_eq!(Settings::default().adb_backend, AdbBackend::Server);
        // A file that does not mention it, including one from before it existed.
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.adb_backend, AdbBackend::Server);
    }

    #[test]
    fn the_adb_backend_is_written_by_name_in_the_settings_file() {
        for (backend, name) in [(AdbBackend::Server, "server"), (AdbBackend::CommandLine, "command_line")] {
            let s = Settings { adb_backend: backend, ..Settings::default() };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(json["adb_backend"], name);
            assert_eq!(serde_json::from_value::<Settings>(json).unwrap().adb_backend, backend);
        }
    }

    #[test]
    fn a_misspelt_backend_falls_back_to_the_default_and_keeps_the_other_settings() {
        for bad in [r#""sever""#, r#""""#, "123", "null", r#"{"a":1}"#] {
            let text = format!(r#"{{"language":"de","adb_backend":{bad},"explorer_menu":true}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.adb_backend, AdbBackend::Server, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
            assert!(s.explorer_menu, "{bad}");
        }
    }

    #[test]
    fn the_keyboard_is_a_physical_one_unless_the_user_chooses_the_sdk_and_is_written_by_name() {
        assert_eq!(Settings::default().keyboard_mode, KeyboardMode::Uhid);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.keyboard_mode, KeyboardMode::Uhid);
        for (mode, name) in [(KeyboardMode::Sdk, "sdk"), (KeyboardMode::Uhid, "uhid")] {
            let json = serde_json::to_value(Settings { keyboard_mode: mode, ..Settings::default() }).unwrap();
            assert_eq!(json["keyboard_mode"], name);
            assert_eq!(serde_json::from_value::<Settings>(json).unwrap().keyboard_mode, mode);
        }
        // A misspelt value is the default and the rest of the file stays.
        for bad in [r#""uhidd""#, r#""aoa""#, "1", "null"] {
            let text = format!(r#"{{"language":"de","keyboard_mode":{bad},"explorer_menu":true}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.keyboard_mode, KeyboardMode::Uhid, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"));
            assert!(s.explorer_menu);
        }
    }

    #[test]
    fn a_device_has_no_favourite_or_recent_apps_until_there_are_some_and_an_old_file_still_loads() {
        assert_eq!(AppUsage::default(), AppUsage { favorites: vec![], recent: vec![] });
        let old: KnownDevice = serde_json::from_str(r#"{"id":"A1","model":"M"}"#).unwrap();
        assert_eq!(old.apps, AppUsage::default());
        let partial: AppUsage = serde_json::from_str(r#"{"favorites":["com.example.a"]}"#).unwrap();
        assert_eq!(partial.favorites, ["com.example.a"]);
        assert!(partial.recent.is_empty());
        assert_eq!(MAX_RECENT_APPS, 20);
    }

    #[test]
    fn a_device_follows_the_general_settings_until_something_is_set_for_it() {
        let d = DeviceSettings::default();
        assert_eq!((d.keyboard_mode, d.video_bit_rate_mbps, d.max_size.pixels()), (None, None, None));
        // Apps open in a display of their own and do not keep the device awake.
        assert!(d.apps_in_virtual_display && !d.keep_awake);
        // A device known before there were settings for it.
        let old: KnownDevice = serde_json::from_str(r#"{"id":"A1","model":"M"}"#).unwrap();
        assert_eq!(old.settings, DeviceSettings::default());
        // What is set is written by name and read back.
        let set = DeviceSettings {
            keyboard_mode: Some(KeyboardMode::Sdk),
            video_bit_rate_mbps: BitRateMbps::new(20),
            max_size: MaxSize::new(1280).unwrap(),
            apps_in_virtual_display: false,
            keep_awake: true,
        };
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["keyboard_mode"], "sdk");
        assert_eq!(json["video_bit_rate_mbps"], 20);
        assert_eq!(json["max_size"], 1280);
        assert_eq!(serde_json::from_value::<DeviceSettings>(json).unwrap(), set);
    }

    #[test]
    fn a_value_of_a_device_that_is_not_allowed_is_the_general_one_and_the_rest_stays() {
        let text = r#"{"keyboard_mode":"aoa","video_bit_rate_mbps":500,"max_size":50,"keep_awake":true}"#;
        let s: DeviceSettings = serde_json::from_str(text).unwrap();
        assert_eq!((s.keyboard_mode, s.video_bit_rate_mbps, s.max_size.pixels()), (None, None, None));
        assert!(s.keep_awake);
        assert_eq!(MaxSize::new(99), None);
        assert_eq!(MaxSize::new(16385), None);
        assert_eq!(MaxSize::new(100).and_then(MaxSize::pixels), Some(100));
        assert_eq!(MaxSize::new(0).and_then(MaxSize::pixels), None);
    }

    #[test]
    fn the_video_bit_rate_is_scrcpys_own_unless_the_user_sets_one_from_1_to_200_megabits() {
        assert_eq!(Settings::default().video_bit_rate_mbps.mbps(), None);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.video_bit_rate_mbps.mbps(), None);
        for (text, mbps) in [("1", Some(1)), ("16", Some(16)), ("200", Some(200)), ("0", None)] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"video_bit_rate_mbps":{text}}}"#)).unwrap();
            assert_eq!(s.video_bit_rate_mbps.mbps(), mbps, "{text}");
        }
        let json = serde_json::to_value(Settings { video_bit_rate_mbps: BitRateMbps::new(24).unwrap(), ..Settings::default() }).unwrap();
        assert_eq!(json["video_bit_rate_mbps"], 24);
        for bad in ["201", "-1", "2.5", r#""8""#, "null", "true"] {
            let text = format!(r#"{{"language":"de","video_bit_rate_mbps":{bad},"explorer_menu":true}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.video_bit_rate_mbps.mbps(), None, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"));
            assert!(s.explorer_menu);
        }
        assert_eq!(BitRateMbps::new(201), None);
    }

    #[test]
    fn files_go_to_a_folder_of_downloads_and_the_explorer_menu_is_off_until_the_user_turns_it_on() {
        let s = Settings::default();
        assert_eq!(s.send_directory, "/sdcard/Download/scrcpy-agent");
        assert_eq!(s.default_device, None);
        assert!(!s.explorer_menu);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.send_directory, "/sdcard/Download/scrcpy-agent");
        let set: Settings =
            serde_json::from_str(r#"{"default_device":"ABC","explorer_menu":true,"send_directory":"/sdcard/x"}"#).unwrap();
        assert_eq!(set.default_device.as_deref(), Some("ABC"));
        assert!(set.explorer_menu);
        assert_eq!(set.send_directory, "/sdcard/x");
    }

    #[test]
    fn the_program_does_not_start_at_the_login_until_the_user_turns_it_on_and_a_bad_value_is_only_lost_for_itself() {
        assert!(!Settings::default().launch_at_login);
        let on: Settings = serde_json::from_str(r#"{"launch_at_login":true,"language":"de"}"#).unwrap();
        assert!(on.launch_at_login);
        let wrong: Settings = serde_json::from_str(r#"{"launch_at_login":"yes","language":"de"}"#).unwrap();
        assert!(!wrong.launch_at_login);
        assert_eq!(wrong.language.as_deref(), Some("de"));
    }

    #[test]
    fn no_file_manager_is_turned_off_until_the_user_does_it_and_a_name_that_is_not_known_is_left_out() {
        use crate::core::integration::file_manager_menu::FileManager;
        assert_eq!(Settings::default().disabled_file_managers, []);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.disabled_file_managers, []);
        let some: Settings = serde_json::from_str(r#"{"disabled_file_managers":["nemo","dolphin"]}"#).unwrap();
        assert_eq!(some.disabled_file_managers, [FileManager::Nemo, FileManager::Dolphin]);
        // A name from a newer version, a typo or another kind of value: only that is lost.
        let odd: Settings =
            serde_json::from_str(r#"{"language":"de","disabled_file_managers":["nemo","Thunar",7,"nautilus"]}"#).unwrap();
        assert_eq!(odd.disabled_file_managers, [FileManager::Nemo, FileManager::Nautilus]);
        assert_eq!(odd.language.as_deref(), Some("de"));
        let wrong: Settings = serde_json::from_str(r#"{"language":"de","disabled_file_managers":"nemo"}"#).unwrap();
        assert_eq!(wrong.disabled_file_managers, []);
        assert_eq!(wrong.language.as_deref(), Some("de"));
    }

    #[test]
    fn the_scrcpy_release_is_the_programs_own_until_the_user_chooses_one_and_a_bad_one_is_only_lost_for_itself() {
        assert_eq!(Settings::default().scrcpy_version, None);
        let chosen: Settings = serde_json::from_str(r#"{"scrcpy_version":"v3.3.4","language":"de"}"#).unwrap();
        assert_eq!(chosen.scrcpy_version.as_ref().map(ReleaseTag::as_str), Some("v3.3.4"));
        assert_eq!(chosen.language.as_deref(), Some("de"));
        for bad in [r#""5.0.1""#, r#""v5.0.1-beta""#, r#""../v5""#, "7", "true", r#""""#, "null"] {
            let text = format!(r#"{{"language":"de","scrcpy_version":{bad}}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap();
            assert_eq!(s.scrcpy_version, None, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
        }
        let written = serde_json::to_value(&chosen).unwrap();
        assert_eq!(written["scrcpy_version"], "v3.3.4");
    }

    #[test]
    fn the_switch_that_looked_for_a_newer_scrcpy_is_gone_and_a_file_that_still_has_it_is_read_all_the_same() {
        let written = serde_json::to_value(Settings::default()).unwrap();
        assert!(written.get("check_updates").is_none(), "it is still written: {written}");
        let old: Settings = serde_json::from_str(r#"{"language":"de","check_updates":false}"#).unwrap();
        assert_eq!(old.language.as_deref(), Some("de"));
    }

    #[test]
    fn the_program_has_the_standard_name_until_the_user_gives_it_one_and_a_bad_name_is_only_lost_for_itself() {
        assert_eq!(Settings::default().app_name, None);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.app_name, None);
        let named: Settings = serde_json::from_str(r#"{"language":"de","app_name":"  Kitchen "}"#).unwrap();
        assert_eq!(named.app_name.as_ref().map(AppName::as_str), Some("Kitchen"));
        assert_eq!(named.language.as_deref(), Some("de"));
        for bad in ["\"a/b\"", "\"\"", "\"   \"", "5", "null", "[\"x\"]", "{}", "\"a\\nb\""] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"language":"de","app_name":{bad}}}"#)).unwrap();
            assert_eq!(s.app_name, None, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
        }
        let json = serde_json::to_value(Settings { app_name: AppName::parse("Kitchen"), ..Settings::default() }).unwrap();
        assert_eq!(json["app_name"], "Kitchen");
        assert!(serde_json::to_value(Settings::default()).unwrap()["app_name"].is_null());
    }

    #[test]
    fn the_programs_own_look_is_the_theme_until_the_user_chooses_another_and_a_bad_value_is_only_lost_for_itself() {
        assert_eq!(Settings::default().theme.as_str(), "default");
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.theme.as_str(), "default");
        // An id that is well formed is kept, whether or not there is such a theme now: the
        // theme may be put back, and the look falls back at the moment it is applied.
        for good in ["night", "night-pack/night", "high-contrast"] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"language":"de","theme":"{good}"}}"#)).unwrap();
            assert_eq!(s.theme.as_str(), good);
            assert_eq!(s.language.as_deref(), Some("de"));
        }
        for bad in ["\"../x\"", "\"\"", "\"a b\"", "5", "null", "[\"night\"]", "{}"] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"language":"de","theme":{bad}}}"#)).unwrap();
            assert_eq!(s.theme.as_str(), "default", "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
        }
        let json = serde_json::to_value(Settings { theme: ThemeId::parse("night").unwrap(), ..Settings::default() }).unwrap();
        assert_eq!(json["theme"], "night");
    }

    #[test]
    fn the_window_of_a_request_waits_two_seconds_after_success_unless_the_user_says_otherwise() {
        assert_eq!(Settings::default().request_close_seconds.seconds(), 2);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.request_close_seconds.seconds(), 2);
        for (text, seconds) in [("0", 0), ("1", 1), ("60", 60)] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"request_close_seconds":{text}}}"#)).unwrap();
            assert_eq!(s.request_close_seconds.seconds(), seconds, "{text}");
        }
        let json = serde_json::to_value(Settings { request_close_seconds: RequestCloseSeconds::new(9).unwrap(), ..Settings::default() }).unwrap();
        assert_eq!(json["request_close_seconds"], 9);
        assert_eq!(RequestCloseSeconds::new(61), None);
        assert_eq!(RequestCloseSeconds::new(60).map(RequestCloseSeconds::seconds), Some(60));
    }

    #[test]
    fn a_time_that_is_not_allowed_for_the_window_of_a_request_is_the_default_and_keeps_the_other_settings() {
        for bad in ["61", "-1", "1.5", r#""2""#, "null", "true", r#"{"a":1}"#, "4294967295"] {
            let text = format!(r#"{{"language":"de","request_close_seconds":{bad},"explorer_menu":true}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.request_close_seconds.seconds(), 2, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
            assert!(s.explorer_menu, "{bad}");
        }
    }

    #[test]
    fn closing_the_window_goes_to_the_tray_unless_the_user_turns_it_off() {
        assert!(Settings::default().close_to_tray);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert!(old.close_to_tray);
        let off: Settings = serde_json::from_str(r#"{"close_to_tray":false,"language":"de"}"#).unwrap();
        assert!(!off.close_to_tray);
        assert_eq!(off.language.as_deref(), Some("de"));
    }

    #[test]
    fn a_device_window_asks_again_every_five_seconds_unless_the_user_says_otherwise() {
        assert_eq!(Settings::default().device_refresh_seconds.seconds(), 5);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.device_refresh_seconds.seconds(), 5);
        for (text, seconds) in [("1", 1), ("30", 30), ("3600", 3600), ("0", 0)] {
            let s: Settings = serde_json::from_str(&format!(r#"{{"device_refresh_seconds":{text}}}"#)).unwrap();
            assert_eq!(s.device_refresh_seconds.seconds(), seconds, "{text}");
        }
        let json = serde_json::to_value(Settings { device_refresh_seconds: RefreshSeconds::new(12).unwrap(), ..Settings::default() }).unwrap();
        assert_eq!(json["device_refresh_seconds"], 12);
    }

    #[test]
    fn a_refresh_time_that_is_not_allowed_is_the_default_and_keeps_the_other_settings() {
        for bad in ["3601", "-1", "1.5", r#""5""#, "null", "true", r#"{"a":1}"#] {
            let text = format!(r#"{{"language":"de","device_refresh_seconds":{bad},"explorer_menu":true}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.device_refresh_seconds.seconds(), 5, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
            assert!(s.explorer_menu, "{bad}");
        }
        assert_eq!(RefreshSeconds::new(3601), None);
    }

    #[test]
    fn scrcpy_runs_officially_unless_the_user_turns_the_proxy_on() {
        assert_eq!(Settings::default().scrcpy_mode, ScrcpyMode::Official);
        let old: Settings = serde_json::from_str(r#"{"language":"de"}"#).unwrap();
        assert_eq!(old.scrcpy_mode, ScrcpyMode::Official);
    }

    #[test]
    fn the_scrcpy_mode_is_written_by_name_in_the_settings_file() {
        for (mode, name) in [(ScrcpyMode::Official, "official"), (ScrcpyMode::Proxy, "proxy")] {
            let s = Settings { scrcpy_mode: mode, ..Settings::default() };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(json["scrcpy_mode"], name);
            assert_eq!(serde_json::from_value::<Settings>(json).unwrap().scrcpy_mode, mode);
        }
    }

    #[test]
    fn a_misspelt_mode_falls_back_to_the_default_and_keeps_the_other_settings() {
        for bad in [r#""prox""#, r#""""#, "1", "null", r#"{"a":1}"#] {
            let text = format!(r#"{{"language":"de","scrcpy_mode":{bad},"adb_backend":"command_line"}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(s.scrcpy_mode, ScrcpyMode::Official, "{bad}");
            assert_eq!(s.language.as_deref(), Some("de"), "{bad}");
            // The other choice made in the same file is not lost either.
            assert_eq!(s.adb_backend, AdbBackend::CommandLine, "{bad}");
        }
    }

    #[test]
    fn the_sides_are_written_in_lower_case() {
        for (side, text) in [(Side::Left, "left"), (Side::Right, "right"), (Side::Top, "top"), (Side::Bottom, "bottom")] {
            let s = Settings { toolbar: ToolbarSettings { side, ..ToolbarSettings::default() }, ..Settings::default() };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(json["toolbar"]["side"], text);
            assert_eq!(serde_json::from_value::<Settings>(json).unwrap().toolbar.side, side);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ScrcpyStatus {
    Checking,
    /// The release that is wanted is not installed and nothing else is: nothing works until the user
    /// decides to download it.
    NeedsDownload { version: String },
    Downloading { version: String, progress: f32 },
    Ready { version: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub scrcpy: ScrcpyStatus,
    /// The release that is wanted (the user's choice, else the default) while it is not installed: the user is
    /// asked whether to download it. Another release may be in use meanwhile.
    pub scrcpy_wanted: Option<String>,
    pub devices: Vec<Device>,
    pub pairing: Vec<PairingService>,
    /// Device ids that currently have an open scrcpy window.
    pub sessions: Vec<String>,
    pub settings: Settings,
    pub data_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Error,
    Info,
}

/// Which message the UI shows for a notice; the UI switches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCode {
    AudioUnavailable,
    SessionFailed,
    AdbStartFailed,
    MdnsUnavailable,
    /// The proxy mode is on, but this scrcpy version has no support for it (or
    /// it could not be started): the screen opened without it.
    ProxyUnavailable,
    /// A setting that is an argument of scrcpy (the keyboard mode, the bit rate) is
    /// not the default, but this scrcpy version has not been checked for it: the
    /// screen opened with scrcpy's own.
    OptionsUnavailable,
    /// A shortcut was opened for an app of a device that is not ready; its
    /// window is shown instead.
    DeviceNotOnline,
}

/// A transient message for the UI (toast). `detail` is raw text from adb or
/// scrcpy, shown to the user as is and never interpreted.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub level: NoticeLevel,
    pub code: NoticeCode,
    pub detail: String,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Event {
    State(Box<Snapshot>),
    Notice(Notice),
    /// A line of the debug log, sent only while the debug console is open.
    Log(super::log::LogLine),
    /// A device was read again (battery, storage and the rest). Sent apart from
    /// `State`, which only changes when the list or a device's state does.
    DeviceInfo(Box<DeviceInfoUpdate>),
    /// A file being sent to a device.
    Transfer(Box<TransferUpdate>),
    /// A file was (or could not be) opened on a device.
    Opened(Box<OpenedUpdate>),
    /// A request from the Explorer menu waits for a device to be chosen, or no
    /// longer does (`None`).
    /// The request of a file manager that the small window shows (`None`: there is none now).
    Request(Option<Box<super::requests::request::RequestView>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    Sending,
    Done,
    Failed,
}

/// How far the sending of one file is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferUpdate {
    /// Names this file's sending; the updates of one sending share it.
    pub id: u64,
    pub device: String,
    /// The name the file has on the device.
    pub name: String,
    pub sent: u64,
    pub total: u64,
    pub phase: TransferPhase,
    /// Why, for `Failed`.
    pub error: Option<super::error::CoreError>,
}

/// Whether a device can open a file, for the list of devices to choose from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenSupport {
    /// `count` apps can open it; `is_default` if the user made one the default
    /// (otherwise the device asks which to use).
    /// `type_known` is false when the kind of the file is not known: the count is then
    /// of every app that can show a file, and the device decides which opens it.
    Apps { count: usize, is_default: bool, type_known: bool },
    NoApp,
    /// The device did not answer.
    Unreachable,
}

/// A request from the Explorer menu that waits for the user to choose a device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceInfo {
    pub action: super::requests::shell_request::ShellAction,
    /// The files, as they were given.
    pub files: Vec<String>,
    /// The request was for the default device but there is none (none was chosen,
    /// or it is not ready), which is why the user is asked.
    pub no_default: bool,
}

/// The end of opening a file on a device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenedUpdate {
    pub device: String,
    pub file: String,
    /// Its path on the device, if it was opened.
    pub remote: Option<String>,
    pub error: Option<super::error::CoreError>,
}

/// What became of one file that was to be sent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SendOutcome {
    /// The local file, as it was given.
    pub file: String,
    /// Its path on the device, if it was sent.
    pub remote: Option<String>,
    pub error: Option<super::error::CoreError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceInfoUpdate {
    pub device: String,
    pub info: super::device::info::DeviceInfo,
}
