//! A single storage page — a contiguous aligned buffer of cells plus
//! row metadata.
//!
//! Fixed capacity at construction time, aligned to the OS page boundary
//! (default 4 KB). Cells are laid out row-major so a
//! single `cells[row * cols..row * cols + cols]` slice gives the entire row
//! — perfect for memcpy / SIMD / GPU upload.

use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::ptr::NonNull;

use crate::cell::Cell;

/// Fixed capacity parameters of a page.
///
/// `cells_cap` is derived from `page_bytes / sizeof(Cell)` — we don't
/// waste padding below the last row; the math is exact because Cell is 8 B
/// and the page is 8-aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCapacity {
    pub cols: u16,
    pub rows_cap: u16,
    pub page_bytes: u32,
}

impl PageCapacity {
    /// Build a capacity that fits as many full rows of `cols` cells as
    /// possible inside `page_bytes` bytes, **capped at 64 rows per page**
    /// so the per-page dirty bitmap fits in a `u64` (see `Page::dirty`).
    ///
    /// Panics if `cols == 0` or `page_bytes < sizeof(Cell)` — those are
    /// programmer-bug conditions that the caller is expected to validate.
    /// For well-formed terminal configurations (cols ≥ 2, page_bytes ≥ 512)
    /// construction always succeeds.
    pub const fn new(cols: u16, page_bytes: u32) -> Self {
        assert!(cols > 0, "cols must be > 0");
        assert!(
            page_bytes as usize >= std::mem::size_of::<Cell>(),
            "page must fit at least one cell"
        );
        let cells_from_bytes = (page_bytes as usize) / std::mem::size_of::<Cell>();
        let raw_rows_cap = cells_from_bytes / cols as usize;
        // Clamp at 64: the dirty bitmap is `u64`, so any row index beyond
        // 63 couldn't be represented. Narrow pages (e.g. cols=1 at 4 KB)
        // would otherwise compute rows_cap = 512 and blow past this.
        let rows_cap = if raw_rows_cap > 64 { 64 } else { raw_rows_cap };
        assert!(rows_cap > 0, "page too small for even one row");
        Self {
            cols,
            rows_cap: rows_cap as u16,
            page_bytes,
        }
    }

    pub const fn cells_cap(self) -> usize {
        self.cols as usize * self.rows_cap as usize
    }
}

/// A single storage page.
///
/// Owns a heap allocation of `cells_cap` cells. Rows are written in-place
/// via [`Page::row_mut`] and read via [`Page::row`]. Bounds-checked in
/// debug, direct in release — row indexing is a hot path.
pub struct Page {
    cap: PageCapacity,
    rows_used: u16,
    cells: NonNull<Cell>,
    /// Dirty-row bitmap. Bit `i` set → row `i` needs redraw.
    /// Up to 64 rows per page; beyond that, additional words would be needed —
    /// 4 KB pages at 80 cols give 6 rows, far below the 64-row limit.
    dirty: u64,
}

// SAFETY: Page owns a unique allocation of Copy Cells.
unsafe impl Send for Page {}
unsafe impl Sync for Page {}

impl Page {
    /// Allocate a new zeroed page with the given capacity.
    pub fn new(cap: PageCapacity) -> Self {
        // `PageCapacity::new` already clamps rows_cap at 64 to keep the
        // dirty bitmap in a single `u64`. This debug_assert guards against
        // callers constructing a capacity struct by hand.
        debug_assert!(
            cap.rows_cap as usize <= 64,
            "rows_cap must be ≤ 64 (dirty bitmap is u64); use PageCapacity::new to clamp"
        );
        let layout = Self::layout(cap);
        // SAFETY: layout is non-zero (checked above), zeroed Cell == Cell::EMPTY.
        let ptr = unsafe { alloc_zeroed(layout) } as *mut Cell;
        let Some(cells) = NonNull::new(ptr) else {
            handle_alloc_error(layout);
        };
        Self {
            cap,
            rows_used: 0,
            cells,
            dirty: 0,
        }
    }

    fn layout(cap: PageCapacity) -> Layout {
        let size = cap.cells_cap() * std::mem::size_of::<Cell>();
        // Align to the requested page size so mmap-style pages are possible
        // later without changing the interface. `PageCapacity::new` has
        // already validated cols > 0 and page_bytes ≥ sizeof(Cell), so
        // `size > 0`. `align` is bounded by `page_bytes: u32`, so it's a
        // valid power-of-two-rounded alignment — `from_size_align_unchecked`
        // is sound and avoids the runtime panic path.
        let align = (cap.page_bytes as usize)
            .next_power_of_two()
            .max(std::mem::align_of::<Cell>());
        // SAFETY: both size and align are validated above; align is a
        // power of two; size ≤ isize::MAX for any realistic page_bytes.
        unsafe { Layout::from_size_align_unchecked(size, align) }
    }

    pub fn capacity(&self) -> PageCapacity {
        self.cap
    }

    pub fn cols(&self) -> u16 {
        self.cap.cols
    }

    pub fn rows_cap(&self) -> u16 {
        self.cap.rows_cap
    }

    pub fn rows_used(&self) -> u16 {
        self.rows_used
    }

    /// Is this page fully filled?
    pub fn is_full(&self) -> bool {
        self.rows_used >= self.cap.rows_cap
    }

    /// Returns the full row slice at `row_ix`, or `None` if out of range.
    pub fn row(&self, row_ix: u16) -> Option<&[Cell]> {
        if row_ix >= self.rows_used {
            return None;
        }
        let start = row_ix as usize * self.cap.cols as usize;
        let len = self.cap.cols as usize;
        // SAFETY: we've bounds-checked row_ix; cells..cells+cells_cap is owned.
        let slice = unsafe { std::slice::from_raw_parts(self.cells.as_ptr().add(start), len) };
        Some(slice)
    }

    /// Returns a mutable slice to the row at `row_ix`, allocating it if the
    /// row is beyond `rows_used` (but within `rows_cap`). Returns `None` if
    /// the row would exceed `rows_cap`.
    pub fn row_mut(&mut self, row_ix: u16) -> Option<&mut [Cell]> {
        if row_ix >= self.cap.rows_cap {
            return None;
        }
        if row_ix >= self.rows_used {
            self.rows_used = row_ix + 1;
        }
        let start = row_ix as usize * self.cap.cols as usize;
        let len = self.cap.cols as usize;
        // SAFETY: bounds-checked against rows_cap; buffer is owned.
        let slice = unsafe { std::slice::from_raw_parts_mut(self.cells.as_ptr().add(start), len) };
        Some(slice)
    }

    /// Append one row's worth of cells. Returns the row index if the page
    /// still has room, `None` otherwise.
    pub fn push_row(&mut self, row: &[Cell]) -> Option<u16> {
        if self.is_full() {
            return None;
        }
        let ix = self.rows_used;
        let dst = self.row_mut(ix)?;
        let n = row.len().min(dst.len());
        dst[..n].copy_from_slice(&row[..n]);
        // Zero-pad the tail to keep invariants clean.
        if n < dst.len() {
            for c in &mut dst[n..] {
                *c = Cell::EMPTY;
            }
        }
        Some(ix)
    }

    pub fn mark_dirty(&mut self, row_ix: u16) {
        if row_ix < self.cap.rows_cap && row_ix < 64 {
            self.dirty |= 1u64 << row_ix;
        }
    }

    /// Swap two rows within the same page, in place. Both rows are
    /// marked dirty. No-op if either index is out of range.
    pub fn swap_rows_in_page(&mut self, a: u16, b: u16) {
        if a == b {
            return;
        }
        if a >= self.rows_used || b >= self.rows_used {
            return;
        }
        let cols = self.cap.cols as usize;
        unsafe {
            let base = self.cells.as_ptr();
            let ap = base.add(a as usize * cols);
            let bp = base.add(b as usize * cols);
            std::ptr::swap_nonoverlapping(ap, bp, cols);
        }
        self.mark_dirty(a);
        self.mark_dirty(b);
    }

    /// Swap one row from `lo` with one row from `hi`. `lo` and `hi` must
    /// be two *different* pages. Both rows are marked dirty. No-op if
    /// either index is out of range for its page.
    pub fn swap_rows_between(lo: &mut Page, lo_ix: u16, hi: &mut Page, hi_ix: u16) {
        if lo_ix >= lo.rows_used || hi_ix >= hi.rows_used {
            return;
        }
        let cols_lo = lo.cap.cols as usize;
        let cols_hi = hi.cap.cols as usize;
        let cols = cols_lo.min(cols_hi);
        unsafe {
            let ap = lo.cells.as_ptr().add(lo_ix as usize * cols_lo);
            let bp = hi.cells.as_ptr().add(hi_ix as usize * cols_hi);
            std::ptr::swap_nonoverlapping(ap, bp, cols);
        }
        lo.mark_dirty(lo_ix);
        hi.mark_dirty(hi_ix);
    }

    pub fn dirty(&self) -> u64 {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = 0;
    }

    /// Reset the page for reuse by the page pool (scrollback prune).
    ///
    /// This is the key O(1) operation behind the scrollback-prune trick:
    /// instead of deallocating the oldest page, we zero it out and hand it
    /// back to the allocator (see `PageList::prune_head`).
    pub fn reset(&mut self) {
        // SAFETY: write_bytes over owned buffer, zero is a valid Cell.
        unsafe {
            std::ptr::write_bytes(self.cells.as_ptr(), 0, self.cap.cells_cap());
        }
        self.rows_used = 0;
        self.dirty = 0;
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let layout = Self::layout(self.cap);
        // SAFETY: allocated with alloc_zeroed + same layout.
        unsafe { dealloc(self.cells.as_ptr() as *mut u8, layout) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyleId;

    #[test]
    fn capacity_math_is_exact_for_4k_page() {
        let cap = PageCapacity::new(80, 4096);
        assert_eq!(cap.cols, 80);
        // 4096 / 8 = 512 cells → 512 / 80 = 6 rows
        assert_eq!(cap.rows_cap, 6);
        assert_eq!(cap.cells_cap(), 480);
    }

    #[test]
    fn push_row_fills_and_reports_full() {
        let cap = PageCapacity::new(4, 128);
        let mut page = Page::new(cap);
        let row: Vec<Cell> = (0..4)
            .map(|i| Cell::ascii(b'a' + i as u8, CellStyleId(0)))
            .collect();
        while !page.is_full() {
            assert!(page.push_row(&row).is_some());
        }
        assert!(page.is_full());
        assert!(page.push_row(&row).is_none());
    }

    #[test]
    fn row_readback_matches_written() {
        let cap = PageCapacity::new(3, 128);
        let mut page = Page::new(cap);
        let row = [
            Cell::ascii(b'x', CellStyleId(1)),
            Cell::ascii(b'y', CellStyleId(2)),
            Cell::ascii(b'z', CellStyleId(3)),
        ];
        let ix = page.push_row(&row).unwrap();
        let got = page.row(ix).unwrap();
        assert_eq!(got, &row);
    }

    #[test]
    fn reset_clears_everything() {
        let cap = PageCapacity::new(2, 128);
        let mut page = Page::new(cap);
        page.push_row(&[
            Cell::ascii(b'a', CellStyleId(5)),
            Cell::ascii(b'b', CellStyleId(6)),
        ]);
        page.mark_dirty(0);
        assert_eq!(page.rows_used(), 1);
        assert_ne!(page.dirty(), 0);
        page.reset();
        assert_eq!(page.rows_used(), 0);
        assert_eq!(page.dirty(), 0);
        // First row now reads as empty cells even though we never wrote new data.
        let mut page = page; // satisfy borrow checker for next call
        let _ = page.row_mut(0).unwrap(); // materialize row 0
        let row = page.row(0).unwrap();
        for &c in row {
            assert_eq!(c, Cell::EMPTY);
        }
    }
}
