//! Colors and glyphs.
//!
//! Every non-ASCII glyph here is either a plain text symbol or an emoji whose
//! default presentation is emoji (two columns everywhere). Text-default emoji
//! (`⚙ 🖥 🗄 ▶ ⚠`) are deliberately absent: their width differs between
//! ratatui and many terminals, which is what produced the border "gaps".

use crate::config::{ColorOverrides, ThemeName};
use anyhow::{Result, anyhow};
use ratatui::style::{Color, Modifier, Style};
use std::str::FromStr;

/// Resolved colors for each UI role.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub accent: Color,
    pub text: Color,
    pub dim: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub border: Color,
    pub stars: Color,
    pub language: Color,
    pub description: Color,
    pub warning: Color,
    pub error: Color,
    /// Monochrome: rely on modifiers instead of color.
    pub mono: bool,
}

impl Theme {
    pub fn preset(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self {
                accent: Color::Cyan,
                text: Color::White,
                dim: Color::Gray,
                selected_fg: Color::LightYellow,
                selected_bg: Color::Rgb(45, 50, 70),
                border: Color::DarkGray,
                stars: Color::Yellow,
                language: Color::Green,
                description: Color::Gray,
                warning: Color::LightYellow,
                error: Color::LightRed,
                mono: false,
            },
            ThemeName::Light => Self {
                accent: Color::Blue,
                text: Color::Black,
                dim: Color::DarkGray,
                selected_fg: Color::Black,
                selected_bg: Color::Rgb(200, 215, 240),
                border: Color::Gray,
                stars: Color::Rgb(160, 110, 0),
                language: Color::Rgb(0, 120, 40),
                description: Color::DarkGray,
                warning: Color::Rgb(160, 90, 0),
                error: Color::Red,
                mono: false,
            },
            ThemeName::HighContrast => Self {
                accent: Color::LightCyan,
                text: Color::White,
                dim: Color::White,
                selected_fg: Color::Black,
                selected_bg: Color::LightYellow,
                border: Color::White,
                stars: Color::LightYellow,
                language: Color::LightGreen,
                description: Color::White,
                warning: Color::LightYellow,
                error: Color::LightRed,
                mono: false,
            },
            ThemeName::Mono => Self {
                accent: Color::Reset,
                text: Color::Reset,
                dim: Color::Reset,
                selected_fg: Color::Reset,
                selected_bg: Color::Reset,
                border: Color::Reset,
                stars: Color::Reset,
                language: Color::Reset,
                description: Color::Reset,
                warning: Color::Reset,
                error: Color::Reset,
                mono: true,
            },
        }
    }

    /// Apply user overrides (ignored for the mono theme).
    pub fn with_overrides(mut self, o: &ColorOverrides) -> Result<Self> {
        if self.mono {
            return Ok(self);
        }
        let slots: [(&Option<String>, &mut Color); 11] = [
            (&o.accent, &mut self.accent),
            (&o.text, &mut self.text),
            (&o.dim, &mut self.dim),
            (&o.selected_fg, &mut self.selected_fg),
            (&o.selected_bg, &mut self.selected_bg),
            (&o.border, &mut self.border),
            (&o.stars, &mut self.stars),
            (&o.language, &mut self.language),
            (&o.description, &mut self.description),
            (&o.warning, &mut self.warning),
            (&o.error, &mut self.error),
        ];
        for (value, slot) in slots {
            if let Some(v) = value {
                *slot = Color::from_str(v).map_err(|_| anyhow!("unknown color {v:?}"))?;
            }
        }
        Ok(self)
    }

    pub fn fg(&self, c: Color) -> Style {
        Style::default().fg(c)
    }

    /// Highlight for the selected row; dimmer when its pane is unfocused.
    pub fn selection(&self, focused: bool) -> Style {
        if self.mono {
            let s = Style::default().add_modifier(Modifier::REVERSED);
            return if focused {
                s.add_modifier(Modifier::BOLD)
            } else {
                s
            };
        }
        if focused {
            Style::default()
                .fg(self.selected_fg)
                .bg(self.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(self.selected_bg)
        }
    }

    pub fn border_style(&self, focused: bool) -> Style {
        if self.mono {
            return if focused {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
        }
        Style::default().fg(if focused { self.accent } else { self.border })
    }
}

/// Glyph set: Unicode (default) or plain ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub ascii: bool,
    pub logo: &'static str,
    pub star: &'static str,
    pub pointer: &'static str,
    pub ellipsis: &'static str,
    pub sep: &'static str,
    pub archived: &'static str,
    pub fork: &'static str,
    pub error: &'static str,
    pub spinner: &'static [&'static str],
    /// Icon of the "<user>'s repos" view.
    pub own: &'static str,
}

impl Glyphs {
    pub const UNICODE: Self = Self {
        ascii: false,
        logo: "✦",
        // v0.1's star; an emoji-presentation codepoint, two columns everywhere.
        star: "⭐",
        pointer: "▸ ",
        ellipsis: "…",
        sep: "·",
        archived: "archived",
        fork: "fork",
        error: "✗",
        spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        own: "👤",
    };

    pub const ASCII: Self = Self {
        ascii: true,
        logo: "*",
        star: "*",
        pointer: "> ",
        ellipsis: "...",
        sep: "-",
        archived: "archived",
        fork: "fork",
        error: "!",
        spinner: &["|", "/", "-", "\\"],
        own: "me",
    };

    pub fn new(ascii: bool) -> Self {
        if ascii { Self::ASCII } else { Self::UNICODE }
    }

    /// Icon for a category. Unicode icons are all emoji-presentation (always
    /// two columns); ASCII icons are two-letter tags.
    pub fn category_icon(&self, cat: &str) -> &'static str {
        if self.ascii {
            return match cat {
                "All" => "**",
                "Rust" => "rs",
                "Python" => "py",
                "AI / ML" => "ai",
                "Security" => "sc",
                "Systems / OS" => "os",
                "Games / Emulation" => "gm",
                "Hardware / FPGA" => "hw",
                "TUI / GUI" => "ui",
                "DevTools" => "dt",
                "Networking" => "nw",
                "Databases" => "db",
                "Web / Frontend" => "wb",
                _ => "--",
            };
        }
        match cat {
            "All" => "🌟",
            "Rust" => "🦀",
            "Python" => "🐍",
            "AI / ML" => "🤖",
            "Security" => "🔐",
            "Systems / OS" => "🐧",
            "Games / Emulation" => "🎮",
            "Hardware / FPGA" => "🔌",
            "TUI / GUI" => "💻",
            "DevTools" => "🔧",
            "Networking" => "🌐",
            "Databases" => "💾",
            "Web / Frontend" => "🌍",
            _ => "📦",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categories::CATEGORIES;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn unicode_icons_are_single_codepoint_wide_emoji() {
        let g = Glyphs::UNICODE;
        assert_eq!(g.own.chars().count(), 1);
        assert_eq!(g.own.width(), 2);
        assert_eq!(crate::text::sanitize(g.own), g.own);
        for cat in CATEGORIES.iter().chain(std::iter::once(&"All")) {
            let icon = g.category_icon(cat);
            assert_eq!(
                icon.chars().count(),
                1,
                "{cat}: {icon:?} must be one codepoint"
            );
            assert_eq!(icon.width(), 2, "{cat}: {icon:?} must be two columns");
            assert_eq!(
                crate::text::sanitize(icon),
                icon,
                "{cat}: icon must survive sanitize"
            );
        }
    }

    #[test]
    fn ascii_glyphs_are_ascii() {
        let g = Glyphs::ASCII;
        for s in [g.logo, g.star, g.pointer, g.ellipsis, g.sep, g.error, g.own] {
            assert!(s.is_ascii(), "{s:?}");
        }
        for cat in CATEGORIES {
            assert!(g.category_icon(cat).is_ascii());
        }
        assert!(g.spinner.iter().all(|s| s.is_ascii()));
    }

    #[test]
    fn unicode_glyphs_have_unambiguous_widths() {
        let g = Glyphs::UNICODE;
        for s in [g.logo, g.ellipsis, g.sep, g.error] {
            assert_eq!(s.width(), 1, "{s:?}");
        }
        // The star is a single emoji-presentation codepoint: always 2 columns.
        assert_eq!(g.star.chars().count(), 1);
        assert_eq!(g.star.width(), 2);
        assert_eq!(crate::text::sanitize(g.star), g.star);
    }

    #[test]
    fn overrides_apply_and_reject_garbage() {
        let o = ColorOverrides {
            accent: Some("magenta".into()),
            selected_bg: Some("#102030".into()),
            ..Default::default()
        };
        let t = Theme::preset(ThemeName::Dark).with_overrides(&o).unwrap();
        assert_eq!(t.accent, Color::Magenta);
        assert_eq!(t.selected_bg, Color::Rgb(0x10, 0x20, 0x30));

        let bad = ColorOverrides {
            text: Some("not-a-color".into()),
            ..Default::default()
        };
        assert!(Theme::preset(ThemeName::Dark).with_overrides(&bad).is_err());
    }
}
