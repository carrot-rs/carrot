//! Mouse-event handlers for the block list.
//!
//! The render pass attaches a `mouse_down` / `mouse_move` /
//! `mouse_up` trio to the scroll-area `div`. Each handler is a thin
//! closure that defers to an inherent method on [`BlockListView`] so
//! the logic lives here, not buried in the render closure. The
//! render method in `block_list.rs` stays layout-only.

use std::time::Duration;

use carrot_term::block::{BlockVariant, SelectionKind, Side};
use inazuma::{
    App, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point as GpuiPoint, Window,
};

use crate::block_list::BlockListView;

/// Threshold in pixels: mouse-up within this radius of mouse-down is
/// treated as a click rather than a drag selection.
const CLICK_DRAG_THRESHOLD_PX: f32 = 3.0;

/// Delay before a single click toggles block selection — gives the
/// double-click path time to cancel the pending toggle.
const BLOCK_TOGGLE_DEFER: Duration = Duration::from_millis(300);

impl BlockListView {
    /// Entry point for `on_mouse_down(Left, ...)`. Starts a text
    /// selection on the block under the cursor and cancels any
    /// pending block-toggle on multi-click.
    pub(crate) fn on_mouse_down_left(
        &mut self,
        event: &MouseDownEvent,
        cell_dims: (Pixels, Pixels),
        cx: &mut Context<Self>,
    ) {
        self.mouse_down_pos = Some(event.position);
        self.clear_all_selections();

        if event.click_count >= 2 {
            self.pending_block_toggle = None;
            self.selected_block = None;
        }

        let (cw, ch) = cell_dims;
        if let Some((block_idx, block_id, grid_point, side)) = self.hit_test(event.position, cw, ch)
        {
            let kind = match event.click_count {
                2 => SelectionKind::Semantic,
                3 => SelectionKind::Lines,
                _ => SelectionKind::Simple,
            };
            let picked_side = side_from_hit(side);
            let anchor = cell_id_from_hit(&self.terminal, block_idx, grid_point);

            let handle = self.terminal.clone();
            let mut term = handle.lock();
            if let Some(entry) = term.block_router_mut().entries_mut().get_mut(block_idx)
                && let Some(block) = entry.variant.as_active_mut()
            {
                block.start_selection(anchor, kind, picked_side);
                if matches!(kind, SelectionKind::Semantic | SelectionKind::Lines) {
                    block.update_selection(anchor, picked_side);
                }
            }
            drop(term);
            self.selecting_block = Some(block_id);
        }

        cx.notify();
    }

    /// Entry point for `on_mouse_move(...)`. Extends an in-flight
    /// text selection toward the cursor. No-op when no selection is
    /// active or no button is pressed.
    pub(crate) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cell_dims: (Pixels, Pixels),
        cx: &mut Context<Self>,
    ) {
        if self.selecting_block.is_none() || event.pressed_button.is_none() {
            return;
        }
        let (cw, ch) = cell_dims;
        if let Some((block_idx, _block_id, grid_point, side)) =
            self.hit_test(event.position, cw, ch)
        {
            let picked_side = side_from_hit(side);
            let active = cell_id_from_hit(&self.terminal, block_idx, grid_point);
            let handle = self.terminal.clone();
            let mut term = handle.lock();
            if let Some(entry) = term.block_router_mut().entries_mut().get_mut(block_idx)
                && let Some(block) = entry.variant.as_active_mut()
            {
                block.update_selection(active, picked_side);
            }
            drop(term);
            cx.notify();
        }
    }

    /// Entry point for `on_mouse_up(Left, ...)`. Finishes drag
    /// selection or defers the click-to-toggle action.
    pub(crate) fn on_mouse_up_left(
        &mut self,
        event: &MouseUpEvent,
        cell_dims: (Pixels, Pixels),
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selecting_block.take();

        if let Some(down_pos) = self.mouse_down_pos.take() {
            let dx = f32::from(event.position.x - down_pos.x).abs();
            let dy = f32::from(event.position.y - down_pos.y).abs();
            let is_click = dx < CLICK_DRAG_THRESHOLD_PX && dy < CLICK_DRAG_THRESHOLD_PX;
            if is_click && event.click_count <= 1 {
                self.handle_click(event.position, cell_dims, cx);
            }
        }

        cx.notify();
    }

    fn handle_click(
        &mut self,
        position: GpuiPoint<Pixels>,
        cell_dims: (Pixels, Pixels),
        cx: &mut Context<Self>,
    ) {
        self.clear_all_selections();
        let (cw, ch) = cell_dims;
        let hit = self.hit_test(position, cw, ch);
        let Some((block_idx, _, _, _)) = hit else {
            self.selected_block = None;
            self.pending_block_toggle = None;
            return;
        };

        let is_finished = {
            let handle = self.terminal.clone();
            let term = handle.lock();
            term.block_router()
                .entries()
                .get(block_idx)
                .is_some_and(|e| matches!(e.variant, BlockVariant::Frozen(_)))
        };
        if !is_finished {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLOCK_TOGGLE_DEFER).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view
                        .pending_block_toggle
                        .as_ref()
                        .is_some_and(|(idx, _)| *idx == block_idx)
                    {
                        view.selected_block = if view.selected_block == Some(block_idx) {
                            None
                        } else {
                            Some(block_idx)
                        };
                        view.pending_block_toggle = None;
                        cx.notify();
                    }
                });
            }
        });
        self.pending_block_toggle = Some((block_idx, task));
    }
}

/// Compute cell dimensions for the current font config. Separated so
/// the render closure can build the `(cw, ch)` tuple once and thread
/// it through every mouse handler without recomputing in three
/// different closures.
pub(crate) fn cell_dimensions_from_config(window: &mut Window, cx: &App) -> (Pixels, Pixels) {
    let (font, font_size, line_height_multiplier, _) = BlockListView::read_config(cx);
    BlockListView::cell_dimensions(&font, font_size, line_height_multiplier, window)
}

/// Translate the column-edge `Side` returned by `hit_test` into the
/// block-selection [`Side`] enum consumed by `BlockSelection`. Both
/// enums share a `Left / Right` shape; they live in different
/// modules because `hit_test` still speaks pixel geometry while the
/// selection API is grid-level.
fn side_from_hit(hit: carrot_term::index::Side) -> Side {
    match hit {
        carrot_term::index::Side::Left => Side::Left,
        carrot_term::index::Side::Right => Side::Right,
    }
}

/// Resolve a `(row, col)` hit point inside `block_idx` to a stable
/// v2 [`CellId`]. The legacy grid-point comes from `hit_test` in
/// viewport-relative coordinates (Line negative = history). The v2
/// grid stores everything under `PageList::first_row_offset()`, so
/// we map from the block's tail backwards.
fn cell_id_from_hit(
    terminal: &carrot_terminal::TerminalHandle,
    block_idx: usize,
    grid_point: carrot_term::index::Point,
) -> carrot_grid::CellId {
    let term = terminal.lock();
    let Some(entry) = term.block_router().entries().get(block_idx) else {
        return carrot_grid::CellId::ROOT;
    };
    let grid = match &entry.variant {
        BlockVariant::Active(block) => block.grid(),
        BlockVariant::Frozen(block) => block.grid(),
    };
    // Viewport row: row 0 = first visible row. History is negative.
    // Clamp to the valid window of the grid.
    let total = grid.total_rows();
    let row_signed = grid_point.line.0.max(0) as usize;
    let row = row_signed.min(total.saturating_sub(1));
    let col = grid_point.column.0.min(u16::MAX as usize) as u16;
    grid.cell_id_at(row, col)
        .unwrap_or(carrot_grid::CellId::ROOT)
}
