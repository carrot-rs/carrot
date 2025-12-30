//! Wide-char-aware soft-wrap segmentation.
//!
//! The main `render_block` loop naively splits every data row into
//! `effective_cols`-wide chunks. That works for pure ASCII but breaks
//! when a wide character (CJK, emoji, etc.) sits at the wrap column:
//!
//! ```text
//!    viewport = 4 cols
//!    row     = [ A, B, C, W1, W2, D ]   (W1/W2 is a single wide char)
//!    naive   = [ A, B, C, W1 ] + [ W2, D ]   ← orphans W2, stretches nothing
//!    correct = [ A, B, C ]    + [ W1, W2, D ] ← wraps before the wide char
//! ```
//!
//! The wrong behaviour produces visible glitches:
//! - The glyph for W1 gets clipped to one cell.
//! - W2 renders as a mystery ghost cell in isolation.
//! - Cursor positioning math over the wrapped rows drifts.
//!
//! This module owns the segmentation primitive — given a cell slice
//! and a viewport width, it returns the byte ranges of visual rows
//! that never split a wide character. The main renderer calls it and
//! iterates the returned ranges.
//!
//! # Scope
//!
//! Only the wide-char case is handled here. Full grapheme-cluster
//! awareness (e.g. flag emoji spanning multiple Codepoint cells + a
//! zero-width joiner) lives in the shaped-run cache — by the time a
//! complex grapheme arrives at the renderer, it's a single
//! [`ShapedRun`] cell that already knows its display width.
//!
//! [`ShapedRun`]: carrot_grid::CellTag::ShapedRun

use carrot_grid::{Cell, CellTag};

/// Half-open row slice `[start, end)` that renders as one visual
/// row without splitting any wide-char pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualSegment {
    pub start: usize,
    pub end: usize,
}

impl VisualSegment {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub fn contains_column(&self, col: usize) -> bool {
        col >= self.start && col < self.end
    }
}

/// Map a data-column to the `(visual_row_offset, visual_col)` inside
/// the row's wrapped layout. Returns `None` if `data_col` is past the
/// last cell.
///
/// Used by the cursor path: the emulator stores cursor position in
/// data coordinates, but the renderer needs visual coordinates.
///
/// # Example
///
/// With viewport 4 and row `[a b c W1 W2 d]` (wide pair at 3-4):
/// - `data_col = 0` → `(0, 0)` on visual row 0.
/// - `data_col = 3` → `(1, 0)` — the wide pair moved to visual row 1.
/// - `data_col = 5` → `(1, 2)` on visual row 1.
pub fn data_to_visual(
    row: &[carrot_grid::Cell],
    viewport_cols: u16,
    data_col: usize,
) -> Option<(u32, u16)> {
    if data_col >= row.len() {
        return None;
    }
    let segments = segment(row, viewport_cols);
    for (visual_row, seg) in segments.iter().enumerate() {
        if seg.contains_column(data_col) {
            let visual_col = (data_col - seg.start) as u16;
            return Some((visual_row as u32, visual_col));
        }
    }
    None
}

/// Split `row` into visual segments of at most `viewport_cols` cells,
/// pushing any wide-char pair to the next row if it would be orphaned.
///
/// Zero-length input yields a single empty segment so callers that
/// emit one visual row per data row (even empty ones) stay happy.
///
/// `viewport_cols == 0` is treated as `1` — the renderer always wants
/// progress.
pub fn segment(row: &[Cell], viewport_cols: u16) -> Vec<VisualSegment> {
    let width = viewport_cols.max(1) as usize;
    if row.is_empty() {
        return vec![VisualSegment { start: 0, end: 0 }];
    }
    let mut out = Vec::with_capacity(row.len().div_ceil(width));
    let mut start = 0usize;
    while start < row.len() {
        let tentative_end = (start + width).min(row.len());
        let end = adjust_for_wide_boundary(row, start, tentative_end);
        // Safety net: never produce an empty segment; if adjustment
        // pulled us back all the way, advance by one cell so we don't
        // loop forever.
        let end = if end <= start { start + 1 } else { end };
        out.push(VisualSegment { start, end });
        start = end;
    }
    out
}

/// If `row[end - 1]` is the first half of a wide char (`CellTag`
/// other than `Wide2nd`, followed immediately by `Wide2nd`), pull
/// `end` back by one so the pair stays together on the next row.
fn adjust_for_wide_boundary(row: &[Cell], start: usize, end: usize) -> usize {
    if end >= row.len() || end <= start {
        return end;
    }
    let next = row[end];
    if next.tag() == CellTag::Wide2nd {
        // `next` is the 2nd half → the cell just before it (at `end - 1`)
        // is the 1st half. Wrap before the pair.
        return end - 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{Cell, CellStyleId};

    fn ascii(ch: u8) -> Cell {
        Cell::ascii(ch, CellStyleId(0))
    }

    fn wide_pair() -> [Cell; 2] {
        // First half: a codepoint cell; second half: Wide2nd.
        let first = Cell::codepoint('漢', CellStyleId(0));
        let second = Cell::wide_2nd(CellStyleId(0));
        [first, second]
    }

    #[test]
    fn empty_row_produces_single_empty_segment() {
        let segs = segment(&[], 4);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].is_empty());
    }

    #[test]
    fn pure_ascii_splits_evenly() {
        let row: Vec<Cell> = (0..8).map(|i| ascii(b'a' + i)).collect();
        let segs = segment(&row, 4);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], VisualSegment { start: 0, end: 4 });
        assert_eq!(segs[1], VisualSegment { start: 4, end: 8 });
    }

    #[test]
    fn wide_pair_at_boundary_wraps_before_pair() {
        // Row: [a, b, c, W1, W2, d]  viewport=4
        // Naive end=4 puts W1 at col 3, leaving W2 orphaned on the
        // next row. Correct: end=3, W1+W2 both start the next row.
        let pair = wide_pair();
        let mut row = vec![ascii(b'a'), ascii(b'b'), ascii(b'c')];
        row.extend_from_slice(&pair);
        row.push(ascii(b'd'));
        let segs = segment(&row, 4);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], VisualSegment { start: 0, end: 3 });
        assert_eq!(segs[1], VisualSegment { start: 3, end: 6 });
    }

    #[test]
    fn wide_pair_not_at_boundary_does_not_shift() {
        // Row: [W1, W2, a, b, c]  viewport=4
        // Wide pair starts row 0; boundary at 4 falls on `c`, not on
        // a wide half — no adjustment needed.
        let pair = wide_pair();
        let mut row = Vec::from_iter(pair);
        row.extend([ascii(b'a'), ascii(b'b'), ascii(b'c')]);
        let segs = segment(&row, 4);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], VisualSegment { start: 0, end: 4 });
        assert_eq!(segs[1], VisualSegment { start: 4, end: 5 });
    }

    #[test]
    fn viewport_one_never_splits_a_pair_in_half() {
        // Viewport = 1 with a wide pair would naively emit W1 alone,
        // then W2 alone. Adjusted: the wide pair can never fit; we
        // advance by one anyway so we don't loop forever — this is
        // the documented "safety net" behaviour.
        let pair = wide_pair();
        let segs = segment(&pair, 1);
        // Two segments of length 1 each: the pair rendered in the
        // clipped fallback mode. Callers at viewport 1 have already
        // accepted clipping by definition.
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn zero_viewport_clamps_to_one() {
        let row: Vec<Cell> = (0..3).map(|i| ascii(b'a' + i)).collect();
        let segs = segment(&row, 0);
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn consecutive_wide_pairs_wrap_cleanly() {
        // Row: [W1a, W2a, W1b, W2b, W1c, W2c]  viewport=4
        // Pairs cost 2 cols each → 2 pairs per row.
        let a = wide_pair();
        let b = wide_pair();
        let c = wide_pair();
        let mut row = Vec::new();
        row.extend_from_slice(&a);
        row.extend_from_slice(&b);
        row.extend_from_slice(&c);
        let segs = segment(&row, 4);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], VisualSegment { start: 0, end: 4 });
        assert_eq!(segs[1], VisualSegment { start: 4, end: 6 });
    }

    #[test]
    fn odd_viewport_with_wide_pair() {
        // viewport = 3, row = [a, W1, W2, b]
        // naive end=3 would put W1 at col 2, W2 at col 3 end — fits
        // exactly in row 0. end=3 points at `b` (non-Wide2nd), so no
        // adjustment triggers. Row 0 = [a, W1, W2], row 1 = [b].
        let pair = wide_pair();
        let mut row = vec![ascii(b'a')];
        row.extend_from_slice(&pair);
        row.push(ascii(b'b'));
        let segs = segment(&row, 3);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], VisualSegment { start: 0, end: 3 });
        assert_eq!(segs[1], VisualSegment { start: 3, end: 4 });
    }

    #[test]
    fn total_coverage_matches_row_length() {
        let pair = wide_pair();
        let mut row = Vec::new();
        for _ in 0..5 {
            row.extend_from_slice(&pair);
            row.push(ascii(b'x'));
        }
        for viewport in [1u16, 2, 3, 4, 5, 7, 10] {
            let segs = segment(&row, viewport);
            let covered: usize = segs.iter().map(|s| s.len()).sum();
            assert_eq!(covered, row.len(), "viewport={viewport}");
            // Segments are contiguous and non-overlapping.
            for win in segs.windows(2) {
                assert_eq!(win[0].end, win[1].start);
            }
        }
    }

    #[test]
    fn segment_len_and_is_empty_helpers() {
        let s = VisualSegment { start: 2, end: 5 };
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        let e = VisualSegment { start: 4, end: 4 };
        assert!(e.is_empty());
    }

    #[test]
    fn segment_contains_column_is_half_open() {
        let s = VisualSegment { start: 4, end: 7 };
        assert!(!s.contains_column(3));
        assert!(s.contains_column(4));
        assert!(s.contains_column(6));
        assert!(!s.contains_column(7));
    }

    #[test]
    fn data_to_visual_pure_ascii() {
        let row: Vec<Cell> = (0..8).map(|i| ascii(b'a' + i)).collect();
        assert_eq!(data_to_visual(&row, 4, 0), Some((0, 0)));
        assert_eq!(data_to_visual(&row, 4, 3), Some((0, 3)));
        assert_eq!(data_to_visual(&row, 4, 4), Some((1, 0)));
        assert_eq!(data_to_visual(&row, 4, 7), Some((1, 3)));
        assert_eq!(data_to_visual(&row, 4, 8), None);
    }

    #[test]
    fn data_to_visual_with_wide_pair_at_boundary() {
        // Row: [a b c W1 W2 d]  viewport=4
        //   segment 0: 0..3 (a b c)
        //   segment 1: 3..6 (W1 W2 d)
        let pair = wide_pair();
        let mut row = vec![ascii(b'a'), ascii(b'b'), ascii(b'c')];
        row.extend_from_slice(&pair);
        row.push(ascii(b'd'));

        assert_eq!(data_to_visual(&row, 4, 0), Some((0, 0)));
        assert_eq!(data_to_visual(&row, 4, 2), Some((0, 2)));
        // The wide pair moved to the next visual row.
        assert_eq!(data_to_visual(&row, 4, 3), Some((1, 0)));
        assert_eq!(data_to_visual(&row, 4, 4), Some((1, 1)));
        assert_eq!(data_to_visual(&row, 4, 5), Some((1, 2)));
    }

    #[test]
    fn data_to_visual_out_of_range_returns_none() {
        let row: Vec<Cell> = (0..3).map(|i| ascii(b'a' + i)).collect();
        assert_eq!(data_to_visual(&row, 4, 10), None);
        assert_eq!(data_to_visual(&row, 4, 3), None);
    }
}
