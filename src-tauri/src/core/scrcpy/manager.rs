//! Finds, downloads, verifies and unpacks the official scrcpy releases. adb,
//! scrcpy and scrcpy-server always come from the same release, because the
//! client and server must have exactly matching versions.
//!
//! Which release is used, and what is done when it is not installed, is `versions`'s; what GitHub tells of a
//! release is read by `release`. This is the folder of the releases that are installed and the downloading.

use super::release::{Offer, ReleaseFailure, OfferError, checksums_url, offer_for, parse_release_list};
use super::versions::ReleaseTag;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const RELEASES_API: &str = "https://api.github.com/repos/Genymobile/scrcpy/releases";
const READY_MARKER: &str = ".ready";

/// How many of the newest releases are listed.
const LIST_LENGTH: u32 = 30;

#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub version: String,
    pub root: PathBuf,
}

impl Installed {
    pub fn adb(&self) -> PathBuf {
        self.root.join(exe("adb"))
    }
    pub fn scrcpy(&self) -> PathBuf {
        self.root.join(exe("scrcpy"))
    }
    pub fn server(&self) -> PathBuf {
        self.root.join("scrcpy-server")
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// How the official release archive for a platform is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    Zip,
    TarGz,
}

impl Archive {
    pub fn extension(self) -> &'static str {
        match self {
            Archive::Zip => "zip",
            Archive::TarGz => "tar.gz",
        }
    }
}

/// Platform part of the official asset name (e.g. `win64`) and the archive
/// format used for it.
fn platform_tag() -> Option<(&'static str, Archive)> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some(("win64", Archive::Zip))
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some(("winarm64", Archive::Zip))
    } else if cfg!(all(windows, target_arch = "x86")) {
        Some(("win32", Archive::Zip))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(("linux-x86_64", Archive::TarGz))
    } else {
        None
    }
}

pub fn asset_name(platform: &str, archive: Archive, tag: &str) -> String {
    format!("scrcpy-{platform}-{tag}.{}", archive.extension())
}

/// Parses `SHA256SUMS.txt` (`<hash>  <name>` per line, `*` marks binary mode).
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            // `sha256sum` marks binary mode with a `*` in front of the name.
            let name = name.trim();
            let name = name.strip_prefix('*').unwrap_or(name);
            let is_sha256 = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            (is_sha256 && !name.is_empty()).then(|| (name.to_string(), hash.to_lowercase()))
        })
        .collect()
}

/// `v5.0.1` -> `[5, 0, 1]`. Strict: a `v` followed by dot-separated numbers
/// and nothing else. The tag comes from the network and is used as a folder
/// name, so anything of another shape is rejected.
pub fn parse_version(tag: &str) -> Option<Vec<u32>> {
    let numbers: Vec<u32> = tag
        .strip_prefix('v')?
        .split('.')
        .map(|part| {
            // `parse` also accepts a leading `+`; only plain digits are valid.
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect::<Option<_>>()?;
    (!numbers.is_empty()).then_some(numbers)
}

/// Every ready install in `base`, the folder of the downloaded releases (`Store::scrcpy_dir`), the newest first.
pub fn installed_versions(base: &Path) -> Vec<Installed> {
    let mut found: Vec<(Vec<u32>, Installed)> = Vec::new();
    for entry in fs::read_dir(base).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = parse_version(&name) else { continue };
        if entry.path().join(READY_MARKER).exists() {
            found.push((version, Installed { version: name, root: entry.path() }));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, installed)| installed).collect()
}

/// The ready install of exactly this release (`v5.0.1` is not `v5.0.10`), if there is one.
pub fn find_version(base: &Path, tag: &str) -> Option<Installed> {
    // The tag is a folder name: only the shape of one gets to be looked for.
    parse_version(tag)?;
    let root = base.join(tag);
    root.join(READY_MARKER).exists().then(|| Installed { version: tag.to_string(), root })
}

fn client() -> Result<reqwest::Client, ReleaseFailure> {
    reqwest::Client::builder()
        .user_agent("scrcpy-agent")
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| ReleaseFailure::Unreachable)
}

/// What GitHub answers with at `url`, as text. A release that is not there is `NotFound`; anything else that
/// is not an answer is `Unreachable`.
async fn get_text(http: &reqwest::Client, url: &str) -> Result<String, ReleaseFailure> {
    let response =
        http.get(url).timeout(Duration::from_secs(20)).send().await.map_err(|_| ReleaseFailure::Unreachable)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ReleaseFailure::NotFound);
    }
    let response = response.error_for_status().map_err(|_| ReleaseFailure::Unreachable)?;
    response.text().await.map_err(|_| ReleaseFailure::Unreachable)
}

/// The newest releases of scrcpy there are, as tags, the newest first.
pub async fn fetch_list() -> Result<Vec<ReleaseTag>, ReleaseFailure> {
    let http = client()?;
    let json = get_text(&http, &format!("{RELEASES_API}?per_page={LIST_LENGTH}")).await?;
    Ok(parse_release_list(&json))
}

/// What there is to download of this release for this system: asks GitHub for the release and for the list of
/// checksums, and downloads nothing else.
pub async fn fetch_offer(tag: &ReleaseTag) -> Result<Offer, ReleaseFailure> {
    let (platform, archive) = platform_tag().ok_or(ReleaseFailure::Unsupported)?;
    let http = client()?;
    let json = get_text(&http, &format!("{RELEASES_API}/tags/{}", tag.as_str())).await?;
    let sums_url = checksums_url(&json).ok_or(ReleaseFailure::Offer(OfferError::NoChecksum))?;
    let sums = get_text(&http, &sums_url).await?;
    offer_for(&json, &sums, platform, archive).map_err(ReleaseFailure::Offer)
}

/// Downloads what is offered into `base`, checks that it is the file whose SHA-256 the offer has, and unpacks it
/// there. `progress(tag, 0..1)` is called while downloading.
pub async fn download(
    base: &Path,
    offer: &Offer,
    progress: &(dyn Fn(&str, f32) + Send + Sync),
) -> Result<Installed, ReleaseFailure> {
    let (_, archive) = platform_tag().ok_or(ReleaseFailure::Unsupported)?;
    let tag = offer.tag.as_str().to_string();
    let storage = |_| ReleaseFailure::Storage;
    fs::create_dir_all(base).map_err(storage)?;
    let part = base.join(format!(".{}.part", offer.asset));
    progress(&tag, 0.0);

    let http = client()?;
    let mut response =
        http.get(&offer.url).send().await.and_then(|r| r.error_for_status()).map_err(|_| ReleaseFailure::Unreachable)?;
    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&part).await.map_err(storage)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| ReleaseFailure::Unreachable)? {
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(storage)?;
        done += chunk.len() as u64;
        if total > 0 {
            progress(&tag, (done as f32 / total as f32).min(1.0));
        }
    }
    file.flush().await.map_err(storage)?;
    drop(file);

    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != offer.sha256 {
        let _ = fs::remove_file(&part);
        return Err(ReleaseFailure::ChecksumMismatch);
    }

    let dest = base.join(&tag);
    let (unpack_from, unpack_to) = (part.clone(), dest.clone());
    let unpacked = tokio::task::spawn_blocking(move || extract(&unpack_from, &unpack_to, archive)).await;
    let _ = fs::remove_file(&part);
    match unpacked {
        Ok(Ok(())) => Ok(Installed { version: tag, root: dest }),
        _ => Err(ReleaseFailure::Unpack),
    }
}

/// Unpacks into `<dest>.tmp`, strips the single top-level folder the official
/// archives contain, then renames into place and writes the ready marker.
fn extract(file: &Path, dest: &Path, archive: Archive) -> anyhow::Result<()> {
    let tmp = dest.with_extension("tmp");
    let _ = fs::remove_dir_all(&tmp);
    let _ = fs::remove_dir_all(dest);
    fs::create_dir_all(&tmp)?;

    match archive {
        Archive::Zip => {
            let mut zip = zip::ZipArchive::new(fs::File::open(file)?)?;
            for i in 0..zip.len() {
                let mut f = zip.by_index(i)?;
                // `enclosed_name` rejects entries that would escape the folder.
                let Some(rel) = f.enclosed_name() else { continue };
                let out = tmp.join(rel);
                if f.is_dir() {
                    fs::create_dir_all(&out)?;
                } else {
                    if let Some(p) = out.parent() {
                        fs::create_dir_all(p)?;
                    }
                    std::io::copy(&mut f, &mut fs::File::create(&out)?)?;
                }
            }
        }
        Archive::TarGz => {
            let gz = flate2::read::GzDecoder::new(fs::File::open(file)?);
            tar::Archive::new(gz).unpack(&tmp)?;
        }
    }

    let mut root = tmp.clone();
    let children: Vec<_> = fs::read_dir(&tmp)?.flatten().collect();
    if children.len() == 1 && children[0].path().is_dir() {
        root = children[0].path();
    }
    fs::write(root.join(READY_MARKER), b"")?;
    fs::rename(&root, dest)?;
    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_official_asset_names() {
        assert_eq!(asset_name("win64", Archive::Zip, "v5.0.1"), "scrcpy-win64-v5.0.1.zip");
        assert_eq!(
            asset_name("linux-x86_64", Archive::TarGz, "v5.0.1"),
            "scrcpy-linux-x86_64-v5.0.1.tar.gz"
        );
    }

    #[test]
    fn parses_checksum_file() {
        let h = "b12a2c4ee8be317422451fc7dcf8ee20a71b5ea7ef9ad73ddd825a25316227a5";
        let m = parse_sha256sums(&format!("{h}  scrcpy-win64-v5.0.1.zip\n{h} *other.bin\nbad line\n"));
        assert_eq!(m.get("scrcpy-win64-v5.0.1.zip").map(String::as_str), Some(h));
        assert!(m.contains_key("other.bin"));
        assert_eq!(m.len(), 2);
        // A hash that is not 64 hex digits is not a SHA-256.
        let not_hex = "z".repeat(64);
        assert!(parse_sha256sums(&format!("{not_hex}  x.zip\n")).is_empty());
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(parse_version("v5.0.1") > parse_version("v5.0.0"));
        assert!(parse_version("v10.0.0") > parse_version("v9.9.9"));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn release_tags_that_are_not_plain_versions_are_rejected() {
        // The tag is used as a folder name, so these must never get through.
        for bad in ["5.0.1", "v", "v5..1", "v5.0.1-beta", "v+5.0", "../v5.0.1", "v5.0/../x", "v5.0.1 "] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    /// A folder of downloaded releases: the names with a marker are ready installs, the others are not.
    fn releases_folder(name: &str, folders: &[(&str, bool)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-releases-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (folder, ready) in folders {
            let path = dir.join(folder);
            fs::create_dir_all(&path).unwrap();
            if *ready {
                fs::write(path.join(READY_MARKER), b"").unwrap();
            }
        }
        dir
    }

    #[test]
    fn every_ready_install_is_listed_newest_first_and_what_is_not_one_is_left_out() {
        let dir = releases_folder(
            "list",
            &[("v3.9.0", true), ("v5.0.1", true), ("v5.0", true), ("v6.0.0", false), ("junk", true), (".part", true), ("v10.0.0", true)],
        );
        let listed: Vec<String> = installed_versions(&dir).into_iter().map(|i| i.version).collect();
        assert_eq!(listed, vec!["v10.0.0", "v5.0.1", "v5.0", "v3.9.0"]);
        assert_eq!(installed_versions(&dir.join("not-there")), vec![]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_is_found_only_by_exactly_its_tag_and_only_when_it_is_ready() {
        let dir = releases_folder("find", &[("v5.0.1", true), ("v5.0.10", true), ("v3.3.4", false)]);
        assert_eq!(find_version(&dir, "v5.0.1").map(|i| (i.version, i.root)), Some(("v5.0.1".to_string(), dir.join("v5.0.1"))));
        assert_eq!(find_version(&dir, "v5.0.10").map(|i| i.version), Some("v5.0.10".to_string()));
        // Another release, one that is not ready, one that is not there, and text that is not a tag.
        for not_found in ["v5.0", "v5.0.2", "v3.3.4", "5.0.1", "../v5.0.1", ""] {
            assert_eq!(find_version(&dir, not_found), None, "{not_found:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Downloads the default release from the real GitHub into a folder of the temporary files, as the program does
    /// (`cargo test --lib live_github_downloads -- --ignored --nocapture`; about 11 MB), and checks that it is found
    /// as an install with adb, scrcpy and the server in it.
    #[tokio::test]
    #[ignore = "downloads from the real GitHub"]
    async fn live_github_downloads_the_default_release_and_it_is_then_installed() {
        let base = std::env::temp_dir().join(format!("scrcpy-agent-live-download-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let tag = ReleaseTag::default_tag();
        let offer = fetch_offer(&tag).await.expect("the offer");
        let last = std::sync::Mutex::new(0.0f32);
        let progress = |_: &str, p: f32| *last.lock().unwrap() = p;
        let installed = download(&base, &offer, &progress).await.expect("the download");
        assert_eq!(installed.version, tag.as_str());
        assert!(installed.adb().is_file() && installed.scrcpy().is_file() && installed.server().is_file());
        assert_eq!(find_version(&base, tag.as_str()), Some(installed));
        assert!((*last.lock().unwrap() - 1.0).abs() < f32::EPSILON, "progress reached the end");
        println!("downloaded and unpacked: {} files", fs::read_dir(base.join(tag.as_str())).unwrap().count());
        let _ = fs::remove_dir_all(&base);
    }

    /// Asks the real GitHub (not run with the others: `cargo test --lib live_github -- --ignored --nocapture`).
    /// Lists the releases, asks for the offer of the default one and checks that it has what the offer needs;
    /// downloads nothing. It shows that the form of what GitHub answers is what `release` reads.
    #[tokio::test]
    #[ignore = "asks the real GitHub"]
    async fn live_github_lists_the_releases_and_offers_the_default() {
        let list = fetch_list().await.expect("the list of releases");
        assert!(!list.is_empty());
        println!("{} releases listed, the newest is {}", list.len(), list[0].as_str());
        let offer = fetch_offer(&ReleaseTag::default_tag()).await.expect("the offer of the default release");
        assert_eq!(offer.tag, ReleaseTag::default_tag());
        assert_eq!(offer.sha256.len(), 64);
        assert!(offer.url.starts_with("https://"));
        println!("offer: {} ({:?} bytes)", offer.asset, offer.size);
        let missing = fetch_offer(&ReleaseTag::parse("v0.0.1").unwrap()).await;
        assert_eq!(missing.map(|_| ()), Err(ReleaseFailure::NotFound));
    }
}
