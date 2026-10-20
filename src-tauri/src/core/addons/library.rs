//! The add-ons that are installed: which folders they are in, what each asks for and what the user
//! gave it, and what each says about itself now.
//!
//! An add-on installed from a folder is *linked*: the program remembers the folder (not a copy of
//! it), so a change in the folder is seen the next time the add-on is read, and removing the add-on
//! never touches the folder. Because the folder can change or go at any time, the list is made by
//! reading every folder again; what it finds is a status, not an error.
//!
//! For each add-on two lists of permissions are kept: `declared`, what its manifest asked for when
//! the user last decided, and `granted`, what the user gave out of that. An add-on that now asks for
//! something that was not in `declared` does not run until the user decides again; one the user
//! declined something of is not asked again for the same thing.

use super::manifest::{AddonManifest, ManifestError, Unsupported, parse_manifest, supported};
use super::permission::AddonPermission;
use crate::core::store::{Global, Installed, Scope};
use crate::core::window::layout::Os;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The most `addon.json` may hold; a larger file is not read.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// The file in the folder of an add-on that says what it is.
pub const MANIFEST_FILE: &str = "addon.json";
/// The file in the folder of an add-on that does what it does.
pub const MAIN_SCRIPT: &str = "main.js";

/// An installed add-on as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonLink {
    pub id: String,
    pub folder: PathBuf,
    /// What the manifest asked for when the user last decided.
    #[serde(default)]
    pub declared: Vec<AddonPermission>,
    /// What the user gave, out of `declared`.
    #[serde(default)]
    pub granted: Vec<AddonPermission>,
    /// Whether the user lets it run. An add-on that is switched off stays installed with what the user gave it.
    #[serde(default = "switched_on")]
    pub enabled: bool,
}

/// What a link written before the switch existed means: on.
fn switched_on() -> bool {
    true
}

/// Every installed add-on, in the order they were installed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstalledAddons {
    pub links: Vec<AddonLink>,
}

/// Why an add-on was not installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallError {
    /// The path is not from the root of the file system.
    NotAbsolute,
    /// There is no folder at the path.
    NotAFolder,
    /// The folder has no `addon.json` that can be read.
    NoManifest,
    /// `addon.json` is larger than `MAX_MANIFEST_BYTES`.
    ManifestTooLarge,
    Manifest(ManifestError),
    /// The folder has no `main.js` (a folder of that name does not count).
    NoMainScript,
    Unsupported(Unsupported),
    /// An add-on with this id is installed already.
    AlreadyInstalled,
    /// There is no installed add-on with this id.
    NotInstalled,
    /// The list of installed add-ons could not be written.
    Storage,
}

/// What an installed add-on is, now. The first that applies is the status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonStatus {
    /// It can run here.
    Ready,
    /// The folder is gone.
    FolderMissing,
    /// `addon.json` is not there, or not readable, or too large.
    NoManifest,
    /// `addon.json` is not valid any more.
    Manifest(ManifestError),
    /// `addon.json` now has another id than the one it was installed under.
    IdChanged,
    NoMainScript,
    Unsupported(Unsupported),
    /// `addon.json` now asks for something it did not when the user decided: it does not run.
    PermissionsChanged,
}

/// An installed add-on for the list: where it is, what its manifest says (if it can be read), what it
/// may do, and its status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonInfo {
    pub id: String,
    pub folder: PathBuf,
    pub manifest: Option<AddonManifest>,
    /// What the user gave that the manifest still asks for: this is what the add-on may do.
    pub granted: Vec<AddonPermission>,
    pub status: AddonStatus,
    /// Whether the user lets it run (see `AddonLink::enabled`); apart from `status`, which says whether it could.
    pub enabled: bool,
}

/// Why `addon.json` of a folder could not be read.
enum ReadError {
    /// Not there, not a file, or not text.
    Missing,
    TooLarge,
    Invalid(ManifestError),
}

fn read_manifest(folder: &Path) -> Result<AddonManifest, ReadError> {
    let path = folder.join(MANIFEST_FILE);
    let meta = std::fs::metadata(&path).map_err(|_| ReadError::Missing)?;
    if !meta.is_file() {
        return Err(ReadError::Missing);
    }
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(ReadError::TooLarge);
    }
    let text = std::fs::read_to_string(&path).map_err(|_| ReadError::Missing)?;
    parse_manifest(&text).map_err(ReadError::Invalid)
}

/// Whether `folder` holds an add-on that can run here, and what it says about itself.
fn check_folder(folder: &Path, os: Os) -> Result<AddonManifest, InstallError> {
    if !folder.is_absolute() {
        return Err(InstallError::NotAbsolute);
    }
    if !folder.is_dir() {
        return Err(InstallError::NotAFolder);
    }
    let manifest = read_manifest(folder).map_err(|e| match e {
        ReadError::Missing => InstallError::NoManifest,
        ReadError::TooLarge => InstallError::ManifestTooLarge,
        ReadError::Invalid(e) => InstallError::Manifest(e),
    })?;
    if !folder.join(MAIN_SCRIPT).is_file() {
        return Err(InstallError::NoMainScript);
    }
    supported(&manifest, os).map_err(InstallError::Unsupported)?;
    Ok(manifest)
}

/// What the user gave out of what the add-on asks for, in the order of the manifest.
fn given(asked: &[AddonPermission], granted: &[AddonPermission]) -> Vec<AddonPermission> {
    asked.iter().copied().filter(|p| granted.contains(p)).collect()
}

/// The add-on of a link as the list shows it, read from its folder now.
fn describe(link: &AddonLink, os: Os) -> AddonInfo {
    let folder = link.folder.as_path();
    let info = |manifest: Option<AddonManifest>, status: AddonStatus| {
        let granted = manifest.as_ref().map(|m| given(&m.permissions, &link.granted)).unwrap_or_default();
        AddonInfo { id: link.id.clone(), folder: link.folder.clone(), manifest, granted, status, enabled: link.enabled }
    };
    if !folder.is_dir() {
        return info(None, AddonStatus::FolderMissing);
    }
    let manifest = match read_manifest(folder) {
        Ok(manifest) => manifest,
        Err(ReadError::Missing | ReadError::TooLarge) => return info(None, AddonStatus::NoManifest),
        Err(ReadError::Invalid(e)) => return info(None, AddonStatus::Manifest(e)),
    };
    let status = if manifest.id != link.id {
        AddonStatus::IdChanged
    } else if !folder.join(MAIN_SCRIPT).is_file() {
        AddonStatus::NoMainScript
    } else if let Err(unsupported) = supported(&manifest, os) {
        AddonStatus::Unsupported(unsupported)
    } else if !manifest.permissions.iter().all(|p| link.declared.contains(p)) {
        AddonStatus::PermissionsChanged
    } else {
        AddonStatus::Ready
    };
    info(Some(manifest), status)
}

/// What the add-on in `folder` is and asks for, to show to the user before it is installed. Nothing
/// is kept; it fails for the same reasons as `install_from_folder`.
pub fn inspect_folder(global: &Scope<Global>, folder: &Path, os: Os) -> Result<AddonManifest, InstallError> {
    let manifest = check_folder(folder, os)?;
    if global.load::<Installed>().links.iter().any(|l| l.id == manifest.id) {
        return Err(InstallError::AlreadyInstalled);
    }
    Ok(manifest)
}

/// Links the add-on in `folder` and returns it as the list would show it. `granted` is what the user
/// gave: any of what the add-on asks for (none at all is allowed), and what it does not ask for is
/// left out.
pub fn install_from_folder(
    global: &Scope<Global>,
    folder: &Path,
    os: Os,
    granted: &[AddonPermission],
) -> Result<AddonInfo, InstallError> {
    let manifest = inspect_folder(global, folder, os)?;
    let mut installed = global.load::<Installed>();
    let link = AddonLink {
        id: manifest.id.clone(),
        folder: folder.to_path_buf(),
        granted: given(&manifest.permissions, granted),
        declared: manifest.permissions.clone(),
        enabled: true,
    };
    installed.links.push(link.clone());
    global.save::<Installed>(&installed).map_err(|_| InstallError::Storage)?;
    Ok(describe(&link, os))
}

/// The user decides again about an installed add-on: what its manifest asks for now becomes `declared`,
/// and `granted` (as above) what was given.
pub fn accept_permissions(
    global: &Scope<Global>,
    id: &str,
    os: Os,
    granted: &[AddonPermission],
) -> Result<AddonInfo, InstallError> {
    let mut installed = global.load::<Installed>();
    let link = installed.links.iter_mut().find(|l| l.id == id).ok_or(InstallError::NotInstalled)?;
    let manifest = check_folder(&link.folder, os)?;
    // The folder holds another add-on now: what is installed under this id is not there any more.
    if manifest.id != id {
        return Err(InstallError::NotInstalled);
    }
    link.granted = given(&manifest.permissions, granted);
    link.declared = manifest.permissions;
    let link = link.clone();
    global.save::<Installed>(&installed).map_err(|_| InstallError::Storage)?;
    Ok(describe(&link, os))
}

/// Lets an installed add-on run or not (what the user gave it and where it is stay), and returns it as the list shows it.
pub fn set_enabled(global: &Scope<Global>, id: &str, os: Os, enabled: bool) -> Result<AddonInfo, InstallError> {
    let mut installed = global.load::<Installed>();
    let link = installed.links.iter_mut().find(|l| l.id == id).ok_or(InstallError::NotInstalled)?;
    link.enabled = enabled;
    let link = link.clone();
    global.save::<Installed>(&installed).map_err(|_| InstallError::Storage)?;
    Ok(describe(&link, os))
}

/// The installed add-ons, each read again from its folder.
pub fn list(global: &Scope<Global>, os: Os) -> Vec<AddonInfo> {
    global.load::<Installed>().links.iter().map(|link| describe(link, os)).collect()
}

/// Forgets the add-on with this id (its folder and the data it kept are left as they are); whether there was one.
pub fn remove(global: &Scope<Global>, id: &str) -> bool {
    let mut installed = global.load::<Installed>();
    let before = installed.links.len();
    installed.links.retain(|l| l.id != id);
    installed.links.len() != before && global.save::<Installed>(&installed).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::addons::manifest::AddonPlatform;
    use crate::core::store::Store;

    // The folders are made in the temporary folder; the manifests are made up, and nothing in them is run.

    const SHELL: AddonPermission = AddonPermission::DeviceShell;
    const PROCESS: AddonPermission = AddonPermission::RunProcess;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir().join(format!("scrcpy-agent-library-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Scratch { root }
        }
        fn data(&self) -> Store {
            Store::open(self.root.join("data"))
        }
        /// A folder of an add-on with this manifest (or none) and a `main.js`.
        fn addon(&self, folder: &str, manifest: Option<&str>) -> PathBuf {
            let dir = self.bare(folder, manifest);
            std::fs::write(dir.join(MAIN_SCRIPT), "// made up").unwrap();
            dir
        }
        /// The same, without `main.js`.
        fn bare(&self, folder: &str, manifest: Option<&str>) -> PathBuf {
            let dir = self.root.join("addons").join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(text) = manifest {
                std::fs::write(dir.join(MANIFEST_FILE), text).unwrap();
            }
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A manifest that asks for nothing.
    fn manifest_of(id: &str) -> String {
        format!(r#"{{"id":"{id}","name":"Tools {id}","version":"0.3.1","api":1}}"#)
    }

    /// A manifest that asks for these permissions (written as the names of the file).
    fn asking(id: &str, permissions: &str) -> String {
        format!(r#"{{"id":"{id}","name":"Tools {id}","version":"0.3.1","permissions":{permissions},"api":1}}"#)
    }

    /// Installs on Linux where that has to work; if it does not, the test fails on this assertion.
    fn must_install(global: &Scope<Global>, folder: &Path, granted: &[AddonPermission]) -> AddonInfo {
        let result = install_from_folder(global, folder, Os::Linux, granted);
        assert!(result.is_ok(), "installing {folder:?}: {result:?}");
        result.unwrap()
    }

    fn ids(list: &[AddonInfo]) -> Vec<&str> {
        list.iter().map(|a| a.id.as_str()).collect()
    }

    fn statuses(global: &Scope<Global>) -> Vec<AddonStatus> {
        list(global, Os::Linux).into_iter().map(|a| a.status).collect()
    }

    #[test]
    fn installing_from_a_folder_links_it_and_the_list_shows_it_ready_with_what_its_manifest_says() {
        let s = Scratch::new("install");
        let global = s.data().global();
        let folder = s.addon("one", Some(&manifest_of("tools-one")));
        let info = must_install(&global, &folder, &[]);
        assert_eq!((info.id.as_str(), &info.folder, &info.status), ("tools-one", &folder, &AddonStatus::Ready));
        let manifest = info.manifest.as_ref().unwrap();
        assert_eq!(
            (manifest.name.as_str(), manifest.version.as_str(), manifest.platforms.clone()),
            ("Tools tools-one", "0.3.1", AddonPlatform::ALL.to_vec())
        );
        assert_eq!(list(&global, Os::Linux), vec![info]);
        // It is linked, not copied: the data folder holds a list, not the add-on.
        assert!(!s.root.join("data").join("addons").join("tools-one").exists());
        // The list is kept: another reading of the same data folder sees it.
        assert_eq!(ids(&list(&s.data().global(), Os::Linux)), ["tools-one"]);
    }

    #[test]
    fn add_ons_are_listed_in_the_order_they_were_installed() {
        let s = Scratch::new("order");
        let global = s.data().global();
        for id in ["zeta", "alpha", "mid"] {
            let folder = s.addon(id, Some(&manifest_of(id)));
            must_install(&global, &folder, &[]);
        }
        assert_eq!(ids(&list(&global, Os::Linux)), ["zeta", "alpha", "mid"]);
    }

    #[test]
    fn a_folder_that_cannot_be_an_add_on_is_refused_with_the_reason_and_nothing_is_kept() {
        let s = Scratch::new("refused");
        let global = s.data().global();
        let file = s.root.join("a-file");
        std::fs::write(&file, "x").unwrap();
        let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(MAX_MANIFEST_BYTES as usize));
        let script_is_a_folder = s.bare("script-folder", Some(&manifest_of("script-folder")));
        std::fs::create_dir_all(script_is_a_folder.join(MAIN_SCRIPT)).unwrap();
        let cases: Vec<(PathBuf, InstallError)> = vec![
            (PathBuf::from("relative/folder"), InstallError::NotAbsolute),
            (s.root.join("not-there"), InstallError::NotAFolder),
            (file, InstallError::NotAFolder),
            (s.addon("empty", None), InstallError::NoManifest),
            (s.addon("big", Some(&big)), InstallError::ManifestTooLarge),
            (s.addon("garbage", Some("not json")), InstallError::Manifest(ManifestError::NotJson)),
            (s.addon("noid", Some(r#"{"name":"x"}"#)), InstallError::Manifest(ManifestError::BadId)),
            (s.bare("no-script", Some(&manifest_of("no-script"))), InstallError::NoMainScript),
            (script_is_a_folder, InstallError::NoMainScript),
            (
                s.addon("future", Some(&manifest_of("future").replace(r#""api":1"#, r#""api":2"#))),
                InstallError::Unsupported(Unsupported::ApiTooNew),
            ),
            (
                s.addon("windows-only", Some(&manifest_of("windows-only").replace(r#""api":1"#, r#""api":1,"platforms":["windows"]"#))),
                InstallError::Unsupported(Unsupported::PlatformNotListed),
            ),
        ];
        for (folder, error) in cases {
            assert_eq!(install_from_folder(&global, &folder, Os::Linux, &[]), Err(error.clone()), "{folder:?}");
            // Looking at it first fails the same way.
            assert_eq!(inspect_folder(&global, &folder, Os::Linux).map(|_| ()), Err(error), "inspect {folder:?}");
        }
        assert_eq!(list(&global, Os::Linux), vec![]);
    }

    #[test]
    fn an_id_that_is_installed_is_not_installed_again_from_the_same_or_another_folder() {
        let s = Scratch::new("twice");
        let global = s.data().global();
        let first = s.addon("first", Some(&manifest_of("same-id")));
        let second = s.addon("second", Some(&manifest_of("same-id")));
        must_install(&global, &first, &[]);
        assert_eq!(install_from_folder(&global, &first, Os::Linux, &[]), Err(InstallError::AlreadyInstalled));
        assert_eq!(install_from_folder(&global, &second, Os::Linux, &[]), Err(InstallError::AlreadyInstalled));
        assert_eq!(inspect_folder(&global, &second, Os::Linux).map(|_| ()), Err(InstallError::AlreadyInstalled));
        let listed = list(&global, Os::Linux);
        assert_eq!((ids(&listed), &listed[0].folder), (vec!["same-id"], &first));
    }

    #[test]
    fn the_list_reads_the_folders_again_so_what_happened_to_them_is_a_status() {
        let s = Scratch::new("status");
        let global = s.data().global();
        let names = ["gone", "no-manifest", "broken", "renamed", "no-script", "newer", "grows", "fine"];
        let folders: Vec<PathBuf> = names.iter().map(|n| s.addon(n, Some(&manifest_of(n)))).collect();
        for folder in &folders {
            must_install(&global, folder, &[]);
        }
        std::fs::remove_dir_all(&folders[0]).unwrap();
        std::fs::remove_file(folders[1].join(MANIFEST_FILE)).unwrap();
        std::fs::write(folders[2].join(MANIFEST_FILE), r#"{"id":"broken"}"#).unwrap();
        std::fs::write(folders[3].join(MANIFEST_FILE), manifest_of("another-id")).unwrap();
        std::fs::remove_file(folders[4].join(MAIN_SCRIPT)).unwrap();
        std::fs::write(folders[5].join(MANIFEST_FILE), manifest_of("newer").replace(r#""api":1"#, r#""api":5"#)).unwrap();
        std::fs::write(folders[6].join(MANIFEST_FILE), asking("grows", r#"["run_process"]"#)).unwrap();
        let status: Vec<(String, AddonStatus)> = list(&global, Os::Linux).into_iter().map(|a| (a.id, a.status)).collect();
        assert_eq!(
            status,
            vec![
                ("gone".to_string(), AddonStatus::FolderMissing),
                ("no-manifest".to_string(), AddonStatus::NoManifest),
                ("broken".to_string(), AddonStatus::Manifest(ManifestError::BadName)),
                ("renamed".to_string(), AddonStatus::IdChanged),
                ("no-script".to_string(), AddonStatus::NoMainScript),
                ("newer".to_string(), AddonStatus::Unsupported(Unsupported::ApiTooNew)),
                ("grows".to_string(), AddonStatus::PermissionsChanged),
                ("fine".to_string(), AddonStatus::Ready),
            ]
        );
        // An add-on that cannot be read still says what it is as far as it is known.
        let listed = list(&global, Os::Linux);
        assert!(listed[0].manifest.is_none() && listed[7].manifest.is_some());
        // The same folder on a system the add-on is not for.
        let windows_only = s.addon("w", Some(&manifest_of("w").replace(r#""api":1"#, r#""api":1,"platforms":["windows"]"#)));
        assert_eq!(install_from_folder(&global, &windows_only, Os::Windows, &[]).map(|a| a.status), Ok(AddonStatus::Ready));
        let there = list(&global, Os::Linux).into_iter().find(|a| a.id == "w").map(|a| a.status);
        assert_eq!(there, Some(AddonStatus::Unsupported(Unsupported::PlatformNotListed)));
    }

    #[test]
    fn what_an_add_on_asks_for_is_shown_before_and_the_user_gives_any_part_of_it() {
        let s = Scratch::new("grants");
        let store = s.data();
        let global = store.global();
        let both = s.addon("both", Some(&asking("both", r#"["device_shell","run_process"]"#)));
        // Looking shows what it asks for and keeps nothing.
        assert_eq!(inspect_folder(&global, &both, Os::Linux).map(|m| m.permissions), Ok(vec![SHELL, PROCESS]));
        assert_eq!(list(&global, Os::Linux), vec![]);
        // Only the second one is given: the first is the user's refusal, and the add-on is installed all the same.
        let info = must_install(&global, &both, &[PROCESS]);
        assert_eq!((info.granted.clone(), info.status.clone()), (vec![PROCESS], AddonStatus::Ready));
        // What is kept says what was asked and what was given.
        let kept = global.load::<Installed>().links;
        assert_eq!((kept[0].declared.clone(), kept[0].granted.clone()), (vec![SHELL, PROCESS], vec![PROCESS]));
        assert_eq!(list(&s.data().global(), Os::Linux)[0].granted, vec![PROCESS]);
        // Nothing given is allowed too.
        let none = s.addon("none", Some(&asking("none", r#"["device_shell"]"#)));
        assert_eq!(must_install(&global, &none, &[]).granted, vec![]);
        // What is given in another order or twice is given once, in the order of the manifest.
        let ordered = s.addon("ordered", Some(&asking("ordered", r#"["device_shell","run_process"]"#)));
        assert_eq!(must_install(&global, &ordered, &[PROCESS, SHELL, PROCESS]).granted, vec![SHELL, PROCESS]);
        // What is not asked for is not given, whatever the user was shown.
        let plain = s.addon("plain", Some(&manifest_of("plain")));
        let info = must_install(&global, &plain, &[SHELL, PROCESS]);
        assert_eq!(info.granted, vec![]);
        let kept = global.load::<Installed>().links;
        assert_eq!(kept.iter().find(|l| l.id == "plain").map(|l| (l.declared.clone(), l.granted.clone())), Some((vec![], vec![])));
    }

    #[test]
    fn an_add_on_that_asks_for_more_than_the_user_decided_on_does_not_run_until_the_user_decides_again() {
        let s = Scratch::new("changed-permissions");
        let global = s.data().global();
        let folder = s.addon("grows", Some(&asking("grows", r#"["device_shell"]"#)));
        must_install(&global, &folder, &[]);
        // The user refused the one it asked for. The same manifest is not asked about again.
        assert_eq!(statuses(&global), vec![AddonStatus::Ready]);
        // The folder is linked, so its author (or anyone who can write there) can change what it asks.
        std::fs::write(folder.join(MANIFEST_FILE), asking("grows", r#"["device_shell","run_process"]"#)).unwrap();
        assert_eq!(statuses(&global), vec![AddonStatus::PermissionsChanged]);
        // The user decides again, giving one of the two.
        let decided = accept_permissions(&global, "grows", Os::Linux, &[PROCESS]);
        assert_eq!(decided.map(|i| (i.status, i.granted)), Ok((AddonStatus::Ready, vec![PROCESS])));
        assert_eq!(statuses(&global), vec![AddonStatus::Ready]);
        let kept = global.load::<Installed>().links;
        assert_eq!((kept[0].declared.clone(), kept[0].granted.clone()), (vec![SHELL, PROCESS], vec![PROCESS]));
        // One that asks for less than it did still runs, and may only do what it still asks for.
        std::fs::write(folder.join(MANIFEST_FILE), asking("grows", r#"["device_shell"]"#)).unwrap();
        let shown = list(&global, Os::Linux);
        assert_eq!((shown[0].status.clone(), shown[0].granted.clone()), (AddonStatus::Ready, vec![]));
        // Asking for what it had asked for before is not a change either (it was decided on).
        std::fs::write(folder.join(MANIFEST_FILE), asking("grows", r#"["run_process"]"#)).unwrap();
        assert_eq!(statuses(&global), vec![AddonStatus::Ready]);
        // There is nothing to decide for an add-on that is not installed.
        assert_eq!(accept_permissions(&global, "nobody", Os::Linux, &[]), Err(InstallError::NotInstalled));
    }

    #[test]
    fn deciding_again_needs_an_add_on_that_can_be_read() {
        let s = Scratch::new("decide-broken");
        let global = s.data().global();
        let folder = s.addon("fragile", Some(&manifest_of("fragile")));
        must_install(&global, &folder, &[]);
        std::fs::write(folder.join(MANIFEST_FILE), "not json").unwrap();
        assert_eq!(
            accept_permissions(&global, "fragile", Os::Linux, &[]).map(|_| ()),
            Err(InstallError::Manifest(ManifestError::NotJson))
        );
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(accept_permissions(&global, "fragile", Os::Linux, &[]).map(|_| ()), Err(InstallError::NotAFolder));
    }

    #[test]
    fn removing_forgets_the_add_on_and_leaves_its_folder_and_its_data_alone() {
        let s = Scratch::new("remove");
        let store = s.data();
        let global = store.global();
        let keep = s.addon("keep", Some(&manifest_of("keep-me")));
        let drop_it = s.addon("drop", Some(&manifest_of("drop-me")));
        must_install(&global, &keep, &[]);
        must_install(&global, &drop_it, &[]);
        // What the add-on kept for itself is the user's data: it stays.
        let data = store.addon("drop-me").unwrap();
        data.save_value(&crate::core::store::DataName::new("note").unwrap(), &serde_json::json!({ "x": 1 })).unwrap();
        assert!(remove(&global, "drop-me"));
        assert_eq!(ids(&list(&global, Os::Linux)), ["keep-me"]);
        assert!(drop_it.join(MANIFEST_FILE).is_file());
        assert!(s.root.join("data").join("addons").join("drop-me").join("note.json").is_file());
        // There being none is said, and is not an error.
        assert!(!remove(&global, "drop-me"));
        assert!(!remove(&global, "never-installed"));
        // It can be installed again.
        must_install(&global, &drop_it, &[]);
    }

    #[test]
    fn a_new_add_on_is_switched_on_and_so_is_one_kept_by_a_version_that_did_not_know_the_switch() {
        let s = Scratch::new("enabled-default");
        let global = s.data().global();
        let folder = s.addon("one", Some(&manifest_of("tools-one")));
        assert!(must_install(&global, &folder, &[]).enabled);
        assert!(list(&global, Os::Linux)[0].enabled);
        // The format of the list before the switch existed (the folder is made up).
        let old: AddonLink = serde_json::from_str(r#"{"id":"old","folder":"/made/up"}"#).unwrap();
        assert!(old.enabled, "a link written without the switch must stay on");
    }

    #[test]
    fn switching_off_is_kept_and_leaves_what_the_user_gave_and_other_add_ons_alone_and_switching_on_undoes_it() {
        let s = Scratch::new("enabled-switch");
        let global = s.data().global();
        let one = s.addon("one", Some(&asking("tools-one", r#"["device_shell"]"#)));
        let two = s.addon("two", Some(&manifest_of("tools-two")));
        must_install(&global, &one, &[SHELL]);
        must_install(&global, &two, &[]);
        let off = set_enabled(&global, "tools-one", Os::Linux, false).unwrap();
        assert_eq!((off.enabled, off.granted, off.status), (false, vec![SHELL], AddonStatus::Ready));
        let listed = list(&global, Os::Linux);
        assert_eq!(listed.iter().map(|a| (a.id.as_str(), a.enabled)).collect::<Vec<_>>(), [("tools-one", false), ("tools-two", true)]);
        // It is kept: another read of the store says the same, and so does deciding the permissions again.
        let decided = accept_permissions(&global, "tools-one", Os::Linux, &[SHELL]).unwrap();
        assert!(!decided.enabled, "deciding permissions must not switch it on");
        let on = set_enabled(&global, "tools-one", Os::Linux, true).unwrap();
        assert_eq!((on.enabled, on.granted), (true, vec![SHELL]));
        assert!(list(&global, Os::Linux).iter().all(|a| a.enabled));
    }

    #[test]
    fn switching_an_add_on_that_is_not_installed_is_refused_and_writes_nothing() {
        let s = Scratch::new("enabled-unknown");
        let global = s.data().global();
        assert_eq!(set_enabled(&global, "nobody", Os::Linux, false).map(|_| ()), Err(InstallError::NotInstalled));
        assert!(list(&global, Os::Linux).is_empty());
    }

    #[test]
    fn an_add_on_whose_folder_is_gone_can_still_be_removed() {
        let s = Scratch::new("remove-gone");
        let global = s.data().global();
        let folder = s.addon("short-lived", Some(&manifest_of("short-lived")));
        must_install(&global, &folder, &[]);
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(remove(&global, "short-lived"));
        assert_eq!(list(&global, Os::Linux), vec![]);
    }

    #[test]
    fn a_list_that_is_broken_is_an_empty_list_and_is_put_aside_not_lost() {
        let s = Scratch::new("broken-list");
        let store = s.data();
        let global = store.global();
        let folder = s.addon("one", Some(&manifest_of("tools-one")));
        must_install(&global, &folder, &[]);
        let file = s.root.join("data").join("installed-addons.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(list(&global, Os::Linux), vec![]);
        // Installing again works, and the broken file is kept beside the new one.
        must_install(&global, &folder, &[]);
        let kept = std::fs::read_dir(s.root.join("data"))
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("installed-addons"))
            .count();
        assert_eq!(kept, 2);
    }
}
