//! Per-block layout cache for pixel↔grid mapping.
//!
//! The render pass builds one [`BlockLayoutEntry`] per visible block
//! each frame. Hit-testing reads those entries to map pixel positions
//! back to `(block_index, block_id, grid point, side)` tuples without
//! reopening the terminal lock.

use std::cell::Cell;
use std::rc::Rc;

pub(crate) use carrot_block_render::GridOriginStore;
use carrot_term::BlockId;

/// Cached layout info for one block — used for pixel→grid hit testing.
#[derive(Clone)]
pub(crate) struct BlockLayoutEntry {
    pub(crate) block_id: BlockId,
    pub(crate) block_index: usize,
    pub(crate) content_rows: usize,
    pub(crate) command_row_count: usize,
    pub(crate) grid_history_size: usize,
    /// Shared slot the grid element writes its actual paint-time origin
    /// into during prepaint. Hit-test reads both x and y from this slot,
    /// so layout-side padding tokens never have to be re-applied
    /// downstream.
    pub(crate) grid_origin_store: GridOriginStore,
}

pub(crate) fn fresh_origin_store() -> GridOriginStore {
    Rc::new(Cell::new(None))
}
