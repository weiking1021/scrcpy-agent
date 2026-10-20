//! The menu of a device card in the device list: the entries the application
//! has itself and those add-ons register, in the order they are shown.

use super::buttons;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Identifies an entry. Add-on entries carry the add-on's id, so two add-ons
/// can use the same entry id and an add-on can never take the place of a
/// built-in entry. In JSON the built-in ones are plain names (`"open_app"`) and
/// an add-on entry is `{"addon": {"addon": "<add-on id>", "id": "<entry id>"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuItemId {
    /// Opens the list of the device's apps to open one.
    OpenApp,
    Addon { addon: String, id: String },
}

/// One entry as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuItem {
    pub id: MenuItemId,
    /// The text of an add-on's entry. A built-in entry has none: the UI has its
    /// own (translated) text for it.
    pub label: Option<String>,
    pub enabled: bool,
}

/// What an add-on does when its entry is chosen, given the id of the device.
pub type MenuHandler = Arc<dyn Fn(&str) + Send + Sync>;

impl From<buttons::RegisterError> for MenuRegisterError {
    /// A button of the toolbar is refused for the same reasons as an entry of a menu.
    fn from(error: buttons::RegisterError) -> Self {
        match error {
            buttons::RegisterError::InvalidId => MenuRegisterError::InvalidId,
            buttons::RegisterError::InvalidLabel => MenuRegisterError::InvalidLabel,
            buttons::RegisterError::AlreadyRegistered => MenuRegisterError::AlreadyRegistered,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuRegisterError {
    /// Add-on and entry ids are 1 to 64 characters of ASCII letters, digits, `_` and `-`.
    InvalidId,
    /// The text is empty, longer than 64 characters or has control characters.
    InvalidLabel,
    AlreadyRegistered,
}

struct Entry {
    id: MenuItemId,
    label: Option<String>,
    handler: Option<MenuHandler>,
}

pub struct DeviceMenu {
    entries: Vec<Entry>,
}

pub(crate) const MAX_LABEL_CHARS: usize = 64;

impl DeviceMenu {
    /// A menu with the built-in entries only.
    pub fn new() -> Self {
        DeviceMenu { entries: vec![Entry { id: MenuItemId::OpenApp, label: None, handler: None }] }
    }

    /// Adds an entry of an add-on after the others.
    pub fn register(
        &mut self,
        addon: &str,
        id: &str,
        label: &str,
        handler: MenuHandler,
    ) -> Result<MenuItemId, MenuRegisterError> {
        if !buttons::valid_name(addon) || !buttons::valid_name(id) {
            return Err(MenuRegisterError::InvalidId);
        }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
            return Err(MenuRegisterError::InvalidLabel);
        }
        let id = MenuItemId::Addon { addon: addon.to_string(), id: id.to_string() };
        if self.entries.iter().any(|e| e.id == id) {
            return Err(MenuRegisterError::AlreadyRegistered);
        }
        self.entries.push(Entry { id: id.clone(), label: Some(label.to_string()), handler: Some(handler) });
        Ok(id)
    }

    /// Removes every entry of one add-on (it was disabled or uninstalled).
    pub fn remove_addon(&mut self, addon: &str) {
        self.entries.retain(|e| !matches!(&e.id, MenuItemId::Addon { addon: owner, .. } if owner == addon));
    }

    /// The entries in the order they are shown. `enabled` is the same for all:
    /// they act on a device that is ready.
    pub fn items(&self, enabled: bool) -> Vec<MenuItem> {
        self.entries.iter().map(|e| MenuItem { id: e.id.clone(), label: e.label.clone(), enabled }).collect()
    }

    /// The function of an add-on's entry.
    pub fn handler(&self, id: &MenuItemId) -> Option<MenuHandler> {
        self.entries.iter().find(|e| e.id == *id).and_then(|e| e.handler.clone())
    }
}

impl Default for DeviceMenu {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies an entry of the menu of an app (right-click on it in the list of
/// apps). Add-on entries carry the add-on's id, as in the menu of a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppMenuItemId {
    Open,
    OpenOnMainScreen,
    ToggleFavorite,
    ShortcutDesktop,
    ShortcutStartMenu,
    Addon { addon: String, id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppMenuItem {
    pub id: AppMenuItemId,
    pub label: Option<String>,
    pub enabled: bool,
}

/// What an add-on does when its entry is chosen, given the device and the package.
pub type AppMenuHandler = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct AppEntry {
    id: AppMenuItemId,
    label: Option<String>,
    handler: Option<AppMenuHandler>,
}

/// The menu of an app: the entries the application has itself and those add-ons
/// register, in the order they are shown.
pub struct AppMenu {
    entries: Vec<AppEntry>,
}

impl AppMenu {
    pub fn new() -> Self {
        let built_in = [
            AppMenuItemId::Open,
            AppMenuItemId::OpenOnMainScreen,
            AppMenuItemId::ToggleFavorite,
            AppMenuItemId::ShortcutDesktop,
            AppMenuItemId::ShortcutStartMenu,
        ];
        AppMenu { entries: built_in.into_iter().map(|id| AppEntry { id, label: None, handler: None }).collect() }
    }

    pub fn register(
        &mut self,
        addon: &str,
        id: &str,
        label: &str,
        handler: AppMenuHandler,
    ) -> Result<AppMenuItemId, MenuRegisterError> {
        if !buttons::valid_name(addon) || !buttons::valid_name(id) {
            return Err(MenuRegisterError::InvalidId);
        }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
            return Err(MenuRegisterError::InvalidLabel);
        }
        let id = AppMenuItemId::Addon { addon: addon.to_string(), id: id.to_string() };
        if self.entries.iter().any(|e| e.id == id) {
            return Err(MenuRegisterError::AlreadyRegistered);
        }
        self.entries.push(AppEntry { id: id.clone(), label: Some(label.to_string()), handler: Some(handler) });
        Ok(id)
    }

    /// Removes every entry of one add-on.
    pub fn remove_addon(&mut self, addon: &str) {
        self.entries.retain(|e| !matches!(&e.id, AppMenuItemId::Addon { addon: owner, .. } if owner == addon));
    }

    /// The entries in the order they are shown; `enabled` is the same for all.
    pub fn items(&self, enabled: bool) -> Vec<AppMenuItem> {
        self.entries.iter().map(|e| AppMenuItem { id: e.id.clone(), label: e.label.clone(), enabled }).collect()
    }

    pub fn handler(&self, id: &AppMenuItemId) -> Option<AppMenuHandler> {
        self.entries.iter().find(|e| e.id == *id).and_then(|e| e.handler.clone())
    }
}

impl Default for AppMenu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn app_addon(addon: &str, id: &str) -> AppMenuItemId {
        AppMenuItemId::Addon { addon: addon.into(), id: id.into() }
    }

    fn app_ids(menu: &AppMenu) -> Vec<AppMenuItemId> {
        menu.items(true).into_iter().map(|i| i.id).collect()
    }

    #[test]
    fn the_menu_of_an_app_has_the_built_in_entries_in_order_and_an_addon_adds_its_own_after_them() {
        let mut menu = AppMenu::new();
        let built_in = [
            AppMenuItemId::Open,
            AppMenuItemId::OpenOnMainScreen,
            AppMenuItemId::ToggleFavorite,
            AppMenuItemId::ShortcutDesktop,
            AppMenuItemId::ShortcutStartMenu,
        ];
        assert_eq!(app_ids(&menu), built_in);
        assert!(menu.items(true).iter().all(|i| i.label.is_none() && i.enabled));
        assert_eq!(menu.register("tools", "share", "Share", Arc::new(|_, _| {})), Ok(app_addon("tools", "share")));
        let items = menu.items(false);
        assert_eq!(items.len(), 6);
        assert_eq!(items[5], AppMenuItem { id: app_addon("tools", "share"), label: Some("Share".into()), enabled: false });
        // An add-on cannot take the place of a built-in entry, and two add-ons may use one id.
        assert_eq!(menu.register("evil", "open", "Open", Arc::new(|_, _| {})), Ok(app_addon("evil", "open")));
        assert_eq!(menu.register("tools", "share", "Again", Arc::new(|_, _| {})), Err(MenuRegisterError::AlreadyRegistered));
        assert_eq!(menu.register("other", "share", "Share too", Arc::new(|_, _| {})), Ok(app_addon("other", "share")));
        assert_eq!(app_ids(&menu)[..5], built_in);
        assert_eq!(app_ids(&menu).len(), 8);
    }

    #[test]
    fn what_is_not_an_id_or_a_text_is_refused_for_the_menu_of_an_app_too_and_nothing_is_added() {
        let mut menu = AppMenu::new();
        for (a, i) in [("", "x"), ("x", ""), ("has space", "x")] {
            assert_eq!(menu.register(a, i, "Text", Arc::new(|_, _| {})), Err(MenuRegisterError::InvalidId), "{a:?} {i:?}");
        }
        for label in ["", "  ", "two\nlines", "x".repeat(65).as_str()] {
            assert_eq!(menu.register("x", "y", label, Arc::new(|_, _| {})), Err(MenuRegisterError::InvalidLabel), "{label:?}");
        }
        assert_eq!(app_ids(&menu).len(), 5);
    }

    #[test]
    fn removing_an_addon_removes_only_its_entries_from_the_menu_of_an_app() {
        let mut menu = AppMenu::new();
        menu.register("one", "a", "A", Arc::new(|_, _| {})).unwrap();
        menu.register("two", "a", "A", Arc::new(|_, _| {})).unwrap();
        menu.remove_addon("one");
        assert_eq!(app_ids(&menu).last(), Some(&app_addon("two", "a")));
        assert_eq!(app_ids(&menu).len(), 6);
    }

    #[test]
    fn choosing_an_addon_entry_calls_its_function_with_the_device_and_the_package() {
        let mut menu = AppMenu::new();
        let chosen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let record = chosen.clone();
        menu.register("tools", "go", "Go", Arc::new(move |d, p| record.lock().unwrap().push((d.into(), p.into())))).unwrap();
        menu.handler(&app_addon("tools", "go")).expect("a function")("dev-1", "com.example.a");
        assert_eq!(*chosen.lock().unwrap(), [("dev-1".to_string(), "com.example.a".to_string())]);
        assert!(menu.handler(&AppMenuItemId::Open).is_none());
        assert!(menu.handler(&app_addon("tools", "other")).is_none());
    }

    #[test]
    fn the_ids_of_the_entries_of_an_app_cross_between_the_ui_and_the_core_as_json() {
        let cases = [
            (AppMenuItemId::OpenOnMainScreen, "\"open_on_main_screen\""),
            (AppMenuItemId::ShortcutStartMenu, "\"shortcut_start_menu\""),
            (app_addon("tools", "go"), r#"{"addon":{"addon":"tools","id":"go"}}"#),
        ];
        for (id, json) in cases {
            assert_eq!(serde_json::to_string(&id).unwrap(), json);
            assert_eq!(serde_json::from_str::<AppMenuItemId>(json).unwrap(), id);
        }
        assert!(serde_json::from_str::<AppMenuItemId>("\"teleport\"").is_err());
    }

    fn addon(addon: &str, id: &str) -> MenuItemId {
        MenuItemId::Addon { addon: addon.into(), id: id.into() }
    }

    fn noop() -> MenuHandler {
        Arc::new(|_| {})
    }

    fn ids(menu: &DeviceMenu) -> Vec<MenuItemId> {
        menu.items(true).into_iter().map(|i| i.id).collect()
    }

    #[test]
    fn a_new_menu_has_only_the_built_in_entry() {
        let menu = DeviceMenu::new();
        assert_eq!(
            menu.items(true),
            [MenuItem { id: MenuItemId::OpenApp, label: None, enabled: true }]
        );
    }

    #[test]
    fn an_addon_entry_is_added_after_the_built_in_ones_with_its_text() {
        let mut menu = DeviceMenu::new();
        assert_eq!(menu.register("clicker", "auto-tap", "Auto tap", noop()), Ok(addon("clicker", "auto-tap")));
        assert_eq!(
            menu.items(true),
            [
                MenuItem { id: MenuItemId::OpenApp, label: None, enabled: true },
                MenuItem { id: addon("clicker", "auto-tap"), label: Some("Auto tap".into()), enabled: true },
            ]
        );
    }

    #[test]
    fn all_entries_are_disabled_together_when_the_device_is_not_ready() {
        let mut menu = DeviceMenu::new();
        menu.register("a", "b", "Text", noop()).unwrap();
        assert!(menu.items(false).iter().all(|i| !i.enabled));
        assert_eq!(menu.items(false).len(), 2);
    }

    #[test]
    fn the_same_entry_twice_is_refused_but_two_addons_may_use_the_same_id() {
        let mut menu = DeviceMenu::new();
        menu.register("one", "go", "One", noop()).unwrap();
        assert_eq!(menu.register("one", "go", "Again", noop()), Err(MenuRegisterError::AlreadyRegistered));
        assert_eq!(menu.register("two", "go", "Two", noop()), Ok(addon("two", "go")));
        assert_eq!(ids(&menu), [MenuItemId::OpenApp, addon("one", "go"), addon("two", "go")]);
    }

    #[test]
    fn an_addon_cannot_take_the_place_of_the_built_in_entry() {
        let mut menu = DeviceMenu::new();
        // An id that looks like the built-in one only makes an add-on entry.
        assert_eq!(menu.register("evil", "open_app", "Open app", noop()), Ok(addon("evil", "open_app")));
        assert_eq!(ids(&menu), [MenuItemId::OpenApp, addon("evil", "open_app")]);
        assert_eq!(menu.items(true)[0].label, None);
    }

    #[test]
    fn ids_and_texts_that_are_not_allowed_are_refused() {
        let mut menu = DeviceMenu::new();
        let too_long = "a".repeat(65);
        for (a, i) in [("", "x"), ("x", ""), ("has space", "x"), ("x", "has space"), ("x", too_long.as_str())] {
            assert_eq!(menu.register(a, i, "Text", noop()), Err(MenuRegisterError::InvalidId), "{a:?} {i:?}");
        }
        let long_text = "t".repeat(65);
        for label in ["", "   ", "\t", "two\nlines", "bell\u{7}", long_text.as_str()] {
            assert_eq!(menu.register("x", "y", label, noop()), Err(MenuRegisterError::InvalidLabel), "{label:?}");
        }
        // Spaces inside and non-ASCII text are fine; 64 characters are the most.
        assert!(menu.register("x", "y", "Open my app", noop()).is_ok());
        assert!(menu.register("x", "z", &"t".repeat(64), noop()).is_ok());
        // Nothing was added by the refused ones.
        assert_eq!(ids(&menu).len(), 3);
    }

    #[test]
    fn removing_an_addon_removes_only_its_entries() {
        let mut menu = DeviceMenu::new();
        menu.register("one", "a", "A", noop()).unwrap();
        menu.register("two", "a", "A", noop()).unwrap();
        menu.register("one", "b", "B", noop()).unwrap();
        menu.remove_addon("one");
        assert_eq!(ids(&menu), [MenuItemId::OpenApp, addon("two", "a")]);
    }

    #[test]
    fn choosing_an_addon_entry_calls_its_function_with_the_device() {
        let mut menu = DeviceMenu::new();
        let chosen: Arc<Mutex<Vec<String>>> = Arc::default();
        let record = chosen.clone();
        menu.register("clicker", "go", "Go", Arc::new(move |device| record.lock().unwrap().push(device.to_string())))
            .unwrap();
        let handler = menu.handler(&addon("clicker", "go")).expect("a function");
        handler("device-1");
        assert_eq!(*chosen.lock().unwrap(), ["device-1"]);
        // The built-in entry and entries nobody registered have none.
        assert!(menu.handler(&MenuItemId::OpenApp).is_none());
        assert!(menu.handler(&addon("clicker", "other")).is_none());
    }

    #[test]
    fn entry_ids_cross_between_the_ui_and_the_core_as_json() {
        let cases = [
            (MenuItemId::OpenApp, "\"open_app\""),
            (addon("clicker", "auto-tap"), r#"{"addon":{"addon":"clicker","id":"auto-tap"}}"#),
        ];
        for (id, json) in cases {
            assert_eq!(serde_json::to_string(&id).unwrap(), json);
            assert_eq!(serde_json::from_str::<MenuItemId>(json).unwrap(), id);
        }
        assert!(serde_json::from_str::<MenuItemId>("\"teleport\"").is_err());
    }
}
