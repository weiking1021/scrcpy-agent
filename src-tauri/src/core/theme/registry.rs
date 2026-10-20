//! The themes the program itself has, and the ones add-ons registered.

use super::{DEFAULT_THEME, MAX_CSS_BYTES, Theme, ThemeBase, ThemeId, ThemeSource, clean_name};

/// Why a theme could not be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    /// The add-on's name, or the theme's name, is not one.
    #[error("not a usable id")]
    BadId,
    /// The name to show is empty, too long, or has a control character.
    #[error("not a usable name")]
    BadName,
    #[error("the stylesheet is too large")]
    CssTooLarge,
    #[error("the add-on already has a theme with this id")]
    Taken,
}

/// The themes of the program, in the order they are listed: its own look first.
pub fn builtin_themes() -> Vec<Theme> {
    let built_in = |id: &str, name: &str, base, css: &str| Theme {
        id: ThemeId::parse(id).expect("the id of a built-in theme"),
        name: name.to_string(),
        base,
        css: css.to_string(),
        source: ThemeSource::BuiltIn,
    };
    vec![
        // The program's own stylesheet is the look: this one adds nothing to it.
        built_in(DEFAULT_THEME, "Default", ThemeBase::Auto, ""),
        built_in("high-contrast", "High contrast", ThemeBase::Dark, include_str!("builtin/high-contrast.css")),
        built_in("compact", "Compact", ThemeBase::Auto, include_str!("builtin/compact.css")),
        built_in("terminal", "Terminal", ThemeBase::Dark, include_str!("builtin/terminal.css")),
        built_in("paper", "Paper", ThemeBase::Light, include_str!("builtin/paper.css")),
        built_in("brutalist", "Brutalist", ThemeBase::Light, include_str!("builtin/brutalist.css")),
        built_in("classic", "Classic", ThemeBase::Light, include_str!("builtin/classic.css")),
    ]
}

/// The themes of the program and of the add-ons.
pub struct ThemeRegistry {
    builtin: Vec<Theme>,
    addons: Vec<Theme>,
}

impl ThemeRegistry {
    pub fn new() -> Self {
        ThemeRegistry { builtin: builtin_themes(), addons: Vec::new() }
    }

    /// Registers the theme `name` of an add-on and returns its id.
    pub fn register_addon(
        &mut self,
        addon: &str,
        id: &str,
        name: &str,
        base: ThemeBase,
        css: &str,
    ) -> Result<ThemeId, ThemeError> {
        let id = ThemeId::addon(addon, id).ok_or(ThemeError::BadId)?;
        let name = clean_name(name).ok_or(ThemeError::BadName)?;
        if css.len() > MAX_CSS_BYTES {
            return Err(ThemeError::CssTooLarge);
        }
        if self.addons.iter().any(|theme| theme.id == id) {
            return Err(ThemeError::Taken);
        }
        self.addons.push(Theme { id: id.clone(), name, base, css: css.to_string(), source: ThemeSource::Addon });
        Ok(id)
    }

    /// Takes back the themes of an add-on.
    pub fn remove_addon(&mut self, addon: &str) {
        self.addons.retain(|theme| theme.id.addon_of() != Some(addon));
    }

    /// The ids of the program's own themes.
    pub fn builtin_ids(&self) -> Vec<ThemeId> {
        self.builtin.iter().map(|theme| theme.id.clone()).collect()
    }

    /// Every theme: the program's, then those of the user (as they are given), then those of
    /// the add-ons in the order they registered.
    pub fn all(&self, user: &[Theme]) -> Vec<Theme> {
        self.builtin.iter().chain(user).chain(&self.addons).cloned().collect()
    }

    /// The theme with this id; the program's own look if there is none (it was taken away, or
    /// it is of an add-on that is gone).
    pub fn resolve(&self, user: &[Theme], wanted: &ThemeId) -> Theme {
        let all = self.all(user);
        let found = all.iter().find(|theme| theme.id == *wanted);
        found.unwrap_or(&self.builtin[0]).clone()
    }
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> ThemeId {
        ThemeId::parse(text).unwrap()
    }

    fn user(text: &str, css: &str) -> Theme {
        Theme { id: id(text), name: text.into(), base: ThemeBase::Auto, css: css.into(), source: ThemeSource::User }
    }

    fn ids(themes: &[Theme]) -> Vec<&str> {
        themes.iter().map(|t| t.id.as_str()).collect()
    }

    /// The ids of the program's own themes, in the order they are listed.
    const BUILTIN: [&str; 7] = ["default", "high-contrast", "compact", "terminal", "paper", "brutalist", "classic"];

    /// The program's own themes and then these.
    fn builtin_and<'a>(more: &[&'a str]) -> Vec<&'a str> {
        BUILTIN.iter().copied().chain(more.iter().copied()).collect()
    }

    #[test]
    fn the_program_has_its_own_look_and_more_and_the_own_look_adds_nothing_to_the_stylesheet() {
        let themes = builtin_themes();
        assert_eq!(ids(&themes), BUILTIN);
        assert_eq!(themes[0].id.as_str(), DEFAULT_THEME);
        assert!(themes.iter().all(|t| t.source == ThemeSource::BuiltIn));
        assert_eq!((themes[0].base, themes[0].css.as_str()), (ThemeBase::Auto, ""));
        // Every other one is a stylesheet of its own, and says what it is made for.
        assert!(themes[1..].iter().all(|t| !t.css.trim().is_empty()));
        let made_for: Vec<ThemeBase> = themes.iter().map(|t| t.base).collect();
        assert_eq!(
            made_for,
            [
                ThemeBase::Auto,
                ThemeBase::Dark,
                ThemeBase::Auto,
                ThemeBase::Dark,
                ThemeBase::Light,
                ThemeBase::Light,
                ThemeBase::Light
            ]
        );
        let builtin_ids: Vec<ThemeId> = BUILTIN.iter().map(|text| id(text)).collect();
        assert_eq!(ThemeRegistry::new().builtin_ids(), builtin_ids);
    }

    #[test]
    fn an_add_on_registers_a_theme_that_is_listed_after_the_others_and_is_gone_with_the_add_on() {
        let mut registry = ThemeRegistry::new();
        let night = registry.register_addon("night-pack", "night", "Night", ThemeBase::Dark, "a { color: red; }");
        assert_eq!(night, Ok(id("night-pack/night")));
        let day = registry.register_addon("night-pack", "day", "Day", ThemeBase::Light, "");
        assert_eq!(day, Ok(id("night-pack/day")));
        registry.register_addon("other", "night", "Night too", ThemeBase::Dark, "").unwrap();

        let all = registry.all(&[user("mine", "")]);
        assert_eq!(ids(&all), builtin_and(&["mine", "night-pack/night", "night-pack/day", "other/night"]));
        let theme = all.iter().find(|t| t.id.as_str() == "night-pack/night").unwrap();
        assert_eq!(
            (theme.name.as_str(), theme.base, theme.css.as_str(), theme.source),
            ("Night", ThemeBase::Dark, "a { color: red; }", ThemeSource::Addon)
        );

        registry.remove_addon("night-pack");
        assert_eq!(ids(&registry.all(&[])), builtin_and(&["other/night"]));
        registry.remove_addon("nobody");
        assert_eq!(ids(&registry.all(&[])), builtin_and(&["other/night"]));
    }

    #[test]
    fn a_theme_that_cannot_be_registered_is_refused_for_a_reason_and_leaves_the_others_alone() {
        let mut registry = ThemeRegistry::new();
        registry.register_addon("pack", "night", "Night", ThemeBase::Dark, "").unwrap();
        let big = "x".repeat(MAX_CSS_BYTES + 1);
        let biggest = "x".repeat(MAX_CSS_BYTES);
        let long_name = "n".repeat(61);
        assert_eq!(registry.register_addon("pack", "night", "Again", ThemeBase::Dark, ""), Err(ThemeError::Taken));
        assert_eq!(registry.register_addon("bad name", "x", "X", ThemeBase::Dark, ""), Err(ThemeError::BadId));
        assert_eq!(registry.register_addon("", "x", "X", ThemeBase::Dark, ""), Err(ThemeError::BadId));
        assert_eq!(registry.register_addon("pack", "a/b", "X", ThemeBase::Dark, ""), Err(ThemeError::BadId));
        assert_eq!(registry.register_addon("pack", "x", "", ThemeBase::Dark, ""), Err(ThemeError::BadName));
        assert_eq!(registry.register_addon("pack", "x", "a\nb", ThemeBase::Dark, ""), Err(ThemeError::BadName));
        assert_eq!(registry.register_addon("pack", "x", &long_name, ThemeBase::Dark, ""), Err(ThemeError::BadName));
        assert_eq!(registry.register_addon("pack", "x", "X", ThemeBase::Dark, &big), Err(ThemeError::CssTooLarge));
        assert!(registry.register_addon("pack", "x", "X", ThemeBase::Dark, &biggest).is_ok());
        assert_eq!(ids(&registry.all(&[])), builtin_and(&["pack/night", "pack/x"]));
    }

    #[test]
    fn the_chosen_theme_is_the_one_with_its_id_wherever_it_is_from_and_otherwise_the_programs_own_look() {
        let mut registry = ThemeRegistry::new();
        registry.register_addon("pack", "night", "Night", ThemeBase::Dark, "from the add-on").unwrap();
        let mine = [user("mine", "from the user")];
        assert_eq!(registry.resolve(&mine, &id("mine")).css, "from the user");
        assert_eq!(registry.resolve(&mine, &id("pack/night")).css, "from the add-on");
        assert_eq!(registry.resolve(&mine, &id("compact")).id, id("compact"));
        // Taken away, never there, or of an add-on that is gone.
        for gone in ["theirs", "pack/day", "gone-pack/night"] {
            let theme = registry.resolve(&mine, &id(gone));
            assert_eq!((theme.id.as_str(), theme.css.as_str(), theme.source), ("default", "", ThemeSource::BuiltIn), "{gone}");
        }
        registry.remove_addon("pack");
        assert_eq!(registry.resolve(&mine, &id("pack/night")).id, id("default"));
    }
}
