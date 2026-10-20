//! Moving what an older version kept in other places into the layout of today.
//!
//! Before, all devices were in one file (`devices.json`, with what is set for each and what the
//! user marked) and each device's list of apps was `apps/<hex id>.json`. Now each device has a
//! folder (`devices/<hex id>/`) with a file for each kind of data. Nothing is thrown away: the
//! file of the old layout is renamed (`devices.json.migrated`, or `.broken` if it could not be
//! read at all), a device that has a folder already keeps what is there, and what is not
//! understood stays where it is.

use super::file::{self, Read};
use super::kinds::{Apps, Profile};
use super::{DeviceKey, Store, StoreError};
use crate::core::apps::AppCache;
use crate::core::model::KnownDevice;
use std::path::Path;

/// What was moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Devices that were put into folders of their own.
    pub devices: usize,
    /// Entries of the old file that were not devices (left out).
    pub skipped: usize,
    /// Lists of apps that were moved.
    pub app_lists: usize,
    /// Files of the old layout that were not understood, and were left where they were.
    pub left_behind: Vec<String>,
}

pub(super) fn migrate_legacy(store: &Store, root: &Path) -> Result<MigrationReport, StoreError> {
    let mut report = MigrationReport::default();
    move_devices(store, root, &mut report)?;
    move_app_lists(store, root, &mut report);
    Ok(report)
}

/// `devices.json`: one folder for each device in it.
fn move_devices(store: &Store, root: &Path, report: &mut MigrationReport) -> Result<(), StoreError> {
    let legacy = root.join("devices.json");
    match file::read::<Vec<serde_json::Value>>(&legacy) {
        Read::Missing => {}
        Read::Broken => {
            // A file that cannot be understood is kept (it may be all that is left of the devices).
            let _ = file::set_aside(&legacy);
        }
        Read::Value(entries) => {
            for entry in entries {
                match serde_json::from_value::<KnownDevice>(entry) {
                    Ok(known) if !known.id.is_empty() => {
                        let key = DeviceKey::from_id(&known.id).expect("an id that is not empty has a key");
                        // What is in the folder of the device is newer than the old file.
                        if !store.device(&key).exists::<Profile>() {
                            store.save_known(&known)?;
                            report.devices += 1;
                        }
                    }
                    _ => report.skipped += 1,
                }
            }
            let moved = root.join("devices.json.migrated");
            std::fs::rename(&legacy, &moved)
                .map_err(|e| StoreError::Io(format!("could not rename {}: {e}", legacy.display())))?;
        }
    }
    Ok(())
}

/// `apps/<hex id>.json`: into the folder of the device.
fn move_app_lists(store: &Store, root: &Path, report: &mut MigrationReport) {
    let folder = root.join("apps");
    let Ok(entries) = std::fs::read_dir(&folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let key = path
            .is_file()
            .then(|| path.extension().and_then(|e| e.to_str()).filter(|e| *e == "json"))
            .flatten()
            .and_then(|_| path.file_stem()?.to_str().and_then(DeviceKey::from_hex));
        let (Some(key), Read::Value(list)) = (key, file::read::<AppCache>(&path)) else {
            report.left_behind.push(name);
            continue;
        };
        let scope = store.device(&key);
        if scope.exists::<Apps>() || scope.save::<Apps>(&list).is_err() {
            report.left_behind.push(name);
            continue;
        }
        let _ = std::fs::remove_file(&path);
        report.app_lists += 1;
    }
    // Only an empty folder goes; one that still has something in it is left alone.
    let _ = std::fs::remove_dir(&folder);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::apps::AppEntry;
    use serde_json::json;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The old file as an older version wrote it (the values are invented).
    fn legacy_devices() -> serde_json::Value {
        json!([
            {
                "id": "TESTDEV0001", "model": "Model A", "label": "Kitchen", "android": "14", "sdk": 34,
                "last_address": "192.168.250.7:40100", "last_seen": 1700000100, "first_seen": 1700000000,
                "settings": { "max_size": 1280 },
                "apps": { "favorites": ["com.example.notes"], "recent": ["com.example.mail", "com.example.notes"] }
            },
            // The oldest shape: only what was kept first.
            { "id": "192.168.1.5:40000", "model": "Model B" }
        ])
    }

    #[test]
    fn the_old_file_of_all_devices_becomes_a_folder_for_each_and_is_renamed_not_deleted() {
        let root = scratch("devices");
        let text = serde_json::to_string_pretty(&legacy_devices()).unwrap();
        std::fs::write(root.join("devices.json"), &text).unwrap();
        let store = Store::open(root.clone());
        assert_eq!(*store.migration(), MigrationReport { devices: 2, ..MigrationReport::default() });
        assert!(!root.join("devices.json").exists());
        assert_eq!(std::fs::read_to_string(root.join("devices.json.migrated")).unwrap(), text);
        let known = store.known_devices();
        assert_eq!(known.len(), 2);
        let a = known.iter().find(|k| k.id == "TESTDEV0001").unwrap();
        assert_eq!((a.model.as_str(), a.label.as_deref(), a.sdk, a.first_seen), ("Model A", Some("Kitchen"), Some(34), 1700000000));
        assert_eq!(a.settings.max_size.pixels(), Some(1280));
        assert_eq!(a.apps.favorites, ["com.example.notes"]);
        assert_eq!(a.apps.recent, ["com.example.mail", "com.example.notes"]);
        let b = known.iter().find(|k| k.id == "192.168.1.5:40000").unwrap();
        assert_eq!((b.model.as_str(), b.label.clone(), b.first_seen), ("Model B", None, 0));
        assert!(b.apps.favorites.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_entry_that_is_not_a_device_is_left_out_and_counted() {
        let root = scratch("skipped");
        let entries = json!([
            { "id": "A1", "model": "ok" },
            { "model": "no id" },
            { "id": "", "model": "empty id" },
            "text",
            5,
            { "id": 7, "model": "id is not text" }
        ]);
        std::fs::write(root.join("devices.json"), entries.to_string()).unwrap();
        let store = Store::open(root.clone());
        assert_eq!((store.migration().devices, store.migration().skipped), (1, 5));
        assert_eq!(store.known_devices().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_device_that_has_a_folder_already_keeps_what_is_there() {
        let root = scratch("existing");
        let first = Store::open(root.clone());
        let mut newer = first.known_devices();
        assert!(newer.is_empty());
        newer.push(KnownDevice { id: "A1".into(), model: "Newer".into(), ..legacy_known("A1") });
        first.save_known(&newer[0]).unwrap();
        std::fs::write(root.join("devices.json"), json!([{ "id": "A1", "model": "Older" }, { "id": "B2", "model": "Other" }]).to_string())
            .unwrap();
        let store = Store::open(root.clone());
        assert_eq!(store.migration().devices, 1);
        let models: Vec<(String, String)> = store.known_devices().into_iter().map(|k| (k.id, k.model)).collect();
        assert_eq!(models, [("A1".to_string(), "Newer".to_string()), ("B2".to_string(), "Other".to_string())]);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn legacy_known(id: &str) -> KnownDevice {
        serde_json::from_value(json!({ "id": id, "model": "m" })).unwrap()
    }

    #[test]
    fn the_list_of_apps_of_each_device_moves_into_its_folder_and_what_is_not_understood_stays() {
        let root = scratch("apps");
        let list = AppCache {
            updated_at: 77,
            apps: vec![AppEntry { package: "com.example.notes".into(), name: "Notes".into(), system: false }],
        };
        std::fs::create_dir_all(root.join("apps")).unwrap();
        let write = |name: &str, text: &str| std::fs::write(root.join("apps").join(name), text).unwrap();
        write("4131.json", &serde_json::to_string(&list).unwrap());
        write("4232.json", "{broken");
        write("not-hex.json", "{}");
        write("4333.txt", "{}");
        let store = Store::open(root.clone());
        assert_eq!(store.migration().app_lists, 1);
        let mut left = store.migration().left_behind.clone();
        left.sort();
        assert_eq!(left, ["4232.json", "4333.txt", "not-hex.json"]);
        assert_eq!(store.device_of("A1").unwrap().load::<Apps>(), list);
        assert!(!root.join("apps").join("4131.json").exists());
        // The others are where they were, and so is the folder they are in.
        for name in ["4232.json", "4333.txt", "not-hex.json"] {
            assert!(root.join("apps").join(name).exists(), "{name}");
        }
        // The folder of a device that only has a list of apps is not a device.
        assert!(store.known_devices().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_folder_of_the_old_lists_goes_and_a_list_that_has_a_new_one_beside_it_stays() {
        let root = scratch("apps-empty");
        std::fs::create_dir_all(root.join("apps")).unwrap();
        std::fs::write(root.join("apps").join("4131.json"), r#"{"updated_at":0,"apps":[]}"#).unwrap();
        Store::open(root.clone());
        assert!(!root.join("apps").exists(), "everything in it was moved, so the folder is gone");
        // A device that has its list already keeps it; the old one stays.
        let root = scratch("apps-newer");
        let store = Store::open(root.clone());
        let newer = AppCache { updated_at: 9, apps: vec![] };
        store.device_of("A1").unwrap().save::<Apps>(&newer).unwrap();
        std::fs::create_dir_all(root.join("apps")).unwrap();
        std::fs::write(root.join("apps").join("4131.json"), r#"{"updated_at":1,"apps":[]}"#).unwrap();
        let store = Store::open(root.clone());
        assert_eq!(store.device_of("A1").unwrap().load::<Apps>(), newer);
        assert_eq!(store.migration().left_behind, ["4131.json"]);
        assert!(root.join("apps").join("4131.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_cannot_be_understood_at_all_is_put_aside_not_lost() {
        let root = scratch("broken");
        std::fs::write(root.join("devices.json"), "{not json").unwrap();
        let store = Store::open(root.clone());
        assert_eq!(*store.migration(), MigrationReport::default());
        assert!(store.known_devices().is_empty());
        assert_eq!(std::fs::read_to_string(root.join("devices.json.broken")).unwrap(), "{not json");
        assert!(!root.join("devices.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_the_folder_again_moves_nothing_and_changes_nothing() {
        let root = scratch("again");
        std::fs::write(root.join("devices.json"), legacy_devices().to_string()).unwrap();
        let first = Store::open(root.clone());
        let known = first.known_devices();
        let second = Store::open(root.clone());
        assert_eq!(*second.migration(), MigrationReport::default());
        assert_eq!(second.known_devices(), known);
        assert!(root.join("devices.json.migrated").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
