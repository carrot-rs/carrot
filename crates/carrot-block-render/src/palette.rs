//! Terminal palette — resolves unresolved [`Color`] tags into concrete
//! Oklch values the GPU paints with.
//!
//! The carrot-grid layer stores colors as tagged references (default, named,
//! indexed, rgb). The renderer holds a [`TerminalPalette`] that maps each
//! named slot + the default fg/bg to concrete Oklch quadruplets. This lets a
//! theme swap change the look of every existing scrollback cell on the next
//! frame, without touching stored data.
//!
//! [`TerminalPalette::CARROT_DARK`] is the fallback used when no theme is
//! wired up (tests, spikes, headless eval). Production callers construct a
//! `TerminalPalette` via [`TerminalPalette::from_theme`], which reads the
//! `terminal.*` tokens out of the active theme.

use carrot_grid::{Color, NamedColor, rgb_to_oklch};
use carrot_theme::ThemeColors;
use inazuma::Oklch;

/// Four Oklch components `(l, c, h, a)` — same layout the glyph atlas and
/// `inazuma::Oklch` expect.
pub type OklchArr = [f32; 4];

/// The set of concrete terminal colors needed to resolve a [`Color`] tag.
///
/// ANSI-16 slots, dim variants (shared with bright if the theme doesn't
/// provide distinct dim colors), default fg/bg, and the cursor slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalPalette {
    pub black: OklchArr,
    pub red: OklchArr,
    pub green: OklchArr,
    pub yellow: OklchArr,
    pub blue: OklchArr,
    pub magenta: OklchArr,
    pub cyan: OklchArr,
    pub white: OklchArr,
    pub bright_black: OklchArr,
    pub bright_red: OklchArr,
    pub bright_green: OklchArr,
    pub bright_yellow: OklchArr,
    pub bright_blue: OklchArr,
    pub bright_magenta: OklchArr,
    pub bright_cyan: OklchArr,
    pub bright_white: OklchArr,
    pub foreground: OklchArr,
    pub background: OklchArr,
    pub cursor: OklchArr,
}

impl TerminalPalette {
    /// Fallback palette matching the shipping Carrot Dark theme. Used when
    /// no active theme is wired up (tests, headless jobs, early
    /// bootstrap before the theme registry is initialised).
    pub const CARROT_DARK: TerminalPalette = TerminalPalette {
        black: [0.20, 0.00, 0.0, 1.0],
        red: [0.60, 0.22, 25.0, 1.0],
        green: [0.80, 0.22, 142.0, 1.0],
        yellow: [0.85, 0.17, 95.0, 1.0],
        blue: [0.62, 0.21, 240.0, 1.0],
        magenta: [0.65, 0.24, 320.0, 1.0],
        cyan: [0.80, 0.13, 200.0, 1.0],
        white: [0.92, 0.01, 240.0, 1.0],
        bright_black: [0.45, 0.00, 0.0, 1.0],
        bright_red: [0.70, 0.24, 25.0, 1.0],
        bright_green: [0.88, 0.24, 142.0, 1.0],
        bright_yellow: [0.92, 0.18, 95.0, 1.0],
        bright_blue: [0.72, 0.22, 240.0, 1.0],
        bright_magenta: [0.75, 0.25, 320.0, 1.0],
        bright_cyan: [0.88, 0.14, 200.0, 1.0],
        bright_white: [0.98, 0.00, 0.0, 1.0],
        foreground: [0.95, 0.00, 0.0, 1.0],
        background: [0.17, 0.00, 0.0, 1.0],
        cursor: [0.6572, 0.1838, 45.27, 1.0],
    };

    /// Resolve a [`Color`] tag. `default_slot` picks the fallback when
    /// the cell carries [`Color::Default`] — the caller supplies the
    /// `foreground` or `background` depending on the channel it's
    /// resolving (fg cells default to fg, bg cells default to bg).
    pub fn resolve(&self, color: Color, default_slot: DefaultSlot) -> OklchArr {
        match color {
            Color::Default => match default_slot {
                DefaultSlot::Foreground => self.foreground,
                DefaultSlot::Background => self.background,
            },
            Color::Named(n) => self.named(n),
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => rgb_to_oklch(r, g, b),
        }
    }

    /// Same as [`resolve`], but applies the *bold-as-bright* convention:
    /// when `bold` is set and the foreground references a base ANSI-16
    /// color (Red, Green, Blue, …), the bright variant is used instead
    /// (BrightRed, BrightGreen, BrightBlue, …). xterm, alacritty,
    /// gnome-terminal and Warp all default to this — without it, every
    /// `\e[1;31m` byte from `eza` / `git` / colored prompts renders in
    /// the muted base shade and the terminal looks washed out.
    ///
    /// `bg` channels and indexed/RGB colors are left untouched.
    pub fn resolve_styled(&self, color: Color, default_slot: DefaultSlot, bold: bool) -> OklchArr {
        if bold && matches!(default_slot, DefaultSlot::Foreground)
            && let Color::Named(n) = color
        {
            return self.named(promote_to_bright(n));
        }
        self.resolve(color, default_slot)
    }

    /// Same as [`resolve`], applying [`promote_to_bright`] when `bold`
    /// is set. Convenience wrapper for callers that already have a
    /// `Color` value rather than a [`CellStyle`].
    #[doc(hidden)]
    pub fn resolve_named_bold(&self, n: NamedColor) -> OklchArr {
        self.named(promote_to_bright(n))
    }

    fn named(&self, n: NamedColor) -> OklchArr {
        match n {
            NamedColor::Black => self.black,
            NamedColor::Red => self.red,
            NamedColor::Green => self.green,
            NamedColor::Yellow => self.yellow,
            NamedColor::Blue => self.blue,
            NamedColor::Magenta => self.magenta,
            NamedColor::Cyan => self.cyan,
            NamedColor::White => self.white,
            NamedColor::BrightBlack | NamedColor::DimBlack => self.bright_black,
            NamedColor::BrightRed | NamedColor::DimRed => self.bright_red,
            NamedColor::BrightGreen | NamedColor::DimGreen => self.bright_green,
            NamedColor::BrightYellow | NamedColor::DimYellow => self.bright_yellow,
            NamedColor::BrightBlue | NamedColor::DimBlue => self.bright_blue,
            NamedColor::BrightMagenta | NamedColor::DimMagenta => self.bright_magenta,
            NamedColor::BrightCyan | NamedColor::DimCyan => self.bright_cyan,
            NamedColor::BrightWhite | NamedColor::DimWhite => self.bright_white,
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                self.foreground
            }
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
        }
    }

    /// xterm 256-color palette lookup. `0..=15` reuse the ANSI-16 slots,
    /// `16..=231` walk a 6×6×6 RGB cube, `232..=255` a 24-step grayscale
    /// ramp — the standard values every terminal agrees on.
    fn indexed(&self, idx: u8) -> OklchArr {
        if (idx as usize) < 16 {
            return self.ansi16_by_index(idx);
        }
        if idx >= 232 {
            let step = idx - 232;
            let v = 8 + step * 10;
            return rgb_to_oklch(v, v, v);
        }
        let i = idx - 16;
        const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let r = STEPS[(i / 36) as usize];
        let g = STEPS[((i / 6) % 6) as usize];
        let b = STEPS[(i % 6) as usize];
        rgb_to_oklch(r, g, b)
    }

    fn ansi16_by_index(&self, idx: u8) -> OklchArr {
        match idx {
            0 => self.black,
            1 => self.red,
            2 => self.green,
            3 => self.yellow,
            4 => self.blue,
            5 => self.magenta,
            6 => self.cyan,
            7 => self.white,
            8 => self.bright_black,
            9 => self.bright_red,
            10 => self.bright_green,
            11 => self.bright_yellow,
            12 => self.bright_blue,
            13 => self.bright_magenta,
            14 => self.bright_cyan,
            _ => self.bright_white,
        }
    }
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self::CARROT_DARK
    }
}

impl TerminalPalette {
    /// Build a palette from the active theme's `terminal.*` tokens.
    /// Swapping the theme and re-constructing a palette re-colours every
    /// scrollback cell on the next frame without touching the grid data.
    pub fn from_theme(colors: &ThemeColors) -> Self {
        let t = &colors.terminal;
        let a = &t.ansi;
        Self {
            black: oklch_to_arr(a.black),
            red: oklch_to_arr(a.red),
            green: oklch_to_arr(a.green),
            yellow: oklch_to_arr(a.yellow),
            blue: oklch_to_arr(a.blue),
            magenta: oklch_to_arr(a.magenta),
            cyan: oklch_to_arr(a.cyan),
            white: oklch_to_arr(a.white),
            bright_black: oklch_to_arr(a.bright_black),
            bright_red: oklch_to_arr(a.bright_red),
            bright_green: oklch_to_arr(a.bright_green),
            bright_yellow: oklch_to_arr(a.bright_yellow),
            bright_blue: oklch_to_arr(a.bright_blue),
            bright_magenta: oklch_to_arr(a.bright_magenta),
            bright_cyan: oklch_to_arr(a.bright_cyan),
            bright_white: oklch_to_arr(a.bright_white),
            foreground: oklch_to_arr(t.foreground),
            background: oklch_to_arr(t.background),
            cursor: oklch_to_arr(t.accent),
        }
    }
}

fn oklch_to_arr(c: Oklch) -> OklchArr {
    [c.l, c.c, c.h, c.a]
}

/// Which default slot a [`Color::Default`] resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSlot {
    Foreground,
    Background,
}

/// Promote a base ANSI-16 named color to its bright variant. Used for
/// the bold-as-bright convention applied at fg-resolution time. Bright,
/// dim, and special slots (Foreground / Background / Cursor) pass
/// through unchanged.
fn promote_to_bright(n: NamedColor) -> NamedColor {
    match n {
        NamedColor::Black => NamedColor::BrightBlack,
        NamedColor::Red => NamedColor::BrightRed,
        NamedColor::Green => NamedColor::BrightGreen,
        NamedColor::Yellow => NamedColor::BrightYellow,
        NamedColor::Blue => NamedColor::BrightBlue,
        NamedColor::Magenta => NamedColor::BrightMagenta,
        NamedColor::Cyan => NamedColor::BrightCyan,
        NamedColor::White => NamedColor::BrightWhite,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fg_resolves_to_foreground_slot() {
        let p = TerminalPalette::CARROT_DARK;
        assert_eq!(
            p.resolve(Color::Default, DefaultSlot::Foreground),
            p.foreground
        );
    }

    #[test]
    fn default_bg_resolves_to_background_slot() {
        let p = TerminalPalette::CARROT_DARK;
        assert_eq!(
            p.resolve(Color::Default, DefaultSlot::Background),
            p.background
        );
    }

    #[test]
    fn named_cyan_resolves_to_cyan_slot() {
        let p = TerminalPalette::CARROT_DARK;
        assert_eq!(
            p.resolve(Color::Named(NamedColor::Cyan), DefaultSlot::Foreground),
            p.cyan,
        );
    }

    #[test]
    fn indexed_below_16_matches_ansi16() {
        let p = TerminalPalette::CARROT_DARK;
        assert_eq!(
            p.resolve(Color::Indexed(3), DefaultSlot::Foreground),
            p.yellow
        );
    }

    #[test]
    fn indexed_231_is_near_white_cube_corner() {
        let p = TerminalPalette::CARROT_DARK;
        let out = p.resolve(Color::Indexed(231), DefaultSlot::Foreground);
        assert!(out[0] > 0.95 && out[0] <= 1.02);
        assert!(out[3] == 1.0);
    }

    #[test]
    fn indexed_232_is_dark_grey() {
        let p = TerminalPalette::CARROT_DARK;
        let out = p.resolve(Color::Indexed(232), DefaultSlot::Foreground);
        assert!(out[0] < 0.3);
        assert!(out[1] < 0.001);
    }

    #[test]
    fn rgb_tag_passes_through_rgb_to_oklch() {
        let p = TerminalPalette::CARROT_DARK;
        let out = p.resolve(Color::Rgb(255, 0, 0), DefaultSlot::Foreground);
        assert!(out[2] > 20.0 && out[2] < 35.0);
        assert!(out[1] > 0.2 && out[1] < 0.3);
    }

    #[test]
    fn default_palette_is_carrot_dark() {
        assert_eq!(TerminalPalette::default(), TerminalPalette::CARROT_DARK);
    }
}
