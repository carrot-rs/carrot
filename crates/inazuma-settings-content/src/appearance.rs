use inazuma_settings_macros::{MergeFrom, with_fallible_options};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::terminal::{CursorShapeContent, TerminalBlink};
use crate::theme::SymbolMapEntry;
use crate::{FontFamilyName, FontSize};

/// Content for the `[appearance]` section in settings.toml.
///
/// In Carrot, the terminal font IS the main font. These settings control
/// the primary visual appearance of the entire application.
#[with_fallible_options]
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct AppearanceSettingsContent {
    /// The main font family used for terminal output and UI.
    ///
    /// Default: "DankMono Nerd Font Mono"
    pub font_family: Option<FontFamilyName>,

    /// The main font size in pixels.
    ///
    /// Default: 15
    pub font_size: Option<FontSize>,

    /// Line height as a multiplier of font size.
    ///
    /// Default: 1.6
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub line_height: Option<f32>,

    /// Default cursor shape for the terminal.
    /// Can be "bar", "block", "underline", or "hollow".
    ///
    /// Default: bar
    pub cursor_style: Option<CursorShapeContent>,

    /// Sets the cursor blinking behavior.
    ///
    /// Default: terminal_controlled
    pub cursor_blink: Option<TerminalBlink>,

    /// The minimum APCA perceptual contrast between foreground and background colors.
    ///
    /// APCA (Accessible Perceptual Contrast Algorithm) is more accurate than WCAG 2.x,
    /// especially for dark mode. Values range from 0 to 106.
    ///
    /// - 0: No contrast adjustment
    /// - 45: Minimum for large fluent text (36px+)
    /// - 60: Minimum for other content text
    /// - 75: Minimum for body text
    /// - 90: Preferred for body text
    ///
    /// Default: 45
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub minimum_contrast: Option<f32>,

    /// Window colorspace for the rendering layer.
    /// Controls how colors are interpreted on wide-gamut (P3) displays.
    ///
    /// - `srgb` (default): Explicit sRGB tagging prevents oversaturation on P3 displays.
    /// - `display_p3`: Enable the wider P3 gamut for richer colors.
    /// - `native`: Use the display's native colorspace without explicit tagging.
    ///
    /// Default: srgb
    pub window_colorspace: Option<AppearanceColorspace>,

    /// Global window opacity (1-100). Applied as a multiplier on every
    /// surface background (title bar, panels, status bar, terminal, cards,
    /// etc.) so the theme background image shines through proportionally.
    /// Mirrors Settings > Appearance > Window > Opacity in the spirit of
    /// classic terminal emulators.
    ///
    /// - 100 = fully opaque, image hidden behind solid bg
    /// - 80 = surfaces at 80% opacity, image bleeds through 20%
    /// - 60 = heavy glass look
    ///
    /// Default: 85
    pub window_opacity: Option<u32>,

    /// Symbol maps: map Unicode ranges to specific font families.
    /// Useful for Nerd Font icons, Powerline glyphs, etc.
    ///
    /// Example in settings.toml:
    /// ```toml
    /// [[appearance.symbol_map]]
    /// start = "E0B0"
    /// end = "E0D7"
    /// font_family = "Symbols Nerd Font Mono"
    /// ```
    pub symbol_map: Option<Vec<SymbolMapEntry>>,
}

/// Window colorspace setting.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceColorspace {
    /// Explicit sRGB tagging — prevents oversaturation on P3 displays.
    #[default]
    Srgb,
    /// Enable the wider Display P3 gamut for richer colors.
    DisplayP3,
    /// Use the display's native colorspace without explicit tagging.
    Native,
}

// `SymbolMapEntry` and `ResolvedSymbolMap` live in `theme.rs` now —
// they belong to the mono font role, not to the Appearance surface.
// Already re-exported via the wildcard in `settings_content.rs`.
