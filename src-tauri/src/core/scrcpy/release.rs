//! What GitHub tells of the releases of scrcpy: which there are, and, for one of them, what is to be
//! downloaded for this system, how large it is and what its SHA-256 must be. The answers are read into types;
//! asking GitHub for them is `manager`'s (so this is the same on every system and is tested without a network).

use super::manager::{Archive, asset_name, parse_sha256sums, parse_version};
use super::versions::ReleaseTag;
use serde::{Deserialize, Serialize};

/// What the user is told before a download and what is then downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Offer {
    pub tag: ReleaseTag,
    /// The name of the file (`scrcpy-win64-v5.0.1.zip`).
    pub asset: String,
    pub url: String,
    /// Its size in bytes, if GitHub says.
    pub size: Option<u64>,
    /// The SHA-256 the file must have, in lower case.
    pub sha256: String,
}

/// Why a release gives no offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferError {
    /// The answer is not a release, or its tag is not the tag of a release.
    BadRelease,
    /// There is no file of this release for this system.
    NoAsset,
    /// The file is not offered over a secure connection.
    Insecure,
    /// The release has no list of checksums, or it has none for the file.
    NoChecksum,
}

/// Why a release could not be listed, offered or downloaded. This crosses to the UI, which tells each in the
/// language of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseFailure {
    /// GitHub could not be reached, or answered with an error.
    Unreachable,
    /// There is no such release.
    NotFound,
    /// This system has no official build of scrcpy.
    Unsupported,
    Offer(OfferError),
    /// What was downloaded is not the file that the checksum is of.
    ChecksumMismatch,
    /// The file could not be unpacked.
    Unpack,
    /// The file could not be written.
    Storage,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    #[serde(default)]
    size: Option<u64>,
    browser_download_url: String,
}

fn secure(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| url.scheme() == "https")
}

/// The releases in the list GitHub gives (`/releases`), as tags, the newest first: the ones that are
/// drafts or pre-releases, and the ones whose tag is not a release tag, are left out.
pub fn parse_release_list(json: &str) -> Vec<ReleaseTag> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return Vec::new() };
    let mut found: Vec<(Vec<u32>, ReleaseTag)> = items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<Release>(item).ok())
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| Some((parse_version(&release.tag_name)?, ReleaseTag::parse(&release.tag_name)?)))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.dedup_by(|a, b| a.1 == b.1);
    found.into_iter().map(|(_, tag)| tag).collect()
}

/// Where the list of checksums of a release is (the file `SHA256SUMS.txt` of its assets), if it has one
/// that is offered over a secure connection.
pub fn checksums_url(release_json: &str) -> Option<String> {
    let release: Release = serde_json::from_str(release_json).ok()?;
    release
        .assets
        .into_iter()
        .find(|asset| asset.name == "SHA256SUMS.txt")
        .map(|asset| asset.browser_download_url)
        .filter(|url| secure(url))
}

/// The offer for this system from a release (`/releases/tags/<tag>`) and the text of its list of checksums.
pub fn offer_for(release_json: &str, checksums: &str, platform: &str, archive: Archive) -> Result<Offer, OfferError> {
    let release: Release = serde_json::from_str(release_json).map_err(|_| OfferError::BadRelease)?;
    let tag = ReleaseTag::parse(&release.tag_name).ok_or(OfferError::BadRelease)?;
    let name = asset_name(platform, archive, tag.as_str());
    let asset = release.assets.iter().find(|asset| asset.name == name).ok_or(OfferError::NoAsset)?;
    if !secure(&asset.browser_download_url) {
        return Err(OfferError::Insecure);
    }
    let sha256 = parse_sha256sums(checksums).remove(&name).ok_or(OfferError::NoChecksum)?;
    Ok(Offer { tag, asset: name, url: asset.browser_download_url.clone(), size: asset.size, sha256 })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The form of the answers is GitHub's (a release has `tag_name`, `draft`, `prerelease` and `assets`, each
    // with `name`, `size` and `browser_download_url`); every value here is made up.

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn tags(list: &[ReleaseTag]) -> Vec<&str> {
        list.iter().map(ReleaseTag::as_str).collect()
    }

    #[test]
    fn the_list_has_the_tags_of_the_releases_newest_first_without_drafts_pre_releases_and_names_that_are_not_tags() {
        let json = r#"[
            {"tag_name":"v3.3.4","draft":false,"prerelease":false,"assets":[]},
            {"tag_name":"v5.0","draft":false,"prerelease":false,"assets":[]},
            {"tag_name":"v5.0.1","draft":false,"prerelease":false,"assets":[]},
            {"tag_name":"nightly","draft":false,"prerelease":false,"assets":[]},
            {"tag_name":"v6.0.0","draft":false,"prerelease":true,"assets":[]},
            {"tag_name":"v9.9.9","draft":true,"prerelease":false,"assets":[]},
            {"tag_name":"v10.0.0","assets":[]}
        ]"#;
        assert_eq!(tags(&parse_release_list(json)), ["v10.0.0", "v5.0.1", "v5.0", "v3.3.4"]);
        // Something that is not a list of releases is none.
        for bad in ["", "{}", "null", r#"["v5.0.1"]"#, "[1,2]"] {
            assert_eq!(parse_release_list(bad), vec![], "{bad:?}");
        }
    }

    fn release(tag: &str, assets: &str) -> String {
        format!(r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"assets":[{assets}]}}"#)
    }

    fn asset(name: &str, size: &str, url: &str) -> String {
        format!(r#"{{"name":"{name}","size":{size},"browser_download_url":"{url}"}}"#)
    }

    fn full_release() -> String {
        release(
            "v5.0.1",
            &[
                asset("scrcpy-win64-v5.0.1.zip", "11000000", "https://example.test/dl/scrcpy-win64-v5.0.1.zip"),
                asset("scrcpy-linux-x86_64-v5.0.1.tar.gz", "9000000", "https://example.test/dl/scrcpy-linux-x86_64-v5.0.1.tar.gz"),
                asset("SHA256SUMS.txt", "400", "https://example.test/dl/SHA256SUMS.txt"),
            ]
            .join(","),
        )
    }

    fn sums() -> String {
        format!("{HASH}  scrcpy-win64-v5.0.1.zip\n{OTHER_HASH} *scrcpy-linux-x86_64-v5.0.1.tar.gz\n")
    }

    #[test]
    fn the_list_of_checksums_is_the_asset_of_that_name_and_only_over_a_secure_connection() {
        assert_eq!(checksums_url(&full_release()).as_deref(), Some("https://example.test/dl/SHA256SUMS.txt"));
        let without = release("v5.0.1", &asset("scrcpy-win64-v5.0.1.zip", "1", "https://example.test/a.zip"));
        assert_eq!(checksums_url(&without), None);
        let insecure = release("v5.0.1", &asset("SHA256SUMS.txt", "1", "http://example.test/SHA256SUMS.txt"));
        assert_eq!(checksums_url(&insecure), None);
        assert_eq!(checksums_url("not json"), None);
    }

    #[test]
    fn the_offer_is_the_file_of_this_system_with_its_url_its_size_and_the_checksum_that_is_listed_for_it() {
        let windows = offer_for(&full_release(), &sums(), "win64", Archive::Zip);
        assert_eq!(
            windows,
            Ok(Offer {
                tag: ReleaseTag::parse("v5.0.1").unwrap_or_else(ReleaseTag::default_tag),
                asset: "scrcpy-win64-v5.0.1.zip".into(),
                url: "https://example.test/dl/scrcpy-win64-v5.0.1.zip".into(),
                size: Some(11_000_000),
                sha256: HASH.into(),
            })
        );
        let linux = offer_for(&full_release(), &sums(), "linux-x86_64", Archive::TarGz);
        assert_eq!(linux.map(|o| (o.asset, o.size, o.sha256)), Ok(("scrcpy-linux-x86_64-v5.0.1.tar.gz".to_string(), Some(9_000_000), OTHER_HASH.to_string())));
        // A checksum written in capitals is the same checksum.
        let capitals = offer_for(&full_release(), &sums().to_uppercase().replace("SCRCPY-WIN64-V5.0.1.ZIP", "scrcpy-win64-v5.0.1.zip"), "win64", Archive::Zip);
        assert_eq!(capitals.map(|o| o.sha256), Ok(HASH.to_string()));
    }

    #[test]
    fn a_release_that_cannot_be_offered_says_why() {
        let sums = sums();
        // Not a release, or a tag that is not a release's.
        assert_eq!(offer_for("not json", &sums, "win64", Archive::Zip), Err(OfferError::BadRelease));
        assert_eq!(offer_for(&full_release().replace("v5.0.1\",\"draft", "../x\",\"draft"), &sums, "win64", Archive::Zip), Err(OfferError::BadRelease));
        // No file for this system.
        assert_eq!(offer_for(&full_release(), &sums, "winarm64", Archive::Zip), Err(OfferError::NoAsset));
        // The file is not offered over a secure connection.
        let insecure = release("v5.0.1", &asset("scrcpy-win64-v5.0.1.zip", "1", "http://example.test/a.zip"));
        assert_eq!(offer_for(&insecure, &sums, "win64", Archive::Zip), Err(OfferError::Insecure));
        // Nothing listed for the file, or a line that is not a checksum.
        assert_eq!(offer_for(&full_release(), "", "win64", Archive::Zip), Err(OfferError::NoChecksum));
        let not_a_checksum = format!("{}  scrcpy-win64-v5.0.1.zip\n", "z".repeat(64));
        assert_eq!(offer_for(&full_release(), &not_a_checksum, "win64", Archive::Zip), Err(OfferError::NoChecksum));
    }

    #[test]
    fn a_size_that_is_not_given_is_not_known() {
        let json = release("v5.0.1", &asset("scrcpy-win64-v5.0.1.zip", "null", "https://example.test/a.zip"));
        assert_eq!(offer_for(&json, &sums(), "win64", Archive::Zip).map(|o| o.size), Ok(None));
    }
}
