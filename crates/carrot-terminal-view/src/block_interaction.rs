//! Block-selection helpers on [`BlockListView`].
//!
//! Hit-testing moved to `block_list/hit_test.rs`. This module keeps
//! the selection-materialisation helpers the terminal pane uses for
//! clipboard / find. Walks the v2 router's active block since that's
//! where `BlockSelection` lives; frozen blocks drop their selection
//! on finalize.

use carrot_term::block::BlockVariant;

use crate::block_list::BlockListView;

impl BlockListView {
    /// Copy the current text selection to a string (for Cmd+C).
    /// Walks v2 router entries looking for a block with an active
    /// selection — typically at most one (the active block, or none
    /// at rest).
    pub fn copy_selection_text(&self) -> Option<String> {
        let handle = self.terminal.clone();
        let term = handle.lock();
        for entry in term.block_router().entries() {
            if let BlockVariant::Active(block) = &entry.variant
                && let Some(sel) = block.selection()
            {
                return Some(sel.to_string(block.grid(), block.graphemes()));
            }
        }
        None
    }

    /// Clear the selection on every active block (frozen blocks
    /// don't carry selection state per A10 → BlockSelection is a
    /// live-block-only concept).
    pub(crate) fn clear_all_selections(&self) {
        let handle = self.terminal.clone();
        let mut term = handle.lock();
        for entry in term.block_router_mut().entries_mut().iter_mut() {
            if let Some(block) = entry.variant.as_active_mut() {
                block.clear_selection();
            }
        }
    }
}
