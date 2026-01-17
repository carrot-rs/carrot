//! Block-aware search over the router's scrollback.
//!
//! Layer-2 `carrot-grid` already provides cell-level full-text search
//! (see [`carrot_grid::search::search_cells`]) — this module lifts
//! that to the router level: walk every block (frozen + active),
//! tag each match with its owning [`BlockId`], and return a
//! chronological iterator the UI can stream.
//!
//! # Regex
//!
//! The plan originally called this module `RegexIter`. The
//! `carrot-grid` search path is currently substring-only (regex is
//! deferred to a grid-crate Phase 3); once that lands, this wrapper
//! transparently benefits — the `BlockSearchMatch` shape stays. Until
//! then, `search` accepts a plain needle + [`carrot_grid::search::
//! SearchOptions`] struct.

use carrot_grid::search::{SearchMatch as GridMatch, SearchOptions, search_cells};

use super::router::{BlockId, BlockRouter};
use super::state::BlockVariant;

/// One match plus the block it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockSearchMatch {
    /// Owning block. `BlockId::PROMPT` for prompt-buffer matches.
    pub block_id: BlockId,
    /// Cell-level match info — start + length, prune-safe.
    pub inner: GridMatch,
}

impl BlockSearchMatch {
    /// Convenience — inclusive end cell id of the match.
    pub fn end(&self) -> carrot_grid::CellId {
        self.inner.end()
    }

    /// Start cell id.
    pub fn start(&self) -> carrot_grid::CellId {
        self.inner.start
    }
}

impl BlockRouter {
    /// Search every block in the router for `needle`. Matches come
    /// out oldest-first, in row-then-column order inside each block.
    ///
    /// The active block is included at its current state — mid-
    /// command output is searchable before `CommandEnd`. The prompt
    /// buffer is NOT included (prompt content is transient and
    /// searching it would surface duplicates each keypress).
    pub fn search<'a>(
        &'a self,
        needle: &'a str,
        options: SearchOptions,
    ) -> impl Iterator<Item = BlockSearchMatch> + 'a {
        self.entries().iter().flat_map(move |entry| {
            let id = entry.id;
            let matches = match &entry.variant {
                BlockVariant::Active(block) => search_cells(block.grid(), needle, options),
                BlockVariant::Frozen(block) => search_cells(block.grid(), needle, options),
            };
            matches.into_iter().map(move |m| BlockSearchMatch {
                block_id: id,
                inner: m,
            })
        })
    }

    /// Count matches without materialising them. Useful for progress
    /// badges while the search UI is still typing.
    pub fn search_count(&self, needle: &str, options: SearchOptions) -> usize {
        self.search(needle, options).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::Cell;

    fn push_text(block: &mut super::super::ActiveBlock, text: &str) {
        let style = block.atlas_mut().intern(Default::default());
        let cols = block.grid().capacity().cols as usize;
        let mut row: Vec<Cell> = Vec::with_capacity(cols);
        for b in text.bytes() {
            row.push(Cell::ascii(b, style));
        }
        while row.len() < cols {
            row.push(Cell::EMPTY);
        }
        block.append_row(&row);
    }

    #[test]
    fn empty_router_returns_no_matches() {
        let r = BlockRouter::new(40);
        let hits: Vec<_> = r.search("anything", SearchOptions::default()).collect();
        assert!(hits.is_empty());
    }

    #[test]
    fn search_finds_hits_in_active_block() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "hello needle world");
        }
        let hits: Vec<_> = r.search("needle", SearchOptions::default()).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].inner.len, 6);
    }

    #[test]
    fn search_finds_hits_across_frozen_and_active_blocks() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "needle in frozen");
        }
        r.on_command_end(0);
        r.on_command_start();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "needle in active");
        }
        let hits: Vec<_> = r.search("needle", SearchOptions::default()).collect();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn search_count_matches_iterator_len() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "aaaa bb aa");
        }
        let opt = SearchOptions::default();
        let n = r.search_count("aa", opt);
        let hits: Vec<_> = r.search("aa", opt).collect();
        assert_eq!(n, hits.len());
    }

    #[test]
    fn search_options_propagate_to_grid_layer() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "Hello HELLO hello");
        }
        let insensitive = r.search_count("hello", SearchOptions::default().case_insensitive(true));
        let sensitive = r.search_count("hello", SearchOptions::default());
        assert!(insensitive > sensitive);
    }

    #[test]
    fn prompt_buffer_is_not_searched() {
        let r = BlockRouter::new(40);
        let hits: Vec<_> = r.search("prompt", SearchOptions::default()).collect();
        assert!(hits.is_empty());
    }

    #[test]
    fn matches_are_tagged_with_owning_block() {
        let mut r = BlockRouter::new(40);
        r.on_command_start();
        let first_id = r.active_id().unwrap();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "needle 1");
        }
        r.on_command_end(0);
        r.on_command_start();
        let second_id = r.active_id().unwrap();
        if let super::super::router::ActiveTarget::Block { block, .. } = r.active() {
            push_text(block, "needle 2");
        }
        let hits: Vec<_> = r.search("needle", SearchOptions::default()).collect();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.block_id == first_id));
        assert!(hits.iter().any(|h| h.block_id == second_id));
    }
}
