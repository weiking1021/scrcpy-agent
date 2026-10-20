//! The themes that can be chosen, and the one that is: what the settings window lists and what
//! every window puts on top of the program's stylesheet.

use super::registry::{ThemeError, ThemeRegistry};
use super::{
    AssetPath, Theme, ThemeAsset, ThemeBase, ThemeId, ThemeInfo, ThemeProblem, ThemeSource, ThemeUrl, load_user_themes,
};
use crate::core::context::Ctx;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Every theme there is, and the folders of the user's themes that are not used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeList {
    pub themes: Vec<ThemeInfo>,
    pub problems: Vec<ThemeProblem>,
}

/// What a window puts on its page: the stylesheet of the chosen theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppliedTheme {
    pub id: ThemeId,
    pub base: ThemeBase,
    pub css: String,
}

pub struct ThemeService {
    ctx: Arc<Ctx>,
    registry: Mutex<ThemeRegistry>,
}

impl ThemeService {
    pub(crate) fn new(ctx: Arc<Ctx>) -> Arc<Self> {
        Arc::new(ThemeService { ctx, registry: Mutex::new(ThemeRegistry::new()) })
    }

    /// The themes of the program, of the user (read from the folder now, so a theme that was
    /// just put there is in the list) and of the add-ons.
    pub fn list(&self) -> ThemeList {
        let registry = self.registry.lock().unwrap();
        let loaded = load_user_themes(&self.ctx.store.theme_files(), &registry.builtin_ids());
        ThemeList {
            themes: registry.all(&loaded.themes).iter().map(Theme::info).collect(),
            problems: loaded.problems,
        }
    }

    /// The theme the settings choose; the program's own look if it is not there any more.
    pub fn applied(&self) -> AppliedTheme {
        let registry = self.registry.lock().unwrap();
        let loaded = load_user_themes(&self.ctx.store.theme_files(), &registry.builtin_ids());
        let theme = registry.resolve(&loaded.themes, &self.ctx.settings().theme);
        AppliedTheme { id: theme.id, base: theme.base, css: theme.css }
    }

    /// Registers a theme of an add-on and returns its id.
    pub fn register_theme(
        &self,
        addon: &str,
        id: &str,
        name: &str,
        base: ThemeBase,
        css: &str,
    ) -> Result<ThemeId, ThemeError> {
        self.registry.lock().unwrap().register_addon(addon, id, name, base, css)
    }

    /// A file of a theme, as a window asks for it. The stylesheet of any theme; the other files
    /// (pictures, fonts) only of a theme of the user, from its folder.
    pub fn asset(&self, id: &ThemeId, path: &AssetPath) -> Option<ThemeAsset> {
        let registry = self.registry.lock().unwrap();
        let loaded = load_user_themes(&self.ctx.store.theme_files(), &registry.builtin_ids());
        // The theme itself, not the look that stands in for a theme that is not there.
        let theme = registry.all(&loaded.themes).into_iter().find(|theme| theme.id == *id)?;
        let bytes = if path.is_stylesheet() {
            theme.css.into_bytes()
        } else if theme.source == ThemeSource::User {
            self.ctx.store.theme_asset(id.as_str(), path)?
        } else {
            return None;
        };
        Some(ThemeAsset { content_type: path.kind().content_type(), bytes })
    }

    /// The file an address (its path, see `ThemeUrl`) asks for; `None` for an address that is not one,
    /// or for a file that is not there or is not given out.
    pub fn serve(&self, path: &str) -> Option<ThemeAsset> {
        let url = ThemeUrl::parse(path)?;
        self.asset(&url.id, &url.path)
    }

    /// Takes back the themes of an add-on.
    pub fn remove_addon(&self, addon: &str) {
        self.registry.lock().unwrap().remove_addon(addon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::CoreOptions;
    use crate::core::model::Settings;
    use crate::core::store::Store;
    use crate::core::theme::{ManifestError, ThemeProblemKind};
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-themes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn start(name: &str) -> (Arc<ThemeService>, Arc<Ctx>, PathBuf) {
        let root = scratch(name);
        let ctx = Ctx::new(Store::open(root.clone()), Arc::new(|_event| {}), CoreOptions::default());
        (ThemeService::new(ctx.clone()), ctx, root)
    }

    fn put_theme(ctx: &Ctx, folder: &str, manifest: Option<&str>, css: Option<&str>) {
        let dir = ctx.store.ensure_themes_dir().unwrap().join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(manifest) = manifest {
            std::fs::write(dir.join("theme.json"), manifest).unwrap();
        }
        if let Some(css) = css {
            std::fs::write(dir.join("theme.css"), css).unwrap();
        }
    }

    fn choose(ctx: &Ctx, theme: &str) {
        let settings = Settings { theme: ThemeId::parse(theme).unwrap(), ..ctx.settings() };
        ctx.use_settings(settings);
    }

    #[test]
    fn the_list_has_the_themes_of_the_program_the_user_and_the_add_ons_and_what_is_wrong_with_a_folder() {
        let (service, ctx, root) = start("list");
        put_theme(&ctx, "night", Some(r#"{"name":"Night","base":"dark"}"#), Some("a {}"));
        put_theme(&ctx, "broken", Some("nope"), Some(""));
        service.register_theme("pack", "day", "Day", ThemeBase::Light, "b {}").unwrap();

        let list = service.list();
        let described: Vec<(&str, &str, ThemeSource)> =
            list.themes.iter().map(|t| (t.id.as_str(), t.name.as_str(), t.source)).collect();
        assert_eq!(
            described,
            [
                ("default", "Default", ThemeSource::BuiltIn),
                ("high-contrast", "High contrast", ThemeSource::BuiltIn),
                ("compact", "Compact", ThemeSource::BuiltIn),
                ("terminal", "Terminal", ThemeSource::BuiltIn),
                ("paper", "Paper", ThemeSource::BuiltIn),
                ("brutalist", "Brutalist", ThemeSource::BuiltIn),
                ("classic", "Classic", ThemeSource::BuiltIn),
                ("night", "Night", ThemeSource::User),
                ("pack/day", "Day", ThemeSource::Addon),
            ]
        );
        assert_eq!(
            list.problems,
            [ThemeProblem { folder: "broken".into(), kind: ThemeProblemKind::BadManifest(ManifestError::NotJson) }]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn asset(path: &str) -> AssetPath {
        AssetPath::parse(path).unwrap()
    }

    fn id(text: &str) -> ThemeId {
        ThemeId::parse(text).unwrap()
    }

    #[test]
    fn the_stylesheet_of_any_theme_is_given_out_as_it_is_and_nothing_of_a_theme_that_is_not_there() {
        let (service, ctx, root) = start("asset-css");
        put_theme(&ctx, "night", Some(r#"{"name":"Night"}"#), Some("user css"));
        service.register_theme("pack", "day", "Day", ThemeBase::Light, "addon css").unwrap();
        let css = |text: &str| Some(ThemeAsset { content_type: "text/css; charset=utf-8", bytes: text.as_bytes().to_vec() });
        assert_eq!(service.asset(&id("night"), &asset("theme.css")), css("user css"));
        assert_eq!(service.asset(&id("pack/day"), &asset("theme.css")), css("addon css"));
        assert_eq!(service.asset(&id("default"), &asset("theme.css")), css(""));
        let compact = service.asset(&id("compact"), &asset("theme.css")).unwrap();
        assert!(!compact.bytes.is_empty());
        // Not there: the look of the program is not given out in its place.
        assert_eq!(service.asset(&id("nobody"), &asset("theme.css")), None);
        assert_eq!(service.asset(&id("pack/night"), &asset("theme.css")), None);
        service.remove_addon("pack");
        assert_eq!(service.asset(&id("pack/day"), &asset("theme.css")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_other_files_of_a_theme_are_given_out_only_for_a_theme_of_the_user_and_with_their_kind() {
        let (service, ctx, root) = start("asset-files");
        put_theme(&ctx, "night", Some(r#"{"name":"Night"}"#), Some(""));
        std::fs::write(ctx.store.themes_dir().join("night").join("bg.svg"), "<svg/>").unwrap();
        std::fs::write(ctx.store.themes_dir().join("night").join("theme.css.png"), [1u8]).unwrap();
        service.register_theme("pack", "day", "Day", ThemeBase::Light, "").unwrap();
        assert_eq!(
            service.asset(&id("night"), &asset("bg.svg")),
            Some(ThemeAsset { content_type: "image/svg+xml", bytes: b"<svg/>".to_vec() })
        );
        assert_eq!(service.asset(&id("night"), &asset("missing.png")), None);
        // A folder that is not a theme (no manifest) gives nothing, though its file is there.
        put_theme(&ctx, "broken", None, Some(""));
        std::fs::write(ctx.store.themes_dir().join("broken").join("bg.png"), [1u8]).unwrap();
        assert_eq!(service.asset(&id("broken"), &asset("bg.png")), None);
        // The themes of the program and of add-ons have no folder.
        assert_eq!(service.asset(&id("compact"), &asset("bg.png")), None);
        assert_eq!(service.asset(&id("pack/day"), &asset("bg.png")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_address_gives_the_file_it_names_and_nothing_for_an_address_that_is_not_one_or_names_no_file() {
        let (service, ctx, root) = start("serve");
        put_theme(&ctx, "night", Some(r#"{"name":"Night"}"#), Some("user css"));
        std::fs::write(ctx.store.themes_dir().join("night").join("bg.svg"), "<svg/>").unwrap();
        service.register_theme("pack", "day", "Day", ThemeBase::Light, "addon css").unwrap();
        let file = |content_type, text: &str| Some(ThemeAsset { content_type, bytes: text.as_bytes().to_vec() });
        assert_eq!(service.serve("/night/theme.css"), file("text/css; charset=utf-8", "user css"));
        assert_eq!(service.serve("/night/bg.svg"), file("image/svg+xml", "<svg/>"));
        assert_eq!(service.serve("/pack~day/theme.css"), file("text/css; charset=utf-8", "addon css"));
        assert_eq!(service.serve("/default/theme.css"), file("text/css; charset=utf-8", ""));
        for none in ["", "/", "/night", "/nobody/theme.css", "/night/missing.png", "/night/../x.png", "/pack~day/bg.png", "/pack/day/theme.css"] {
            assert_eq!(service.serve(none), None, "{none:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_applied_theme_is_the_chosen_one_and_the_programs_own_look_when_it_is_not_there() {
        let (service, ctx, root) = start("applied");
        let own = AppliedTheme { id: ThemeId::parse("default").unwrap(), base: ThemeBase::Auto, css: String::new() };
        assert_eq!(service.applied(), own);

        put_theme(&ctx, "night", Some(r#"{"name":"Night","base":"dark"}"#), Some("a {}"));
        choose(&ctx, "night");
        assert_eq!(
            service.applied(),
            AppliedTheme { id: ThemeId::parse("night").unwrap(), base: ThemeBase::Dark, css: "a {}".into() }
        );

        service.register_theme("pack", "day", "Day", ThemeBase::Light, "b {}").unwrap();
        choose(&ctx, "pack/day");
        assert_eq!(
            service.applied(),
            AppliedTheme { id: ThemeId::parse("pack/day").unwrap(), base: ThemeBase::Light, css: "b {}".into() }
        );

        // The add-on goes: its theme is not there, and the look is the program's own.
        service.remove_addon("pack");
        assert_eq!(service.applied(), own);
        // A theme of the user that was taken away.
        choose(&ctx, "night");
        std::fs::remove_dir_all(ctx.store.themes_dir().join("night")).unwrap();
        assert_eq!(service.applied(), own);
        let _ = std::fs::remove_dir_all(&root);
    }
}
