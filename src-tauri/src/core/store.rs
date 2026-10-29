//! The data folder: everything the program keeps between runs, and the only code that knows
//! where any of it is.
//!
//! ```text
//! <data folder>/
//!   settings.json              the general settings            (kinds::Settings)
//!   devices/<hex id>/          one folder for each device      (DeviceKey names it)
//!     profile.json settings.json usage.json apps.json …        (kinds::Profile, Options, Usage, Apps)
//!     addons/<addon>/<name>.json                               (data of an add-on for this device)
//!   addons/<addon>/<name>.json data of an add-on
//!   scrcpy/                    the downloaded scrcpy releases
//!   locales/                   language packs the user dropped in
//! ```
//!
//! A part of the program that needs data asks the store for a scope (`global()`, `device(key)`,
//! `addon(name)`) and then for a kind of data in it (`scope.load::<Shortcuts>()`): it never
//! builds a path. The kinds are in `kinds`; how a file is read and written is in `file`;
//! `migrate` moves what an older version kept in other places.

mod file;
mod key;
mod kinds;
mod migrate;
mod scope;

pub use file::Written;
pub use key::DeviceKey;
pub use kinds::{Apps, DeviceProfile, Installed, Options, Profile, Settings, Shortcuts, Usage};
pub use migrate::MigrationReport;
pub use scope::{Addon, Data, DataName, Device, Global, Loaded, Problem, Scope, ScopeKind};

use super::error::CoreError;
use super::model::KnownDevice;
use super::theme::{AssetPath, MAX_ASSET_BYTES, ThemeId};
use std::path::{Path, PathBuf};

/// A file could not be written or taken away.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(String),
    #[error("the value could not be written as a file: {0}")]
    Encode(String),
}

impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        CoreError::Storage(e.to_string())
    }
}

/// A language pack the user put into the data folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LocalePack {
    pub code: String,
    pub content: String,
}

/// What one folder of the themes of the user holds. A part that is not there, or is not
/// text, is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeFiles {
    /// The name of the folder: the id of the theme.
    pub folder: String,
    /// `theme.json`.
    pub manifest: Option<String>,
    /// `theme.css`.
    pub css: Option<String>,
}

const THEMES: &str = "themes";
/// The two files of a theme.
const THEME_MANIFEST: &str = "theme.json";
const THEME_CSS: &str = "theme.css";
const DEVICES: &str = "devices";
const ADDONS: &str = "addons";
const SCRCPY: &str = "scrcpy";
const LOCALES: &str = "locales";

/// The data folder.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    migration: MigrationReport,
}

impl Store {
    /// Opens the data folder; what an older version kept in other places is moved first.
    pub fn open(root: PathBuf) -> Store {
        let mut store = Store { root, migration: MigrationReport::default() };
        let root = store.root.clone();
        // A migration that fails leaves the old files as they were, and the program starts without them.
        store.migration = migrate::migrate_legacy(&store, &root).unwrap_or_default();
        store
    }

    /// Deployment resources of the optional Windows 11 file menu.
    pub fn windows11_menu_dir(&self) -> PathBuf {
        self.root().join("windows11-menu")
    }

    /// The folder itself (for showing it to the user).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What moving the files of an older version did when the folder was opened.
    pub fn migration(&self) -> &MigrationReport {
        &self.migration
    }

    /// The general data of the program.
    pub fn global(&self) -> Scope<Global> {
        Scope::new(self.root.clone())
    }

    /// The data of a device.
    pub fn device(&self, key: &DeviceKey) -> Scope<Device> {
        Scope::new(self.devices_dir().join(key.as_hex()))
    }

    /// The data of the device with this id; `None` for an id that is empty (it has no place).
    pub fn device_of(&self, id: &str) -> Option<Scope<Device>> {
        DeviceKey::from_id(id).map(|key| self.device(&key))
    }

    /// The data of an add-on; `None` for a name that is not one (see `buttons::valid_name`).
    pub fn addon(&self, name: &str) -> Option<Scope<Addon>> {
        crate::core::addons::buttons::valid_name(name).then(|| Scope::new(self.addons_dir().join(name)))
    }

    /// Every device that has a folder with a profile that is its own, as it is kept now, in the
    /// order the devices were first seen in.
    pub fn known_devices(&self) -> Vec<KnownDevice> {
        let Ok(entries) = std::fs::read_dir(self.devices_dir()) else { return Vec::new() };
        let mut found: Vec<(DeviceKey, KnownDevice)> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| DeviceKey::from_hex(&entry.file_name().to_string_lossy()))
            .filter_map(|key| {
                let scope = self.device(&key);
                let profile = scope.load::<Profile>();
                // The profile says whose it is: one that is not this folder's is not trusted.
                (!profile.id.is_empty() && profile.id == key.id()).then(|| {
                    let known = KnownDevice {
                        id: profile.id,
                        model: profile.model,
                        label: profile.label,
                        android: profile.android,
                        sdk: profile.sdk,
                        last_address: profile.last_address,
                        last_seen: profile.last_seen,
                        first_seen: profile.first_seen,
                        settings: scope.load::<Options>(),
                        apps: scope.load::<Usage>(),
                    };
                    (key, known)
                })
            })
            .collect();
        found.sort_by(|(a_key, a), (b_key, b)| (a.first_seen, a_key).cmp(&(b.first_seen, b_key)));
        found.into_iter().map(|(_, known)| known).collect()
    }

    /// Writes what is kept about a device (in the files of its folder; those that did not
    /// change are not written).
    pub fn save_known(&self, known: &KnownDevice) -> Result<(), StoreError> {
        let scope = self
            .device_of(&known.id)
            .ok_or_else(|| StoreError::Io("a device without an id has no place to be kept in".into()))?;
        scope.save::<Profile>(&DeviceProfile {
            id: known.id.clone(),
            model: known.model.clone(),
            label: known.label.clone(),
            android: known.android.clone(),
            sdk: known.sdk,
            last_address: known.last_address.clone(),
            last_seen: known.last_seen,
            first_seen: known.first_seen,
        })?;
        scope.save::<Options>(&known.settings)?;
        scope.save::<Usage>(&known.apps)?;
        Ok(())
    }

    /// Takes a device and all of its data away.
    pub fn forget_device(&self, id: &str) -> Result<(), StoreError> {
        let Some(key) = DeviceKey::from_id(id) else { return Ok(()) };
        let folder = self.devices_dir().join(key.as_hex());
        match std::fs::remove_dir_all(&folder) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(StoreError::Io(format!("could not take away {}: {e}", folder.display())))
            }
            _ => Ok(()),
        }
    }

    /// Where the downloaded scrcpy releases are.
    pub fn scrcpy_dir(&self) -> PathBuf {
        self.root.join(SCRCPY)
    }

    /// Where the language packs of the user are.
    pub fn locales_dir(&self) -> PathBuf {
        self.root.join(LOCALES)
    }

    /// The same folder, made if it is not there.
    pub fn ensure_locales_dir(&self) -> Result<PathBuf, StoreError> {
        let dir = self.locales_dir();
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(format!("could not make {}: {e}", dir.display())))?;
        Ok(dir)
    }

    /// The language packs the user put into the folder: a file `<code>.json` is the pack of the
    /// language `<code>`; anything else in it is not one.
    pub fn locale_packs(&self) -> Vec<LocalePack> {
        let Ok(entries) = std::fs::read_dir(self.locales_dir()) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_file() || path.extension()?.to_str()? != "json" {
                    return None;
                }
                Some(LocalePack { code: path.file_stem()?.to_str()?.to_string(), content: std::fs::read_to_string(&path).ok()? })
            })
            .collect()
    }

    /// Where the themes of the user are: a folder for each, with `theme.json` and `theme.css`.
    pub fn themes_dir(&self) -> PathBuf {
        self.root.join(THEMES)
    }

    /// The same folder, made if it is not there.
    pub fn ensure_themes_dir(&self) -> Result<PathBuf, StoreError> {
        let dir = self.themes_dir();
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(format!("could not make {}: {e}", dir.display())))?;
        Ok(dir)
    }

    /// What the folders of the themes hold, by the name of the folder. Anything in the
    /// folder of the themes that is not a folder is not a theme.
    pub fn theme_files(&self) -> Vec<ThemeFiles> {
        let Ok(entries) = std::fs::read_dir(self.themes_dir()) else { return Vec::new() };
        let mut found: Vec<ThemeFiles> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_dir() {
                    return None;
                }
                Some(ThemeFiles {
                    folder: entry.file_name().to_str()?.to_string(),
                    manifest: std::fs::read_to_string(path.join(THEME_MANIFEST)).ok(),
                    css: std::fs::read_to_string(path.join(THEME_CSS)).ok(),
                })
            })
            .collect();
        found.sort_by(|a, b| a.folder.cmp(&b.folder));
        found
    }

    /// A file of the folder of a theme of the user: `None` if there is no such file, if it is
    /// larger than `MAX_ASSET_BYTES`, or if the path leads out of the folder of the theme (a
    /// link that points elsewhere).
    pub fn theme_asset(&self, folder: &str, path: &AssetPath) -> Option<Vec<u8>> {
        // The folder is one name of a theme; anything else (`..`, a path) is no theme.
        let id = ThemeId::parse(folder).filter(|id| id.addon_of().is_none())?;
        let inside = std::fs::canonicalize(self.themes_dir().join(id.as_str())).ok()?;
        // Where the path really leads, links followed: it has to be in the folder of the theme.
        let file = std::fs::canonicalize(inside.join(path.as_str())).ok()?;
        if !file.starts_with(&inside) || !file.is_file() {
            return None;
        }
        let size = std::fs::metadata(&file).ok()?.len();
        if size > MAX_ASSET_BYTES as u64 {
            return None;
        }
        std::fs::read(&file).ok()
    }

    pub(super) fn devices_dir(&self) -> PathBuf {
        self.root.join(DEVICES)
    }

    pub(super) fn addons_dir(&self) -> PathBuf {
        self.root.join(ADDONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::apps::{AppCache, AppEntry};
    use crate::core::model::{AppUsage, DeviceSettings, MaxSize};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A device with every kind of data set to something that is not the default.
    pub(super) fn device(id: &str, first_seen: u64) -> KnownDevice {
        KnownDevice {
            id: id.into(),
            model: format!("Model {id}"),
            label: Some("Kitchen".into()),
            android: Some("14".into()),
            sdk: Some(34),
            last_address: Some("192.168.250.7:40100".into()),
            last_seen: first_seen + 100,
            first_seen,
            settings: DeviceSettings { max_size: MaxSize::new(1280).unwrap(), ..DeviceSettings::default() },
            apps: AppUsage { favorites: vec!["com.example.notes".into()], recent: vec!["com.example.mail".into()] },
        }
    }

    #[test]
    fn each_device_is_a_folder_named_by_its_key_with_a_file_for_each_kind_of_data() {
        let root = scratch("layout");
        let store = Store::open(root.clone());
        store.save_known(&device("AB", 5)).unwrap();
        let folder = root.join("devices").join("4142");
        for file in ["profile.json", "settings.json", "usage.json"] {
            assert!(folder.join(file).is_file(), "{file}");
        }
        assert!(!root.join("devices.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_devices_are_read_back_whole_from_the_folder_they_were_written_to() {
        let root = scratch("roundtrip");
        let store = Store::open(root.clone());
        let (a, b) = (device("TESTDEV0001", 10), device("192.168.1.5:40000", 20));
        store.save_known(&b).unwrap();
        store.save_known(&a).unwrap();
        // The order is the one the devices were first seen in, whatever order they were written in.
        assert_eq!(Store::open(root.clone()).known_devices(), vec![a.clone(), b.clone()]);
        // Writing again changes what is there (the label, what the user marked), and nothing else.
        let renamed = KnownDevice { label: Some("Desk".into()), apps: AppUsage::default(), ..a.clone() };
        store.save_known(&renamed).unwrap();
        assert_eq!(Store::open(root.clone()).known_devices(), vec![renamed, b]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_id_is_never_a_path_and_stays_in_the_folder_of_the_devices() {
        let root = scratch("path");
        let store = Store::open(root.clone());
        store.save_known(&device("../../escape/x", 1)).unwrap();
        assert!(!root.join("../../escape").exists());
        assert_eq!(store.known_devices()[0].id, "../../escape/x");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_is_broken_loses_that_kind_of_data_only() {
        let root = scratch("broken");
        let store = Store::open(root.clone());
        let a = device("A1", 1);
        store.save_known(&a).unwrap();
        std::fs::write(root.join("devices").join("4131").join("usage.json"), "{broken").unwrap();
        let known = store.known_devices();
        assert_eq!(known, vec![KnownDevice { apps: AppUsage::default(), ..a }]);
        // The broken file was kept.
        assert!(root.join("devices").join("4131").join("usage.json.broken").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_is_a_device_only_when_it_has_a_profile_that_is_its_own() {
        let root = scratch("profiles");
        let store = Store::open(root.clone());
        store.save_known(&device("A1", 1)).unwrap();
        // A folder that has only data of other kinds (a list of apps kept after the profile went).
        let lonely = DeviceKey::from_id("B2").unwrap();
        store.device(&lonely).save::<Apps>(&AppCache::default()).unwrap();
        // A folder with the profile of another device in it, and one with a name that is not a key.
        let other = DeviceKey::from_id("C3").unwrap();
        store.device(&other).save::<Profile>(&DeviceProfile { id: "not C3".into(), ..DeviceProfile::default() }).unwrap();
        std::fs::create_dir_all(root.join("devices").join("not-a-key")).unwrap();
        std::fs::write(root.join("devices").join("loose-file.json"), "{}").unwrap();
        let ids: Vec<String> = store.known_devices().into_iter().map(|k| k.id).collect();
        assert_eq!(ids, ["A1"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn forgetting_a_device_takes_its_whole_folder_away_and_only_that_one() {
        let root = scratch("forget");
        let store = Store::open(root.clone());
        store.save_known(&device("A1", 1)).unwrap();
        store.save_known(&device("B2", 2)).unwrap();
        let a = store.device_of("A1").unwrap();
        a.save::<Apps>(&AppCache { updated_at: 5, apps: vec![AppEntry { package: "a.b".into(), name: "AB".into(), system: false }] })
            .unwrap();
        a.addon("tools").unwrap().save_value(&scope::DataName::new("note").unwrap(), &serde_json::json!({ "x": 1 })).unwrap();
        store.forget_device("A1").unwrap();
        assert!(!root.join("devices").join("4131").exists());
        let ids: Vec<String> = store.known_devices().into_iter().map(|k| k.id).collect();
        assert_eq!(ids, ["B2"]);
        // A device that is not there is not an error, and neither is an id that is empty.
        store.forget_device("A1").unwrap();
        store.forget_device("").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_data_of_a_device_is_asked_of_its_scope_by_the_kind_and_each_device_has_its_own() {
        let root = scratch("scopes");
        let store = Store::open(root.clone());
        assert!(store.device_of("").is_none());
        let (a, b) = (store.device_of("A1").unwrap(), store.device_of("B2").unwrap());
        let cache = AppCache { updated_at: 77, apps: vec![] };
        assert_eq!(a.load::<Apps>(), AppCache::default());
        a.save::<Apps>(&cache).unwrap();
        assert_eq!(a.load::<Apps>(), cache);
        assert_eq!(b.load::<Apps>(), AppCache::default());
        assert!(root.join("devices").join("4131").join("apps.json").is_file());
        // The general settings are in the folder itself.
        let general = store.global();
        let mut settings = general.load::<Settings>();
        settings.language = Some("ja".into());
        general.save::<Settings>(&settings).unwrap();
        assert!(root.join("settings.json").is_file());
        assert_eq!(Store::open(root.clone()).global().load::<Settings>().language.as_deref(), Some("ja"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_addon_has_a_place_of_its_own_by_name_alone_and_for_a_device_and_cannot_leave_it() {
        let root = scratch("addons");
        let store = Store::open(root.clone());
        for bad in ["", " ", "a b", "../x", "a/b", "a.b"] {
            assert!(store.addon(bad).is_none(), "{bad:?}");
        }
        let name = |n: &str| scope::DataName::new(n).unwrap();
        for bad in ["", "..", "a/b", "a b", "a.json"] {
            assert!(scope::DataName::new(bad).is_none(), "{bad:?}");
        }
        let tools = store.addon("tools").unwrap();
        assert_eq!(tools.load_value(&name("prefs")), serde_json::Value::Null);
        tools.save_value(&name("prefs"), &serde_json::json!({ "a": [1, 2] })).unwrap();
        assert_eq!(tools.load_value(&name("prefs")), serde_json::json!({ "a": [1, 2] }));
        assert!(root.join("addons").join("tools").join("prefs.json").is_file());
        // Another add-on, and the same add-on for a device, are other places.
        assert_eq!(store.addon("other").unwrap().load_value(&name("prefs")), serde_json::Value::Null);
        let for_device = store.device_of("A1").unwrap().addon("tools").unwrap();
        assert_eq!(for_device.load_value(&name("prefs")), serde_json::Value::Null);
        for_device.save_value(&name("prefs"), &serde_json::json!(true)).unwrap();
        assert!(root.join("devices").join("4131").join("addons").join("tools").join("prefs.json").is_file());
        assert_eq!(tools.load_value(&name("prefs")), serde_json::json!({ "a": [1, 2] }));
        // Taking a value away.
        tools.remove_value(&name("prefs"));
        assert_eq!(tools.load_value(&name("prefs")), serde_json::Value::Null);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_theme_folders_are_listed_by_name_with_what_each_holds_and_nothing_else_is_a_theme() {
        let root = scratch("themes");
        let store = Store::open(root.clone());
        assert_eq!(store.theme_files(), []);
        let dir = store.ensure_themes_dir().unwrap();
        assert_eq!(dir, root.join("themes"));
        let night = dir.join("night");
        let half = dir.join("half");
        std::fs::create_dir_all(&night).unwrap();
        std::fs::create_dir_all(&half).unwrap();
        std::fs::write(night.join("theme.json"), r#"{"name":"Night"}"#).unwrap();
        std::fs::write(night.join("theme.css"), "body { color: red; }").unwrap();
        std::fs::write(half.join("theme.json"), "{}").unwrap();
        // Not a folder, and a file in a theme that is not one of its two.
        std::fs::write(dir.join("readme.txt"), "a note").unwrap();
        std::fs::write(night.join("notes.txt"), "a note").unwrap();
        // A file that is not text is as if it was not there.
        let binary = dir.join("binary");
        std::fs::create_dir_all(&binary).unwrap();
        std::fs::write(binary.join("theme.css"), [0xff, 0xfe, 0xfd]).unwrap();
        assert_eq!(
            store.theme_files(),
            [
                ThemeFiles { folder: "binary".into(), manifest: None, css: None },
                ThemeFiles { folder: "half".into(), manifest: Some("{}".into()), css: None },
                ThemeFiles {
                    folder: "night".into(),
                    manifest: Some(r#"{"name":"Night"}"#.into()),
                    css: Some("body { color: red; }".into()),
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn asset(path: &str) -> AssetPath {
        AssetPath::parse(path).unwrap()
    }

    #[test]
    fn a_file_of_a_theme_is_given_out_from_the_folder_of_that_theme_and_from_no_other_place() {
        let root = scratch("theme-assets");
        let store = Store::open(root.clone());
        let night = store.ensure_themes_dir().unwrap().join("night");
        std::fs::create_dir_all(night.join("images")).unwrap();
        std::fs::write(night.join("images").join("bg.png"), [1u8, 2, 3]).unwrap();
        std::fs::write(night.join("top.png"), [9u8]).unwrap();
        // Another theme has a file of the same name; a folder has the name of a file.
        let other = store.themes_dir().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("top.png"), [7u8]).unwrap();
        std::fs::create_dir_all(night.join("folder.png")).unwrap();
        // A file outside the themes, one level up from the theme.
        std::fs::write(store.themes_dir().join("secret.png"), [5u8]).unwrap();

        assert_eq!(store.theme_asset("night", &asset("images/bg.png")), Some(vec![1, 2, 3]));
        assert_eq!(store.theme_asset("night", &asset("top.png")), Some(vec![9]));
        assert_eq!(store.theme_asset("other", &asset("top.png")), Some(vec![7]));
        assert_eq!(store.theme_asset("night", &asset("missing.png")), None);
        assert_eq!(store.theme_asset("night", &asset("folder.png")), None);
        assert_eq!(store.theme_asset("nobody", &asset("top.png")), None);
        // A folder name that is a way out is no theme.
        assert_eq!(store.theme_asset("..", &asset("secret.png")), None);
        assert_eq!(store.theme_asset("night/..", &asset("secret.png")), None);
        assert_eq!(store.theme_asset("", &asset("secret.png")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_of_a_theme_that_is_too_large_is_not_given_out() {
        let root = scratch("theme-assets-large");
        let store = Store::open(root.clone());
        let night = store.ensure_themes_dir().unwrap().join("night");
        std::fs::create_dir_all(&night).unwrap();
        std::fs::write(night.join("big.png"), vec![0u8; crate::core::theme::MAX_ASSET_BYTES + 1]).unwrap();
        std::fs::write(night.join("biggest.png"), vec![0u8; crate::core::theme::MAX_ASSET_BYTES]).unwrap();
        assert_eq!(store.theme_asset("night", &asset("big.png")), None);
        assert_eq!(store.theme_asset("night", &asset("biggest.png")).map(|b| b.len()), Some(crate::core::theme::MAX_ASSET_BYTES));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A link inside a theme that points out of its folder gives nothing (the link is made by
    /// whoever made the theme; the file it points at is not theirs to give out). Only where
    /// links can be made without a privilege.
    #[cfg(unix)]
    #[test]
    fn a_link_in_a_theme_that_points_out_of_its_folder_gives_nothing() {
        let root = scratch("theme-assets-link");
        let store = Store::open(root.clone());
        let night = store.ensure_themes_dir().unwrap().join("night");
        std::fs::create_dir_all(&night).unwrap();
        let outside = root.join("outside.png");
        std::fs::write(&outside, [4u8]).unwrap();
        std::os::unix::fs::symlink(&outside, night.join("link.png")).unwrap();
        std::fs::write(night.join("own.png"), [6u8]).unwrap();
        std::os::unix::fs::symlink(night.join("own.png"), night.join("inside.png")).unwrap();
        assert_eq!(store.theme_asset("night", &asset("link.png")), None);
        // A link that stays inside the folder is fine.
        assert_eq!(store.theme_asset("night", &asset("inside.png")), Some(vec![6]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_language_packs_are_the_json_files_of_the_folder_by_the_code_in_their_name_and_nothing_else() {
        let root = scratch("locales");
        let store = Store::open(root.clone());
        assert_eq!(store.locale_packs(), []);
        let dir = store.ensure_locales_dir().unwrap();
        assert_eq!(dir, root.join("locales"));
        assert!(dir.is_dir());
        std::fs::write(dir.join("de.json"), r#"{"a":"b"}"#).unwrap();
        std::fs::write(dir.join("zh-TW.json"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a pack").unwrap();
        std::fs::create_dir(dir.join("folder.json")).unwrap();
        let mut packs = store.locale_packs();
        packs.sort_by(|a, b| a.code.cmp(&b.code));
        assert_eq!(
            packs,
            [
                LocalePack { code: "de".into(), content: r#"{"a":"b"}"#.into() },
                LocalePack { code: "zh-TW".into(), content: "{}".into() },
            ]
        );
        // Asking again for the folder changes nothing in it.
        store.ensure_locales_dir().unwrap();
        assert_eq!(store.locale_packs().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_other_folders_of_the_program_are_named_here_and_nowhere_else() {
        let root = scratch("dirs");
        let store = Store::open(root.clone());
        assert_eq!(store.root(), root);
        assert_eq!(store.scrcpy_dir(), root.join("scrcpy"));
        assert_eq!(store.locales_dir(), root.join("locales"));
    }
}
