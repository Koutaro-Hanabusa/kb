//! Colour themes.
//!
//! A theme is a pair of 256-colour indices: `primary` for the things you scan
//! for (paths, notebook names) and `secondary` for supporting detail. The names
//! and values are `nb`'s, read out of its source, so `color_theme` means the
//! same thing in both tools.

/// A theme: the two colour indices it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub primary: u8,
    pub secondary: u8,
}

/// The theme used when nothing is configured — `nb`'s own default colours.
pub const DEFAULT: Theme = Theme { name: "nb", primary: 69, secondary: 8 };

/// Every built-in theme, in the order `settings colors themes` lists them.
///
/// `lavender`, `mage`, `mint`, and `monochrome` are defined in `nb` but absent
/// from its own listing; they are accepted here and left out of the list for the
/// same reason.
pub const THEMES: [Theme; 11] = [
    Theme { name: "blacklight", primary: 39, secondary: 56 },
    Theme { name: "console", primary: 40, secondary: 28 },
    Theme { name: "desert", primary: 179, secondary: 95 },
    Theme { name: "electro", primary: 200, secondary: 62 },
    Theme { name: "forest", primary: 29, secondary: 59 },
    DEFAULT,
    Theme { name: "ocean", primary: 75, secondary: 26 },
    Theme { name: "raspberry", primary: 162, secondary: 90 },
    Theme { name: "smoke", primary: 248, secondary: 241 },
    Theme { name: "unicorn", primary: 183, secondary: 153 },
    Theme { name: "utility", primary: 227, secondary: 8 },
];

/// Themes `nb` understands but does not advertise.
const UNLISTED: [Theme; 4] = [
    Theme { name: "lavender", primary: 183, secondary: 61 },
    Theme { name: "mage", primary: 199, secondary: 55 },
    Theme { name: "mint", primary: 43, secondary: 60 },
    Theme { name: "monochrome", primary: 248, secondary: 241 },
];

impl Theme {
    pub fn by_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        THEMES
            .iter()
            .chain(UNLISTED.iter())
            .copied()
            .find(|theme| theme.name == name)
    }
}

/// Resolve the active theme and any per-colour overrides.
///
/// `color_primary` and `color_secondary` win over the theme, so a theme can be
/// adopted and then adjusted — the same layering `nb` uses.
pub fn resolve(
    theme_name: Option<&str>,
    primary: Option<&str>,
    secondary: Option<&str>,
) -> Theme {
    let mut theme = theme_name.and_then(Theme::by_name).unwrap_or(DEFAULT);
    if let Some(colour) = primary.and_then(parse_colour) {
        theme.primary = colour;
    }
    if let Some(colour) = secondary.and_then(parse_colour) {
        theme.secondary = colour;
    }
    theme
}

fn parse_colour(value: &str) -> Option<u8> {
    value.trim().parse().ok()
}

/// The ANSI escape that selects a 256-colour foreground.
pub fn foreground(colour: u8) -> String {
    format!("\u{1b}[38;5;{colour}m")
}

/// The 256-colour palette, as `settings colors` prints it.
pub fn palette() -> String {
    let mut out = String::new();
    for colour in 0..=255u8 {
        out.push_str(&format!("{}{colour:>4}\u{1b}[0m", foreground(colour)));
        if colour % 16 == 15 {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values read out of `nb`'s source; they have to keep matching for
    /// `color_theme` to mean the same thing in both tools.
    #[test]
    fn themes_match_nb() {
        assert_eq!(Theme::by_name("blacklight").unwrap().primary, 39);
        assert_eq!(Theme::by_name("blacklight").unwrap().secondary, 56);
        assert_eq!(Theme::by_name("console").unwrap().primary, 40);
        assert_eq!(Theme::by_name("desert").unwrap().secondary, 95);
        assert_eq!(Theme::by_name("electro").unwrap().primary, 200);
        assert_eq!(Theme::by_name("forest").unwrap().primary, 29);
        assert_eq!(Theme::by_name("ocean").unwrap().primary, 75);
        assert_eq!(Theme::by_name("raspberry").unwrap().secondary, 90);
        assert_eq!(Theme::by_name("smoke").unwrap().primary, 248);
        assert_eq!(Theme::by_name("unicorn").unwrap().secondary, 153);
        assert_eq!(Theme::by_name("utility").unwrap().secondary, 8);
        // `nb`'s own default, from the fallback below the theme table.
        assert_eq!(DEFAULT.primary, 69);
        assert_eq!(DEFAULT.secondary, 8);
    }

    /// Defined in `nb` but missing from its own `settings colors themes` output.
    #[test]
    fn unlisted_themes_still_resolve() {
        assert_eq!(Theme::by_name("lavender").unwrap().primary, 183);
        assert_eq!(Theme::by_name("mage").unwrap().primary, 199);
        assert_eq!(Theme::by_name("mint").unwrap().primary, 43);
        assert_eq!(Theme::by_name("monochrome").unwrap().primary, 248);
        assert!(!THEMES.iter().any(|theme| theme.name == "lavender"));
    }

    #[test]
    fn names_are_matched_loosely() {
        assert_eq!(Theme::by_name("  OCEAN  ").unwrap().name, "ocean");
        assert!(Theme::by_name("nonexistent").is_none());
    }

    #[test]
    fn an_unknown_theme_falls_back_to_the_default() {
        assert_eq!(resolve(Some("nonsense"), None, None), DEFAULT);
        assert_eq!(resolve(None, None, None), DEFAULT);
    }

    #[test]
    fn explicit_colours_override_the_theme() {
        let theme = resolve(Some("ocean"), Some("196"), None);
        assert_eq!(theme.primary, 196);
        assert_eq!(theme.secondary, 26); // still ocean's

        let theme = resolve(Some("ocean"), None, Some("240"));
        assert_eq!(theme.primary, 75);
        assert_eq!(theme.secondary, 240);
    }

    #[test]
    fn a_colour_that_is_not_a_number_is_ignored() {
        assert_eq!(resolve(Some("ocean"), Some("blue"), None).primary, 75);
        assert_eq!(resolve(Some("ocean"), Some("999"), None).primary, 75);
    }

    #[test]
    fn renders_256_colour_escapes() {
        assert_eq!(foreground(75), "\u{1b}[38;5;75m");
        let palette = palette();
        assert_eq!(palette.lines().count(), 16);
        assert!(palette.contains("\u{1b}[38;5;255m"));
    }
}
