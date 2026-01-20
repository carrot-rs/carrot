use inazuma::{AnyElement, Pixels};
use inazuma_component_registry::{Component, ComponentScope};
use smallvec::SmallVec;

use crate::prelude::*;

/// Glass UI container for dock panels (Vertical Tabs, Project Panel, Outline,
/// Git, Agent, etc.). Renders the panel as a floating card on top of the
/// workspace background image overlay.
///
/// The panel:
/// - has rounded corners
/// - sits with a small margin off the window edges
/// - uses `panel.background` from the theme (opaque — the background image
///   is a separate overlay, see CLAUDE.md "Design System: Glass UI Pattern")
/// - accepts an optional header child that renders above the content
///
/// # Example
///
/// ```ignore
/// FloatingPanel::new()
///     .header(search_bar)
///     .child(tab_list)
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct FloatingPanel {
    header: Option<AnyElement>,
    radius: Pixels,
    margin: Pixels,
    gap: Pixels,
    children: SmallVec<[AnyElement; 2]>,
}

impl FloatingPanel {
    pub fn new() -> Self {
        Self {
            header: None,
            radius: px(8.),
            margin: px(8.),
            gap: px(6.),
            children: SmallVec::new(),
        }
    }

    /// Optional header (e.g. search bar + controls) rendered above children.
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// Override the corner radius (default 8px).
    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    /// Override the margin from the window edges (default 8px).
    pub fn margin(mut self, margin: Pixels) -> Self {
        self.margin = margin;
        self
    }

    /// Override the vertical gap between header and content (default 6px).
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }
}

impl Default for FloatingPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for FloatingPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Component for FloatingPanel {
    fn scope() -> ComponentScope {
        ComponentScope::Layout
    }

    fn description() -> Option<&'static str> {
        Some(
            "Glass UI container for dock panels (Vertical Tabs, Project Panel, \
             Outline, Git, Agent, etc.). Renders as a floating card with rounded \
             corners, opaque bg, and small margin from the window edges. The \
             theme background image overlay layers above this panel.",
        )
    }
}

impl RenderOnce for FloatingPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();

        // Outer wrapper provides symmetric spacing from the dock edges via
        // padding (margin + size_full overflows on the right side in inazuma).
        // Inner wrapper carries the rounded surface, bg and content.
        v_flex().size_full().p(self.margin).child(
            v_flex()
                .size_full()
                .rounded(self.radius)
                .bg(colors.panel.background)
                .overflow_hidden()
                .when_some(self.header, |parent, header| parent.child(header))
                .child(
                    v_flex()
                        .size_full()
                        .flex_1()
                        .gap(self.gap)
                        .overflow_hidden()
                        .children(self.children),
                ),
        )
    }
}
