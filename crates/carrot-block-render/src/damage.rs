//! Per-cell damage tracking.
//!
//! The renderer's goal is to emit zero draw calls for frames where
//! nothing changed, and to emit draws only for cells that actually
//! differ from what's on screen. Terminal output streams tend to touch
//! a handful of cells per frame even during heavy workloads
//! (e.g. `yes` updates one row; `cargo build` ticks a progress bar) —
//! re-rendering 80×24 cells every frame is a wasteful CPU-GPU bridge.
//!
//! # Data model
//!
//! - [`CellSignature`] — 8 bytes that uniquely identify the visible
//!   state of a cell for damage comparison: `(content, tag, style_id,
//!   flag_bits)`. Two cells with the same signature render identically
//!   and need no redraw.
//! - [`FrameState`] — the set of signatures from the previous frame,
//!   indexed by `(visual_row, visual_col)`. Optional: a fresh renderer
//!   with no history takes a `None` here and draws everything as if it
//!   were new.
//! - [`Damage`] — the result of comparing the new frame's cells against
//!   the previous `FrameState`. Either `Full` (repaint everything) or
//!   `Partial` (a dense bitset of changed cells).
//!
//! # Performance
//!
//! Signature comparison is a single 64-bit equality check per cell.
//! Computing damage for an 80×24 viewport is ~2 k comparisons, which
//! fits in a single cache line sweep and finishes in microseconds. The
//! real payoff is downstream: when damage is sparse, the GPU upload
//! shrinks from ~2 k cells to a handful.

use carrot_grid::{Cell, CellStyleId, CellTag};

/// 8-byte compact identifier of a cell's visible state.
///
/// Derived purely from the cell's rendered payload — no position, no
/// time, no animation state. Two cells sharing a signature render
/// identically and do not need to be redrawn.
///
/// Layout: we already have `Cell` as a packed u64. `CellSignature` just
/// wraps the same bits but renames the semantics ("what was rendered?"
/// vs "what is the cell?") and makes the API intent clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellSignature(u64);

impl CellSignature {
    /// Derive from a `Cell`. The `dirty` flag bit (bit 40) is masked
    /// out — it's housekeeping, not a visual property. Reserved bits
    /// (44..64) are cleared for identity stability across future
    /// Cell-layout additions.
    pub fn from_cell(cell: Cell) -> Self {
        let raw = cell.to_bits();
        let dirty_bit: u64 = 1 << 40;
        let visual_mask: u64 = 0x0FFF_FFFF_FFFF; // bits 0..44
        Self(raw & visual_mask & !dirty_bit)
    }

    /// The underlying bits, for fast equality / hashing.
    pub fn to_bits(self) -> u64 {
        self.0
    }

    /// Soft accessors for debugging and tests.
    pub fn content(self) -> u32 {
        (self.0 & 0x001F_FFFF) as u32
    }

    pub fn tag(self) -> CellTag {
        match ((self.0 >> 21) & 0b111) as u8 {
            0 => CellTag::Ascii,
            1 => CellTag::Codepoint,
            2 => CellTag::Grapheme,
            3 => CellTag::Wide2nd,
            4 => CellTag::Image,
            5 => CellTag::ShapedRun,
            6 => CellTag::CustomRender,
            _ => CellTag::Reserved,
        }
    }

    pub fn style(self) -> CellStyleId {
        CellStyleId(((self.0 >> 24) & 0xFFFF) as u16)
    }
}

/// Signatures of every visible cell from the previous frame.
///
/// Dense row-major layout: `rows × cols` entries, indexed as
/// `row * cols + col`. `rows` and `cols` are the **visual** dimensions
/// (after display-only soft-wrap), not the data dimensions.
#[derive(Clone, Debug, Default)]
pub struct FrameState {
    rows: u32,
    cols: u16,
    cells: Vec<CellSignature>,
}

impl FrameState {
    /// Empty state — equivalent to "nothing has been rendered yet".
    /// Comparing against this returns [`Damage::Full`].
    pub fn empty() -> Self {
        Self::default()
    }

    /// Preallocate for a known viewport.
    pub fn with_viewport(rows: u32, cols: u16) -> Self {
        Self {
            rows,
            cols,
            cells: vec![CellSignature::default(); (rows as usize) * (cols as usize)],
        }
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn get(&self, row: u32, col: u16) -> Option<CellSignature> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let ix = (row as usize) * (self.cols as usize) + (col as usize);
        self.cells.get(ix).copied()
    }

    /// Overwrite all signatures. Used after a successful paint to
    /// record "what's on screen now".
    pub fn replace_cells(&mut self, rows: u32, cols: u16, cells: Vec<CellSignature>) {
        debug_assert_eq!(cells.len(), (rows as usize) * (cols as usize));
        self.rows = rows;
        self.cols = cols;
        self.cells = cells;
    }
}

/// Result of comparing a new frame's cells against a `FrameState`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    /// Every cell is considered changed — the renderer must emit a
    /// full frame. Returned when viewport dimensions changed or when
    /// the caller started with `FrameState::empty()`.
    Full,
    /// A dense bitset (one bit per cell in `rows × cols`) of changed
    /// cells. Index: `row * cols + col`. `Partial` with all bits set
    /// is semantically equivalent to `Full`; the renderer may handle
    /// it either way.
    Partial {
        rows: u32,
        cols: u16,
        dirty: Vec<u64>,
    },
}

impl Damage {
    /// True if this damage marks the given `(row, col)` cell as needing
    /// redraw. `Full` always returns true.
    pub fn contains(&self, row: u32, col: u16) -> bool {
        match self {
            Damage::Full => true,
            Damage::Partial { cols, dirty, .. } => {
                let ix = (row as usize) * (*cols as usize) + (col as usize);
                let word_ix = ix / 64;
                let bit_ix = ix % 64;
                dirty
                    .get(word_ix)
                    .map(|w| w & (1u64 << bit_ix) != 0)
                    .unwrap_or(false)
            }
        }
    }

    /// How many cells are marked dirty. `Full` reports `rows * cols`.
    pub fn dirty_count(&self, total_cells: usize) -> usize {
        match self {
            Damage::Full => total_cells,
            Damage::Partial { dirty, .. } => dirty.iter().map(|w| w.count_ones() as usize).sum(),
        }
    }

    /// Returns `true` if nothing needs redraw.
    pub fn is_clean(&self) -> bool {
        match self {
            Damage::Full => false,
            Damage::Partial { dirty, .. } => dirty.iter().all(|&w| w == 0),
        }
    }
}

/// Compute damage by diffing a fresh array of signatures against the
/// previous `FrameState`. `new_cells` is row-major
/// (`row * cols + col`), `rows × cols`.
///
/// Fast path: when dimensions differ from `prev`, returns [`Damage::Full`].
/// Otherwise does a single pass over `new_cells` with 64-bit compares.
pub fn compute_damage(
    prev: &FrameState,
    new_cells: &[CellSignature],
    rows: u32,
    cols: u16,
) -> Damage {
    let total = (rows as usize) * (cols as usize);
    debug_assert_eq!(new_cells.len(), total);
    if prev.rows != rows || prev.cols != cols || prev.cells.len() != total {
        return Damage::Full;
    }
    let words = total.div_ceil(64);
    let mut dirty = vec![0u64; words];
    for (i, (&a, &b)) in prev.cells.iter().zip(new_cells.iter()).enumerate() {
        if a != b {
            dirty[i / 64] |= 1u64 << (i % 64);
        }
    }
    Damage::Partial { rows, cols, dirty }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::Cell;

    fn sig(c: u8, style: u16) -> CellSignature {
        CellSignature::from_cell(Cell::ascii(c, CellStyleId(style)))
    }

    #[test]
    fn signature_ignores_dirty_bit() {
        let a = Cell::ascii(b'x', CellStyleId(3));
        let b = a.with_dirty(true);
        assert_eq!(CellSignature::from_cell(a), CellSignature::from_cell(b));
    }

    #[test]
    fn signature_tracks_content_style_and_tag() {
        let a = sig(b'a', 1);
        let b = sig(b'b', 1);
        let c = sig(b'a', 2);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.content(), b'a' as u32);
        assert_eq!(a.style().0, 1);
        assert_eq!(a.tag(), CellTag::Ascii);
    }

    #[test]
    fn empty_state_implies_full_damage() {
        let prev = FrameState::empty();
        let new: Vec<_> = (0..6).map(|i| sig(b'a' + i, 0)).collect();
        let dmg = compute_damage(&prev, &new, 2, 3);
        assert_eq!(dmg, Damage::Full);
    }

    #[test]
    fn identical_cells_produce_no_damage() {
        let cells: Vec<_> = (0..6).map(|i| sig(b'a' + i, 0)).collect();
        let mut prev = FrameState::with_viewport(2, 3);
        prev.replace_cells(2, 3, cells.clone());
        let dmg = compute_damage(&prev, &cells, 2, 3);
        assert!(dmg.is_clean());
        assert_eq!(dmg.dirty_count(6), 0);
    }

    #[test]
    fn one_changed_cell_marks_exactly_one_bit() {
        let a: Vec<_> = (0..6).map(|i| sig(b'a' + i, 0)).collect();
        let mut b = a.clone();
        b[3] = sig(b'Z', 0);

        let mut prev = FrameState::with_viewport(2, 3);
        prev.replace_cells(2, 3, a);

        let dmg = compute_damage(&prev, &b, 2, 3);
        assert!(dmg.contains(1, 0)); // row 1, col 0 → index 3
        assert_eq!(dmg.dirty_count(6), 1);
        assert!(!dmg.contains(0, 0));
        assert!(!dmg.contains(0, 1));
    }

    #[test]
    fn style_change_is_detected() {
        let a = vec![sig(b'x', 0); 4];
        let mut b = a.clone();
        b[2] = sig(b'x', 5); // same char, different style

        let mut prev = FrameState::with_viewport(2, 2);
        prev.replace_cells(2, 2, a);

        let dmg = compute_damage(&prev, &b, 2, 2);
        assert!(dmg.contains(1, 0));
        assert_eq!(dmg.dirty_count(4), 1);
    }

    #[test]
    fn viewport_resize_falls_back_to_full() {
        let a = vec![sig(b'x', 0); 4];
        let mut prev = FrameState::with_viewport(2, 2);
        prev.replace_cells(2, 2, a);

        let b = vec![sig(b'x', 0); 9];
        let dmg = compute_damage(&prev, &b, 3, 3);
        assert_eq!(dmg, Damage::Full);
    }

    #[test]
    fn damage_contains_respects_bounds() {
        let dmg = Damage::Partial {
            rows: 2,
            cols: 3,
            dirty: vec![0b0000_0100], // only bit 2 set → row 0, col 2
        };
        assert!(!dmg.contains(0, 0));
        assert!(!dmg.contains(0, 1));
        assert!(dmg.contains(0, 2));
        assert!(!dmg.contains(1, 0));
    }

    #[test]
    fn many_cells_benchmark_shape() {
        // Simulate an 80×24 viewport with one changed cell at the
        // middle — a realistic incremental update. Exercises the
        // multi-word bitset path.
        let rows = 24u32;
        let cols = 80u16;
        let total = (rows as usize) * (cols as usize);
        let a: Vec<_> = (0..total).map(|i| sig((i % 256) as u8, 0)).collect();
        let mut b = a.clone();
        b[total / 2] = sig(b'*', 9);

        let mut prev = FrameState::with_viewport(rows, cols);
        prev.replace_cells(rows, cols, a);

        let dmg = compute_damage(&prev, &b, rows, cols);
        assert_eq!(dmg.dirty_count(total), 1);
    }
}
