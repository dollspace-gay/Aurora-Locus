//! A viewer's own look of the pages: light, dark or the device's choice, a
//! colour scheme and a font. Kept per operator (and per account in the
//! portal), and written on `<html>` as three classes, so the page draws in
//! its colours and font from the first paint with no script and no inline
//! style. Pages before sign-in use the device's mode, the default scheme
//! and the system font, so they load no font file.

/// Light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Follow the device.
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Same as the device",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// The accent of links, buttons and the current page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    Sky,
    Violet,
    Teal,
    Graphite,
}

impl Scheme {
    pub const ALL: [Self; 4] = [Self::Sky, Self::Violet, Self::Teal, Self::Graphite];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sky => "sky",
            Self::Violet => "violet",
            Self::Teal => "teal",
            Self::Graphite => "graphite",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Sky => "Sky (the default)",
            Self::Violet => "Violet",
            Self::Teal => "Teal",
            Self::Graphite => "Graphite",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// One face for text and one for code, bundled, or the device's own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Font {
    /// Atkinson Hyperlegible, with JetBrains Mono for code.
    #[default]
    Hyperlegible,
    System,
    /// IBM Plex Sans, with IBM Plex Mono for code.
    Plex,
}

impl Font {
    pub const ALL: [Self; 3] = [Self::Hyperlegible, Self::System, Self::Plex];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hyperlegible => "hyperlegible",
            Self::System => "system",
            Self::Plex => "plex",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hyperlegible => "Atkinson Hyperlegible (the default)",
            Self::System => "Your device's fonts, for text and code",
            Self::Plex => "IBM Plex",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// One viewer's choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Theme {
    pub mode: Mode,
    pub scheme: Scheme,
    pub font: Font,
}

impl Theme {
    /// From stored names; an unknown one is the default.
    #[must_use]
    pub fn from_stored(mode: &str, scheme: &str, font: &str) -> Self {
        Self {
            mode: Mode::parse(mode).unwrap_or_default(),
            scheme: Scheme::parse(scheme).unwrap_or_default(),
            font: Font::parse(font).unwrap_or_default(),
        }
    }

    /// The classes on `<html>`.
    #[must_use]
    pub fn class(self) -> String {
        format!(
            "mode-{} scheme-{} font-{}",
            self.mode.as_str(),
            self.scheme.as_str(),
            self.font.as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::CSS;

    #[test]
    fn names_round_trip_and_unknown_is_the_default() {
        assert_eq!(
            Theme::from_stored("dark", "teal", "plex").class(),
            "mode-dark scheme-teal font-plex"
        );
        assert_eq!(
            Theme::from_stored("purple", "nope", "comic"),
            Theme::default()
        );
        assert_eq!(
            Theme::default().class(),
            "mode-system scheme-sky font-hyperlegible"
        );
    }

    /// Every choice the pages offer is styled; each font after `:root`,
    /// which it overrides at the same specificity.
    #[test]
    fn every_choice_is_styled() {
        for s in Scheme::ALL {
            assert!(CSS.contains(&format!(".scheme-{}", s.as_str())), "{s:?}");
        }
        assert!(CSS.contains(".mode-dark") && CSS.contains(".mode-light"));
        let last_root = CSS.rfind("\n:root").expect(":root");
        for f in Font::ALL {
            let rule = format!("\n.font-{} {{", f.as_str());
            let at = CSS.find(&rule).unwrap_or_else(|| panic!("{rule:?}"));
            assert!(at > last_root, "{rule:?} comes before :root");
        }
    }
}
