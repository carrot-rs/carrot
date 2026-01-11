//! Unresolved terminal colors.
//!
//! Cells carry color as a tagged reference — `Default` (theme foreground /
//! background), `Named(NamedColor)` (ANSI 16 + semantic slots), `Indexed(u8)`
//! (xterm 256-color palette), or `Rgb(r,g,b)` (24-bit truecolor). The
//! concrete Oklch values these map to live in the render layer's
//! `TerminalPalette`, which is built from the active theme. Theme changes
//! thus re-colour every existing scrollback cell on the next frame without
//! touching stored data.
//!
//! The enum is small and `Copy` so it slots into `CellStyle` cheaply.

/// ANSI named color slot. Mirrors the standard 16-color palette (8 base +
/// 8 bright) plus the semantic slots a terminal emulator tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    /// Dim variants collapse to the same palette slot as bright when the
    /// renderer doesn't have a dedicated dim entry; tracked explicitly so
    /// palettes that *do* dim gracefully can differentiate.
    DimBlack,
    DimRed,
    DimGreen,
    DimYellow,
    DimBlue,
    DimMagenta,
    DimCyan,
    DimWhite,
    /// Default foreground. Resolves to the active theme's text color.
    Foreground,
    BrightForeground,
    DimForeground,
    /// Default background. Resolves to the active theme's background color.
    Background,
    /// Cursor color slot. Resolves to the theme's cursor / accent color.
    Cursor,
}

/// An unresolved terminal cell color. Resolution against the active
/// palette happens at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// Theme default — foreground for `fg`, background for `bg`. The
    /// renderer picks the right slot from context.
    #[default]
    Default,
    /// ANSI named color.
    Named(NamedColor),
    /// xterm 256-color palette index.
    Indexed(u8),
    /// 24-bit truecolor.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Equivalent of `NamedColor::Foreground` — the palette's default fg slot.
    pub const DEFAULT_FG: Self = Self::Named(NamedColor::Foreground);
    /// Equivalent of `NamedColor::Background` — the palette's default bg slot.
    pub const DEFAULT_BG: Self = Self::Named(NamedColor::Background);
}

/// Convert an 8-bit sRGB triple to Oklch `(l, c, h, 1.0)`. Pure math, no
/// palette lookup; used by the renderer once it has decided that an
/// incoming `Color::Rgb` needs to reach the GPU.
pub fn rgb_to_oklch(r: u8, g: u8, b: u8) -> [f32; 4] {
    let r = srgb_to_linear(r as f32 / 255.0);
    let g = srgb_to_linear(g as f32 / 255.0);
    let b = srgb_to_linear(b as f32 / 255.0);

    // Linear sRGB → LMS (cone response).
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    // LMS → Oklab.
    let l_ok = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let b_ok = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;

    // Oklab → Oklch.
    let chroma = (a * a + b_ok * b_ok).sqrt();
    let hue = b_ok.atan2(a).to_degrees();
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };

    [l_ok, chroma, hue, 1.0]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_default_variant() {
        assert_eq!(Color::default(), Color::Default);
    }

    #[test]
    fn rgb_pure_red_approximates_known_hue() {
        let out = rgb_to_oklch(255, 0, 0);
        // Known: pure red Oklch hue is around 29°, chroma ≈ 0.26.
        assert!(out[2] > 20.0 && out[2] < 35.0);
        assert!(out[1] > 0.2 && out[1] < 0.3);
        assert_eq!(out[3], 1.0);
    }
}
