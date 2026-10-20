//! The buttons of the floating toolbar: the built-in ones, the ones add-ons
//! register, and the user's choice of which to show and in what order.

use crate::core::scrcpy::protocol::{ControlMessage, KeyAction};
use std::collections::HashSet;
use std::sync::Arc;

/// What an add-on does when its button of the toolbar is pressed: given the device and, when the toolbar is of
/// the window of an app, the package of that app (none for the screen of the device itself).
pub type ButtonHandler = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;

/// Identifies a button. Add-on buttons carry the add-on's id, so two add-ons
/// can use the same button id and an add-on can never take the place of a
/// built-in button. In JSON the built-in ones are plain lower-case names
/// (`"back"`, `"volume_up"`) and an add-on button is
/// `{"addon": {"addon": "<add-on id>", "id": "<button id>"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonId {
    Back,
    Home,
    AppSwitch,
    Power,
    VolumeUp,
    VolumeDown,
    Rotate,
    Notifications,
    SendFile,
    Screenshot,
    Addon { addon: String, id: String },
}

/// What a built-in button does on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCommand {
    /// Presses an Android key (the number is the Android key code).
    KeyEvent(u32),
    /// Opens the notification panel.
    ExpandNotifications,
}

/// The action of a built-in button. Buttons that have none yet (rotate, send
/// file, screenshot) and add-on buttons, whose action belongs to the add-on,
/// give `None`.
pub fn device_command(button: &ButtonId) -> Option<DeviceCommand> {
    // Key codes from Android's key code list (`KEYCODE_*`).
    match button {
        ButtonId::Back => Some(DeviceCommand::KeyEvent(4)),
        ButtonId::Home => Some(DeviceCommand::KeyEvent(3)),
        ButtonId::AppSwitch => Some(DeviceCommand::KeyEvent(187)),
        ButtonId::Power => Some(DeviceCommand::KeyEvent(26)),
        ButtonId::VolumeUp => Some(DeviceCommand::KeyEvent(24)),
        ButtonId::VolumeDown => Some(DeviceCommand::KeyEvent(25)),
        ButtonId::Notifications => Some(DeviceCommand::ExpandNotifications),
        ButtonId::Rotate | ButtonId::SendFile | ButtonId::Screenshot | ButtonId::Addon { .. } => None,
    }
}

/// The shell command that carries out `command` on a device.
pub fn shell_command(command: &DeviceCommand) -> String {
    match command {
        DeviceCommand::KeyEvent(code) => format!("input keyevent {code}"),
        // `cmd statusbar expand-notifications` is a command of Android's status bar service.
        DeviceCommand::ExpandNotifications => "cmd statusbar expand-notifications".to_string(),
    }
}

/// The control messages that carry out `command` through the proxy: faster than
/// starting a command on the device, and in the order they must be sent.
pub fn control_messages(command: &DeviceCommand) -> Vec<ControlMessage> {
    match command {
        DeviceCommand::KeyEvent(code) => [KeyAction::Down, KeyAction::Up]
            .into_iter()
            .map(|action| ControlMessage::InjectKeycode { action, keycode: *code, repeat: 0, meta_state: 0 })
            .collect(),
        DeviceCommand::ExpandNotifications => vec![ControlMessage::ExpandNotificationPanel],
    }
}

/// The control messages of a button that can only act through the proxy's
/// control socket (there is no command on the device that does it).
pub fn control_only_messages(button: &ButtonId) -> Option<Vec<ControlMessage>> {
    match button {
        ButtonId::Rotate => Some(vec![ControlMessage::RotateDevice]),
        _ => None,
    }
}

/// Why a button cannot be pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    /// It has no action yet.
    NotYet,
    /// It acts through the control socket, and this screen was not opened with
    /// the proxy (or scrcpy has no control socket).
    NeedsProxy,
}

/// One entry of the toolbar as the toolbar window shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolbarButton {
    pub id: ButtonId,
    /// The text of a button of an add-on (the built-in ones have their own, translated, in the UI).
    pub label: Option<String>,
    /// False for a button that cannot be pressed now; it is shown greyed out.
    pub enabled: bool,
    /// Why not, when it cannot be pressed.
    pub unavailable: Option<Unavailable>,
}

/// The toolbar's buttons in the order they are shown. `control` says whether
/// the screen of the device has a control socket through the proxy. `adb_works`
/// says whether a command through adb reaches what the toolbar belongs to: it
/// does for the device's own screen, but not for the window of an app on a
/// display of its own, where only the control socket reaches the window.
pub fn toolbar_buttons(
    available: &[ButtonId],
    prefs: &[ButtonPref],
    control: bool,
    adb_works: bool,
) -> Vec<ToolbarButton> {
    resolve_buttons(available, prefs)
        .into_iter()
        .map(|id| {
            let unavailable = if matches!(id, ButtonId::Addon { .. }) {
                // What it does is the add-on's own, not the proxy's.
                None
            } else if device_command(&id).is_some() {
                (!adb_works && !control).then_some(Unavailable::NeedsProxy)
            } else if control_only_messages(&id).is_some() {
                (!control).then_some(Unavailable::NeedsProxy)
            } else {
                Some(Unavailable::NotYet)
            };
            ToolbarButton { id, label: None, enabled: unavailable.is_none(), unavailable }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// Add-on and button ids are 1 to 64 characters of ASCII letters, digits,
    /// `_` and `-`.
    InvalidId,
    /// The text is empty, longer than 64 characters or has control characters.
    InvalidLabel,
    AlreadyRegistered,
}

/// A button: its id, and for an add-on's its text and what it does.
struct ButtonEntry {
    id: ButtonId,
    label: Option<String>,
    handler: Option<ButtonHandler>,
}

/// All buttons that exist right now: the built-in ones, in their default
/// order, followed by those that add-ons registered.
pub struct ButtonRegistry {
    buttons: Vec<ButtonEntry>,
}

/// The built-in buttons in their default order.
fn built_in() -> Vec<ButtonId> {
    vec![
        ButtonId::Back,
        ButtonId::Home,
        ButtonId::AppSwitch,
        ButtonId::Power,
        ButtonId::VolumeUp,
        ButtonId::VolumeDown,
        ButtonId::Rotate,
        ButtonId::Notifications,
        ButtonId::SendFile,
        ButtonId::Screenshot,
    ]
}

const MAX_NAME_LEN: usize = 64;

/// 1 to 64 ASCII letters, digits, `_` or `-`.
pub(crate) fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME_LEN).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl ButtonRegistry {
    /// A registry holding only the built-in buttons.
    pub fn new() -> Self {
        ButtonRegistry { buttons: built_in().into_iter().map(|id| ButtonEntry { id, label: None, handler: None }).collect() }
    }

    /// Registers a button of an add-on, with its text and what it does, and returns its id.
    pub fn register(
        &mut self,
        addon: &str,
        id: &str,
        label: &str,
        handler: ButtonHandler,
    ) -> Result<ButtonId, RegisterError> {
        if !valid_name(addon) || !valid_name(id) {
            return Err(RegisterError::InvalidId);
        }
        let label = label.trim();
        if label.is_empty()
            || label.chars().count() > super::menu::MAX_LABEL_CHARS
            || label.chars().any(char::is_control)
        {
            return Err(RegisterError::InvalidLabel);
        }
        let id = ButtonId::Addon { addon: addon.to_string(), id: id.to_string() };
        if self.buttons.iter().any(|b| b.id == id) {
            return Err(RegisterError::AlreadyRegistered);
        }
        self.buttons.push(ButtonEntry { id: id.clone(), label: Some(label.to_string()), handler: Some(handler) });
        Ok(id)
    }

    /// Removes every button of one add-on (it was disabled or uninstalled).
    pub fn remove_addon(&mut self, addon: &str) {
        self.buttons.retain(|b| !matches!(&b.id, ButtonId::Addon { addon: owner, .. } if owner == addon));
    }

    pub fn all(&self) -> Vec<ButtonId> {
        self.buttons.iter().map(|b| b.id.clone()).collect()
    }

    /// The text of a button of an add-on.
    pub fn label_of(&self, id: &ButtonId) -> Option<String> {
        self.buttons.iter().find(|b| &b.id == id).and_then(|b| b.label.clone())
    }

    /// What a button of an add-on does.
    pub fn handler(&self, id: &ButtonId) -> Option<ButtonHandler> {
        self.buttons.iter().find(|b| &b.id == id).and_then(|b| b.handler.clone())
    }
}

impl Default for ButtonRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// One entry of the user's toolbar layout, in the order the user arranged.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ButtonPref {
    pub id: ButtonId,
    pub visible: bool,
}

/// One row of the list of the toolbar's buttons in the settings: every button there is, shown or not, in the order the
/// toolbar has them (with the text an add-on gave its button).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LayoutEntry {
    pub id: ButtonId,
    pub label: Option<String>,
    pub visible: bool,
}

/// Every button that exists, in the order the toolbar has them, each with whether it is shown: what `resolve_buttons`
/// keeps is the entries that are shown. The rules are those of `resolve_buttons`.
pub fn resolve_layout(available: &[ButtonId], prefs: &[ButtonPref]) -> Vec<ButtonPref> {
    let mut decided: HashSet<&ButtonId> = HashSet::new();
    let mut layout = Vec::new();
    for pref in prefs {
        if available.contains(&pref.id) && decided.insert(&pref.id) {
            layout.push(pref.clone());
        }
    }
    for button in available {
        if decided.insert(button) {
            layout.push(ButtonPref { id: button.clone(), visible: true });
        }
    }
    layout
}

/// The buttons to show, in order.
///
/// * The user's layout (`prefs`) decides order and visibility.
/// * Buttons that exist (`available`) but are not in the layout yet, such as
///   a freshly installed add-on's, are shown after the others.
/// * Layout entries for buttons that no longer exist are ignored; the layout
///   itself is not changed, so they come back if the add-on does.
/// * If a button appears twice in the layout, its first entry counts.
pub fn resolve_buttons(available: &[ButtonId], prefs: &[ButtonPref]) -> Vec<ButtonId> {
    resolve_layout(available, prefs).into_iter().filter(|p| p.visible).map(|p| p.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addon(addon: &str, id: &str) -> ButtonId {
        ButtonId::Addon { addon: addon.into(), id: id.into() }
    }
    fn pref(id: ButtonId, visible: bool) -> ButtonPref {
        ButtonPref { id, visible }
    }
    fn built_in() -> Vec<ButtonId> {
        vec![
            ButtonId::Back,
            ButtonId::Home,
            ButtonId::AppSwitch,
            ButtonId::Power,
            ButtonId::VolumeUp,
            ButtonId::VolumeDown,
            ButtonId::Rotate,
            ButtonId::Notifications,
            ButtonId::SendFile,
            ButtonId::Screenshot,
        ]
    }

    // layout -------------------------------------------------------------------

    fn shown(entries: &[ButtonPref]) -> Vec<(ButtonId, bool)> {
        entries.iter().map(|p| (p.id.clone(), p.visible)).collect()
    }

    #[test]
    fn without_a_layout_every_button_is_there_and_shown_in_the_order_they_exist() {
        let available = built_in();
        let layout = resolve_layout(&available, &[]);
        assert_eq!(layout.len(), available.len());
        assert!(layout.iter().all(|p| p.visible));
        assert_eq!(layout.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), available);
    }

    #[test]
    fn the_layout_keeps_the_order_the_user_arranged_with_the_hidden_buttons_in_it_and_adds_what_is_new_after() {
        let mut available = built_in();
        available.push(addon("tools", "tap"));
        let prefs = [
            pref(ButtonId::Home, true),
            pref(addon("tools", "tap"), false),
            pref(addon("gone", "x"), true), // an add-on that is not there now
            pref(ButtonId::Home, false),    // a second entry of the same button
            pref(ButtonId::Back, false),
        ];
        let layout = resolve_layout(&available, &prefs);
        let head = shown(&layout[..3]);
        assert_eq!(
            head,
            vec![(ButtonId::Home, true), (addon("tools", "tap"), false), (ButtonId::Back, false)],
            "the user's entries first, in their order, the hidden ones with them; the one of a missing add-on and the repeat left out"
        );
        // Everything else that exists comes after, shown, in the order it exists.
        let rest: Vec<ButtonId> = layout[3..].iter().map(|p| p.id.clone()).collect();
        let expected_rest: Vec<ButtonId> = available
            .iter()
            .filter(|id| !matches!(id, ButtonId::Home | ButtonId::Back) && **id != addon("tools", "tap"))
            .cloned()
            .collect();
        assert_eq!(rest, expected_rest);
        assert!(layout[3..].iter().all(|p| p.visible));
        assert_eq!(layout.len(), available.len());
    }

    #[test]
    fn the_buttons_that_are_shown_are_the_entries_of_the_layout_that_are_visible() {
        let available = built_in();
        let prefs = [pref(ButtonId::Rotate, true), pref(ButtonId::Home, false), pref(ButtonId::Back, true)];
        let from_layout: Vec<ButtonId> = resolve_layout(&available, &prefs).into_iter().filter(|p| p.visible).map(|p| p.id).collect();
        assert_eq!(resolve_buttons(&available, &prefs), from_layout);
        assert_eq!(&from_layout[..2], &[ButtonId::Rotate, ButtonId::Back]);
    }

    // registry -----------------------------------------------------------------

    #[test]
    fn a_new_registry_has_the_built_in_buttons_in_their_default_order() {
        assert_eq!(ButtonRegistry::new().all(), built_in());
    }

    /// A button registered with a text and a handler that does nothing.
    fn add(reg: &mut ButtonRegistry, addon_id: &str, button_id: &str) -> Result<ButtonId, RegisterError> {
        reg.register(addon_id, button_id, "Label", Arc::new(|_, _| {}))
    }

    #[test]
    fn an_addon_button_is_added_after_the_built_in_ones() {
        let mut reg = ButtonRegistry::new();
        assert_eq!(add(&mut reg, "clicker", "auto-tap"), Ok(addon("clicker", "auto-tap")));
        let mut expected = built_in();
        expected.push(addon("clicker", "auto-tap"));
        assert_eq!(reg.all(), expected);
    }

    #[test]
    fn registering_the_same_button_twice_is_refused() {
        let mut reg = ButtonRegistry::new();
        assert!(add(&mut reg, "clicker", "auto-tap").is_ok());
        assert_eq!(add(&mut reg, "clicker", "auto-tap"), Err(RegisterError::AlreadyRegistered));
        assert_eq!(reg.all().len(), 11);
    }

    #[test]
    fn two_addons_may_use_the_same_button_id() {
        let mut reg = ButtonRegistry::new();
        assert!(add(&mut reg, "one", "go").is_ok());
        assert_eq!(add(&mut reg, "two", "go"), Ok(addon("two", "go")));
        // Both are really kept, not just reported as accepted.
        let mut expected = built_in();
        expected.push(addon("one", "go"));
        expected.push(addon("two", "go"));
        assert_eq!(reg.all(), expected);
    }

    #[test]
    fn an_addon_cannot_take_the_place_of_a_built_in_button() {
        let mut reg = ButtonRegistry::new();
        // Even an id that looks like a built-in one only creates an add-on button.
        assert_eq!(add(&mut reg, "evil", "back"), Ok(addon("evil", "back")));
        // The built-in Back button is still there once, in its place, and the
        // add-on's button is a separate one at the end.
        let mut expected = built_in();
        expected.push(addon("evil", "back"));
        assert_eq!(reg.all(), expected);
    }

    #[test]
    fn removing_an_addon_removes_only_its_buttons() {
        let mut reg = ButtonRegistry::new();
        for (a, b) in [("one", "a"), ("two", "a"), ("one", "b")] {
            assert!(add(&mut reg, a, b).is_ok(), "{a} {b}");
        }
        reg.remove_addon("one");
        let mut expected = built_in();
        expected.push(addon("two", "a"));
        assert_eq!(reg.all(), expected);
    }

    #[test]
    fn ids_that_are_empty_too_long_or_use_other_characters_are_refused() {
        let mut reg = ButtonRegistry::new();
        let too_long = "a".repeat(65);
        for (addon_id, button_id) in [
            ("", "x"),
            ("x", ""),
            ("has space", "x"),
            ("x", "has space"),
            ("x", "knöpf"),
            ("x", too_long.as_str()),
            ("a/b", "x"),
        ] {
            assert_eq!(add(&mut reg, addon_id, button_id), Err(RegisterError::InvalidId), "{addon_id:?} {button_id:?}");
        }
        assert!(add(&mut reg, "x", &"a".repeat(64)).is_ok());
        assert!(add(&mut reg, "under_score-1", "Id_9").is_ok());
    }

    #[test]
    fn a_text_that_is_empty_too_long_or_has_a_control_character_is_refused_and_one_that_is_fine_is_trimmed() {
        let mut reg = ButtonRegistry::new();
        let long = "n".repeat(65);
        for label in ["", "   ", long.as_str(), "two\nlines"] {
            let result = reg.register("x", "go", label, Arc::new(|_, _| {}));
            assert_eq!(result, Err(RegisterError::InvalidLabel), "{label:?}");
        }
        assert_eq!(reg.all().len(), 10, "nothing was kept");
        assert_eq!(reg.register("x", "go", "  Do it  ", Arc::new(|_, _| {})), Ok(addon("x", "go")));
        assert_eq!(reg.label_of(&addon("x", "go")).as_deref(), Some("Do it"));
        assert!(reg.register("x", "long", &"n".repeat(64), Arc::new(|_, _| {})).is_ok());
    }

    #[test]
    fn a_button_has_the_text_and_the_handler_it_was_registered_with_and_the_built_in_ones_have_none() {
        use std::sync::Mutex;
        let mut reg = ButtonRegistry::new();
        type Call = (String, Option<String>);
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let seen = calls.clone();
        let handler: ButtonHandler = Arc::new(move |device, app| seen.lock().unwrap().push((device.into(), app.map(String::from))));
        assert_eq!(reg.register("tools", "tap", "Tap", handler), Ok(addon("tools", "tap")));
        assert_eq!(reg.label_of(&addon("tools", "tap")).as_deref(), Some("Tap"));
        let found = reg.handler(&addon("tools", "tap"));
        assert!(found.is_some(), "the handler is kept");
        if let Some(handler) = found {
            handler("dev-1", None);
            handler("dev-2", Some("com.example.app"));
        }
        assert_eq!(
            *calls.lock().unwrap(),
            vec![("dev-1".to_string(), None), ("dev-2".to_string(), Some("com.example.app".to_string()))]
        );
        assert_eq!((reg.label_of(&ButtonId::Back), reg.handler(&ButtonId::Back).is_some()), (None, false));
        assert_eq!((reg.label_of(&addon("tools", "other")), reg.handler(&addon("tools", "other")).is_some()), (None, false));
        // What was registered goes with its add-on.
        reg.remove_addon("tools");
        assert_eq!((reg.label_of(&addon("tools", "tap")), reg.handler(&addon("tools", "tap")).is_some()), (None, false));
    }

    // what a button does ---------------------------------------------------------

    #[test]
    fn built_in_buttons_press_the_matching_android_key() {
        // Key codes from Android's own key code list.
        let expected = [
            (ButtonId::Back, 4),
            (ButtonId::Home, 3),
            (ButtonId::AppSwitch, 187),
            (ButtonId::Power, 26),
            (ButtonId::VolumeUp, 24),
            (ButtonId::VolumeDown, 25),
        ];
        for (button, code) in expected {
            assert_eq!(device_command(&button), Some(DeviceCommand::KeyEvent(code)), "{button:?}");
        }
    }

    #[test]
    fn the_notifications_button_opens_the_notification_panel() {
        assert_eq!(device_command(&ButtonId::Notifications), Some(DeviceCommand::ExpandNotifications));
    }

    #[test]
    fn buttons_without_an_action_of_their_own_give_none() {
        for button in [ButtonId::Rotate, ButtonId::SendFile, ButtonId::Screenshot, addon("clicker", "auto-tap")] {
            assert_eq!(device_command(&button), None, "{button:?}");
        }
    }

    #[test]
    fn commands_become_the_shell_commands_that_carry_them_out() {
        // The device is chosen by the connection, so the serial is not part of the command.
        assert_eq!(shell_command(&DeviceCommand::KeyEvent(4)), "input keyevent 4");
        assert_eq!(shell_command(&DeviceCommand::KeyEvent(187)), "input keyevent 187");
        assert_eq!(shell_command(&DeviceCommand::ExpandNotifications), "cmd statusbar expand-notifications");
    }

    #[test]
    fn commands_have_control_messages_too_a_key_goes_down_and_then_up() {
        let key = |action, keycode| ControlMessage::InjectKeycode { action, keycode, repeat: 0, meta_state: 0 };
        assert_eq!(
            control_messages(&DeviceCommand::KeyEvent(187)),
            [key(KeyAction::Down, 187), key(KeyAction::Up, 187)]
        );
        assert_eq!(control_messages(&DeviceCommand::ExpandNotifications), [ControlMessage::ExpandNotificationPanel]);
    }

    #[test]
    fn button_ids_cross_between_the_ui_and_the_core_as_json() {
        let cases = [
            (ButtonId::Back, "\"back\""),
            (ButtonId::AppSwitch, "\"app_switch\""),
            (ButtonId::VolumeUp, "\"volume_up\""),
            (ButtonId::SendFile, "\"send_file\""),
            (addon("clicker", "auto-tap"), r#"{"addon":{"addon":"clicker","id":"auto-tap"}}"#),
        ];
        for (button, json) in cases {
            assert_eq!(serde_json::to_string(&button).unwrap(), json);
            assert_eq!(serde_json::from_str::<ButtonId>(json).unwrap(), button);
        }
        assert!(serde_json::from_str::<ButtonId>("\"teleport\"").is_err());
    }

    // resolve_buttons ----------------------------------------------------------

    #[test]
    fn without_a_layout_every_button_is_shown_in_the_default_order() {
        assert_eq!(resolve_buttons(&built_in(), &[]), built_in());
    }

    #[test]
    fn the_users_order_wins_and_hidden_buttons_are_left_out() {
        let prefs = [
            pref(ButtonId::Home, true),
            pref(ButtonId::Back, true),
            pref(ButtonId::Power, false),
        ];
        // Home and Back first, Power left out, everything else follows in the
        // default order.
        assert_eq!(
            resolve_buttons(&built_in(), &prefs),
            vec![
                ButtonId::Home,
                ButtonId::Back,
                ButtonId::AppSwitch,
                ButtonId::VolumeUp,
                ButtonId::VolumeDown,
                ButtonId::Rotate,
                ButtonId::Notifications,
                ButtonId::SendFile,
                ButtonId::Screenshot,
            ]
        );
    }

    #[test]
    fn a_newly_registered_button_appears_at_the_end() {
        let mut available = built_in();
        available.push(addon("clicker", "auto-tap"));
        let prefs = [pref(ButtonId::Back, true)];
        // Everything the layout does not mention, the new button last.
        assert_eq!(resolve_buttons(&available, &prefs), available);
    }

    #[test]
    fn layout_entries_for_buttons_that_no_longer_exist_are_ignored() {
        let prefs = [pref(addon("gone", "x"), true), pref(ButtonId::Home, true)];
        // Home moves to the front; the entry for the missing add-on button
        // changes nothing else.
        let mut expected = vec![ButtonId::Home];
        expected.extend(built_in().into_iter().filter(|b| *b != ButtonId::Home));
        assert_eq!(resolve_buttons(&built_in(), &prefs), expected);
    }

    #[test]
    fn when_a_button_is_listed_twice_the_first_entry_counts() {
        // The first entry hides Back; the second one is ignored.
        let prefs = [pref(ButtonId::Back, false), pref(ButtonId::Back, true)];
        let expected: Vec<_> = built_in().into_iter().filter(|b| *b != ButtonId::Back).collect();
        assert_eq!(resolve_buttons(&built_in(), &prefs), expected);
        // ...and the other way round.
        let prefs = [pref(ButtonId::Back, true), pref(ButtonId::Back, false)];
        assert_eq!(resolve_buttons(&built_in(), &prefs), built_in());
    }

    #[test]
    fn hiding_everything_leaves_an_empty_toolbar() {
        let prefs: Vec<_> = built_in().into_iter().map(|b| pref(b, false)).collect();
        assert!(resolve_buttons(&built_in(), &prefs).is_empty());
    }
}
