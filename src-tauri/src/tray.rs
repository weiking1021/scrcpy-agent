//! The icon in the system tray and its menu: show the window, a submenu with the
//! devices that are ready (a click on one opens its screen), a submenu with all the
//! devices (a click on one opens its window) and quit. The menu is rebuilt when
//! the list of devices changes. The core has no language packs, so the texts
//! come from the UI (`set_tray_texts`).

use crate::core::model::{Device, DeviceState, NoticeCode, NoticeLevel};
use serde::Deserialize;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const SHOW: &str = "tray-show";
const QUIT: &str = "tray-quit";
/// The id of a device's entry is this prefix and the label of its window, which
/// holds the device id in hexadecimal (so any id gives an id Tauri accepts).
const DEVICE_PREFIX: &str = "tray-device:";
/// The same for the entries that open the screen of a device.
const SCREEN_PREFIX: &str = "tray-screen:";

/// The texts of the menu, in the language of the UI.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrayTexts {
    /// What the program is called, for the tooltip of the icon.
    pub app_name: String,
    pub show: String,
    pub quit: String,
    /// The submenu with the devices that a screen can be opened for.
    pub open_screen: String,
    /// The submenu with the devices.
    pub devices: String,
    /// Its only entry when there is no device.
    pub none: String,
    pub state_online: String,
    pub state_unauthorized: String,
    pub state_offline: String,
    pub state_discovered: String,
    pub state_disconnected: String,
}

impl Default for TrayTexts {
    fn default() -> Self {
        TrayTexts {
            app_name: crate::core::app_name::DEFAULT_APP_NAME.into(),
            show: "Show scrcpy-agent".into(),
            quit: "Quit".into(),
            open_screen: "Open screen".into(),
            devices: "Devices".into(),
            none: "None".into(),
            state_online: "online".into(),
            state_unauthorized: "unauthorized".into(),
            state_offline: "offline".into(),
            state_discovered: "found".into(),
            state_disconnected: "not connected".into(),
        }
    }
}

/// The tooltip of the icon: the name the program has.
pub fn tooltip_of(texts: &TrayTexts) -> String {
    texts.app_name.clone()
}

/// One entry of the devices submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    /// The device it opens; `None` for the entry that says there is none.
    pub device: Option<String>,
    pub text: String,
}

/// The entries of the devices submenu: every device of the list, by the name it is
/// shown with, and the state of the ones that are not ready; one entry that says
/// so when there is none.
pub fn device_entries(devices: &[Device], texts: &TrayTexts) -> Vec<DeviceEntry> {
    if devices.is_empty() {
        return vec![DeviceEntry { device: None, text: texts.none.clone() }];
    }
    devices
        .iter()
        .map(|d| {
            let name = d.label.as_deref().unwrap_or(&d.name);
            let text = match d.state {
                DeviceState::Online => name.to_string(),
                DeviceState::Unauthorized => format!("{name} ({})", texts.state_unauthorized),
                DeviceState::Offline => format!("{name} ({})", texts.state_offline),
                DeviceState::Discovered => format!("{name} ({})", texts.state_discovered),
                DeviceState::Disconnected => format!("{name} ({})", texts.state_disconnected),
            };
            DeviceEntry { device: Some(d.id.clone()), text }
        })
        .collect()
}

/// What a menu entry asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
    /// Open the window of a device (what is known about it and its apps).
    OpenDeviceWindow(String),
    /// Open the screen of a device.
    OpenScreen(String),
}

/// The entries of the submenu for opening a screen: the devices that are ready, by the
/// name they are shown with; one entry that says so when there is none.
pub fn screen_entries(devices: &[Device], texts: &TrayTexts) -> Vec<DeviceEntry> {
    let ready: Vec<DeviceEntry> = devices
        .iter()
        .filter(|d| d.state == DeviceState::Online)
        .map(|d| DeviceEntry { device: Some(d.id.clone()), text: d.label.as_deref().unwrap_or(&d.name).to_string() })
        .collect();
    if ready.is_empty() { vec![DeviceEntry { device: None, text: texts.none.clone() }] } else { ready }
}

/// The id of a menu entry that opens the screen of a device.
fn screen_entry_id(device_id: &str) -> String {
    format!("{SCREEN_PREFIX}{}", crate::windows::device_window_label(device_id))
}

/// The id of a menu entry that opens the window of a device.
fn entry_id(device_id: &str) -> String {
    format!("{DEVICE_PREFIX}{}", crate::windows::device_window_label(device_id))
}

/// What the menu entry with this id asks for; `None` for an id that is not exactly one the menu made.
pub fn action_of_entry(id: &str) -> Option<TrayAction> {
    match id {
        SHOW => Some(TrayAction::Show),
        QUIT => Some(TrayAction::Quit),
        _ => {
            let device_of = |prefix: &str| crate::windows::device_id_of_label(id.strip_prefix(prefix)?);
            device_of(DEVICE_PREFIX)
                .map(TrayAction::OpenDeviceWindow)
                .or_else(|| device_of(SCREEN_PREFIX).map(TrayAction::OpenScreen))
        }
    }
}

/// What the menu was last built from, so it is only rebuilt when it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuContent {
    screens: Vec<DeviceEntry>,
    devices: Vec<DeviceEntry>,
    texts: TrayTexts,
}

impl MenuContent {
    fn of(devices: &[Device], texts: &TrayTexts) -> Self {
        MenuContent { screens: screen_entries(devices, texts), devices: device_entries(devices, texts), texts: texts.clone() }
    }
}

pub struct TrayState<R: tauri::Runtime> {
    tray: TrayIcon<R>,
    texts: Mutex<TrayTexts>,
    built: Mutex<Option<MenuContent>>,
}

fn build_menu<R: tauri::Runtime>(app: &AppHandle<R>, content: &MenuContent) -> tauri::Result<Menu<R>> {
    let texts = &content.texts;
    let show = MenuItem::with_id(app, SHOW, &texts.show, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, &texts.quit, true, None::<&str>)?;
    let screens = Submenu::with_id(app, "tray-screens", &texts.open_screen, true)?;
    for entry in &content.screens {
        let id = entry.device.as_deref().map_or_else(|| "tray-none".to_string(), screen_entry_id);
        screens.append(&MenuItem::with_id(app, id, &entry.text, entry.device.is_some(), None::<&str>)?)?;
    }
    let devices = Submenu::with_id(app, "tray-devices", &texts.devices, true)?;
    for entry in &content.devices {
        let id = entry.device.as_deref().map_or_else(|| "tray-none".to_string(), entry_id);
        devices.append(&MenuItem::with_id(app, id, &entry.text, entry.device.is_some(), None::<&str>)?)?;
    }
    Menu::with_items(app, &[&show, &screens, &devices, &quit])
}

/// Makes the icon and its menu. Called once at start.
pub fn setup<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let texts = TrayTexts::default();
    let content = MenuContent::of(&[], &texts);
    let menu = build_menu(app.handle(), &content)?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(tooltip_of(&texts))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let Some(action) = action_of_entry(event.id().as_ref()) else { return };
            match action {
                TrayAction::Show => crate::windows::show_main(app),
                TrayAction::Quit => app.exit(0),
                // Windows and processes are made from a task of their own, not from this event.
                TrayAction::OpenDeviceWindow(device) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(core) = app.try_state::<crate::Shared>() {
                            let _ = crate::windows::open_device_window(&app, &core, &device);
                        }
                    });
                }
                TrayAction::OpenScreen(device) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let Some(core) = app.try_state::<crate::Shared>() else { return };
                        let core = core.inner().clone();
                        if let Err(error) = crate::commands::sessions::open_screen(app, core.clone(), &device).await {
                            core.notice(
                                NoticeLevel::Error,
                                NoticeCode::SessionFailed,
                                error.to_string(),
                                Some(device),
                            );
                        }
                    });
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::windows::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    app.manage(TrayState { tray, texts: Mutex::new(texts), built: Mutex::new(Some(content)) });
    Ok(())
}

/// Makes the menu show these devices (does nothing if it already does).
pub fn refresh<R: tauri::Runtime>(app: &AppHandle<R>, devices: &[Device]) {
    let Some(state) = app.try_state::<TrayState<R>>() else { return };
    let content = MenuContent::of(devices, &state.texts.lock().unwrap());
    let mut built = state.built.lock().unwrap();
    if built.as_ref() == Some(&content) {
        return;
    }
    if let Ok(menu) = build_menu(app, &content) {
        let _ = state.tray.set_menu(Some(menu));
        *built = Some(content);
    }
}

/// The texts of the menu, from the UI; the devices that were last shown are put
/// into the menu again with them.
#[tauri::command]
pub fn set_tray_texts<R: tauri::Runtime>(app: AppHandle<R>, texts: TrayTexts) {
    let Some(state) = app.try_state::<TrayState<R>>() else { return };
    // The name the program has may have changed.
    let _ = state.tray.set_tooltip(Some(tooltip_of(&texts)));
    *state.texts.lock().unwrap() = texts;
    let devices = app.try_state::<crate::Shared>().map(|core| core.snapshot().devices).unwrap_or_default();
    refresh(&app, &devices);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, label: Option<&str>, state: DeviceState) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            label: label.map(str::to_string),
            state,
            transport: None,
            adb_serial: None,
            address: None,
            android: None,
            sdk: None,
            known: true,
        }
    }

    fn entry(device: Option<&str>, text: &str) -> DeviceEntry {
        DeviceEntry { device: device.map(str::to_string), text: text.into() }
    }

    #[test]
    fn without_devices_the_submenu_has_one_entry_that_says_so_and_opens_nothing() {
        assert_eq!(device_entries(&[], &TrayTexts::default()), [entry(None, "None")]);
    }

    #[test]
    fn each_device_is_listed_by_its_own_name_if_it_has_one_and_with_its_state_unless_it_is_ready() {
        let devices = [
            device("A1", "Model A", None, DeviceState::Online),
            device("B2", "Model B", Some("Kitchen"), DeviceState::Online),
            device("C3", "Model C", None, DeviceState::Unauthorized),
            device("D4", "Model D", None, DeviceState::Offline),
            device("E5", "Model E", None, DeviceState::Discovered),
            device("F6", "Model F", Some("Desk"), DeviceState::Disconnected),
        ];
        assert_eq!(
            device_entries(&devices, &TrayTexts::default()),
            [
                entry(Some("A1"), "Model A"),
                entry(Some("B2"), "Kitchen"),
                entry(Some("C3"), "Model C (unauthorized)"),
                entry(Some("D4"), "Model D (offline)"),
                entry(Some("E5"), "Model E (found)"),
                entry(Some("F6"), "Desk (not connected)"),
            ]
        );
    }

    #[test]
    fn the_texts_of_the_language_are_used() {
        let texts = TrayTexts { none: "nothing".into(), state_offline: "gone".into(), ..TrayTexts::default() };
        assert_eq!(device_entries(&[], &texts), [entry(None, "nothing")]);
        assert_eq!(
            device_entries(&[device("D4", "Model D", None, DeviceState::Offline)], &texts),
            [entry(Some("D4"), "Model D (gone)")]
        );
    }

    #[test]
    fn the_icon_says_the_name_the_program_has_and_the_standard_name_until_it_is_given_another() {
        assert_eq!(TrayTexts::default().app_name, crate::core::app_name::DEFAULT_APP_NAME);
        assert_eq!(tooltip_of(&TrayTexts::default()), "ScrcpyAgent");
        let kitchen = TrayTexts { app_name: "Kitchen".into(), ..TrayTexts::default() };
        assert_eq!(tooltip_of(&kitchen), "Kitchen");
    }

    #[test]
    fn the_screen_submenu_lists_only_the_devices_that_are_ready_by_the_name_they_are_shown_with() {
        let devices = [
            device("A1", "Model A", None, DeviceState::Online),
            device("C3", "Model C", None, DeviceState::Unauthorized),
            device("B2", "Model B", Some("Kitchen"), DeviceState::Online),
            device("D4", "Model D", None, DeviceState::Offline),
            device("F6", "Model F", Some("Desk"), DeviceState::Disconnected),
        ];
        assert_eq!(
            screen_entries(&devices, &TrayTexts::default()),
            [entry(Some("A1"), "Model A"), entry(Some("B2"), "Kitchen")]
        );
    }

    #[test]
    fn the_screen_submenu_says_so_when_no_device_is_ready_and_in_the_language_of_the_texts() {
        let texts = TrayTexts { none: "nothing".into(), ..TrayTexts::default() };
        assert_eq!(screen_entries(&[], &texts), [entry(None, "nothing")]);
        let not_ready = [device("D4", "Model D", None, DeviceState::Offline)];
        assert_eq!(screen_entries(&not_ready, &texts), [entry(None, "nothing")]);
    }

    #[test]
    fn each_entry_id_gives_back_exactly_its_action_and_nothing_else_does() {
        assert_eq!(action_of_entry("tray-show"), Some(TrayAction::Show));
        assert_eq!(action_of_entry("tray-quit"), Some(TrayAction::Quit));
        for id in ["TESTDEV0001", "192.168.1.5:40000", "adb-x._adb-tls-connect._tcp."] {
            assert_eq!(action_of_entry(&entry_id(id)), Some(TrayAction::OpenDeviceWindow(id.to_string())));
            assert_eq!(action_of_entry(&screen_entry_id(id)), Some(TrayAction::OpenScreen(id.to_string())));
        }
        // The two kinds of entry of one device are different entries.
        assert_ne!(entry_id("A1"), screen_entry_id("A1"));
        for other in [
            "", "tray-none", "tray-devices", "tray-screens", "tray-device:", "tray-device:zz", "tray-screen:",
            "tray-screen:zz", "tray-screen:4a", "Tray-Show", "tray-show ", "device-4142",
        ] {
            assert_eq!(action_of_entry(other), None, "{other:?}");
        }
    }

}
