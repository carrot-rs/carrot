//! `BlockSnapshot` — owned, UI-free copy of a block's grid data.
//!
//! Sits in Layer 1 (carrot-grid) because it carries pure cell + style
//! data and the [`GridBounds`] that describe them. Layer 2
//! (carrot-term) ships [`PageList`]-backed live blocks; Layer 4
//! (carrot-block-render) feeds these snapshots into GPU passes; future
//! consumers (search export, AI context, screenshot pipelines) only
//! need this one type. None of those callers should pay for a Layer-4
//! dependency just to extract block data.
//!
//! The snapshot is **raw** — cells carry [`crate::Color`] tags, not
//! resolved Oklch. Color resolution lives in `carrot-block-render::palette`
//! (or any other consumer that knows about themes); the snapshot is
//! theme-agnostic by construction.

use crate::Cell;
use crate::coordinates::GridBounds;
use crate::page_list::PageList;
use crate::style::CellStyle;

/// Owned per-frame snapshot of a block's grid data.
///
/// Equivalent to a flattened `(GridBounds, Vec<Vec<Cell>>, Vec<CellStyle>)`
/// — the bounds are built fresh from the source `PageList` at construction
/// time and travel together with the cells, so consumers can iterate the
/// snapshot via `bounds` without re-checking the source. This is the API
/// every block-data consumer (renderer, search export, AI context) takes.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSnapshot {
    /// Validated dimensions of the snapshot at construction time.
    /// `bounds.total_rows()` always matches `rows.len()`.
    pub bounds: GridBounds,
    /// Rows of cells in data order. Each row is `bounds.columns()` long
    /// (the source `PageList` enforces fixed-width rows).
    pub rows: Vec<Vec<Cell>>,
    /// Cell-style atlas indexed by [`crate::CellStyleId`]. Index 0 must
    /// be the default style — matches [`crate::CellStyleAtlas`] invariants.
    pub atlas: Vec<CellStyle>,
}

impl BlockSnapshot {
    /// Empty snapshot — zero-row block, default-style atlas. Useful for
    /// "no active block" placeholders without `Option<BlockSnapshot>`.
    pub fn empty() -> Self {
        Self {
            bounds: GridBounds::from_pages(&PageList::new(crate::PageCapacity::new(1, 64))),
            rows: Vec::new(),
            atlas: vec![CellStyle::DEFAULT],
        }
    }

    /// Build from already-owned row data and a pre-fabricated bounds.
    /// Used when the caller already has the rows materialized (e.g.
    /// from `RenderView`) and wants to wrap them with bounds + atlas.
    pub fn from_owned(bounds: GridBounds, rows: Vec<Vec<Cell>>, atlas: Vec<CellStyle>) -> Self {
        debug_assert_eq!(
            bounds.total_rows(),
            rows.len(),
            "BlockSnapshot::from_owned: bounds.total_rows() must match rows.len()"
        );
        Self {
            bounds,
            rows,
            atlas,
        }
    }

    /// Walk a [`PageList`] and an atlas slice, cloning into an owned
    /// snapshot. The bounds are built fresh from `pages`, so resize-stale
    /// `content_rows` can't leak in.
    pub fn from_pages(pages: &PageList, atlas: &[CellStyle]) -> Self {
        let bounds = GridBounds::from_pages(pages);
        let mut rows = Vec::with_capacity(bounds.total_rows());
        for (_, row) in bounds.iter(pages) {
            rows.push(row.to_vec());
        }
        Self {
            bounds,
            rows,
            atlas: atlas.to_vec(),
        }
    }

    /// Look up a style by id, falling back to the default style for
    /// unknown ids — matches the renderer's resolution semantics.
    pub fn style(&self, id: crate::CellStyleId) -> CellStyle {
        self.atlas
            .get(id.0 as usize)
            .copied()
            .unwrap_or(CellStyle::DEFAULT)
    }

    /// Convenience: total rows in the snapshot.
    #[inline]
    pub fn total_rows(&self) -> usize {
        self.bounds.total_rows()
    }

    /// Convenience: column count.
    #[inline]
    pub fn columns(&self) -> u16 {
        self.bounds.columns()
    }

    /// Convenience: borrow a single row by index.
    #[inline]
    pub fn row(&self, ix: usize) -> Option<&[Cell]> {
        self.rows.get(ix).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, CellStyleAtlas, PageCapacity};

    fn make_pages(cols: u16, rows: usize) -> PageList {
        let cap = PageCapacity::new(cols, 4096);
        let mut pages = PageList::new(cap);
        let blank: Vec<Cell> = vec![Cell::default(); cols as usize];
        for _ in 0..rows {
            pages.append_row(&blank);
        }
        pages
    }

    #[test]
    fn empty_snapshot_has_default_atlas() {
        let s = BlockSnapshot::empty();
        assert_eq!(s.total_rows(), 0);
        assert_eq!(s.atlas.len(), 1);
        assert_eq!(s.atlas[0], CellStyle::DEFAULT);
    }

    #[test]
    fn from_pages_round_trips_dimensions() {
        let pages = make_pages(40, 6);
        let atlas = CellStyleAtlas::new();
        let s = BlockSnapshot::from_pages(&pages, atlas.as_slice());
        assert_eq!(s.total_rows(), 6);
        assert_eq!(s.columns(), 40);
        assert_eq!(s.rows.len(), 6);
        for row in &s.rows {
            assert_eq!(row.len(), 40);
        }
    }

    #[test]
    fn from_owned_validates_bounds_match() {
        let pages = make_pages(10, 3);
        let bounds = GridBounds::from_pages(&pages);
        let rows: Vec<Vec<Cell>> = (0..3).map(|_| vec![Cell::default(); 10]).collect();
        let atlas = vec![CellStyle::DEFAULT];
        let s = BlockSnapshot::from_owned(bounds, rows, atlas);
        assert_eq!(s.total_rows(), 3);
    }

    #[test]
    fn style_lookup_falls_back_to_default() {
        let pages = make_pages(8, 1);
        let atlas: Vec<CellStyle> = vec![CellStyle::DEFAULT];
        let s = BlockSnapshot::from_pages(&pages, &atlas);
        // Unknown id → default
        assert_eq!(s.style(crate::CellStyleId(99)), CellStyle::DEFAULT);
        // In-range id → that style
        assert_eq!(s.style(crate::CellStyleId(0)), CellStyle::DEFAULT);
    }
}
