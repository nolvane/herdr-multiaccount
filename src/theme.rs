//! Colours and icons, the same palette as the Herdr Git Panel.

use ratatui::style::Color;

pub const TEXT: Color = Color::Rgb(0xE4, 0xE4, 0xEF);
pub const MUTED: Color = Color::Rgb(0xA6, 0xAB, 0xCF);
pub const FAINT: Color = Color::Rgb(0x7A, 0x7F, 0xA3);
pub const BORDER: Color = Color::Rgb(0x3A, 0x3E, 0x5C);
pub const SELECTED_BG: Color = Color::Rgb(0x30, 0x33, 0x55);
pub const ACCENT: Color = Color::Rgb(0x9D, 0x8C, 0xF7);
pub const GREEN: Color = Color::Rgb(0x3F, 0xD2, 0x8A);
pub const RED: Color = Color::Rgb(0xF0, 0x56, 0x6D);
pub const YELLOW: Color = Color::Rgb(0xE8, 0xB9, 0x4E);
/// The footer bar.
pub const BAR_BG: Color = Color::Rgb(0x25, 0x28, 0x3E);
/// The empty part of a usage bar.
pub const TRACK: Color = Color::Rgb(0x3A, 0x3E, 0x5C);

pub struct Icons {
    pub nerd: bool,
}

impl Icons {
    fn pick(&self, nerd: &'static str, plain: &'static str) -> &'static str {
        if self.nerd {
            nerd
        } else {
            plain
        }
    }

    /// In front of the title.
    pub fn title(&self) -> &'static str {
        self.pick("\u{f415}", "◉")
    }

    /// The active account.
    pub fn active(&self) -> &'static str {
        self.pick("\u{f444}", "●")
    }

    pub fn space(&self) -> &'static str {
        self.pick("\u{f51e}", "▣")
    }

    pub fn fallback(&self) -> &'static str {
        self.pick("\u{f443}", "⇄")
    }

    pub fn warning(&self) -> &'static str {
        self.pick("\u{f421}", "!")
    }

    pub fn refresh(&self) -> &'static str {
        self.pick("\u{f46a}", "↻")
    }

    pub fn key(&self) -> &'static str {
        self.pick("\u{f43d}", "⚿")
    }

    pub fn running(&self) -> &'static str {
        self.pick("\u{f500}", "▶")
    }
}

/// Text colour that reads on a coloured background.
pub fn on(bg: Color) -> Color {
    match bg {
        Color::Rgb(r, g, b) => {
            let l = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
            if l > 150.0 {
                Color::Rgb(0x16, 0x17, 0x22)
            } else {
                Color::Rgb(0xF5, 0xF5, 0xFA)
            }
        }
        _ => TEXT,
    }
}

/// The colour of a usage share: green, yellow from 70 %, red from 90 %.
pub fn level(used: f64) -> Color {
    if used >= 90.0 {
        RED
    } else if used >= 70.0 {
        YELLOW
    } else {
        GREEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_contrast() {
        assert_eq!(level(10.0), GREEN);
        assert_eq!(level(75.0), YELLOW);
        assert_eq!(level(95.0), RED);
        assert_eq!(on(Color::Rgb(255, 255, 255)), Color::Rgb(0x16, 0x17, 0x22));
    }
}
