//! What an add-on is told as it happens: a device becomes usable or stops being, a device is added (this
//! computer knows it for the first time: it is not the pairing of adb, which is on the phone, and a device
//! that comes by USB is added too), a screen opens or closes, a file is sent or opened. Each is a value of `AddonEvent`; the functions here make them
//! from what the core already knows (the list of the devices, the screens, the updates of a sending), so nothing
//! is guessed from text. An event only says what happened: it needs no permission, and it carries no serial
//! number and no address of a device, which the add-ons have no use for.

use super::api::{DeviceEventData, FilesOpenedEventData, FilesSentEventData, ScreenEventData, SettingChangedEventData};
use crate::core::apps::SessionKey;
use crate::core::error::CoreError;
use crate::core::model::{Device, DeviceState, OpenedUpdate, Snapshot, TransferPhase, TransferUpdate};
use serde_json::Value;

/// The names an add-on subscribes by (`host.on("device.online", ...)`).
pub const EVENT_NAMES: [&str; 8] = [
    "device.online",
    "device.offline",
    "device.added",
    "screen.started",
    "screen.ended",
    "files.sent",
    "files.opened",
    "settings.changed",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddonEvent {
    /// The user changed a setting of this add-on (only this add-on is told).
    SettingChanged { key: String, value: super::settings::SettingValue },
    /// A device became usable. `name` is what the program calls it (the user's own name for it, else its model).
    DeviceOnline { device: String, name: String },
    /// A device that was usable is not any more (it went away, or it is not ready).
    DeviceOffline { device: String, name: String },
    /// A device this computer did not know is known now (the first time it is seen usable, by USB or by Wi-Fi).
    DeviceAdded { device: String, name: String },
    /// A screen opened: the whole screen of the device (`app` is none) or the window of an app.
    ScreenStarted { device: String, app: Option<String> },
    ScreenEnded { device: String, app: Option<String> },
    /// A file was sent to a device; `error` is the code of why it was not.
    FilesSent { device: String, name: String, error: Option<String> },
    /// A file was opened on a device; `error` is the code of why it was not.
    FilesOpened { device: String, file: String, error: Option<String> },
}

impl AddonEvent {
    /// The name an add-on subscribes by.
    pub fn name(&self) -> &'static str {
        match self {
            AddonEvent::DeviceOnline { .. } => "device.online",
            AddonEvent::DeviceOffline { .. } => "device.offline",
            AddonEvent::DeviceAdded { .. } => "device.added",
            AddonEvent::ScreenStarted { .. } => "screen.started",
            AddonEvent::ScreenEnded { .. } => "screen.ended",
            AddonEvent::FilesSent { .. } => "files.sent",
            AddonEvent::FilesOpened { .. } => "files.opened",
            AddonEvent::SettingChanged { .. } => "settings.changed",
        }
    }

    /// What the callbacks are given: an object with these members (and no others).
    pub fn data(&self) -> Value {
        // The types of `api` are what the TypeScript types are written from, so what arrives is what they say.
        let data = match self {
            AddonEvent::SettingChanged { key, value } => {
                serde_json::to_value(SettingChangedEventData { key: key.clone(), value: value.clone() })
            }
            AddonEvent::DeviceOnline { device, name }
            | AddonEvent::DeviceOffline { device, name }
            | AddonEvent::DeviceAdded { device, name } => {
                serde_json::to_value(DeviceEventData { device: device.clone(), name: name.clone() })
            }
            AddonEvent::ScreenStarted { device, app } | AddonEvent::ScreenEnded { device, app } => {
                serde_json::to_value(ScreenEventData { device: device.clone(), app: app.clone() })
            }
            AddonEvent::FilesSent { device, name, error } => serde_json::to_value(FilesSentEventData {
                device: device.clone(),
                name: name.clone(),
                ok: error.is_none(),
                error: error.clone(),
            }),
            AddonEvent::FilesOpened { device, file, error } => serde_json::to_value(FilesOpenedEventData {
                device: device.clone(),
                file: file.clone(),
                ok: error.is_none(),
                error: error.clone(),
            }),
        };
        data.unwrap_or(Value::Null)
    }
}

/// The name a device is shown by: the user's own, else the model.
fn shown(device: &Device) -> String {
    device.label.clone().unwrap_or_else(|| device.name.clone())
}

fn usable(device: &Device) -> bool {
    device.state == DeviceState::Online
}

/// What happened to the devices between two lists: the ones that stopped being usable (in the order of the old
/// list), the ones that became usable and the ones that are added (in the order of the new).
pub fn device_events(before: &[Device], after: &[Device]) -> Vec<AddonEvent> {
    let was = |id: &str| before.iter().find(|d| d.id == id);
    let is = |id: &str| after.iter().find(|d| d.id == id);
    let mut events = Vec::new();
    for device in before.iter().filter(|d| usable(d)) {
        if !is(&device.id).is_some_and(usable) {
            events.push(AddonEvent::DeviceOffline { device: device.id.clone(), name: shown(device) });
        }
    }
    for device in after.iter().filter(|d| usable(d)) {
        if !was(&device.id).is_some_and(usable) {
            events.push(AddonEvent::DeviceOnline { device: device.id.clone(), name: shown(device) });
        }
    }
    for device in after.iter().filter(|d| d.known) {
        if !was(&device.id).is_some_and(|d| d.known) {
            events.push(AddonEvent::DeviceAdded { device: device.id.clone(), name: shown(device) });
        }
    }
    events
}

/// The event of the screen a key names: the screen of a device, or the window of an app on it.
fn screen_event(key: &str, started: bool) -> AddonEvent {
    let key = SessionKey::parse(key);
    let app = key.app.map(|app| app.as_str().to_string());
    if started {
        AddonEvent::ScreenStarted { device: key.device, app }
    } else {
        AddonEvent::ScreenEnded { device: key.device, app }
    }
}

/// What happened to the screens between two lists of their keys: the ones that closed, then the ones that opened.
pub fn screen_events(before: &[String], after: &[String]) -> Vec<AddonEvent> {
    let ended = before.iter().filter(|key| !after.contains(key)).map(|key| screen_event(key, false));
    let started = after.iter().filter(|key| !before.contains(key)).map(|key| screen_event(key, true));
    ended.chain(started).collect()
}

/// What happened between two states: the devices, then the screens.
pub fn snapshot_events(before: &Snapshot, after: &Snapshot) -> Vec<AddonEvent> {
    let mut events = device_events(&before.devices, &after.devices);
    events.extend(screen_events(&before.sessions, &after.sessions));
    events
}

/// What is the case now, for an add-on that starts: the devices that are usable, then the screens that are open.
pub fn current_events(snapshot: &Snapshot) -> Vec<AddonEvent> {
    let devices = snapshot
        .devices
        .iter()
        .filter(|d| usable(d))
        .map(|d| AddonEvent::DeviceOnline { device: d.id.clone(), name: shown(d) });
    let screens = snapshot.sessions.iter().map(|key| screen_event(key, true));
    devices.chain(screens).collect()
}

/// The end of a sending (the updates while it goes on are not events).
pub fn transfer_event(update: &TransferUpdate) -> Option<AddonEvent> {
    let error = match update.phase {
        TransferPhase::Sending => return None,
        TransferPhase::Done => None,
        // A failure that does not say why is still one.
        TransferPhase::Failed => Some(update.error.as_ref().map_or_else(|| "other".to_string(), CoreError::code)),
    };
    Some(AddonEvent::FilesSent { device: update.device.clone(), name: update.name.clone(), error })
}

pub fn opened_event(update: &OpenedUpdate) -> AddonEvent {
    AddonEvent::FilesOpened {
        device: update.device.clone(),
        file: update.file.clone(),
        error: update.error.as_ref().map(CoreError::code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ScrcpyStatus, Settings};
    use serde_json::json;

    // Devices and updates are made up; their form is the program's own.

    fn device(id: &str, name: &str, state: DeviceState, known: bool) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            label: None,
            state,
            transport: None,
            adb_serial: None,
            address: None,
            android: None,
            sdk: None,
            known,
        }
    }

    fn online(device: &str, name: &str) -> AddonEvent {
        AddonEvent::DeviceOnline { device: device.into(), name: name.into() }
    }
    fn offline(device: &str, name: &str) -> AddonEvent {
        AddonEvent::DeviceOffline { device: device.into(), name: name.into() }
    }
    fn added(device: &str, name: &str) -> AddonEvent {
        AddonEvent::DeviceAdded { device: device.into(), name: name.into() }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_event_has_the_name_an_add_on_subscribes_by_and_the_members_its_callbacks_are_given() {
        let all = [
            (online("d1", "Phone"), "device.online", json!({ "device": "d1", "name": "Phone" })),
            (offline("d1", "Phone"), "device.offline", json!({ "device": "d1", "name": "Phone" })),
            (added("d1", "Phone"), "device.added", json!({ "device": "d1", "name": "Phone" })),
            (
                AddonEvent::ScreenStarted { device: "d1".into(), app: None },
                "screen.started",
                json!({ "device": "d1", "app": null }),
            ),
            (
                AddonEvent::ScreenEnded { device: "d1".into(), app: Some("com.example.app".into()) },
                "screen.ended",
                json!({ "device": "d1", "app": "com.example.app" }),
            ),
            (
                AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: None },
                "files.sent",
                json!({ "device": "d1", "name": "a.txt", "ok": true, "error": null }),
            ),
            (
                AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: Some("not_online".into()) },
                "files.sent",
                json!({ "device": "d1", "name": "a.txt", "ok": false, "error": "not_online" }),
            ),
            (
                AddonEvent::FilesOpened { device: "d1".into(), file: "b.pdf".into(), error: Some("no_app".into()) },
                "files.opened",
                json!({ "device": "d1", "file": "b.pdf", "ok": false, "error": "no_app" }),
            ),
            (
                AddonEvent::SettingChanged { key: "level".into(), value: super::super::settings::SettingValue::Number(7) },
                "settings.changed",
                json!({ "key": "level", "value": 7 }),
            ),
            (
                AddonEvent::SettingChanged { key: "name".into(), value: super::super::settings::SettingValue::Text("x".into()) },
                "settings.changed",
                json!({ "key": "name", "value": "x" }),
            ),
        ];
        for (event, name, data) in all {
            assert_eq!((event.name(), event.data()), (name, data), "{event:?}");
        }
        // The names there are, each once, are the ones a subscription is checked against.
        let names: Vec<&str> = [
            online("d", "n").name(),
            offline("d", "n").name(),
            added("d", "n").name(),
            AddonEvent::ScreenStarted { device: "d".into(), app: None }.name(),
            AddonEvent::ScreenEnded { device: "d".into(), app: None }.name(),
            AddonEvent::FilesSent { device: "d".into(), name: "n".into(), error: None }.name(),
            AddonEvent::FilesOpened { device: "d".into(), file: "f".into(), error: None }.name(),
            AddonEvent::SettingChanged { key: "k".into(), value: super::super::settings::SettingValue::Boolean(true) }.name(),
        ]
        .to_vec();
        assert_eq!(names, EVENT_NAMES);
    }

    #[test]
    fn a_device_that_becomes_usable_is_online_and_one_that_stops_being_usable_is_offline() {
        let known = |state| device("d1", "Phone", state, true);
        assert_eq!(device_events(&[known(DeviceState::Disconnected)], &[known(DeviceState::Online)]), vec![online("d1", "Phone")]);
        assert_eq!(device_events(&[known(DeviceState::Online)], &[known(DeviceState::Disconnected)]), vec![offline("d1", "Phone")]);
        assert_eq!(device_events(&[known(DeviceState::Unauthorized)], &[known(DeviceState::Online)]), vec![online("d1", "Phone")]);
        assert_eq!(device_events(&[known(DeviceState::Online)], &[known(DeviceState::Unauthorized)]), vec![offline("d1", "Phone")]);
        // One that goes out of the list altogether, and one that was never usable.
        assert_eq!(device_events(&[known(DeviceState::Online)], &[]), vec![offline("d1", "Phone")]);
        assert_eq!(device_events(&[known(DeviceState::Disconnected)], &[]), vec![]);
        // Nothing changed.
        assert_eq!(device_events(&[known(DeviceState::Online)], &[known(DeviceState::Online)]), vec![]);
        assert_eq!(device_events(&[], &[]), vec![]);
    }

    #[test]
    fn a_device_is_called_by_the_name_the_user_gave_it_and_a_new_name_alone_is_no_event() {
        let mut named = device("d1", "Model", DeviceState::Online, true);
        named.label = Some("Kitchen tablet".into());
        let mut asleep = named.clone();
        asleep.state = DeviceState::Disconnected;
        assert_eq!(device_events(&[asleep], &[named.clone()]), vec![online("d1", "Kitchen tablet")]);
        let mut renamed = named.clone();
        renamed.label = Some("Other".into());
        assert_eq!(device_events(&[named], &[renamed]), vec![]);
    }

    #[test]
    fn a_device_is_added_when_it_was_not_known_and_is_now_and_not_when_it_was_known_before() {
        let stranger = device("d1", "Phone", DeviceState::Discovered, false);
        let usable = device("d1", "Phone", DeviceState::Online, true);
        // A stranger that connects: usable, and known now.
        assert_eq!(
            device_events(std::slice::from_ref(&stranger), std::slice::from_ref(&usable)),
            vec![online("d1", "Phone"), added("d1", "Phone")]
        );
        // One that was not in the list at all.
        assert_eq!(device_events(&[], std::slice::from_ref(&usable)), vec![online("d1", "Phone"), added("d1", "Phone")]);
        // One this computer knew (it shows as disconnected) that connects again: not added now.
        let remembered = device("d1", "Phone", DeviceState::Disconnected, true);
        assert_eq!(device_events(&[remembered], &[usable]), vec![online("d1", "Phone")]);
        // A stranger that stays one.
        assert_eq!(device_events(std::slice::from_ref(&stranger), std::slice::from_ref(&stranger)), vec![]);
    }

    #[test]
    fn several_changes_come_in_the_order_offline_then_online_then_added_each_in_the_order_of_its_list() {
        let before = [
            device("a", "A", DeviceState::Online, true),
            device("b", "B", DeviceState::Online, true),
            device("c", "C", DeviceState::Discovered, false),
        ];
        let after = [
            device("c", "C", DeviceState::Online, true),
            device("d", "D", DeviceState::Online, true),
            device("b", "B", DeviceState::Disconnected, true),
        ];
        assert_eq!(
            device_events(&before, &after),
            vec![offline("a", "A"), offline("b", "B"), online("c", "C"), online("d", "D"), added("c", "C"), added("d", "D")]
        );
    }

    #[test]
    fn a_screen_that_closed_ends_and_one_that_opened_starts_and_the_window_of_an_app_names_the_app() {
        let started = |device: &str, app: Option<&str>| AddonEvent::ScreenStarted { device: device.into(), app: app.map(String::from) };
        let ended = |device: &str, app: Option<&str>| AddonEvent::ScreenEnded { device: device.into(), app: app.map(String::from) };
        assert_eq!(screen_events(&[], &strings(&["d1"])), vec![started("d1", None)]);
        assert_eq!(screen_events(&strings(&["d1"]), &[]), vec![ended("d1", None)]);
        assert_eq!(
            screen_events(&strings(&["d1#app:com.example.app"]), &strings(&["d1", "d2#app:org.example.other"])),
            vec![ended("d1", Some("com.example.app")), started("d1", None), started("d2", Some("org.example.other"))]
        );
        assert_eq!(screen_events(&strings(&["d1"]), &strings(&["d1"])), vec![]);
        // A key that is not an app's is the device's own, whatever it looks like.
        assert_eq!(screen_events(&[], &strings(&["d1#app:"])), vec![started("d1#app:", None)]);
    }

    fn snapshot(devices: Vec<Device>, sessions: &[&str]) -> Snapshot {
        Snapshot {
            scrcpy: ScrcpyStatus::Checking,
            scrcpy_wanted: None,
            devices,
            pairing: vec![],
            sessions: strings(sessions),
            settings: Settings::default(),
            data_dir: String::new(),
        }
    }

    #[test]
    fn what_happened_between_two_states_is_the_devices_then_the_screens() {
        let before = snapshot(vec![device("d1", "Phone", DeviceState::Online, true)], &["d1"]);
        let after = snapshot(vec![device("d1", "Phone", DeviceState::Disconnected, true)], &[]);
        assert_eq!(
            snapshot_events(&before, &after),
            vec![offline("d1", "Phone"), AddonEvent::ScreenEnded { device: "d1".into(), app: None }]
        );
    }

    #[test]
    fn what_is_the_case_now_is_the_usable_devices_then_the_open_screens() {
        let now = snapshot(
            vec![
                device("d1", "Phone", DeviceState::Online, true),
                device("d2", "Tablet", DeviceState::Disconnected, true),
                device("d3", "Watch", DeviceState::Online, true),
            ],
            &["d1", "d3#app:com.example.app"],
        );
        assert_eq!(
            current_events(&now),
            vec![
                online("d1", "Phone"),
                online("d3", "Watch"),
                AddonEvent::ScreenStarted { device: "d1".into(), app: None },
                AddonEvent::ScreenStarted { device: "d3".into(), app: Some("com.example.app".into()) },
            ]
        );
        assert_eq!(current_events(&snapshot(vec![], &[])), vec![]);
    }

    fn update(phase: TransferPhase, error: Option<CoreError>) -> TransferUpdate {
        TransferUpdate { id: 1, device: "d1".into(), name: "a.txt".into(), sent: 3, total: 10, phase, error }
    }

    #[test]
    fn only_the_end_of_a_sending_is_an_event_with_the_code_of_why_it_failed() {
        assert_eq!(transfer_event(&update(TransferPhase::Sending, None)), None);
        assert_eq!(
            transfer_event(&update(TransferPhase::Done, None)),
            Some(AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: None })
        );
        assert_eq!(
            transfer_event(&update(TransferPhase::Failed, Some(CoreError::NotOnline))),
            Some(AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: Some("not_online".into()) })
        );
        // A failure that says nothing of why is still a failure.
        assert_eq!(
            transfer_event(&update(TransferPhase::Failed, None)),
            Some(AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: Some("other".into()) })
        );
    }

    #[test]
    fn a_file_that_was_opened_or_not_is_an_event_with_the_code_of_why_not() {
        let opened = OpenedUpdate { device: "d1".into(), file: "b.pdf".into(), remote: Some("/sdcard/b.pdf".into()), error: None };
        assert_eq!(opened_event(&opened), AddonEvent::FilesOpened { device: "d1".into(), file: "b.pdf".into(), error: None });
        let failed = OpenedUpdate { remote: None, error: Some(CoreError::NoApp), ..opened };
        assert_eq!(
            opened_event(&failed),
            AddonEvent::FilesOpened { device: "d1".into(), file: "b.pdf".into(), error: Some("no_app".into()) }
        );
    }
}
