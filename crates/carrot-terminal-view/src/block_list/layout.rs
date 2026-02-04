//! Per-block layout cache for pixel↔grid mapping.
//!
//! The render pass builds one [`BlockLayoutEntry`] per visible block
//! each frame. Hit-testing reads those entries to map pixel positions
//! back to `(block_index, block_id, grid point, side)` tuples without
//! reopening the terminal lock.

use std::cell::Cell;
use std::rc::Rc;

use carrot_term::BlockId;
use inazuma::Pixels;

/// Shared slot that `TerminalGridElement` writes the grid's Y origin
/// into during prepaint, and `hit_test` reads during input handling.
/// `None` = the element hasn't laid out yet this frame, so the caller
/// falls back to a geometric estimate from the block's bounds.
pub(crate) type GridOriginStore = Rc<Cell<Option<Pixels>>>;

/// Cached layout info for one block — used for pixel→grid hit testing.
#[derive(Clone)]
pub(crate) struct BlockLayoutEntry {
    pub(crate) block_id: BlockId,
    pub(crate) block_index: usize,
    pub(crate) content_rows: usize,
    pub(crate) command_row_count: usize,
    pub(crate) grid_history_size: usize,
    /// Shared store: the grid element writes actual grid origin Y
    /// during prepaint, hit_test reads it for exact pixel→row mapping
    /// without sub-pixel drift.
    pub(crate) grid_origin_store: GridOriginStore,
}

pub(crate) fn fresh_origin_store() -> GridOriginStore {
    Rc::new(Cell::new(None))
}
