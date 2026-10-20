//! `addon.json`: what an add-on says about itself. It is read into types (nothing is guessed from
//! the text): an add-on whose manifest is not exactly right is not installed, and the reason is a
//! value (`ManifestError`), not a message.
//!
//! The manifest says who the add-on is and what it asks to be allowed (`permissions`); what it does is
//! in `main.js`, which registers it through the interface.

use super::buttons::valid_name;
use super::permission::AddonPermission;
use super::settings::SettingSpec;
use crate::core::window::layout::Os;
use serde::{Deserialize, Serialize};

/// The version of the interface between the program and add-ons that this program has. An add-on
/// says which version it was written for (`api`); a newer one than this is not run.
pub const SUPPORTED_API: u32 = 1;

pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_VERSION_CHARS: usize = 40;

/// A system an add-on can say it works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonPlatform {
    Windows,
    Linux,
    Android,
}

impl AddonPlatform {
    pub const ALL: [AddonPlatform; 3] = [AddonPlatform::Windows, AddonPlatform::Linux, AddonPlatform::Android];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub platforms: Vec<AddonPlatform>,
    pub permissions: Vec<AddonPermission>,
    pub api: u32,
    /// What the user can set for the add-on (see `settings`); none if it declares none.
    pub settings: Vec<SettingSpec>,
}

/// Why an `addon.json` is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestError {
    NotJson,
    NotAnObject,
    /// Missing, or not a name (1 to 64 ASCII letters, digits, `_` or `-`).
    BadId,
    /// Missing, empty, longer than `MAX_NAME_CHARS`, or with a control character.
    BadName,
    /// Missing, empty, longer than `MAX_VERSION_CHARS`, or with a control character.
    BadVersion,
    /// Not a list of known permissions, without repeats.
    BadPermissions,
    /// Not a list of known systems, without repeats, with at least one.
    BadPlatforms,
    /// Missing, or not a whole number from 1.
    BadApi,
    /// `settings` is not a list of declarations that are right (see `settings::SettingsProblem` for what is checked).
    BadSettings,
}

/// Why an add-on that is fine as a manifest cannot run here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unsupported {
    /// It was written for a newer interface than `SUPPORTED_API`.
    ApiTooNew,
    /// This system is not one of its `platforms`.
    PlatformNotListed,
}

/// Reads `addon.json`: an object with `id`, `name`, `version`, `api` and, optionally, `permissions`
/// (none if it is left out) and `platforms` (all systems if it is left out). Other members are left
/// alone (a later version may add some).
pub fn parse_manifest(text: &str) -> Result<AddonManifest, ManifestError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| ManifestError::NotJson)?;
    let object = value.as_object().ok_or(ManifestError::NotAnObject)?;
    let id = match object.get("id") {
        Some(serde_json::Value::String(id)) if valid_name(id) => id.clone(),
        _ => return Err(ManifestError::BadId),
    };
    let name = clean_text(object.get("name"), MAX_NAME_CHARS).ok_or(ManifestError::BadName)?;
    let version = clean_text(object.get("version"), MAX_VERSION_CHARS).ok_or(ManifestError::BadVersion)?;
    let permissions = match object.get("permissions") {
        None => Vec::new(),
        Some(list) => known_list::<AddonPermission>(list).ok_or(ManifestError::BadPermissions)?,
    };
    let platforms = match object.get("platforms") {
        None => AddonPlatform::ALL.to_vec(),
        Some(list) => known_list::<AddonPlatform>(list).filter(|l| !l.is_empty()).ok_or(ManifestError::BadPlatforms)?,
    };
    let api = object
        .get("api")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n >= 1)
        .ok_or(ManifestError::BadApi)?;
    let settings = match object.get("settings") {
        None => Vec::new(),
        Some(list) => super::settings::parse_settings(list).map_err(|_| ManifestError::BadSettings)?,
    };
    Ok(AddonManifest { id, name, version, platforms, permissions, api, settings })
}

/// A text member: a string, without the spaces around it, from 1 to `max` characters, with no control character.
fn clean_text(value: Option<&serde_json::Value>, max: usize) -> Option<String> {
    let text = value?.as_str()?.trim();
    let usable = !text.is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control);
    usable.then(|| text.to_string())
}

/// A list of known names (as the type `T` has them) without repeats; `None` for anything else.
fn known_list<T: serde::de::DeserializeOwned + PartialEq>(value: &serde_json::Value) -> Option<Vec<T>> {
    let mut known: Vec<T> = Vec::new();
    for item in value.as_array()? {
        let parsed: T = serde_json::from_value(item.clone()).ok()?;
        if known.contains(&parsed) {
            return None;
        }
        known.push(parsed);
    }
    Some(known)
}

/// Whether the add-on can run on this system with this program.
pub fn supported(manifest: &AddonManifest, os: Os) -> Result<(), Unsupported> {
    if manifest.api > SUPPORTED_API {
        return Err(Unsupported::ApiTooNew);
    }
    let here = match os {
        Os::Windows => Some(AddonPlatform::Windows),
        Os::Linux => Some(AddonPlatform::Linux),
        // The systems the add-ons do not run on (yet).
        Os::Other => None,
    };
    match here {
        Some(platform) if manifest.platforms.contains(&platform) => Ok(()),
        _ => Err(Unsupported::PlatformNotListed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `addon.json` is this project's own format (nothing outside defines it); every value here is made up.

    fn full() -> &'static str {
        r#"{
            "id": "hello-tools",
            "name": "Hello tools",
            "version": "1.2.0",
            "permissions": ["device_shell", "run_process"],
            "platforms": ["windows", "linux"],
            "api": 1
        }"#
    }

    fn minimal() -> &'static str {
        r#"{"id":"small","name":"Small","version":"0.1","api":1}"#
    }

    fn manifest(json: &str) -> Result<AddonManifest, ManifestError> {
        parse_manifest(json)
    }

    /// The text of `base` with one member replaced (or taken away, with `None`).
    fn with_in(base: &str, member: &str, value: Option<&str>) -> String {
        let mut object: serde_json::Map<String, serde_json::Value> = serde_json::from_str(base).unwrap();
        match value {
            Some(v) => object.insert(member.to_string(), serde_json::from_str(v).unwrap()),
            None => object.remove(member),
        };
        serde_json::Value::Object(object).to_string()
    }

    fn with(member: &str, value: Option<&str>) -> String {
        with_in(full(), member, value)
    }

    #[test]
    fn a_complete_manifest_is_read_into_its_types() {
        assert_eq!(
            manifest(full()),
            Ok(AddonManifest {
                id: "hello-tools".into(),
                name: "Hello tools".into(),
                version: "1.2.0".into(),
                platforms: vec![AddonPlatform::Windows, AddonPlatform::Linux],
                permissions: vec![AddonPermission::DeviceShell, AddonPermission::RunProcess],
                api: 1,
                settings: vec![],
            })
        );
    }

    #[test]
    fn a_manifest_of_four_members_asks_for_nothing_and_is_for_every_system() {
        assert_eq!(
            manifest(minimal()),
            Ok(AddonManifest {
                id: "small".into(),
                name: "Small".into(),
                version: "0.1".into(),
                platforms: AddonPlatform::ALL.to_vec(),
                permissions: vec![],
                api: 1,
                settings: vec![],
            })
        );
        // An empty list of permissions is the same as none.
        assert_eq!(manifest(&with_in(minimal(), "permissions", Some("[]"))).map(|m| m.permissions), Ok(vec![]));
    }

    #[test]
    fn other_members_are_left_alone_and_spaces_around_texts_are_not_part_of_them() {
        assert_eq!(manifest(&with("future", Some("[1,2]"))).map(|m| m.id), Ok("hello-tools".to_string()));
        let spaced = with("name", Some(r#""  Hello tools  ""#));
        assert_eq!(manifest(&spaced).map(|m| m.name), Ok("Hello tools".to_string()));
        let spaced = with("version", Some(r#"" 1.2.0 ""#));
        assert_eq!(manifest(&spaced).map(|m| m.version), Ok("1.2.0".to_string()));
    }

    #[test]
    fn the_settings_an_add_on_declares_are_read_into_the_manifest_and_a_wrong_declaration_makes_it_not_one() {
        let declared = r#"[{"id":"level","type":"number","label":"Level","default":5,"min":0,"max":10},
                            {"id":"on","type":"boolean","label":"On","default":false}]"#;
        let read = manifest(&with("settings", Some(declared))).expect("a good manifest");
        assert_eq!(read.settings.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["level", "on"]);
        assert_eq!(
            read.settings[0].kind,
            super::super::settings::SettingKind::Number { default: 5, min: 0, max: 10 }
        );
        // None declared is the same as an empty list.
        assert_eq!(manifest(minimal()).map(|m| m.settings.len()), Ok(0));
        assert_eq!(manifest(&with("settings", Some("[]"))).map(|m| m.settings.len()), Ok(0));
        for wrong in [
            r#""level""#,
            "null",
            r#"{"id":"level"}"#,
            r#"[{"id":"level","type":"number","label":"L","default":50,"min":0,"max":10}]"#,
            r#"[{"id":"a","type":"boolean","label":"A","default":true},{"id":"a","type":"boolean","label":"B","default":true}]"#,
        ] {
            assert_eq!(manifest(&with("settings", Some(wrong))), Err(ManifestError::BadSettings), "{wrong}");
        }
    }

    #[test]
    fn text_that_is_not_an_object_is_told_apart_from_an_object_that_is_wrong() {
        assert_eq!(manifest(""), Err(ManifestError::NotJson));
        assert_eq!(manifest("{"), Err(ManifestError::NotJson));
        assert_eq!(manifest("[1]"), Err(ManifestError::NotAnObject));
        assert_eq!(manifest(r#""text""#), Err(ManifestError::NotAnObject));
        assert_eq!(manifest("null"), Err(ManifestError::NotAnObject));
    }

    #[test]
    fn every_member_that_is_missing_or_wrong_is_refused_with_the_reason_of_that_member() {
        let long_name = format!("\"{}\"", "n".repeat(MAX_NAME_CHARS + 1));
        let long_version = format!("\"{}\"", "1".repeat(MAX_VERSION_CHARS + 1));
        let cases: Vec<(&str, Option<&str>, ManifestError)> = vec![
            ("id", None, ManifestError::BadId),
            ("id", Some(r#""""#), ManifestError::BadId),
            ("id", Some(r#""a/b""#), ManifestError::BadId),
            ("id", Some(r#""..""#), ManifestError::BadId),
            ("id", Some(r#""has space""#), ManifestError::BadId),
            ("id", Some("7"), ManifestError::BadId),
            ("name", None, ManifestError::BadName),
            ("name", Some(r#""   ""#), ManifestError::BadName),
            ("name", Some(&long_name), ManifestError::BadName),
            ("name", Some(r#""two\nlines""#), ManifestError::BadName),
            ("name", Some("true"), ManifestError::BadName),
            ("version", None, ManifestError::BadVersion),
            ("version", Some(r#""""#), ManifestError::BadVersion),
            ("version", Some(&long_version), ManifestError::BadVersion),
            ("version", Some("1.2"), ManifestError::BadVersion),
            ("permissions", Some(r#"["root"]"#), ManifestError::BadPermissions),
            ("permissions", Some(r#"["run_process","run_process"]"#), ManifestError::BadPermissions),
            ("permissions", Some(r#""run_process""#), ManifestError::BadPermissions),
            ("permissions", Some(r#"["Run_Process"]"#), ManifestError::BadPermissions),
            ("permissions", Some(r#"[1]"#), ManifestError::BadPermissions),
            ("permissions", Some("null"), ManifestError::BadPermissions),
            ("platforms", Some("[]"), ManifestError::BadPlatforms),
            ("platforms", Some(r#"["macos"]"#), ManifestError::BadPlatforms),
            ("platforms", Some(r#"["linux","linux"]"#), ManifestError::BadPlatforms),
            ("platforms", Some(r#""linux""#), ManifestError::BadPlatforms),
            ("platforms", Some(r#"["Linux"]"#), ManifestError::BadPlatforms),
            ("api", None, ManifestError::BadApi),
            ("api", Some("0"), ManifestError::BadApi),
            ("api", Some("-1"), ManifestError::BadApi),
            ("api", Some("1.5"), ManifestError::BadApi),
            ("api", Some(r#""1""#), ManifestError::BadApi),
        ];
        for (member, value, error) in cases {
            assert_eq!(manifest(&with(member, value)), Err(error), "{member} = {value:?}");
        }
    }

    #[test]
    fn the_longest_allowed_texts_are_allowed() {
        let name = "ü".repeat(MAX_NAME_CHARS);
        let version = "v".repeat(MAX_VERSION_CHARS);
        assert_eq!(manifest(&with("name", Some(&format!("\"{name}\"")))).map(|m| m.name), Ok(name));
        assert_eq!(manifest(&with("version", Some(&format!("\"{version}\"")))).map(|m| m.version), Ok(version));
        let id = "i".repeat(64);
        assert_eq!(manifest(&with("id", Some(&format!("\"{id}\"")))).map(|m| m.id), Ok(id));
    }

    #[test]
    fn an_add_on_for_a_newer_interface_or_another_system_is_not_supported_and_the_reason_is_said() {
        let on = |json: &str, os: Os| {
            let parsed = manifest(json);
            assert!(parsed.is_ok(), "{parsed:?}");
            supported(&parsed.unwrap(), os)
        };
        assert_eq!(on(full(), Os::Windows), Ok(()));
        assert_eq!(on(full(), Os::Linux), Ok(()));
        assert_eq!(on(&with("api", Some("2")), Os::Linux), Err(Unsupported::ApiTooNew));
        assert_eq!(on(&with("platforms", Some(r#"["windows"]"#)), Os::Linux), Err(Unsupported::PlatformNotListed));
        // A system the add-ons do not run on (yet).
        assert_eq!(on(full(), Os::Other), Err(Unsupported::PlatformNotListed));
        // The interface is checked before the system.
        assert_eq!(on(&with("api", Some("9")), Os::Other), Err(Unsupported::ApiTooNew));
    }
}
