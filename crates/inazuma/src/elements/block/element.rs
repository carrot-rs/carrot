use super::layout::{LayoutResponse, layout_entries};
use super::state::{BlockEntry, RenderEntryFn, ensure_all_measured};
use super::{BlockId, BlockState};
use crate::{
    App, Bounds, ContentMask, DispatchPhase, Element, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Overflow, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, Style, StyleRefinement, Styled, Window, px,
};
use inazuma_sum_tree::SumTree;
use refineable::Refineable as _;

/// A virtualised, terminal-block-aware vertical list element.
pub struct Block {
    pub(super) state: BlockState,
    pub(super) render_entry: Box<RenderEntryFn>,
    pub(super) style: StyleRefinement,
}

/// Construct a new `blocks()` element.
pub fn blocks(
    state: BlockState,
    render_entry: impl FnMut(BlockId, &mut Window, &mut App) -> crate::AnyElement + 'static,
) -> Block {
    Block {
        state,
        render_entry: Box::new(render_entry),
        style: StyleRefinement::default(),
    }
}

/// Frame state the `Block` element caches between `prepaint` and `paint`.
pub struct BlockPrepaintState {
    hitbox: Hitbox,
    layout: LayoutResponse,
}

impl Element for Block {
    type RequestLayoutState = ();
    type PrepaintState = BlockPrepaintState;

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.overflow.y = Overflow::Scroll;
        style.refine(&self.style);
        let layout_id = window.with_text_style(style.text_style().cloned(), |window| {
            window.request_layout(style, None, cx)
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> BlockPrepaintState {
        let state = &mut *self.state.0.borrow_mut();
        state.reset = false;

        let mut style = Style::default();
        style.refine(&self.style);

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        if state
            .last_layout_bounds
            .is_none_or(|last_bounds| last_bounds.size.width != bounds.size.width)
        {
            let reset: SumTree<BlockEntry> = SumTree::from_iter(
                state.entries.iter().map(|entry| BlockEntry {
                    id: entry.id,
                    metadata: entry.metadata.clone(),
                    size: None,
                    focus_handle: entry.focus_handle.clone(),
                    fold: entry.fold.clone(),
                }),
                (),
            );
            state.entries = reset;
            state.measured_all = false;
        }

        ensure_all_measured(state, bounds.size.width, &mut self.render_entry, window, cx);

        let padding = style
            .padding
            .to_pixels(bounds.size.into(), window.rem_size());

        let mut layout = layout_entries(
            state,
            Some(bounds.size.width),
            bounds.size.height,
            &padding,
            &mut self.render_entry,
            window,
            cx,
        );
        state.last_resolved_scroll_top = Some(layout.scroll_top);
        state.last_leading_space = layout.leading_space;

        if bounds.size.height > padding.top + padding.bottom {
            let origin = bounds.origin + Point::new(px(0.0), padding.top);
            let start_y = origin.y + layout.leading_space - layout.scroll_top.offset_in_entry;
            let mut y = start_y;
            for entry in &mut layout.entry_layouts {
                let entry_origin = Point::new(origin.x, y);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    entry.element.prepaint_at(entry_origin, window, cx);
                });
                y += entry.size.height;
            }

            // Pinned footer: anchor at the bottom of the viewport,
            // rendered on top of the scrollable content.
            if let Some(footer) = layout.pinned_footer.as_mut() {
                let footer_y = bounds.origin.y + bounds.size.height - footer.size.height;
                let footer_origin = Point::new(bounds.origin.x, footer_y);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    footer.element.prepaint_at(footer_origin, window, cx);
                });
            }
        }
        state.last_layout_bounds = Some(bounds);
        state.last_padding = Some(padding);
        BlockPrepaintState { hitbox, layout }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for entry in &mut prepaint.layout.entry_layouts {
                entry.element.paint(window, cx);
            }
            // Pinned footer is painted last so it draws on top of any
            // scrollable content that would otherwise overlap.
            if let Some(footer) = prepaint.layout.pinned_footer.as_mut() {
                footer.element.paint(window, cx);
            }
        });

        let block_state = self.state.clone();
        let height = bounds.size.height;
        let scroll_top = prepaint.layout.scroll_top;
        let hitbox_id = prepaint.hitbox.id;
        let mut accumulated_scroll_delta = ScrollDelta::default();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _cx| {
            if phase == DispatchPhase::Bubble && hitbox_id.should_handle_scroll(window) {
                accumulated_scroll_delta = accumulated_scroll_delta.coalesce(event.delta);
                let pixel_delta = accumulated_scroll_delta.pixel_delta(px(20.0));
                block_state
                    .0
                    .borrow_mut()
                    .scroll(&scroll_top, height, pixel_delta);
                // Mutating the inner state alone does not invalidate the
                // window — without this `refresh()` the new scroll
                // position only reaches the GPU on the next unrelated
                // frame trigger (PTY tick, window event, …), so trackpad
                // motion looks chunky. Mirrors Zed's `cx.notify(view)`
                // at the end of `gpui::list::scroll`.
                window.refresh();
            }
        });
    }
}

impl IntoElement for Block {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for Block {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
