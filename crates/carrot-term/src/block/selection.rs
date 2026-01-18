//! Stable, prune-safe block selection model.
//!
//! The legacy block module tracked selections as `(row, col)` pairs
//! relative to the block grid. Any head-prune shifted those indices and
//! broke the selection. [`BlockSelection`] instead anchors on
//! [`carrot_grid::CellId`] — the row-origin-based identifier that
//! stays valid across prunes (pruned rows resolve to `None`, which
//! callers treat as "gracefully drop the selection anchor").
//!
//! One selection is attached to an [`crate::block::ActiveBlock`] at
//! a time. Frozen blocks never carry a selection — selection while
//! scrolling through history is UI-layer state (Layer 5 tracks it per
//! frozen block externally, not in the data model).
//!
//! # API shape
//!
//! - [`BlockSelection::new`] opens a selection at an anchor cell.
//! - [`BlockSelection::update`] moves the drag end without touching
//!   the anchor — the normalized range flips direction if the drag
//!   crosses the anchor.
//! - [`BlockSelection::range`] returns the normalized `(start, end)`.
//! - [`BlockSelection::contains`] answers point-in-selection for the
//!   kind (Simple, Lines, Block).
//! - [`BlockSelection::to_string`] materialises the selected text by
//!   walking the live `PageList`. Cells that resolve to `None`
//!   (pruned) contribute nothing — the copy is best-effort.
//!
//! Semantic selection (double-click-word) is modelled as a `Kind`
//! variant here for API completeness; resolving word boundaries is a
//! Phase-G UI concern and happens in Layer 5. The backend treats
//! `Semantic` the same as `Simple` for range + contains purposes.

use carrot_grid::{Cell, CellId, CellTag, GraphemeStore, PageList};

use super::active::ActiveBlock;

/// Style of a block selection.
///
/// `Block` is column-rectangular; `Lines` spans whole rows; `Simple`
/// and `Semantic` are linear (row-major, character-by-character) —
/// `Semantic` additionally signals the UI layer to extend the ends of
/// the range to word boundaries before rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Anchor → cursor, flat, one-line or multi-line.
    Simple,
    /// Like `Simple` but the UI extends both ends to word boundaries.
    Semantic,
    /// Whole rows between anchor row and cursor row.
    Lines,
    /// Rectangular: rows and columns between anchor and cursor.
    Block,
}

/// Which edge of the active cell the cursor is logically on. Matches
/// the legacy `Side` enum — UI widgets use it for caret-on-edge hit
/// testing at the pixel level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Active edge is at the left of the cell under the cursor.
    Left,
    /// Active edge is at the right of the cell under the cursor.
    Right,
}

/// A live block selection. Attached to an [`ActiveBlock`] via
/// [`ActiveBlock::start_selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSelection {
    /// First cell the user clicked — stays put across drag updates.
    pub anchor: CellId,
    /// Current drag end. Updates on every `update` call.
    pub active: CellId,
    pub kind: SelectionKind,
    pub side: Side,
}

impl BlockSelection {
    /// Open a new selection at `anchor`. The active end starts at the
    /// same cell; the selection is "empty" in range terms until the
    /// first [`update`](Self::update) call.
    pub fn new(anchor: CellId, kind: SelectionKind, side: Side) -> Self {
        Self {
            anchor,
            active: anchor,
            kind,
            side,
        }
    }

    /// Move the drag end without touching the anchor.
    pub fn update(&mut self, active: CellId, side: Side) {
        self.active = active;
        self.side = side;
    }

    /// Normalized `(start, end)` pair. `start ≤ end` by `CellId`
    /// ordering (`origin` first, then `col`).
    pub fn range(&self) -> (CellId, CellId) {
        let (a, b) = (self.anchor, self.active);
        if a <= b { (a, b) } else { (b, a) }
    }

    /// `true` when `id` falls inside the selection under the current
    /// kind's geometry.
    pub fn contains(&self, id: CellId) -> bool {
        let (start, end) = self.range();
        match self.kind {
            SelectionKind::Simple | SelectionKind::Semantic => {
                id.origin > start.origin && id.origin < end.origin
                    || (id.origin == start.origin
                        && id.origin == end.origin
                        && id.col >= start.col
                        && id.col <= end.col)
                    || (id.origin == start.origin && id.origin != end.origin && id.col >= start.col)
                    || (id.origin == end.origin && id.origin != start.origin && id.col <= end.col)
            }
            SelectionKind::Lines => id.origin >= start.origin && id.origin <= end.origin,
            SelectionKind::Block => {
                let (lo_col, hi_col) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };
                id.origin >= start.origin
                    && id.origin <= end.origin
                    && id.col >= lo_col
                    && id.col <= hi_col
            }
        }
    }

    /// Materialise selected text by walking the live grid. Pruned
    /// anchors / missing cells contribute an empty string; see module
    /// docs. Row breaks are inserted between rows at both `Simple`
    /// and `Lines` boundaries; `Block` inserts a break per row.
    pub fn to_string(&self, grid: &PageList, graphemes: &GraphemeStore) -> String {
        let (start, end) = self.range();
        let mut out = String::new();
        match self.kind {
            SelectionKind::Simple | SelectionKind::Semantic => {
                self.extract_linear(grid, graphemes, start, end, &mut out);
            }
            SelectionKind::Lines => {
                self.extract_lines(grid, graphemes, start.origin, end.origin, &mut out);
            }
            SelectionKind::Block => {
                let (lo_col, hi_col) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };
                self.extract_block(
                    grid,
                    graphemes,
                    start.origin,
                    end.origin,
                    lo_col,
                    hi_col,
                    &mut out,
                );
            }
        }
        out
    }

    fn extract_linear(
        &self,
        grid: &PageList,
        graphemes: &GraphemeStore,
        start: CellId,
        end: CellId,
        out: &mut String,
    ) {
        for origin in start.origin..=end.origin {
            let Some(row_ix) = resolve_row(grid, origin) else {
                continue;
            };
            let Some(row) = grid.row(row_ix) else {
                continue;
            };
            let first_col = if origin == start.origin { start.col } else { 0 };
            let last_col = if origin == end.origin {
                end.col
            } else {
                (row.len() as u16).saturating_sub(1)
            };
            append_row_range(row, graphemes, first_col, last_col, out);
            if origin != end.origin {
                out.push('\n');
            }
        }
    }

    fn extract_lines(
        &self,
        grid: &PageList,
        graphemes: &GraphemeStore,
        start_origin: u64,
        end_origin: u64,
        out: &mut String,
    ) {
        for origin in start_origin..=end_origin {
            let Some(row_ix) = resolve_row(grid, origin) else {
                continue;
            };
            let Some(row) = grid.row(row_ix) else {
                continue;
            };
            let last = (row.len() as u16).saturating_sub(1);
            append_row_range(row, graphemes, 0, last, out);
            if origin != end_origin {
                out.push('\n');
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_block(
        &self,
        grid: &PageList,
        graphemes: &GraphemeStore,
        start_origin: u64,
        end_origin: u64,
        lo_col: u16,
        hi_col: u16,
        out: &mut String,
    ) {
        for origin in start_origin..=end_origin {
            let Some(row_ix) = resolve_row(grid, origin) else {
                continue;
            };
            let Some(row) = grid.row(row_ix) else {
                continue;
            };
            let last = hi_col.min((row.len() as u16).saturating_sub(1));
            append_row_range(row, graphemes, lo_col, last, out);
            if origin != end_origin {
                out.push('\n');
            }
        }
    }
}

fn resolve_row(grid: &PageList, origin: u64) -> Option<usize> {
    let first = grid.first_row_offset();
    origin.checked_sub(first).map(|d| d as usize)
}

fn append_row_range(
    row: &[Cell],
    graphemes: &GraphemeStore,
    first_col: u16,
    last_col: u16,
    out: &mut String,
) {
    let end = (last_col as usize + 1).min(row.len());
    let start = (first_col as usize).min(end);
    for cell in &row[start..end] {
        cell_text(*cell, graphemes, out);
    }
}

fn cell_text(cell: Cell, graphemes: &GraphemeStore, out: &mut String) {
    match cell.tag() {
        CellTag::Ascii => {
            let b = cell.content() as u8;
            if b != 0 {
                out.push(b as char);
            } else {
                out.push(' ');
            }
        }
        CellTag::Codepoint => {
            if let Some(c) = char::from_u32(cell.content()) {
                out.push(c);
            }
        }
        CellTag::Grapheme => {
            let id = carrot_grid::GraphemeIndex(cell.content());
            if let Some(s) = graphemes.get(id) {
                out.push_str(s);
            }
        }
        // Ghost cells / image cells / shaped runs / custom / reserved
        // contribute nothing textually. Wide2nd is already covered by
        // the preceding primary cell.
        _ => {}
    }
}

// ─── ActiveBlock selection API ───────────────────────────────────

impl ActiveBlock {
    /// Start a fresh selection at `anchor` with the given kind + side.
    /// Replaces any prior selection on this block.
    pub fn start_selection(&mut self, anchor: CellId, kind: SelectionKind, side: Side) {
        self.selection_slot_mut()
            .replace(BlockSelection::new(anchor, kind, side));
    }

    /// Update the drag end of the current selection. No-op if no
    /// selection is active.
    pub fn update_selection(&mut self, active: CellId, side: Side) {
        if let Some(sel) = self.selection_slot_mut().as_mut() {
            sel.update(active, side);
        }
    }

    /// Clear the selection on this block. No-op if none is active.
    pub fn clear_selection(&mut self) {
        self.selection_slot_mut().take();
    }

    /// Current selection, if any.
    pub fn selection(&self) -> Option<&BlockSelection> {
        self.selection_slot().as_ref()
    }

    /// Mutable access to the current selection (for external consumers
    /// that tweak side / kind without replacing the whole struct).
    pub fn selection_mut(&mut self) -> Option<&mut BlockSelection> {
        self.selection_slot_mut().as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{Cell, CellStyleAtlas, PageCapacity, PageList};

    fn fresh_grid(cols: u16) -> PageList {
        PageList::new(PageCapacity::new(cols, 4096))
    }

    fn push_text(grid: &mut PageList, atlas: &mut CellStyleAtlas, text: &str) {
        let style = atlas.intern(Default::default());
        let mut row: Vec<Cell> = Vec::with_capacity(text.len());
        for b in text.bytes() {
            row.push(Cell::ascii(b, style));
        }
        // Pad to cols
        while row.len() < grid.capacity().cols as usize {
            row.push(Cell::EMPTY);
        }
        grid.append_row(&row);
    }

    #[test]
    fn new_selection_has_anchor_equal_active() {
        let sel = BlockSelection::new(CellId::new(3, 5), SelectionKind::Simple, Side::Left);
        assert_eq!(sel.anchor, sel.active);
        assert_eq!(sel.range(), (CellId::new(3, 5), CellId::new(3, 5)));
    }

    #[test]
    fn update_moves_active_keeps_anchor() {
        let mut sel = BlockSelection::new(CellId::new(2, 1), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(4, 8), Side::Right);
        assert_eq!(sel.anchor, CellId::new(2, 1));
        assert_eq!(sel.active, CellId::new(4, 8));
        assert_eq!(sel.side, Side::Right);
    }

    #[test]
    fn range_normalizes_order() {
        let mut sel = BlockSelection::new(CellId::new(5, 2), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(3, 9), Side::Left);
        let (start, end) = sel.range();
        assert_eq!(start, CellId::new(3, 9));
        assert_eq!(end, CellId::new(5, 2));
    }

    #[test]
    fn simple_contains_cells_on_intermediate_rows() {
        let mut sel = BlockSelection::new(CellId::new(1, 5), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(3, 2), Side::Right);
        assert!(sel.contains(CellId::new(2, 0)));
        assert!(sel.contains(CellId::new(2, 20)));
    }

    #[test]
    fn simple_excludes_cells_before_start_on_same_row() {
        let mut sel = BlockSelection::new(CellId::new(1, 5), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(3, 2), Side::Right);
        assert!(!sel.contains(CellId::new(1, 4)));
        assert!(sel.contains(CellId::new(1, 5)));
    }

    #[test]
    fn simple_includes_full_single_row_range() {
        let mut sel = BlockSelection::new(CellId::new(2, 1), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(2, 7), Side::Right);
        assert!(sel.contains(CellId::new(2, 1)));
        assert!(sel.contains(CellId::new(2, 4)));
        assert!(sel.contains(CellId::new(2, 7)));
        assert!(!sel.contains(CellId::new(2, 0)));
        assert!(!sel.contains(CellId::new(2, 8)));
    }

    #[test]
    fn lines_covers_whole_rows_regardless_of_col() {
        let mut sel = BlockSelection::new(CellId::new(1, 30), SelectionKind::Lines, Side::Left);
        sel.update(CellId::new(3, 5), Side::Right);
        assert!(sel.contains(CellId::new(1, 0)));
        assert!(sel.contains(CellId::new(3, 200)));
        assert!(!sel.contains(CellId::new(0, 30)));
        assert!(!sel.contains(CellId::new(4, 5)));
    }

    #[test]
    fn block_covers_rectangle() {
        let mut sel = BlockSelection::new(CellId::new(1, 5), SelectionKind::Block, Side::Left);
        sel.update(CellId::new(3, 10), Side::Right);
        assert!(sel.contains(CellId::new(2, 7)));
        assert!(sel.contains(CellId::new(3, 5)));
        assert!(!sel.contains(CellId::new(2, 4)));
        assert!(!sel.contains(CellId::new(2, 11)));
        assert!(!sel.contains(CellId::new(0, 7)));
    }

    #[test]
    fn to_string_extracts_linear_selection() {
        let mut grid = fresh_grid(10);
        let mut atlas = CellStyleAtlas::new();
        push_text(&mut grid, &mut atlas, "hello");
        push_text(&mut grid, &mut atlas, "world");
        // Rows are at origin 0 and 1 (first_row_offset=0).
        let sel_start = CellId::new(0, 1);
        let sel_end = CellId::new(1, 2);
        let mut sel = BlockSelection::new(sel_start, SelectionKind::Simple, Side::Left);
        sel.update(sel_end, Side::Right);
        let gstore = GraphemeStore::new();
        let s = sel.to_string(&grid, &gstore);
        assert_eq!(s, "ello     \nwor");
    }

    #[test]
    fn to_string_block_selection_extracts_rectangle() {
        let mut grid = fresh_grid(10);
        let mut atlas = CellStyleAtlas::new();
        push_text(&mut grid, &mut atlas, "ABCDEFGHIJ");
        push_text(&mut grid, &mut atlas, "abcdefghij");
        push_text(&mut grid, &mut atlas, "0123456789");
        let mut sel = BlockSelection::new(CellId::new(0, 2), SelectionKind::Block, Side::Left);
        sel.update(CellId::new(2, 5), Side::Right);
        let gstore = GraphemeStore::new();
        let s = sel.to_string(&grid, &gstore);
        assert_eq!(s, "CDEF\ncdef\n2345");
    }

    #[test]
    fn selection_anchor_survives_update_across_prune() {
        let mut sel = BlockSelection::new(CellId::new(10, 0), SelectionKind::Simple, Side::Left);
        sel.update(CellId::new(12, 5), Side::Right);
        // Anchor retained even after row 10 was logically pruned
        // (prune shifts first_row_offset upward; anchor.origin stays
        // as the original append sequence number).
        assert_eq!(sel.anchor, CellId::new(10, 0));
    }

    #[test]
    fn active_block_owns_selection_slot() {
        let mut block = ActiveBlock::new(20);
        assert!(block.selection().is_none());
        block.start_selection(CellId::new(0, 2), SelectionKind::Simple, Side::Left);
        assert!(block.selection().is_some());
        block.update_selection(CellId::new(1, 5), Side::Right);
        let sel = block.selection().expect("still set");
        assert_eq!(sel.active, CellId::new(1, 5));
        block.clear_selection();
        assert!(block.selection().is_none());
    }
}
