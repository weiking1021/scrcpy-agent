//! The looks of the program. A theme is a stylesheet that is put on top of the program's own:
//! it can change the variables the program's stylesheet is made of (colours, fonts, the
//! roundness of corners), and it may add rules of its own.
//!
//! Where themes come from: the program (`registry::builtin_themes`), the folder of the themes
//! of the user (`theme.json` and `theme.css` in a folder named by the id) and add-ons
//! (`service::ThemeService::register_theme`). A theme is chosen by its id (the setting `theme`).

pub mod registry;
pub mod service;

use crate::core::store::ThemeFiles;
use serde::{Deserialize, Serialize};

/// The id of the theme that is the program's own look.
pub const DEFAULT_THEME: &str = "default";

/// The most a stylesheet of a theme may be, in bytes.
pub const MAX_CSS_BYTES: usize = 256 * 1024;

/// The most characters the name of a theme may have.
pub const MAX_NAME_CHARS: usize = 60;

/// The most a file of a theme (a picture, a font) may be, in bytes.
pub const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;

/// The name of the stylesheet of a theme, as a window asks for it.
pub const STYLESHEET: &str = "theme.css";

/// What kind of file a file of a theme is, by its extension: only these are given out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Css,
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Woff2,
    Woff,
    Ttf,
    Otf,
}

impl AssetKind {
    /// The kind for an extension (without the dot, in any case); `None` for one that is not given out.
    pub fn of_extension(extension: &str) -> Option<AssetKind> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "css" => AssetKind::Css,
            "png" => AssetKind::Png,
            "jpg" | "jpeg" => AssetKind::Jpeg,
            "gif" => AssetKind::Gif,
            "webp" => AssetKind::Webp,
            "svg" => AssetKind::Svg,
            "woff2" => AssetKind::Woff2,
            "woff" => AssetKind::Woff,
            "ttf" => AssetKind::Ttf,
            "otf" => AssetKind::Otf,
            _ => return None,
        })
    }

    pub fn content_type(self) -> &'static str {
        match self {
            AssetKind::Css => "text/css; charset=utf-8",
            AssetKind::Png => "image/png",
            AssetKind::Jpeg => "image/jpeg",
            AssetKind::Gif => "image/gif",
            AssetKind::Webp => "image/webp",
            AssetKind::Svg => "image/svg+xml",
            AssetKind::Woff2 => "font/woff2",
            AssetKind::Woff => "font/woff",
            AssetKind::Ttf => "font/ttf",
            AssetKind::Otf => "font/otf",
        }
    }
}

/// The most bytes a path of a file of a theme may have.
const MAX_PATH_BYTES: usize = 200;

/// A path of a file inside the folder of a theme, in the form a window asks for it: names
/// with `/` between them, no way out of the folder, and an extension that is given out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPath {
    text: String,
    kind: AssetKind,
}

impl AssetPath {
    /// `None` for anything that is not such a path: an empty one, one that starts or ends with
    /// `/`, has an empty name, `.` or `..`, a `\`, a `:`, a character that is not an ASCII letter,
    /// digit or one of `_-.`, more than 200 bytes, or an extension that is not an `AssetKind`.
    pub fn parse(text: &str) -> Option<AssetPath> {
        if text.is_empty() || text.len() > MAX_PATH_BYTES {
            return None;
        }
        let name_ok = |name: &str| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        };
        if !text.split('/').all(name_ok) {
            return None;
        }
        // The file is the last name: it has a name of its own before its extension.
        let file = text.rsplit('/').next()?;
        let (stem, extension) = file.rsplit_once('.')?;
        if stem.is_empty() {
            return None;
        }
        Some(AssetPath { text: text.to_string(), kind: AssetKind::of_extension(extension)? })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn kind(&self) -> AssetKind {
        self.kind
    }

    /// Whether it is the stylesheet of the theme.
    pub fn is_stylesheet(&self) -> bool {
        self.text == STYLESHEET
    }
}

/// What a window asks for by address: the file `path` of the theme `id`. The address (its path) is
/// `/<theme>/<path>`, where `<theme>` is the id with a `~` where it has a `/` (a theme of an add-on:
/// `night-pack~night`), so that the id and the path cannot be mixed up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeUrl {
    pub id: ThemeId,
    pub path: AssetPath,
}

impl ThemeUrl {
    /// The path of an address, without the query and what comes after it; `None` for one that is not
    /// exactly of this form.
    pub fn parse(path: &str) -> Option<ThemeUrl> {
        let (segment, file) = path.strip_prefix('/')?.split_once('/')?;
        // At most one `~`: the add-on and the name.
        let parts: Vec<&str> = segment.split('~').collect();
        let id = match parts.as_slice() {
            [name] => ThemeId::parse(name)?,
            [addon, name] => ThemeId::addon(addon, name)?,
            _ => return None,
        };
        Some(ThemeUrl { id, path: AssetPath::parse(file)? })
    }

    /// The first part of the address of the files of a theme.
    pub fn segment(id: &ThemeId) -> String {
        id.as_str().replace('/', "~")
    }

    /// The path of the address of this file.
    pub fn to_path(&self) -> String {
        format!("/{}/{}", Self::segment(&self.id), self.path.as_str())
    }
}

/// A file of a theme, ready to be given to a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeAsset {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The id of a theme: a name (1 to 64 ASCII letters, digits, `_` or `-`), or the name of an
/// add-on and a name, with a `/` between: `night-pack/night`. The ones of the program and of
/// the user have no `/`, so an add-on can never take the place of another theme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ThemeId(String);

impl ThemeId {
    /// `None` for text that is not an id.
    pub fn parse(text: &str) -> Option<ThemeId> {
        let parts: Vec<&str> = text.split('/').collect();
        (matches!(parts.len(), 1 | 2) && parts.iter().all(|part| valid_name(part))).then(|| ThemeId(text.to_string()))
    }

    /// The id of the theme `name` of the add-on `addon`.
    pub fn addon(addon: &str, name: &str) -> Option<ThemeId> {
        (valid_name(addon) && valid_name(name)).then(|| ThemeId(format!("{addon}/{name}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The add-on this theme belongs to, if it is one of an add-on's.
    pub fn addon_of(&self) -> Option<&str> {
        self.0.split_once('/').map(|(addon, _)| addon)
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        ThemeId(DEFAULT_THEME.to_string())
    }
}

impl TryFrom<String> for ThemeId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        ThemeId::parse(&text).ok_or_else(|| format!("{text:?} is not the id of a theme"))
    }
}

impl From<ThemeId> for String {
    fn from(id: ThemeId) -> String {
        id.0
    }
}

/// What a theme is made for: it decides the colour scheme that the system's own parts
/// (scroll bars, form controls) are drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeBase {
    /// Light or dark as the system is.
    #[default]
    Auto,
    Light,
    Dark,
}

/// Who a theme is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSource {
    BuiltIn,
    User,
    Addon,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub id: ThemeId,
    pub name: String,
    pub base: ThemeBase,
    pub css: String,
    pub source: ThemeSource,
}

/// What the settings window shows of a theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeInfo {
    pub id: ThemeId,
    pub name: String,
    pub base: ThemeBase,
    pub source: ThemeSource,
}

impl Theme {
    pub fn info(&self) -> ThemeInfo {
        ThemeInfo { id: self.id.clone(), name: self.name.clone(), base: self.base, source: self.source }
    }
}

/// What is read from `theme.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeManifest {
    pub name: String,
    pub base: ThemeBase,
}

/// Why a `theme.json` is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestError {
    NotJson,
    NotAnObject,
    MissingName,
    /// Not text, empty, longer than `MAX_NAME_CHARS`, or with a control character.
    BadName,
    /// A `base` that is not `auto`, `light` or `dark`.
    BadBase,
}

/// Reads `theme.json`: an object with a `name` and, optionally, a `base`. Other members are
/// left alone (a later version may add some).
pub fn parse_manifest(text: &str) -> Result<ThemeManifest, ManifestError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| ManifestError::NotJson)?;
    let object = value.as_object().ok_or(ManifestError::NotAnObject)?;
    let name = match object.get("name") {
        None => return Err(ManifestError::MissingName),
        Some(serde_json::Value::String(name)) => clean_name(name).ok_or(ManifestError::BadName)?,
        Some(_) => return Err(ManifestError::BadName),
    };
    let base = match object.get("base") {
        None => ThemeBase::Auto,
        Some(base @ serde_json::Value::String(_)) => {
            serde_json::from_value(base.clone()).map_err(|_| ManifestError::BadBase)?
        }
        Some(_) => return Err(ManifestError::BadBase),
    };
    Ok(ThemeManifest { name, base })
}

/// 1 to 64 ASCII letters, digits, `_` or `-`: the same rule as the names of add-ons.
fn valid_name(name: &str) -> bool {
    crate::core::addons::buttons::valid_name(name)
}

/// The name to show for a theme, without the spaces around it; `None` if it is empty, has more
/// than `MAX_NAME_CHARS` characters or has a control character.
pub(crate) fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARS && !name.chars().any(char::is_control))
        .then(|| name.to_string())
}

/// Why a folder of the themes of the user is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "error", rename_all = "snake_case")]
pub enum ThemeProblemKind {
    /// The name of the folder is not an id.
    BadFolderName,
    MissingManifest,
    MissingCss,
    BadManifest(ManifestError),
    CssTooLarge,
    /// The id is one of the program's own themes.
    TakenByBuiltIn,
}

/// A folder of the themes of the user that is not used, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeProblem {
    pub folder: String,
    #[serde(flatten)]
    pub kind: ThemeProblemKind,
}

/// The themes of the user, and what was wrong with the folders that are not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedThemes {
    /// By id.
    pub themes: Vec<Theme>,
    /// In the order the folders came in.
    pub problems: Vec<ThemeProblem>,
}

/// The themes in the folders of the user's themes, by id, and the folders that are not themes.
/// `builtin` are the ids that are not free.
pub fn load_user_themes(files: &[ThemeFiles], builtin: &[ThemeId]) -> LoadedThemes {
    let mut loaded = LoadedThemes::default();
    for folder in files {
        match load_one(folder, builtin) {
            Ok(theme) => loaded.themes.push(theme),
            Err(kind) => loaded.problems.push(ThemeProblem { folder: folder.folder.clone(), kind }),
        }
    }
    loaded.themes.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    loaded
}

fn load_one(files: &ThemeFiles, builtin: &[ThemeId]) -> Result<Theme, ThemeProblemKind> {
    // A folder is one name: an id with a slash is one of an add-on's.
    let id = ThemeId::parse(&files.folder)
        .filter(|id| id.addon_of().is_none())
        .ok_or(ThemeProblemKind::BadFolderName)?;
    if builtin.contains(&id) {
        return Err(ThemeProblemKind::TakenByBuiltIn);
    }
    let manifest = files.manifest.as_deref().ok_or(ThemeProblemKind::MissingManifest)?;
    let css = files.css.as_deref().ok_or(ThemeProblemKind::MissingCss)?;
    let manifest = parse_manifest(manifest).map_err(ThemeProblemKind::BadManifest)?;
    if css.len() > MAX_CSS_BYTES {
        return Err(ThemeProblemKind::CssTooLarge);
    }
    Ok(Theme { id, name: manifest.name, base: manifest.base, css: css.to_string(), source: ThemeSource::User })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_id_is_a_name_or_the_name_of_an_add_on_and_a_name() {
        for good in ["default", "high-contrast", "my_theme2", "A", "night-pack/night", "a-b/c_d"] {
            assert_eq!(ThemeId::parse(good).as_ref().map(ThemeId::as_str), Some(good), "{good:?}");
        }
        let too_long = "a".repeat(65);
        let too_long_add_on = format!("{too_long}/b");
        for bad in [
            "", "a b", "a/b/c", "/x", "x/", "a//b", "../x", "x/..", ".", "ä", "ab\n", "a:b", too_long.as_str(),
            too_long_add_on.as_str(),
        ] {
            assert_eq!(ThemeId::parse(bad), None, "{bad:?}");
        }
        assert!(ThemeId::parse(&"a".repeat(64)).is_some());
    }

    #[test]
    fn a_path_in_a_theme_is_names_with_slashes_that_cannot_leave_the_folder_and_ends_in_a_kind_that_is_given_out() {
        for good in ["bg.png", "images/bg.PNG", "fonts/a-b_c.woff2", "theme.css", "a/b/c/d.svg", "x.v2.jpeg"] {
            assert_eq!(AssetPath::parse(good).as_ref().map(AssetPath::as_str), Some(good), "{good:?}");
        }
        let long = format!("{}.png", "a".repeat(197));
        let longest = format!("{}.png", "a".repeat(196));
        for bad in [
            "", "/a.png", "a.png/", "a//b.png", "../a.png", "a/../b.png", "./a.png", "a/./b.png", "..", ".",
            "a\\b.png", "C:/a.png", "C:a.png", "a.exe", "a", "a.", ".png", "a b.png", "ä.png", "a.png\n",
            "a.php", "a.html", "a.js", long.as_str(),
        ] {
            assert_eq!(AssetPath::parse(bad), None, "{bad:?}");
        }
        assert!(AssetPath::parse(&longest).is_some());
    }

    #[test]
    fn the_kind_of_a_file_is_its_extension_in_any_case_and_has_its_content_type() {
        let kinds = [
            ("css", AssetKind::Css, "text/css; charset=utf-8"),
            ("png", AssetKind::Png, "image/png"),
            ("jpg", AssetKind::Jpeg, "image/jpeg"),
            ("jpeg", AssetKind::Jpeg, "image/jpeg"),
            ("gif", AssetKind::Gif, "image/gif"),
            ("webp", AssetKind::Webp, "image/webp"),
            ("svg", AssetKind::Svg, "image/svg+xml"),
            ("woff2", AssetKind::Woff2, "font/woff2"),
            ("woff", AssetKind::Woff, "font/woff"),
            ("ttf", AssetKind::Ttf, "font/ttf"),
            ("otf", AssetKind::Otf, "font/otf"),
        ];
        for (extension, kind, content_type) in kinds {
            assert_eq!(AssetKind::of_extension(extension), Some(kind), "{extension}");
            assert_eq!(AssetKind::of_extension(&extension.to_uppercase()), Some(kind), "{extension}");
            assert_eq!(kind.content_type(), content_type);
            let path = AssetPath::parse(&format!("a.{extension}")).unwrap();
            assert_eq!(path.kind(), kind);
        }
        for other in ["", "exe", "html", "js", "json", "txt", "pngx", "p"] {
            assert_eq!(AssetKind::of_extension(other), None, "{other:?}");
        }
        assert!(AssetPath::parse("theme.css").unwrap().is_stylesheet());
        assert!(!AssetPath::parse("sub/theme.css").unwrap().is_stylesheet());
        assert!(!AssetPath::parse("theme.png").unwrap().is_stylesheet());
    }

    #[test]
    fn the_address_of_a_file_of_a_theme_is_the_theme_with_a_tilde_for_a_slash_and_then_the_path() {
        let url = |id: &str, path: &str| ThemeUrl { id: ThemeId::parse(id).unwrap(), path: AssetPath::parse(path).unwrap() };
        assert_eq!(ThemeUrl::parse("/default/theme.css"), Some(url("default", "theme.css")));
        assert_eq!(ThemeUrl::parse("/night-pack~night/images/bg.png"), Some(url("night-pack/night", "images/bg.png")));
        assert_eq!(ThemeUrl::parse("/my_theme-2/fonts/a.woff2"), Some(url("my_theme-2", "fonts/a.woff2")));
        // A slash in the first part is the start of the path, so that is a theme without an add-on.
        assert_eq!(ThemeUrl::parse("/night-pack/night/theme.css"), Some(url("night-pack", "night/theme.css")));

        assert_eq!(ThemeUrl::segment(&ThemeId::parse("night").unwrap()), "night");
        assert_eq!(ThemeUrl::segment(&ThemeId::parse("night-pack/night").unwrap()), "night-pack~night");
        for (id, path) in [("default", "theme.css"), ("night-pack/night", "images/bg.png")] {
            let wanted = url(id, path);
            assert_eq!(ThemeUrl::parse(&wanted.to_path()), Some(wanted.clone()));
        }
        assert_eq!(url("night-pack/night", "theme.css").to_path(), "/night-pack~night/theme.css");
    }

    #[test]
    fn an_address_that_is_not_exactly_of_that_form_is_no_theme_file() {
        for bad in [
            "", "/", "/night", "/night/", "//theme.css", "/night//theme.css", "night/theme.css", "/a~b~c/x.png",
            "/night~/x.png", "/~night/x.png", "/night/../x.png", "/../night/x.png", "/night/x.exe", "/Ni ght/x.png",
            "/night/x.png?v=1", "/night/%2e%2e/x.png", "/night\\x.png", "/night/x.png#a", "/night~pack~x/x.png",
        ] {
            assert_eq!(ThemeUrl::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_add_on_of_an_id_is_what_comes_before_the_slash() {
        let id = ThemeId::parse("night-pack/night").unwrap();
        assert_eq!(id.addon_of(), Some("night-pack"));
        assert_eq!(ThemeId::parse("night").unwrap().addon_of(), None);
        assert_eq!(ThemeId::addon("night-pack", "night"), Some(id));
        assert_eq!(ThemeId::addon("bad name", "night"), None);
        assert_eq!(ThemeId::addon("night-pack", "a/b"), None);
        assert_eq!(ThemeId::addon("", "night"), None);
    }

    #[test]
    fn the_default_id_is_default_and_an_id_is_written_as_the_text_it_is() {
        assert_eq!(ThemeId::default().as_str(), DEFAULT_THEME);
        assert_eq!(serde_json::to_string(&ThemeId::parse("night-pack/night").unwrap()).unwrap(), r#""night-pack/night""#);
        assert!(serde_json::from_str::<ThemeId>(r#""../x""#).is_err());
        assert!(serde_json::from_str::<ThemeId>("5").is_err());
    }

    #[test]
    fn a_manifest_has_a_name_and_may_say_what_the_theme_is_made_for() {
        let manifest = |name: &str, base| ThemeManifest { name: name.into(), base };
        assert_eq!(parse_manifest(r#"{"name":"Night","base":"dark"}"#), Ok(manifest("Night", ThemeBase::Dark)));
        assert_eq!(parse_manifest(r#"{"name":"Day","base":"light"}"#), Ok(manifest("Day", ThemeBase::Light)));
        assert_eq!(parse_manifest(r#"{"name":"Any","base":"auto"}"#), Ok(manifest("Any", ThemeBase::Auto)));
        assert_eq!(parse_manifest(r#"{"name":"No base"}"#), Ok(manifest("No base", ThemeBase::Auto)));
        // Members of a later version are left alone, and spaces around the name are not part of it.
        assert_eq!(
            parse_manifest(r#"{"name":"  Spaced ","author":"someone","version":2,"base":"dark"}"#),
            Ok(manifest("Spaced", ThemeBase::Dark))
        );
    }

    #[test]
    fn a_manifest_that_is_not_one_is_refused_for_a_reason_of_its_own() {
        let long = format!(r#"{{"name":"{}"}}"#, "n".repeat(MAX_NAME_CHARS + 1));
        let cases = [
            ("not json", ManifestError::NotJson),
            ("", ManifestError::NotJson),
            ("[]", ManifestError::NotAnObject),
            ("\"Night\"", ManifestError::NotAnObject),
            ("{}", ManifestError::MissingName),
            (r#"{"base":"dark"}"#, ManifestError::MissingName),
            (r#"{"name":5}"#, ManifestError::BadName),
            (r#"{"name":""}"#, ManifestError::BadName),
            (r#"{"name":"   "}"#, ManifestError::BadName),
            (r#"{"name":"a\nb"}"#, ManifestError::BadName),
            (long.as_str(), ManifestError::BadName),
            (r#"{"name":"N","base":"purple"}"#, ManifestError::BadBase),
            (r#"{"name":"N","base":3}"#, ManifestError::BadBase),
            (r#"{"name":"N","base":"Dark"}"#, ManifestError::BadBase),
        ];
        for (text, error) in cases {
            assert_eq!(parse_manifest(text), Err(error), "{text:?}");
        }
        let longest = format!(r#"{{"name":"{}"}}"#, "n".repeat(MAX_NAME_CHARS));
        assert!(parse_manifest(&longest).is_ok());
    }

    fn files(folder: &str, manifest: Option<&str>, css: Option<&str>) -> ThemeFiles {
        ThemeFiles { folder: folder.into(), manifest: manifest.map(str::to_string), css: css.map(str::to_string) }
    }

    #[test]
    fn the_themes_of_the_user_are_the_folders_that_hold_a_manifest_and_a_stylesheet() {
        let night = r#"{"name":"Night","base":"dark"}"#;
        let loaded = load_user_themes(
            &[
                files("night", Some(night), Some("body { color: white; }")),
                files("day", Some(r#"{"name":"Day"}"#), Some("")),
            ],
            &[ThemeId::parse("default").unwrap()],
        );
        assert_eq!(loaded.problems, []);
        // By id, whatever the order the folders came in.
        assert_eq!(
            loaded.themes,
            [
                Theme {
                    id: ThemeId::parse("day").unwrap(),
                    name: "Day".into(),
                    base: ThemeBase::Auto,
                    css: String::new(),
                    source: ThemeSource::User,
                },
                Theme {
                    id: ThemeId::parse("night").unwrap(),
                    name: "Night".into(),
                    base: ThemeBase::Dark,
                    css: "body { color: white; }".into(),
                    source: ThemeSource::User,
                },
            ]
        );
    }

    #[test]
    fn a_folder_that_is_not_a_theme_is_left_out_and_said_with_its_reason() {
        let ok = r#"{"name":"N"}"#;
        let big = "x".repeat(MAX_CSS_BYTES + 1);
        let biggest = "x".repeat(MAX_CSS_BYTES);
        let loaded = load_user_themes(
            &[
                files("Bad Name", Some(ok), Some("")),
                files("a-no-manifest", None, Some("")),
                files("b-no-css", Some(ok), None),
                files("c-not-json", Some("nope"), Some("")),
                files("d-too-big", Some(ok), Some(&big)),
                files("default", Some(ok), Some("")),
                files("e-biggest", Some(ok), Some(&biggest)),
                files("f/g", Some(ok), Some("")),
            ],
            &[ThemeId::parse("default").unwrap()],
        );
        assert_eq!(loaded.themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["e-biggest"]);
        let problem = |folder: &str, kind| ThemeProblem { folder: folder.into(), kind };
        assert_eq!(
            loaded.problems,
            [
                problem("Bad Name", ThemeProblemKind::BadFolderName),
                problem("a-no-manifest", ThemeProblemKind::MissingManifest),
                problem("b-no-css", ThemeProblemKind::MissingCss),
                problem("c-not-json", ThemeProblemKind::BadManifest(ManifestError::NotJson)),
                problem("d-too-big", ThemeProblemKind::CssTooLarge),
                problem("default", ThemeProblemKind::TakenByBuiltIn),
                problem("f/g", ThemeProblemKind::BadFolderName),
            ]
        );
    }
}
