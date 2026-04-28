mod accent;
mod colors;
mod default_colors;
mod fallback_themes;
mod font_family_cache;
mod global;
pub mod icon_theme;
mod icon_theme_schema;
mod loader;
mod players;
mod registry;
mod scale;
mod schema;
mod status;
mod syntax;
mod system;
mod theme;
mod theme_settings_provider;
mod ui_density;

pub use accent::AccentColors;
pub use colors::ThemeColors;
pub use colors::{
    BlockColors, ChartColors, ChipColors, EditorColors, MinimapColors, PaneColors, PanelColors,
    ScrollbarColors, SearchColors, StatusBarColors, TabColors, TerminalAnsiColors, TerminalColors,
    TitleBarColors, ToolbarColors, VersionControlColors, VimColors,
};
pub use colors::{
    BlockColorsRefinement, ChartColorsRefinement, ChipColorsRefinement, EditorColorsRefinement,
    MinimapColorsRefinement, PaneColorsRefinement, PanelColorsRefinement,
    ScrollbarColorsRefinement, SearchColorsRefinement, StatusBarColorsRefinement,
    TabColorsRefinement, TerminalAnsiColorsRefinement, TerminalColorsRefinement,
    TitleBarColorsRefinement, ToolbarColorsRefinement, VersionControlColorsRefinement,
    VimColorsRefinement,
};
pub use colors::{ThemeColorField, ThemeColorsRefinement, all_theme_colors};
pub use default_colors::*;
pub use fallback_themes::{
    apply_status_color_defaults, apply_theme_color_defaults, carrot_default_themes,
};
pub use font_family_cache::FontFamilyCache;
pub use global::{ActiveTheme, GlobalTheme};
pub use icon_theme::IconTheme;
pub use icon_theme_schema::{IconThemeFamilyContent, deserialize_icon_theme};
pub use inazuma_settings_content::ResolvedSymbolMap;
pub use loader::{load_theme_from_toml, load_theme_from_toml_with_base_dir, parse_color};
pub use players::{PlayerColor, PlayerColors};
pub use registry::{
    GlobalThemeRegistry, IconThemeNotFoundError, ThemeMeta, ThemeNotFoundError, ThemeRegistry,
};
pub use scale::{ColorScale, ColorScaleStep};
pub use schema::{AppearanceContent, ThemeColorsContent};
pub use status::{
    DiagnosticColors, StatusColors, StatusColorsRefinement, StatusStyle, StatusStyleRefinement,
};
pub use syntax::SyntaxTheme;
pub use system::SystemColors;
pub use theme::{
    Appearance, CLIENT_SIDE_DECORATION_ROUNDING, CLIENT_SIDE_DECORATION_SHADOW, LoadThemes,
    SystemAppearance, Theme, ThemeBackgroundImage, ThemeFamily, ThemeStyles,
};
pub use theme_settings_provider::{
    FontRole, ThemeSettingsProvider, body_font, body_font_size, code_font, code_font_size,
    set_theme_settings_provider, symbol_map_for, terminal_font, terminal_font_size, theme_settings,
};
pub use ui_density::UiDensity;

/// The name of the default dark theme.
pub const DEFAULT_DARK_THEME: &str = "Carrot Dark";

/// The name of the default light theme.
pub const DEFAULT_LIGHT_THEME: &str = "Carrot Light";
