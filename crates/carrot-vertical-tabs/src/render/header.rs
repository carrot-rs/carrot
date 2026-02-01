//! Control bar rendered at the top of the vertical tabs panel.
//!
//! Three cells in a single row:
//!
//! - **Search input** — live-filter the tab list by title / subtitle
//!   (handled via the `search_query` accessor on the panel).
//! - **Settings popover trigger** — opens the density / view-mode /
//!   metadata popup anchored below the Sliders icon. The trigger's
//!   `selected_style` override keeps the icon in its default colour
//!   while the popover is open (so the toggled state doesn't flash
//!   the accent colour for the open duration).
//! - **New-session button** — dispatches `new_session` on the panel,
//!   which hops through the workspace to spawn a fresh session.

use carrot_theme::ActiveTheme;
use carrot_ui::{
    ButtonCustomVariant, ButtonStyle, Color, IconButton, IconName, IconSize, PopoverMenu, Sizable,
    input::Input, prelude::*,
};
use inazuma::{AnyElement, Context, Corner, IntoElement, ParentElement, Styled, point, px};

use crate::{VerticalTabsPanel, settings_popup};

impl VerticalTabsPanel {
    /// Build the control bar at the top of the panel. Returns an
    /// `AnyElement` so render callers can drop it into a v_flex
    /// without reasoning about the concrete element type.
    pub(crate) fn build_control_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.clone();
        let colors = cx.theme().colors();
        let settings_trigger_selected = ButtonStyle::Custom(
            ButtonCustomVariant::new(cx)
                .color(colors.ghost_element_hover)
                .hover(colors.ghost_element_hover)
                .active(colors.ghost_element_hover),
        );
        h_flex()
            .w_full()
            .px_2()
            .py_1p5()
            .gap_1()
            .items_center()
            .child(
                Input::new(&self.search_input)
                    .prefix(
                        carrot_ui::Icon::new(IconName::MagnifyingGlass)
                            .size(IconSize::XSmall)
                            .color(carrot_ui::Color::Muted),
                    )
                    .appearance(false)
                    .bordered(false)
                    .with_size(carrot_ui::Size::Small),
            )
            .child(
                // Inline settings popup — mirrors the sidebar control
                // bar in the reference. Writes land in the same TOML
                // keys that Settings > Appearance > Tabs edits.
                PopoverMenu::new("vertical-tabs-settings-popup")
                    .trigger(
                        IconButton::new("vertical-tabs-settings", IconName::Sliders)
                            .icon_size(IconSize::Small)
                            .selected_icon_color(Color::Default)
                            .selected_style(settings_trigger_selected)
                            .tooltip(carrot_ui::Tooltip::small("Tab list settings"))
                            .tooltip_placement(inazuma::TooltipPlacement::BelowElement),
                    )
                    // Top-left of popup aligns with the trigger's
                    // bottom so it drops down-right of the sliders
                    // icon, floating beside the panel.
                    .anchor(Corner::TopLeft)
                    .offset(point(px(0.), px(4.)))
                    .menu(move |window, cx| {
                        Some(settings_popup::SettingsPopup::build(ws.clone(), window, cx))
                    }),
            )
            .child(
                IconButton::new("vertical-tabs-new", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .tooltip(carrot_ui::Tooltip::small("New session"))
                    .tooltip_placement(inazuma::TooltipPlacement::BelowElement)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_session(window, cx);
                    })),
            )
            .into_any_element()
    }
}
