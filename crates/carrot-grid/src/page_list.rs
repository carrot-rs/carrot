//! Doubly-ended queue of storage pages.
//!
//! # Invariants
//!
//! - Non-tail pages are always **full** (`rows_used == rows_cap`). The tail
//!   is the only page that may be partial. This invariant is enforced
//!   because `PageList::append_row` is the only entry point that mutates
//!   row data, and it only writes into the tail.
//! - With that invariant, every row index has an O(1) `(page_ix, row_ix)`
//!   mapping: `page_ix = row / rows_per_page`, `row_ix = row % rows_per_page`
//!   (adjusted for the running prune offset).
//!
//! # Contract
//!
//! - `append_row`: **O(1)** amortized. The tail page is mutated in-place;
//!   when full, a new tail is pushed (or popped from the recycle pool).
//! - `prune_head`: **O(1)**. `VecDeque::pop_front` + `Page::reset` + push
//!   onto recycle pool. No memmove.
//! - `row(ix)`: **O(1)**. Direct divmod into the deque.
//! - `rows(range)`: **O(range.len())**. Starts at the correct page in O(1),
//!   then walks forward exactly `range.len()` rows.
//!
//! These are the claims the `grid.rs` bench validates.

use std::collections::VecDeque;

use crate::cell::Cell;
use crate::cell_id::CellId;
use crate::page::{Page, PageCapacity};

/// Doubly-ended queue of pages. See module docs for the invariant that
/// makes constant-time row lookup work.
pub struct PageList {
    cap: PageCapacity,
    pages: VecDeque<Page>,
    /// Recycled pages, ready for reuse by `append_row` when the tail fills.
    pool: Vec<Page>,
    /// Rolling offset — total rows ever appended, minus currently-present.
    /// Used to translate caller-visible absolute row ids back to
    /// deque-local positions after pruning.
    first_row_offset: u64,
}

impl PageList {
    pub fn new(cap: PageCapacity) -> Self {
        Self {
            cap,
            pages: VecDeque::new(),
            pool: Vec::new(),
            first_row_offset: 0,
        }
    }

    pub fn capacity(&self) -> PageCapacity {
        self.cap
    }

    /// Rows currently reachable via `row(0..total_rows())`.
    pub fn total_rows(&self) -> usize {
        match (self.pages.len(), self.pages.back()) {
            (0, _) => 0,
            (n, Some(tail)) => {
                // All but the last page are full (invariant).
                let full = n - 1;
                full * self.cap.rows_cap as usize + tail.rows_used() as usize
            }
            _ => 0,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn pool_size(&self) -> usize {
        self.pool.len()
    }

    /// Append a single row. **O(1)** amortized.
    ///
    /// Oversized rows are truncated to `cols`, undersized rows are
    /// zero-padded — the grid always stores full-width rows so downstream
    /// slice access stays cache-linear.
    pub fn append_row(&mut self, row: &[Cell]) {
        loop {
            if let Some(tail) = self.pages.back_mut()
                && !tail.is_full()
            {
                tail.push_row(row);
                return;
            }
            // Tail full or no page yet — acquire a fresh page and try again.
            let page = self.pool.pop().unwrap_or_else(|| Page::new(self.cap));
            self.pages.push_back(page);
            // loop repeats; the pushed page is now the tail.
        }
    }

    /// Return the row at index `ix`. **O(1)**. `None` if out of range.
    pub fn row(&self, ix: usize) -> Option<&[Cell]> {
        let rows_per_page = self.cap.rows_cap as usize;
        let page_ix = ix / rows_per_page;
        let row_in_page = ix % rows_per_page;
        let page = self.pages.get(page_ix)?;
        // Tail may be partial — row_in_page must be within rows_used.
        if row_in_page >= page.rows_used() as usize {
            return None;
        }
        page.row(row_in_page as u16)
    }

    /// Mutable row access at index `ix`. **O(1)**. Returns `None` if the
    /// row is out of range. Mutating a row here does **not** auto-mark
    /// it dirty — callers should pair with [`Self::mark_dirty`] so the
    /// renderer picks up the edit on the next pass.
    pub fn row_mut(&mut self, ix: usize) -> Option<&mut [Cell]> {
        let rows_per_page = self.cap.rows_cap as usize;
        if rows_per_page == 0 {
            return None;
        }
        let page_ix = ix / rows_per_page;
        let row_in_page = ix % rows_per_page;
        let page = self.pages.get_mut(page_ix)?;
        if row_in_page >= page.rows_used() as usize {
            return None;
        }
        page.row_mut(row_in_page as u16)
    }

    /// Swap the cell content of two rows in place. **O(cols)**. Both
    /// rows must exist; if either index is out of range the call is a
    /// no-op. Both rows are marked dirty. Used by scroll region edits
    /// and [`Self::insert_blank_rows`] / [`Self::delete_rows`].
    pub fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let total = self.total_rows();
        if a >= total || b >= total {
            return;
        }
        let rows_per_page = self.cap.rows_cap as usize;
        if rows_per_page == 0 {
            return;
        }
        let (pa, ra) = (a / rows_per_page, a % rows_per_page);
        let (pb, rb) = (b / rows_per_page, b % rows_per_page);
        if pa == pb {
            if let Some(page) = self.pages.get_mut(pa) {
                page.swap_rows_in_page(ra as u16, rb as u16);
            }
            return;
        }
        // Cross-page swap: `VecDeque::make_contiguous` hands back a
        // single flat `&mut [Page]`. With two distinct indices we can
        // reach both pages at once via a single `split_at_mut`.
        let pages = self.pages.make_contiguous();
        let (lo, hi, lo_r, hi_r) = if pa < pb {
            (pa, pb, ra, rb)
        } else {
            (pb, pa, rb, ra)
        };
        let (head, tail) = pages.split_at_mut(hi);
        if let (Some(lp), Some(hp)) = (head.get_mut(lo), tail.first_mut()) {
            Page::swap_rows_between(lp, lo_r as u16, hp, hi_r as u16);
        }
    }

    /// Zero the content of row `ix` in place (all cells reset to
    /// [`Cell::EMPTY`]) and mark it dirty. No-op if out of range.
    pub fn clear_row(&mut self, ix: usize) {
        if let Some(row) = self.row_mut(ix) {
            for cell in row.iter_mut() {
                *cell = Cell::EMPTY;
            }
        }
        self.mark_dirty(ix);
    }

    /// Shift a contiguous row range by `n` rows upward (toward lower
    /// indices), blanking the rows vacated at the bottom. This matches
    /// `SU` (scroll-up), `IL` (insert-line) upper half, and
    /// `reverse_index` at bottom of region.
    ///
    /// `top..=bot` is the scroll region (inclusive). Rows outside the
    /// region are untouched.
    pub fn scroll_region_up(&mut self, top: usize, bot: usize, n: usize) {
        if top > bot || n == 0 {
            return;
        }
        let total = self.total_rows();
        if total == 0 || top >= total {
            return;
        }
        let bot = bot.min(total - 1);
        if top > bot {
            return;
        }
        let span = bot + 1 - top;
        let n = n.min(span);
        for i in top..=bot.saturating_sub(n) {
            self.swap_rows(i, i + n);
        }
        for i in (bot + 1).saturating_sub(n)..=bot {
            self.clear_row(i);
        }
    }

    /// Shift a contiguous row range by `n` rows downward (toward higher
    /// indices), blanking the rows vacated at the top. Matches `SD`,
    /// `IL` lower half, and `RI` (reverse index at top of region).
    pub fn scroll_region_down(&mut self, top: usize, bot: usize, n: usize) {
        if top > bot || n == 0 {
            return;
        }
        let total = self.total_rows();
        if total == 0 || top >= total {
            return;
        }
        let bot = bot.min(total - 1);
        if top > bot {
            return;
        }
        let span = bot + 1 - top;
        let n = n.min(span);
        for i in (top + n..=bot).rev() {
            self.swap_rows(i, i - n);
        }
        for i in top..top + n {
            self.clear_row(i);
        }
    }

    /// Iterate rows over `[start, end)`. **O(end - start)** work — the
    /// iterator seeks to the right page in O(1) and walks from there.
    pub fn rows(&self, start: usize, end: usize) -> RowIter<'_> {
        let end = end.min(self.total_rows());
        let start = start.min(end);
        let rows_per_page = self.cap.rows_cap as usize;
        let page_ix = if rows_per_page == 0 {
            0
        } else {
            start / rows_per_page
        };
        let row_in_page = if rows_per_page == 0 {
            0
        } else {
            (start % rows_per_page) as u16
        };
        RowIter {
            pages: &self.pages,
            page_ix,
            row_in_page,
            remaining: end - start,
        }
    }

    /// Drop the oldest page. **O(1)**. Returns the number of rows evicted.
    ///
    /// The reset page is pushed onto the recycle pool — the next
    /// `append_row` that needs a fresh page will reuse this memory with
    /// zero syscalls.
    pub fn prune_head(&mut self) -> Option<u16> {
        let mut head = self.pages.pop_front()?;
        let rows = head.rows_used();
        self.first_row_offset += rows as u64;
        head.reset();
        self.pool.push(head);
        Some(rows)
    }

    /// Total rows pruned since construction — useful for translating
    /// long-lived row ids into current positions.
    pub fn first_row_offset(&self) -> u64 {
        self.first_row_offset
    }

    /// Construct a stable [`CellId`] for the cell at `(row, col)`.
    /// The `row` is a current (post-prune) row index; the returned id
    /// survives further pruning. Returns `None` if `row` is out of
    /// range or `col >= cols`.
    pub fn cell_id_at(&self, row: usize, col: u16) -> Option<CellId> {
        if col >= self.cap.cols {
            return None;
        }
        if row >= self.total_rows() {
            return None;
        }
        let origin = self.first_row_offset + row as u64;
        Some(CellId::new(origin, col))
    }

    /// Resolve a stable [`CellId`] back to current coordinates, if
    /// the row is still in the in-memory scrollback. Returns `None`
    /// if the row has been pruned (the id is past-historical).
    pub fn cell_coords_for_id(&self, id: CellId) -> Option<(usize, u16)> {
        if id.col >= self.cap.cols {
            return None;
        }
        let current_row = id.origin.checked_sub(self.first_row_offset)?;
        let current_row = current_row as usize;
        if current_row >= self.total_rows() {
            return None;
        }
        Some((current_row, id.col))
    }

    /// Convenience: fetch the `Cell` at a given `CellId`. Returns
    /// `None` if the id's row has been pruned or the col is out of
    /// range.
    pub fn cell_at_id(&self, id: CellId) -> Option<Cell> {
        let (row, col) = self.cell_coords_for_id(id)?;
        self.row(row).and_then(|r| r.get(col as usize).copied())
    }

    /// Mark row `ix` dirty so the next render pass can re-upload only
    /// that row. No-op if `ix` is out of range.
    pub fn mark_dirty(&mut self, ix: usize) {
        let rows_per_page = self.cap.rows_cap as usize;
        if rows_per_page == 0 {
            return;
        }
        let page_ix = ix / rows_per_page;
        let row_in_page = ix % rows_per_page;
        if let Some(page) = self.pages.get_mut(page_ix) {
            if row_in_page < page.rows_used() as usize {
                page.mark_dirty(row_in_page as u16);
            }
        }
    }
}

/// Iterator over a row range. Lifetime-tied to the `PageList`.
pub struct RowIter<'a> {
    pages: &'a VecDeque<Page>,
    page_ix: usize,
    row_in_page: u16,
    remaining: usize,
}

impl<'a> Iterator for RowIter<'a> {
    type Item = &'a [Cell];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        while let Some(page) = self.pages.get(self.page_ix) {
            if (self.row_in_page as usize) < page.rows_used() as usize {
                let row = page.row(self.row_in_page)?;
                self.row_in_page += 1;
                self.remaining -= 1;
                return Some(row);
            }
            self.page_ix += 1;
            self.row_in_page = 0;
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for RowIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyleId;

    fn row(content: &[u8]) -> Vec<Cell> {
        content
            .iter()
            .map(|&b| Cell::ascii(b, CellStyleId(0)))
            .collect()
    }

    #[test]
    fn append_and_read_across_pages() {
        // 4-col, 128-byte page → 4 rows per page. We append 10 → 3 pages.
        let cap = PageCapacity::new(4, 128);
        let mut list = PageList::new(cap);
        for i in 0..10u8 {
            list.append_row(&row(&[b'a' + i; 4]));
        }
        assert_eq!(list.total_rows(), 10);
        assert!(list.page_count() >= 3);
        for i in 0..10u8 {
            let got = list.row(i as usize).expect("row in range");
            assert_eq!(got[0].content(), (b'a' + i) as u32);
        }
    }

    #[test]
    fn rows_iterator_walks_requested_range_only() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..20u8 {
            list.append_row(&row(&[i, i]));
        }
        let collected: Vec<u8> = list.rows(5, 15).map(|r| r[0].content() as u8).collect();
        assert_eq!(collected, (5..15u8).collect::<Vec<_>>());
    }

    #[test]
    fn prune_recycles_into_pool() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..16u8 {
            list.append_row(&row(&[i, i]));
        }
        let pages_before = list.page_count();
        assert!(pages_before > 0);
        let rows_pruned = list.prune_head().expect("head present");
        assert_eq!(list.page_count(), pages_before - 1);
        assert_eq!(list.pool_size(), 1);
        assert_eq!(list.first_row_offset(), rows_pruned as u64);
        // After appending more, the pool shrinks — we reused a page.
        for i in 0..4u8 {
            list.append_row(&row(&[i, i]));
        }
        assert_eq!(list.pool_size(), 0);
    }

    #[test]
    fn rows_iter_stops_at_actual_end() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..5u8 {
            list.append_row(&row(&[i, i]));
        }
        let got: Vec<_> = list.rows(3, 100).collect();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn rows_iter_size_hint_matches_actual() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..10u8 {
            list.append_row(&row(&[i, i]));
        }
        let iter = list.rows(2, 7);
        assert_eq!(iter.len(), 5);
        assert_eq!(iter.size_hint(), (5, Some(5)));
    }

    #[test]
    fn mark_dirty_targets_the_right_row() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..8u8 {
            list.append_row(&row(&[i, i]));
        }
        list.mark_dirty(5);
        // Row 5 is on page 5 / 4 = 1, row 5 % 4 = 1.
        let rows_per_page = list.capacity().rows_cap as usize;
        let page_ix = 5 / rows_per_page;
        let row_in_page = 5 % rows_per_page;
        let page_dirty = list.pages[page_ix].dirty();
        assert_ne!(page_dirty & (1u64 << row_in_page), 0);
    }

    #[test]
    fn total_rows_is_exact_with_partial_tail() {
        // 4 cols × 4 rows/page. Push 10 rows → 2 full pages + 2 on tail.
        let cap = PageCapacity::new(4, 128);
        let mut list = PageList::new(cap);
        for i in 0..10u8 {
            list.append_row(&row(&[b'a' + i; 4]));
        }
        assert_eq!(list.total_rows(), 10);
        assert_eq!(list.page_count(), 3);
    }

    #[test]
    fn non_tail_pages_stay_full_after_many_appends() {
        // This is the O(1) invariant. Push 100 rows into a 4-row-per-page
        // list: pages 0..24 must be at rows_used == rows_cap.
        let cap = PageCapacity::new(4, 128);
        let mut list = PageList::new(cap);
        for i in 0..100u8 {
            list.append_row(&row(&[b'a' + (i % 26); 4]));
        }
        let last = list.page_count() - 1;
        for ix in 0..last {
            assert_eq!(
                list.pages[ix].rows_used(),
                cap.rows_cap,
                "page {ix} violated full-non-tail invariant",
            );
        }
    }

    #[test]
    fn cell_id_for_current_row_reflects_offset() {
        let cap = PageCapacity::new(3, 64);
        let mut list = PageList::new(cap);
        for i in 0..6u8 {
            list.append_row(&row(&[i, i, i]));
        }
        let id = list.cell_id_at(2, 1).expect("in range");
        assert_eq!(id.origin, 2);
        assert_eq!(id.col, 1);
    }

    #[test]
    fn cell_id_out_of_range_returns_none() {
        let cap = PageCapacity::new(3, 64);
        let mut list = PageList::new(cap);
        list.append_row(&row(&[0, 0, 0]));
        assert!(list.cell_id_at(1, 0).is_none());
        assert!(list.cell_id_at(0, 3).is_none());
    }

    #[test]
    fn cell_id_survives_prune() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..10u8 {
            list.append_row(&row(&[i, i]));
        }
        // Capture an id for row 5 col 0.
        let id = list.cell_id_at(5, 0).expect("row 5");
        assert_eq!(id.origin, 5);

        let pruned = list.prune_head().expect("head present");
        assert!(pruned >= 1);

        // Id still resolves — same cell, just shifted in current coords.
        let (new_row, col) = list.cell_coords_for_id(id).expect("still in mem");
        assert_eq!(new_row, 5 - pruned as usize);
        assert_eq!(col, 0);
    }

    #[test]
    fn pruned_cell_id_is_none() {
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..10u8 {
            list.append_row(&row(&[i, i]));
        }
        let id = list.cell_id_at(0, 0).expect("row 0");
        list.prune_head();
        // Row 0 has been pruned; its id still exists but no longer
        // resolves to a live cell.
        assert!(list.cell_coords_for_id(id).is_none());
        assert!(list.cell_at_id(id).is_none());
    }

    #[test]
    fn cell_at_id_reads_correct_cell() {
        let cap = PageCapacity::new(3, 64);
        let mut list = PageList::new(cap);
        for i in 0..4u8 {
            list.append_row(&row(&[b'a' + i, b'b' + i, b'c' + i]));
        }
        let id = list.cell_id_at(2, 1).expect("cell");
        let cell = list.cell_at_id(id).expect("still present");
        assert_eq!(cell.content(), (b'b' + 2) as u32);
    }

    #[test]
    fn row_mut_returns_writable_slice() {
        let cap = PageCapacity::new(3, 64);
        let mut list = PageList::new(cap);
        for _ in 0..3 {
            list.append_row(&row(&[0, 0, 0]));
        }
        let mutable = list.row_mut(1).expect("row 1");
        mutable[0] = Cell::ascii(b'X', CellStyleId(0));
        // Read-back confirms the write stuck.
        let readback = list.row(1).expect("row 1");
        assert_eq!(readback[0].content(), b'X' as u32);
    }

    #[test]
    fn swap_rows_within_same_page() {
        let cap = PageCapacity::new(2, 128);
        let mut list = PageList::new(cap);
        list.append_row(&row(&[b'a', b'a']));
        list.append_row(&row(&[b'b', b'b']));
        list.swap_rows(0, 1);
        assert_eq!(list.row(0).unwrap()[0].content(), b'b' as u32);
        assert_eq!(list.row(1).unwrap()[0].content(), b'a' as u32);
    }

    #[test]
    fn swap_rows_across_pages() {
        // 2 rows per page; rows 0-1 page 0, rows 2-3 page 1, rows 4-5 page 2.
        let cap = PageCapacity::new(2, 64);
        let mut list = PageList::new(cap);
        for i in 0..6u8 {
            list.append_row(&row(&[b'a' + i, b'a' + i]));
        }
        list.swap_rows(0, 5);
        assert_eq!(list.row(0).unwrap()[0].content(), (b'a' + 5) as u32);
        assert_eq!(list.row(5).unwrap()[0].content(), b'a' as u32);
    }

    #[test]
    fn scroll_region_up_shifts_and_blanks() {
        // Rows 0..5 with distinct content; scroll region 1..=3 up by 1.
        let cap = PageCapacity::new(2, 128);
        let mut list = PageList::new(cap);
        for i in 0..5u8 {
            list.append_row(&row(&[b'a' + i, b'a' + i]));
        }
        list.scroll_region_up(1, 3, 1);
        // Row 0 untouched, row 1 now holds old row 2, row 2 old row 3,
        // row 3 blank, row 4 untouched.
        assert_eq!(list.row(0).unwrap()[0].content(), b'a' as u32);
        assert_eq!(list.row(1).unwrap()[0].content(), b'c' as u32);
        assert_eq!(list.row(2).unwrap()[0].content(), b'd' as u32);
        assert_eq!(list.row(3).unwrap()[0].content(), 0);
        assert_eq!(list.row(4).unwrap()[0].content(), b'e' as u32);
    }

    #[test]
    fn scroll_region_down_shifts_and_blanks() {
        let cap = PageCapacity::new(2, 128);
        let mut list = PageList::new(cap);
        for i in 0..5u8 {
            list.append_row(&row(&[b'a' + i, b'a' + i]));
        }
        list.scroll_region_down(1, 3, 1);
        // Row 0 untouched, row 1 blank, row 2 old row 1 (b), row 3 old
        // row 2 (c), row 4 untouched.
        assert_eq!(list.row(0).unwrap()[0].content(), b'a' as u32);
        assert_eq!(list.row(1).unwrap()[0].content(), 0);
        assert_eq!(list.row(2).unwrap()[0].content(), b'b' as u32);
        assert_eq!(list.row(3).unwrap()[0].content(), b'c' as u32);
        assert_eq!(list.row(4).unwrap()[0].content(), b'e' as u32);
    }
}
