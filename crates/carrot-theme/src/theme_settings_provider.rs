use inazuma::{App, Font, Global, Pixels};
use inazuma_settings_content::ResolvedSymbolMap;

use crate::UiDensity;

/// Three semantic font roles every Carrot surface picks from. The user
/// only configures two slots in `theme.fonts.{ui,mono}` — the
/// resolver maps the three roles onto those two:
///
/// * `Body` → `theme.fonts.ui`
/// * `Code` → `theme.fonts.mono`
/// * `Terminal` → `theme.fonts.mono`
///
/// Adding a future override slot (e.g. `[theme.fonts.overrides.terminal]`)
/// is non-breaking because every call-site already goes through
/// `theme_settings(cx).font(role, cx)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// Proportional UI text — palette, sidebar, tabs, chrome.
    Body,
    /// Monospace code — editor buffer, REPL output, markdown code.
    Code,
    /// Monospace terminal grid render.
    Terminal,
}

/// Trait for providing theme-related settings (fonts, font sizes, UI density)
/// without coupling to the concrete settings infrastructure.
///
/// A concrete implementation is registered as a global by the `theme_settings` crate.
pub trait ThemeSettingsProvider: Send + Sync + 'static {
    /// Resolved [`Font`] for a given role.
    fn font<'a>(&'a self, role: FontRole, cx: &'a App) -> &'a Font;

    /// Resolved font size for a given role, applying any in-memory
    /// zoom adjustment.
    fn font_size(&self, role: FontRole, cx: &App) -> Pixels;

    /// Returns the resolved line-height multiplier for the given role.
    /// Body uses a standard UI line-height (1.3); Code and Terminal
    /// share the configured `theme.fonts.mono.line_height` value.
    fn line_height(&self, role: FontRole, cx: &App) -> f32;

    /// Returns the resolved Unicode-range overrides for the given role.
    /// Empty slice for [`FontRole::Body`] — proportional fonts can't
    /// host monospace-aligned symbol overrides.
    fn symbol_map_for<'a>(
        &'a self,
        role: FontRole,
        cx: &'a App,
    ) -> &'a [ResolvedSymbolMap];

    // ── Legacy shims ───────────────────────────────────────────────────
    // Kept while ~78 call-sites migrate over. Every one of these will
    // disappear once the rename is done.

    /// Returns the font used for UI elements.
    fn ui_font<'a>(&'a self, cx: &'a App) -> &'a Font;

    /// Returns the font used for buffers and the terminal.
    fn buffer_font<'a>(&'a self, cx: &'a App) -> &'a Font;

    /// Returns the UI font size in pixels.
    fn ui_font_size(&self, cx: &App) -> Pixels;

    /// Returns the buffer font size in pixels.
    fn buffer_font_size(&self, cx: &App) -> Pixels;

    /// Returns the current UI density setting.
    fn ui_density(&self, cx: &App) -> UiDensity;
}

// ── Convenience accessors — preferred entry-point for call-sites ──────
//
// `body_font(cx)` / `code_font(cx)` / `terminal_font(cx)` plus their
// `_size` variants are how every consumer should read the configured
// fonts. They route through the role resolver, so adding a future
// override slot for any specific role is non-breaking.

pub fn body_font<'a>(cx: &'a App) -> &'a Font {
    theme_settings(cx).font(FontRole::Body, cx)
}

pub fn code_font<'a>(cx: &'a App) -> &'a Font {
    theme_settings(cx).font(FontRole::Code, cx)
}

pub fn terminal_font<'a>(cx: &'a App) -> &'a Font {
    theme_settings(cx).font(FontRole::Terminal, cx)
}

pub fn body_font_size(cx: &App) -> Pixels {
    theme_settings(cx).font_size(FontRole::Body, cx)
}

pub fn code_font_size(cx: &App) -> Pixels {
    theme_settings(cx).font_size(FontRole::Code, cx)
}

pub fn terminal_font_size(cx: &App) -> Pixels {
    theme_settings(cx).font_size(FontRole::Terminal, cx)
}

pub fn symbol_map_for<'a>(role: FontRole, cx: &'a App) -> &'a [ResolvedSymbolMap] {
    theme_settings(cx).symbol_map_for(role, cx)
}

struct GlobalThemeSettingsProvider(Box<dyn ThemeSettingsProvider>);

impl Global for GlobalThemeSettingsProvider {}

/// Registers the global [`ThemeSettingsProvider`] implementation.
///
/// This should be called during application initialization by the crate
/// that owns the concrete theme settings (e.g. `theme_settings`).
pub fn set_theme_settings_provider(provider: Box<dyn ThemeSettingsProvider>, cx: &mut App) {
    cx.set_global(GlobalThemeSettingsProvider(provider));
}

/// Returns the global [`ThemeSettingsProvider`].
///
/// Panics if no provider has been registered via [`set_theme_settings_provider`].
pub fn theme_settings(cx: &App) -> &dyn ThemeSettingsProvider {
    &*cx.global::<GlobalThemeSettingsProvider>().0
}
