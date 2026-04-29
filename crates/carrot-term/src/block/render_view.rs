//! Lock-once-extract render view.
//!
//! The render path is the hot-spot for terminal throughput: if the
//! renderer holds the `Term` lock while painting, every incoming PTY
//! byte queues up behind the GPU pipeline. [`RenderView`] is the
//! agreed-upon hand-off shape — the caller takes the lock, calls
//! [`crate::term::Term::render_view`], and drops the lock before
//! doing any rendering work.
//!
//! Frozen blocks are zero-copy: the router stores them as
//! `Arc<FrozenBlock>`, and the view clones each `Arc` (cheap, just a
//! refcount bump). Active block state is snapshotted more eagerly
//! — rows are materialised into `Vec<Row>` because the writer keeps
//! mutating the underlying `PageList`. A future refactor (Phase-F
//! debt) swaps to `Arc<[Cell]>` rows for cheaper clones.

use std::sync::Arc;

use carrot_grid::{BlockSnapshot, CellStyle, CellStyleAtlas, GraphemeStore, HyperlinkStore};

use super::active::ActiveBlock;
use super::display::DisplayState;
use super::frozen::FrozenBlock;
use super::kind::BlockKind;
use super::live_frame::LiveFrameRegion;
use super::router::{BlockId, BlockRouter, RouterBlockMetadata, RouterEntry};
use super::selection::BlockSelection;
use super::state::BlockVariant;
use super::vt_writer::VtWriterState;

/// A full-viewport snapshot of everything the renderer needs.
///
/// Held entirely in `Arc`-friendly shapes so the caller drops the
/// `Term` lock immediately after construction. Nothing here borrows
/// from the live terminal state.
pub struct RenderView {
    /// Frozen history, oldest-first. Cheap `Arc` clone per block.
    pub frozen: Vec<FrozenView>,
    /// Currently-running block, if any. Separate from `frozen`
    /// because active rendering memoizes on `sync_update_frame_id`
    /// while frozen rendering is keyed on the `Arc` identity alone.
    pub active: Option<ActiveBlockView>,
    /// Scrollback offset at snapshot time.
    pub display_offset: usize,
    /// Viewport dims at snapshot time — (cols, rows).
    pub grid_dims: (u16, u16),
}

/// Frozen-block view — one per entry in `RenderView::frozen`.
pub struct FrozenView {
    pub id: BlockId,
    pub block: Arc<FrozenBlock>,
    pub metadata: RouterBlockMetadata,
    /// Lifecycle marker carried over from the active block (sticky).
    /// Renderers route Shell blocks inline and Tui blocks through
    /// the PinnedFooter surface.
    pub kind: BlockKind,
}

/// Active-block snapshot. The grid data lives in [`BlockSnapshot`] —
/// rows + atlas + bounds in one bundle, with `bounds.first_row_offset`
/// available so selection mapping doesn't need to cache the prune
/// offset on the side. Hyperlink / grapheme stores are `Arc`-cloned so
/// multiple successive views share the interning state without a copy.
///
/// The active view carries NO cursor field. Shell-block carets live in
/// `carrot-cmdline`; TUI-block cursors live on the `carrot-term` VT
/// state and are rendered by Layer 4 via a separate pass against that
/// state, not via a field on the block view.
pub struct ActiveBlockView {
    pub id: BlockId,
    /// Owned snapshot of the live block's grid data + atlas + bounds.
    /// `snapshot.bounds.columns()` is the viewport column count — no
    /// duplicate `cols` field required.
    pub snapshot: BlockSnapshot,
    pub hyperlinks: Arc<HyperlinkStore>,
    pub graphemes: Arc<GraphemeStore>,
    pub metadata: RouterBlockMetadata,
    pub selection: Option<BlockSelection>,
    pub live_frame: Option<LiveFrameRegion>,
    /// Lifecycle marker — `Shell` until promoted to `Tui` by the
    /// detector. Sticky for the block's lifetime.
    pub kind: BlockKind,
    /// Monotonic frame id sourced from `ActiveBlock::generation` — the
    /// single Lamport counter the producer ticks on every observable
    /// mutation. Layer 5 memoizes its rendered snapshot keyed on
    /// `(block_id, frame_id)`.
    ///
    /// Was previously a row-count proxy; that mis-keyed in-place
    /// updates (progress bars, `\r` overwrites, partial-line emission)
    /// because the row count stayed flat while cell content advanced.
    pub sync_update_frame_id: u64,
}

impl RenderView {
    /// Build a view from the raw inputs. Normally callers go through
    /// [`crate::term::Term::render_view`]; this constructor stays
    /// accessible so `block`-only test code can construct views
    /// without a full `Term`.
    pub fn build(
        router: &BlockRouter,
        vt_state: &VtWriterState,
        display: &DisplayState,
        grid_dims: (u16, u16),
    ) -> Self {
        let mut frozen = Vec::new();
        let mut active = None;
        for entry in router.entries() {
            match &entry.variant {
                BlockVariant::Frozen(block) => {
                    let kind = block.kind();
                    frozen.push(FrozenView {
                        id: entry.id,
                        block: block.clone(),
                        metadata: entry.metadata.clone(),
                        kind,
                    });
                }
                BlockVariant::Active(block) => {
                    // There is usually at most one active block; if a
                    // second appears (nested / out-of-order command
                    // lifecycle), we take the first and drop the rest —
                    // the renderer's active-memoize key assumes a
                    // single live block per frame.
                    if active.is_none() {
                        active = Some(active_view(entry, block, vt_state));
                    }
                }
            }
        }
        RenderView {
            frozen,
            active,
            display_offset: display.display_offset,
            grid_dims,
        }
    }
}

fn active_view(
    entry: &RouterEntry,
    block: &ActiveBlock,
    vt_state: &VtWriterState,
) -> ActiveBlockView {
    // `vt_state` is intentionally unused for cursor data (plan A10:
    // cursor lives on the VT emulator, not the view). It stays in the
    // signature so future callers that need VT-derived fields (e.g.
    // scroll region for soft-wrap hints) don't need a new entry point.
    let _ = vt_state;
    let snapshot = BlockSnapshot::from_pages(block.grid(), block.atlas().as_slice());
    let hyperlinks = Arc::new(block.hyperlinks().clone());
    let graphemes = Arc::new(block.graphemes().clone());
    ActiveBlockView {
        id: entry.id,
        sync_update_frame_id: block.generation(),
        snapshot,
        hyperlinks,
        graphemes,
        metadata: entry.metadata.clone(),
        selection: block.selection().copied(),
        live_frame: block.live_frame().cloned(),
        kind: block.kind(),
    }
}

/// Wrapper around [`CellStyleAtlas`] for consumers that want to hand
/// the atlas onward without copying the backing `Vec`. Returned when
/// a consumer already holds an `Arc<CellStyleAtlas>` they want to
/// thread through a `RenderView`.
pub fn atlas_arc(atlas: &CellStyleAtlas) -> Arc<[CellStyle]> {
    Arc::from(atlas.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_router_produces_empty_view() {
        let r = BlockRouter::new(40);
        let state = VtWriterState::new(40, 24);
        let display = DisplayState::new();
        let view = RenderView::build(&r, &state, &display, (40, 24));
        assert!(view.frozen.is_empty());
        assert!(view.active.is_none());
        assert_eq!(view.display_offset, 0);
    }

    #[test]
    fn active_block_surfaces_in_view() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        let state = VtWriterState::new(40, 24);
        let display = DisplayState::new();
        let view = RenderView::build(&r, &state, &display, (40, 24));
        assert!(view.active.is_some());
        assert_eq!(view.frozen.len(), 0);
    }

    #[test]
    fn frozen_blocks_survive_snapshot() {
        use carrot_grid::Cell;
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        // One row of output — without it the silent-success rule in
        // `on_command_end` would drop the entry.
        if let crate::block::ActiveTarget::Block { block, .. } = r.active() {
            block.append_row(&[Cell::default()]);
        }
        r.on_command_end(0);
        r.on_command_start();
        // Exit 1 — failures stay regardless of empty output.
        r.on_command_end(1);
        let state = VtWriterState::new(40, 24);
        let display = DisplayState::new();
        let view = RenderView::build(&r, &state, &display, (40, 24));
        assert_eq!(view.frozen.len(), 2);
        assert_eq!(view.frozen[0].metadata.exit_code, Some(0));
        assert_eq!(view.frozen[1].metadata.exit_code, Some(1));
        assert!(view.active.is_none());
    }

    #[test]
    fn display_offset_and_dims_propagate() {
        let r = BlockRouter::new(40);
        let state = VtWriterState::new(40, 24);
        let mut display = DisplayState::new();
        display.display_offset = 17;
        let view = RenderView::build(&r, &state, &display, (100, 30));
        assert_eq!(view.display_offset, 17);
        assert_eq!(view.grid_dims, (100, 30));
    }
}
