//! Drag-and-drop types for reordering session cards.
//!
//! `DraggedVerticalTab` is the payload emitted from a card's `on_drag`
//! handler — it carries the source session index so the drop-target
//! row can dispatch a `Workspace::move_session(from, to)` without
//! re-resolving state.
//!
//! `DraggedVerticalTabView` is the floating preview that follows the
//! cursor while dragging. Rendered as an `Entity<Self>` by
//! `on_drag`'s builder so gpui can position it at the pointer.

use carrot_theme::ActiveTheme;
use carrot_ui::{IconName, h_flex, prelude::*};
use inazuma::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Window};

/// Drag payload for reordering session cards in the vertical tab list.
/// Carries the source index; the drop handler looks up source/target
/// and calls `Workspace::move_session`.
#[derive(Clone)]
pub(crate) struct DraggedVerticalTab {
    pub(crate) index: usize,
    pub(crate) label: SharedString,
}

/// Floating preview rendered under the cursor while dragging a card.
pub(crate) struct DraggedVerticalTabView {
    pub(crate) label: SharedString,
}

impl Render for DraggedVerticalTabView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .px_3()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(inazuma::px(4.))
            .bg(colors.elevated_surface)
            .opacity(0.9)
            .child(
                carrot_ui::Icon::new(IconName::Terminal)
                    .size(carrot_ui::IconSize::Small)
                    .color(carrot_ui::Color::Default),
            )
            .child(
                div()
                    .text_size(inazuma::px(12.))
                    .text_color(colors.text)
                    .child(self.label.clone()),
            )
    }
}
