use inazuma::Pixels;
use inazuma_settings_framework::{DockSide, RegisterSetting, Settings};

pub use inazuma_settings_framework::{
    AdditionalMetadata, PaneTitleSource, VerticalTabsDensity, VerticalTabsViewMode,
};

#[derive(Debug, Clone, Copy, PartialEq, RegisterSetting)]
pub struct VerticalTabsSettings {
    /// Whether to show the panel button in the status bar.
    pub button: bool,
    /// Default panel width in pixels.
    pub default_width: Pixels,
    /// Which dock side the panel lives in.
    pub dock: DockSide,
    /// Display density: compact (single-line) or expanded (multi-line with metadata).
    pub density: VerticalTabsDensity,
    /// View mode: show all panes or only the focused pane per session.
    pub view_mode: VerticalTabsViewMode,
    /// What to show as the main pane title.
    pub pane_title: PaneTitleSource,
    /// Additional metadata shown as subtitle in compact mode.
    pub additional_metadata: AdditionalMetadata,
    /// Whether to show PR link badges in expanded mode.
    pub show_pr_link: bool,
    /// Whether to show diff stats (+N -M) in expanded mode.
    pub show_diff_stats: bool,
    /// Whether the hover detail sidecar is shown.
    pub show_details_on_hover: bool,
}

impl Settings for VerticalTabsSettings {
    fn from_settings(content: &inazuma_settings_framework::SettingsContent) -> Self {
        let panel = content.vertical_tabs.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            default_width: panel.default_width.map(inazuma::px).unwrap(),
            dock: panel.dock.unwrap(),
            density: panel.density.unwrap(),
            view_mode: panel.view_mode.unwrap(),
            pane_title: panel.pane_title.unwrap(),
            additional_metadata: panel.additional_metadata.unwrap(),
            show_pr_link: panel.show_pr_link.unwrap(),
            show_diff_stats: panel.show_diff_stats.unwrap(),
            show_details_on_hover: panel.show_details_on_hover.unwrap(),
        }
    }
}
