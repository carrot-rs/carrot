use inazuma_settings_macros::{MergeFrom, with_fallible_options};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::terminal::{CursorShapeContent, TerminalBlink};

/// Content for the `[appearance]` section in settings.toml.
///
/// Holds cursor / contrast / colorspace knobs. Fonts live in
/// `theme.fonts.{ui,mono}` and are read via the `body_font(cx)` /
/// `code_font(cx)` / `terminal_font(cx)` convenience accessors.
#[with_fallible_options]
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct AppearanceSettingsContent {
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

