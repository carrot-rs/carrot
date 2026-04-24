use carrot_theme::ActiveTheme;
use carrot_ui::{Color, Icon, IconName, IconSize, h_flex, v_flex};
use carrot_workspace::Workspace;
/// Settings popover for the vertical-tabs panel. Mirrors the reference
/// sidebar settings UI: sectioned popup with segmented controls for
/// View as / Density, radio-style lists for Pane title as / Additional
/// metadata, and bottom toggles for the expanded-mode badges and the
/// hover detail sidecar.
///
/// All mutations go through `update_settings_file`, which rewrites the
/// user's TOML config. The SettingsStore picks up the file change and
/// refreshes the global, so the panel re-renders with the new state
/// without any manual notify. Kept in sync with the equivalent section
/// in `carrot-settings-ui::appearance_page()`: both UIs write the same
/// `content.vertical_tabs.*` fields.
use inazuma::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use inazuma_settings_framework::{
    AdditionalMetadata, PaneTitleSource, Settings, VerticalTabsDensity, VerticalTabsViewMode,
    update_settings_file,
};

use crate::vertical_tabs_settings::VerticalTabsSettings;

pub(crate) struct SettingsPopup {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
}

impl SettingsPopup {
    pub(crate) fn new(
        workspace: WeakEntity<Workspace>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Build the popup as an `AnyView` for use as a PopoverMenu `menu`
    /// callback target. The popup self-manages its focus + dismissal.
    pub(crate) fn build(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(workspace.clone(), window, cx))
    }
}

impl Focusable for SettingsPopup {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for SettingsPopup {}

impl Render for SettingsPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = *VerticalTabsSettings::get_global(cx);
        let colors = cx.theme().colors();

        v_flex()
            .key_context("VerticalTabsSettingsPopup")
            .track_focus(&self.focus_handle)
            // Click anywhere outside the popup → dismiss. Matches how
            // every other popover in the app behaves; without this the
            // user has to re-click the trigger icon to close.
            .on_mouse_down_out(cx.listener(|_, _, _, cx| {
                cx.emit(DismissEvent);
                cx.notify();
            }))
            // Fixed narrow width — the popup must float beside the
            // panel, not fill it. `flex_none` stops parent flex
            // layouts (inside PopoverMenu) from stretching it wider.
            .w(px(220.))
            .flex_none()
            // Popup bg sits at `surface` so the segmented-control track
            // (`element_hover`) and its selected cell (`element_active`)
            // read as progressively LIGHTER layers on top — matching
            // the reference popup hierarchy.
            .bg(colors.surface)
            .rounded_md()
            .shadow_md()
            .py_1p5()
            .gap_0p5()
            // View as — segmented control (pill container with two cells).
            .child(self.section_header("View as", cx))
            .child(self.view_mode_segmented(settings, cx))
            .child(self.divider(cx))
            // Density — segmented control with icons.
            .child(self.section_header("Density", cx))
            .child(self.density_segmented(settings, cx))
            .child(self.divider(cx))
            // Pane title as — radio list.
            .child(self.section_header("Pane title as", cx))
            .child(self.pane_title_rows(settings, cx))
            .child(self.divider(cx))
            .map(|el| {
                // Additional metadata only matters in Compact — Expanded
                // has its own dedicated description + badges rows.
                if matches!(settings.density, VerticalTabsDensity::Compact) {
                    el.child(self.section_header("Additional metadata", cx))
                        .child(self.additional_metadata_rows(settings, cx))
                        .child(self.divider(cx))
                } else {
                    el.child(self.section_header("Show", cx))
                        .child(self.toggle_row(
                            "PR link",
                            settings.show_pr_link,
                            toggle_show_pr_link(self.workspace.clone(), !settings.show_pr_link),
                            cx,
                        ))
                        .child(self.toggle_row(
                            "Diff stats",
                            settings.show_diff_stats,
                            toggle_show_diff_stats(
                                self.workspace.clone(),
                                !settings.show_diff_stats,
                            ),
                            cx,
                        ))
                        .child(self.divider(cx))
                }
            })
            .child(self.toggle_row(
                "Show details on hover",
                settings.show_details_on_hover,
                toggle_show_details_on_hover(
                    self.workspace.clone(),
                    !settings.show_details_on_hover,
                ),
                cx,
            ))
    }
}

impl SettingsPopup {
    fn section_header(&self, label: &'static str, cx: &App) -> impl IntoElement {
        div()
            .px_2p5()
            .pt_1()
            .pb_0p5()
            .text_size(px(10.5))
            .text_color(cx.theme().colors().text_muted)
            .child(label)
    }

    fn divider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Edge-to-edge whisper-thin separator. Solid theme-border tokens
        // (border / border_variant) are too bright against the popup's
        // `surface` bg because the two sit only ~0.04 L apart by theme
        // design — rendering a 1px stripe at that contrast reads as a
        // visible gray line. Alpha'ing the foreground text color keeps
        // the divider theme-adaptive (white in dark themes, near-black
        // in light themes) and naturally subtle against any surface.
        div()
            .my_1()
            .w_full()
            .h(px(1.))
            .bg(cx.theme().colors().text.alpha(0.06))
    }

    fn view_mode_segmented(
        &self,
        settings: VerticalTabsSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws1 = self.workspace.clone();
        let ws2 = self.workspace.clone();
        segmented_two(
            "vertical-tabs-view-mode-seg",
            SegmentedCell::label("Panes"),
            SegmentedCell::label("Tabs"),
            matches!(settings.view_mode, VerticalTabsViewMode::Panes),
            move |_, _, cx| set_view_mode(&ws1, VerticalTabsViewMode::Panes, cx),
            move |_, _, cx| set_view_mode(&ws2, VerticalTabsViewMode::Tabs, cx),
            cx,
        )
    }

    fn density_segmented(
        &self,
        settings: VerticalTabsSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws1 = self.workspace.clone();
        let ws2 = self.workspace.clone();
        segmented_two(
            "vertical-tabs-density-seg",
            SegmentedCell::icon(IconName::TextAlignJustify),
            SegmentedCell::icon(IconName::LayoutGrid),
            matches!(settings.density, VerticalTabsDensity::Compact),
            move |_, _, cx| set_density(&ws1, VerticalTabsDensity::Compact, cx),
            move |_, _, cx| set_density(&ws2, VerticalTabsDensity::Expanded, cx),
            cx,
        )
    }

    fn pane_title_rows(
        &self,
        settings: VerticalTabsSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .child(self.radio_row(
                "pane-title-command",
                "Command / Conversation",
                matches!(settings.pane_title, PaneTitleSource::Command),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_pane_title(&ws, PaneTitleSource::Command, cx)
                },
                cx,
            ))
            .child(self.radio_row(
                "pane-title-directory",
                "Working Directory",
                matches!(settings.pane_title, PaneTitleSource::Directory),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_pane_title(&ws, PaneTitleSource::Directory, cx)
                },
                cx,
            ))
            .child(self.radio_row(
                "pane-title-branch",
                "Branch",
                matches!(settings.pane_title, PaneTitleSource::Branch),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_pane_title(&ws, PaneTitleSource::Branch, cx)
                },
                cx,
            ))
    }

    fn additional_metadata_rows(
        &self,
        settings: VerticalTabsSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .child(self.radio_row(
                "metadata-branch",
                "Branch",
                matches!(settings.additional_metadata, AdditionalMetadata::Branch),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_additional_metadata(&ws, AdditionalMetadata::Branch, cx)
                },
                cx,
            ))
            .child(self.radio_row(
                "metadata-directory",
                "Working Directory",
                matches!(settings.additional_metadata, AdditionalMetadata::Directory),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_additional_metadata(&ws, AdditionalMetadata::Directory, cx)
                },
                cx,
            ))
            .child(self.radio_row(
                "metadata-command",
                "Command / Conversation",
                matches!(settings.additional_metadata, AdditionalMetadata::Command),
                {
                    let ws = self.workspace.clone();
                    move |_, _, cx| set_additional_metadata(&ws, AdditionalMetadata::Command, cx)
                },
                cx,
            ))
    }

    fn radio_row(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let label_color = if selected {
            colors.text
        } else {
            colors.text_muted
        };
        h_flex()
            .id(id)
            .w_full()
            .items_center()
            .gap_1p5()
            .pl_2p5()
            .pr_2p5()
            .py_0p5()
            .cursor_pointer()
            .hover(|el| el.bg(colors.element_hover))
            .child(
                // Reserve a fixed-width slot for the checkmark so label
                // text aligns across selected and unselected rows.
                div().w(px(14.)).flex_none().when(selected, |el| {
                    el.child(Icon::new(IconName::Check).size(IconSize::XSmall))
                }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(label_color)
                    .child(label.into()),
            )
            .on_click(on_click)
    }

    fn toggle_row(
        &self,
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id: SharedString = format!("toggle-{}", label.into()).into();
        let label_text = id.clone();
        let colors = cx.theme().colors();
        h_flex()
            .id(inazuma::ElementId::Name(id.clone()))
            .w_full()
            .items_center()
            .gap_1p5()
            .pl_2p5()
            .pr_2p5()
            .py_0p5()
            .cursor_pointer()
            .hover(|el| el.bg(colors.element_hover))
            .child(div().w(px(14.)).flex_none().when(selected, |el| {
                el.child(Icon::new(IconName::Check).size(IconSize::XSmall))
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    // Strip the "toggle-" prefix we added to make the
                    // element id unique; the visible label is the
                    // original string without it.
                    .child(label_text.trim_start_matches("toggle-").to_string()),
            )
            .on_click(on_click)
    }
}

// ---------------------------------------------------------------------------
// Segmented-control primitive (two cells, left/right)
// ---------------------------------------------------------------------------

enum SegmentedCell {
    Label(&'static str),
    Icon(IconName),
}

impl SegmentedCell {
    fn label(text: &'static str) -> Self {
        Self::Label(text)
    }
    fn icon(name: IconName) -> Self {
        Self::Icon(name)
    }
    fn render(&self, active: bool, colors: &carrot_theme::ThemeColors) -> inazuma::AnyElement {
        let color = if active {
            colors.text
        } else {
            colors.text_muted
        };
        match self {
            Self::Label(text) => div()
                .text_size(px(12.))
                .text_color(color)
                .child(*text)
                .into_any_element(),
            Self::Icon(name) => Icon::new(*name)
                .size(IconSize::Small)
                .color(if active { Color::Default } else { Color::Muted })
                .into_any_element(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn segmented_two(
    base_id: &'static str,
    left: SegmentedCell,
    right: SegmentedCell,
    left_selected: bool,
    on_left: impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static,
    on_right: impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<SettingsPopup>,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    // Three-layer hierarchy (matches the reference popup):
    //   popup bg (colors.surface)     — darkest, on the outside
    //   track   (colors.element_hover) — intermediate, the pill container
    //   selected (colors.element_active) — lightest, the active cell
    // Each layer is ~0.05 L lighter than the one below, so both the
    // track's inset and the selected cell's lift read clearly against
    // the neighbouring layer.
    let track_bg = colors.element_hover;
    let selected_bg = colors.element_active;
    div().px_2p5().py_0p5().child(
        h_flex()
            .w_full()
            .p_0p5()
            .gap_0p5()
            .rounded_md()
            .bg(track_bg)
            .child(
                div()
                    .id(inazuma::ElementId::Name(format!("{base_id}-left").into()))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(left_selected, |el| el.bg(selected_bg))
                    .child(left.render(left_selected, colors))
                    .on_click(on_left),
            )
            .child(
                div()
                    .id(inazuma::ElementId::Name(format!("{base_id}-right").into()))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(!left_selected, |el| el.bg(selected_bg))
                    .child(right.render(!left_selected, colors))
                    .on_click(on_right),
            ),
    )
}

// ---------------------------------------------------------------------------
// Setting-update helpers — one per field. Each resolves the user's
// settings file via `workspace.app_state().fs` and dispatches an update
// through the SettingsStore. The store picks the file change back up
// and notifies every `.get_global::<VerticalTabsSettings>()` consumer.
// ---------------------------------------------------------------------------

fn set_view_mode(workspace: &WeakEntity<Workspace>, value: VerticalTabsViewMode, cx: &mut App) {
    let Some(ws) = workspace.upgrade() else {
        return;
    };
    let fs = ws.read(cx).app_state().fs.clone();
    update_settings_file(fs, cx, move |content, _| {
        content.vertical_tabs.get_or_insert_default().view_mode = Some(value);
    });
}

fn set_density(workspace: &WeakEntity<Workspace>, value: VerticalTabsDensity, cx: &mut App) {
    let Some(ws) = workspace.upgrade() else {
        return;
    };
    let fs = ws.read(cx).app_state().fs.clone();
    update_settings_file(fs, cx, move |content, _| {
        content.vertical_tabs.get_or_insert_default().density = Some(value);
    });
}

fn set_pane_title(workspace: &WeakEntity<Workspace>, value: PaneTitleSource, cx: &mut App) {
    let Some(ws) = workspace.upgrade() else {
        return;
    };
    let fs = ws.read(cx).app_state().fs.clone();
    update_settings_file(fs, cx, move |content, _| {
        content.vertical_tabs.get_or_insert_default().pane_title = Some(value);
    });
}

fn set_additional_metadata(
    workspace: &WeakEntity<Workspace>,
    value: AdditionalMetadata,
    cx: &mut App,
) {
    let Some(ws) = workspace.upgrade() else {
        return;
    };
    let fs = ws.read(cx).app_state().fs.clone();
    update_settings_file(fs, cx, move |content, _| {
        content
            .vertical_tabs
            .get_or_insert_default()
            .additional_metadata = Some(value);
    });
}

fn toggle_show_pr_link(
    workspace: WeakEntity<Workspace>,
    value: bool,
) -> impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let Some(ws) = workspace.upgrade() else {
            return;
        };
        let fs = ws.read(cx).app_state().fs.clone();
        update_settings_file(fs, cx, move |content, _| {
            content.vertical_tabs.get_or_insert_default().show_pr_link = Some(value);
        });
    }
}

fn toggle_show_diff_stats(
    workspace: WeakEntity<Workspace>,
    value: bool,
) -> impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let Some(ws) = workspace.upgrade() else {
            return;
        };
        let fs = ws.read(cx).app_state().fs.clone();
        update_settings_file(fs, cx, move |content, _| {
            content
                .vertical_tabs
                .get_or_insert_default()
                .show_diff_stats = Some(value);
        });
    }
}

fn toggle_show_details_on_hover(
    workspace: WeakEntity<Workspace>,
    value: bool,
) -> impl Fn(&inazuma::ClickEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| {
        let Some(ws) = workspace.upgrade() else {
            return;
        };
        let fs = ws.read(cx).app_state().fs.clone();
        update_settings_file(fs, cx, move |content, _| {
            content
                .vertical_tabs
                .get_or_insert_default()
                .show_details_on_hover = Some(value);
        });
    }
}
