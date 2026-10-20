//! Which scrcpy release the program uses: the one the user chose, or else the default; what is to be done
//! when it is not installed; and the change from one installed release to another.
//!
//! A release is the whole of adb, scrcpy and scrcpy-server (they must be of one version), so changing the
//! release changes the adb that serves the devices too: an adb server that runs keeps serving, whatever
//! another adb is started, so the change stops the server and starts it again with the adb of the new release.

use super::manager::{Installed, find_version, installed_versions, parse_version};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The release the program is checked against and uses unless the user chose another. It is the only
/// place that names a release: a newer default is a change of this line.
pub const DEFAULT_TAG: &str = "v5.0.1";

/// The tag of a scrcpy release (`v5.0.1`): a `v` and dot-separated numbers, nothing else (it is a name
/// of a folder, and it comes from a file or the network).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReleaseTag(String);

impl ReleaseTag {
    pub fn parse(text: &str) -> Option<ReleaseTag> {
        parse_version(text).map(|_| ReleaseTag(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The release the program is checked against.
    pub fn default_tag() -> ReleaseTag {
        ReleaseTag(DEFAULT_TAG.to_string())
    }
}

impl TryFrom<String> for ReleaseTag {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        ReleaseTag::parse(&text).ok_or_else(|| format!("not the tag of a release: {text:?}"))
    }
}

impl From<ReleaseTag> for String {
    fn from(tag: ReleaseTag) -> String {
        tag.0
    }
}

/// What to do at the start, given the release that is wanted and what is installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// What can be used right away: the wanted release if it is installed, else the newest that is.
    pub use_now: Option<Installed>,
    /// The wanted release is not installed: the user is asked whether to download it.
    pub download_needed: bool,
}

pub fn plan(base: &Path, wanted: &ReleaseTag) -> Plan {
    match find_version(base, wanted.as_str()) {
        Some(installed) => Plan { use_now: Some(installed), download_needed: false },
        None => Plan { use_now: installed_versions(base).into_iter().next(), download_needed: true },
    }
}

/// The adb programs of the releases, as far as the change of release needs them.
pub trait AdbProcess {
    /// Stops the adb server (the server that answers at the usual place), as the adb at `adb` does.
    fn kill_server(&self, adb: &Path) -> impl std::future::Future<Output = Result<(), String>>;
    /// Starts the adb server, as the adb at `adb` does; one that is running is left running.
    fn start_server(&self, adb: &Path) -> impl std::future::Future<Output = Result<(), String>>;
}

/// Why the change of release did not come to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchError {
    /// The adb of the new release could not bring the server up (the text is the system's, for showing only).
    ServerNotStarted(String),
}

/// Makes the adb of `new` the one that serves: the server of the old release is stopped and one is started with
/// the adb of the new. Nothing is done for the same release.
pub async fn switch_adb(old: &Installed, new: &Installed, adb: &impl AdbProcess) -> Result<(), SwitchError> {
    if old == new {
        return Ok(());
    }
    // The old server may not be running (nothing to stop): that does not stop the new one from starting.
    let _ = adb.kill_server(&old.adb()).await;
    adb.start_server(&new.adb()).await.map_err(SwitchError::ServerNotStarted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn tag(text: &str) -> ReleaseTag {
        ReleaseTag::parse(text).unwrap_or_else(|| ReleaseTag(format!("(not a tag: {text})")))
    }

    #[test]
    fn the_default_release_is_one_that_is_a_release_tag() {
        assert_eq!(DEFAULT_TAG, "v5.0.1");
        assert_eq!(ReleaseTag::parse(DEFAULT_TAG), Some(ReleaseTag(DEFAULT_TAG.to_string())));
        assert_eq!(ReleaseTag::default_tag().as_str(), DEFAULT_TAG);
    }

    #[test]
    fn a_release_tag_is_a_v_and_numbers_and_nothing_else() {
        for good in ["v5.0.1", "v5.0", "v10.2.30", "v3"] {
            assert_eq!(ReleaseTag::parse(good).map(|t| t.as_str().to_string()), Some(good.to_string()), "{good}");
        }
        for bad in ["", "5.0.1", "v", "v5..1", "v5.0.1-beta", "v+5.0", "../v5.0.1", "v5.0/../x", "v5.0.1 ", " v5.0.1", "V5.0.1"] {
            assert_eq!(ReleaseTag::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_release_tag_is_written_in_a_file_as_its_text_and_a_text_that_is_not_one_is_refused() {
        let written = serde_json::to_string(&tag("v3.3.4"));
        assert_eq!(written.as_deref().ok(), Some(r#""v3.3.4""#));
        assert_eq!(serde_json::from_str::<ReleaseTag>(r#""v5.0.1""#).ok(), Some(tag("v5.0.1")));
        for bad in [r#""5.0.1""#, r#""../x""#, "7", "null", r#""""#] {
            assert!(serde_json::from_str::<ReleaseTag>(bad).is_err(), "{bad}");
        }
    }

    fn folder(name: &str, ready: &[&str], other: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-plan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (names, is_ready) in [(ready, true), (other, false)] {
            for n in names {
                std::fs::create_dir_all(dir.join(n)).unwrap();
                if is_ready {
                    std::fs::write(dir.join(n).join(".ready"), b"").unwrap();
                }
            }
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn versions(plan: &Plan) -> Option<String> {
        plan.use_now.as_ref().map(|i| i.version.clone())
    }

    #[test]
    fn the_wanted_release_is_used_when_it_is_installed_even_if_a_newer_one_is() {
        let dir = folder("wanted", &["v5.0.1", "v6.0.0", "v3.3.4"], &[]);
        let plan = plan(&dir, &tag("v5.0.1"));
        assert_eq!((versions(&plan), plan.download_needed), (Some("v5.0.1".to_string()), false));
        let older = super::plan(&dir, &tag("v3.3.4"));
        assert_eq!((versions(&older), older.download_needed), (Some("v3.3.4".to_string()), false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wanted_release_that_is_not_installed_is_to_be_asked_about_and_the_newest_that_is_installed_serves_meanwhile() {
        let dir = folder("missing", &["v3.3.4", "v5.0.0"], &["v5.0.1"]);
        let plan = plan(&dir, &tag("v5.0.1"));
        assert_eq!((versions(&plan), plan.download_needed), (Some("v5.0.0".to_string()), true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_nothing_installed_nothing_can_be_used_and_the_wanted_release_is_to_be_asked_about() {
        let dir = folder("nothing", &[], &["v5.0.1"]);
        let plan = plan(&dir, &tag("v5.0.1"));
        assert_eq!((versions(&plan), plan.download_needed), (None, true));
        let nowhere = super::plan(&dir.join("not-there"), &tag("v5.0.1"));
        assert_eq!((versions(&nowhere), nowhere.download_needed), (None, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The programs of adb are a fake that writes down what it was told.

    #[derive(Default)]
    struct FakeAdb {
        calls: Mutex<Vec<String>>,
        kill_fails: bool,
        start_fails: bool,
    }

    impl AdbProcess for FakeAdb {
        async fn kill_server(&self, adb: &Path) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("kill {}", adb.display()));
            if self.kill_fails { Err("no server".into()) } else { Ok(()) }
        }
        async fn start_server(&self, adb: &Path) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("start {}", adb.display()));
            if self.start_fails { Err("cannot start".into()) } else { Ok(()) }
        }
    }

    fn installed(version: &str) -> Installed {
        Installed { version: version.to_string(), root: PathBuf::from("releases").join(version) }
    }

    #[tokio::test]
    async fn changing_the_release_stops_the_server_of_the_old_and_starts_one_with_the_adb_of_the_new() {
        let (old, new) = (installed("v3.3.4"), installed("v5.0.1"));
        let adb = FakeAdb::default();
        assert_eq!(switch_adb(&old, &new, &adb).await, Ok(()));
        let expected = vec![format!("kill {}", old.adb().display()), format!("start {}", new.adb().display())];
        assert_eq!(*adb.calls.lock().unwrap(), expected);
    }

    #[tokio::test]
    async fn the_same_release_changes_nothing() {
        let adb = FakeAdb::default();
        assert_eq!(switch_adb(&installed("v5.0.1"), &installed("v5.0.1"), &adb).await, Ok(()));
        assert_eq!(*adb.calls.lock().unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_server_that_was_not_running_does_not_stop_the_new_one_from_starting_and_one_that_does_not_start_is_an_error() {
        let (old, new) = (installed("v3.3.4"), installed("v5.0.1"));
        let not_running = FakeAdb { kill_fails: true, ..FakeAdb::default() };
        assert_eq!(switch_adb(&old, &new, &not_running).await, Ok(()));
        assert_eq!(not_running.calls.lock().unwrap().len(), 2, "it still started");
        let stuck = FakeAdb { start_fails: true, ..FakeAdb::default() };
        assert_eq!(switch_adb(&old, &new, &stuck).await, Err(SwitchError::ServerNotStarted("cannot start".into())));
    }
}
