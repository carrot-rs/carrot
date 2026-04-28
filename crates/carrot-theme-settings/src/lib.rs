mod schema;
mod settings;
mod theme_settings;

pub use carrot_theme::UiDensity;
pub use schema::{
    HighlightStyleContent, StatusColorsContent, ThemeColorsContent, ThemeContent,
    ThemeFamilyContent, ThemeStyleContent, deserialize_user_theme, status_colors_refinement,
    syntax_overrides, theme_colors_refinement,
};
pub use settings::{
    AgentFontSize, BufferLineHeight, FontFamilyName, IconThemeName, IconThemeSelection,
    ThemeAppearanceMode, ThemeName, ThemeSelection, ThemeSettings, adjust_agent_buffer_font_size,
    adjust_agent_ui_font_size, adjust_body_font_size, adjust_mono_font_size, appearance_to_mode,
    clamp_font_size, default_theme, observe_mono_font_size_adjustment,
    reset_agent_buffer_font_size, reset_agent_ui_font_size, reset_body_font_size,
    reset_mono_font_size, set_mode, set_theme, setup_body_font,
};
pub use theme_settings::{init, load_user_theme, reload_icon_theme, reload_theme};
