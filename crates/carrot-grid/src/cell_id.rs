//! Stable cell identifiers.
//!
//! Terminal grids today live in `(row_index, col_index)` coordinate
//! space. Those coordinates **shift** when scrollback is pruned from
//! the head — row `N` today becomes row `N - pruned_rows` tomorrow.
//! That works fine for the single-viewer model: the render pass just
//! reads the current coords. But any feature that wants to **refer**
//! to a specific cell over time — highlight persistence, CRDT-based
//! multi-player sessions, pair-programming annotations, block-replay
//! diffs — needs identifiers that survive the shift.
//!
//! [`CellId`] is that identifier. Two-part structure:
//!
//! - `origin`: a u64 set when the cell's row was first appended to
//!   the PageList. Derived from the monotonic `first_row_offset`
//!   counter — it only ever increases. The row at `origin` today is
//!   the row at `origin` forever, even after the PageList has been
//!   pruned underneath it.
//! - `col`: column inside the row, same as today's coords.
//!
//! A pruned cell still has a valid `CellId` — it just no longer maps
//! to any row in the current PageList. The resolver ([`PageList::
//! cell_at_id`]) returns `None` in that case, which lets the caller
//! gracefully drop a stale reference (annotations fade out, CRDT ops
//! get discarded, etc.).
//!
//! # Why not just the pointer?
//!
//! The cell pointer in memory is fragile — pages are recycled, the
//! Vec moves on resize, and in a future disk-backed scrollback the
//! cell isn't even in memory. `CellId` is logical / monotonic, not
//! positional.
//!
//! # Multi-player story
//!
//! Future CRDT ops carry `CellId`s, not indices. A client editing
//! row 42 doesn't send "edit row 42" — it sends "edit CellId {
//! origin: 12345, col: 10 }". The server applies that op regardless
//! of the peer's current scrollback offset. Offsets can differ
//! between clients (their scrollbacks may have pruned at different
//! points) without the op losing its anchor.

/// Stable identifier for one cell, monotonic-append based.
///
/// `origin` is the row's append-sequence number (matches
/// [`crate::PageList::first_row_offset`] at the time the row was
/// appended, plus the offset within the current in-memory range).
/// `col` is the column inside the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellId {
    /// Monotonic row-origin number. First row ever appended is `0`;
    /// it only increases, never wraps back.
    pub origin: u64,
    /// Column index inside the row.
    pub col: u16,
}

impl CellId {
    /// Construct a CellId from raw values. `col` is not validated
    /// against any particular grid width — the caller guarantees
    /// `col < cols` at the time of use.
    pub const fn new(origin: u64, col: u16) -> Self {
        Self { origin, col }
    }

    /// Convenience: the "root" CellId at `(0, 0)`. Not particularly
    /// meaningful on its own but useful as a default / placeholder.
    pub const ROOT: Self = Self { origin: 0, col: 0 };
}

/// Range of `CellId`s that span a row (`origin`, 0..cols).
///
/// Returned by [`CellIdRow::row_span`] — convenient when a caller
/// wants to iterate every cell in a row without re-computing cols.
#[derive(Clone, Copy, Debug)]
pub struct CellIdRow {
    pub origin: u64,
    pub cols: u16,
}

impl CellIdRow {
    pub const fn new(origin: u64, cols: u16) -> Self {
        Self { origin, cols }
    }

    /// Iterator over all cells in this row, left-to-right.
    pub fn row_span(&self) -> CellIdSpan {
        CellIdSpan {
            origin: self.origin,
            col: 0,
            cols: self.cols,
        }
    }
}

/// Iterator over CellIds in one row.
#[derive(Clone, Copy, Debug)]
pub struct CellIdSpan {
    origin: u64,
    col: u16,
    cols: u16,
}

impl Iterator for CellIdSpan {
    type Item = CellId;
    fn next(&mut self) -> Option<CellId> {
        if self.col >= self.cols {
            return None;
        }
        let id = CellId::new(self.origin, self.col);
        self.col += 1;
        Some(id)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.cols - self.col) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CellIdSpan {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_id_equality_and_ordering() {
        let a = CellId::new(5, 3);
        let b = CellId::new(5, 3);
        let c = CellId::new(5, 4);
        let d = CellId::new(6, 0);
        assert_eq!(a, b);
        assert!(a < c);
        assert!(c < d); // different origins order first, then col
    }

    #[test]
    fn root_is_zero_zero() {
        assert_eq!(CellId::ROOT, CellId::new(0, 0));
    }

    #[test]
    fn row_span_iterates_left_to_right() {
        let row = CellIdRow::new(42, 5);
        let ids: Vec<_> = row.row_span().collect();
        assert_eq!(
            ids,
            vec![
                CellId::new(42, 0),
                CellId::new(42, 1),
                CellId::new(42, 2),
                CellId::new(42, 3),
                CellId::new(42, 4),
            ]
        );
    }

    #[test]
    fn row_span_size_hint_is_exact() {
        let span = CellIdRow::new(7, 10).row_span();
        assert_eq!(span.size_hint(), (10, Some(10)));
        assert_eq!(span.len(), 10);
    }

    #[test]
    fn row_span_is_empty_for_zero_cols() {
        let ids: Vec<_> = CellIdRow::new(99, 0).row_span().collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn cell_id_is_cheap_copy() {
        // 10 bytes total — fits easily in a register on any modern
        // architecture. Verify that the representation stayed tight.
        assert_eq!(std::mem::size_of::<CellId>(), 16);
        // u64 + u16 with natural padding = 16 bytes. Compact enough
        // that CRDT ops can inline them.
    }
}
