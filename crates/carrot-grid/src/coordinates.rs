//! `GridBounds` — die einzige zulässige Koordinaten- und Iterations-API
//! für [`PageList`]-basierte Grids.
//!
//! Vor `GridBounds` haben drei verschiedene Stellen unabhängig über
//! Block-Grids iteriert (Snapshot-Extraction, Selection-Materialisierung,
//! Search) — jede mit eigener `row_offset`-Arithmetik, eigenen
//! Bounds-Checks und eigener Stale-`content_rows`-Anfälligkeit.
//! Resultat: Resize→Panic-Klasse, weil mindestens eine Iterationsstelle
//! keinen `.min()`-Hotfix hatte.
//!
//! Diese Datei zentralisiert das in einen Typ mit drei Verantwortungen:
//!
//! 1. **Frische Bounds** — `from_pages()` zieht `total_rows`,
//!    `first_row_offset` und `cols` zur Konstruktionszeit aus dem
//!    [`PageList`]. Niemand liest gecachte `content_rows`-Werte.
//! 2. **Bounds-Checked Konvertierungen** — `row_to_origin`,
//!    `origin_to_row`, `pixel_to_row`, `clamped_range` returnen
//!    `Option`/clamped — keine Panics, keine Underflows.
//! 3. **Iteration** — `iter(pages)` / `iter_range(pages, range)` sind
//!    die einzigen Wege, über Block-Rows zu iterieren. Sie liefern
//!    [`RowAddr`]-Tupel mit Index *und* absolutem `CellId`-Origin, so
//!    dass Caller nie selbst `first_row_offset + index` rechnen.
//!
//! `PageList::rows()` bleibt als Low-Level-Primitive für `GridBounds`'s
//! Implementierung; produktive Konsumenten gehen durch `GridBounds`.

use std::ops::Range;

use crate::Cell;
use crate::page_list::PageList;

/// Validierte Block-Dimensionen + Iterations-API.
///
/// Erstellt aus einem [`PageList`] über [`GridBounds::from_pages`]. Die
/// Werte werden zur Konstruktionszeit gezogen — also **frisch**, nie aus
/// einem gecachten Block-Feld. Eine `GridBounds`-Instanz reflektiert den
/// Stand zum Zeitpunkt der Konstruktion; wer den `PageList`
/// zwischendrin mutiert, baut eine neue Instanz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridBounds {
    total_rows: usize,
    first_row_offset: u64,
    columns: u16,
}

/// Adresse einer Row in einem Block — Index relativ zum aktuellen
/// `PageList` *und* absoluter [`crate::CellId`]-Origin (überlebt
/// Page-Pruning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowAddr {
    /// Position in `[0, total_rows)`.
    pub index: usize,
    /// Absoluter Origin = `first_row_offset + index`. Stable über
    /// Pruning hinweg — `CellId` baut darauf auf.
    pub origin: u64,
}

impl GridBounds {
    /// Frischer Snapshot der Bounds eines [`PageList`]. Liest
    /// `total_rows` / `first_row_offset` / `cols` direkt — kein Caching.
    pub fn from_pages(pages: &PageList) -> Self {
        let cap = pages.capacity();
        Self {
            total_rows: pages.total_rows(),
            first_row_offset: pages.first_row_offset(),
            columns: cap.cols,
        }
    }

    /// Anzahl der aktuell gespeicherten Rows.
    #[inline]
    pub fn total_rows(&self) -> usize {
        self.total_rows
    }

    /// Spaltenanzahl pro Row.
    #[inline]
    pub fn columns(&self) -> u16 {
        self.columns
    }

    /// Prune-Offset des ersten Rows. Wird gebraucht für stable
    /// `CellId`-Konvertierungen — `iter`/`iter_range` machen das aber
    /// schon implizit, externe Caller brauchen das selten.
    #[inline]
    pub fn first_row_offset(&self) -> u64 {
        self.first_row_offset
    }

    /// `true` wenn der Block keine Rows hat.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_rows == 0
    }

    /// Den vollen Iterations-Range (`0..total_rows`).
    #[inline]
    pub fn full_range(&self) -> Range<usize> {
        0..self.total_rows
    }

    /// Bounds-checked Range — beide Endpoints werden auf `[0, total_rows]`
    /// geclamped, ein leerer Range kommt zurück wenn `start >= end` nach
    /// dem Clampen oder bei reversed input.
    pub fn clamped_range(&self, range: Range<usize>) -> Range<usize> {
        let start = range.start.min(self.total_rows);
        let end = range.end.min(self.total_rows);
        if start > end { 0..0 } else { start..end }
    }

    /// Row-Index → absoluter `CellId`-Origin. `None` wenn out-of-bounds.
    #[inline]
    pub fn row_to_origin(&self, row: usize) -> Option<u64> {
        if row >= self.total_rows {
            None
        } else {
            Some(self.first_row_offset + row as u64)
        }
    }

    /// Absoluter `CellId`-Origin → Row-Index. `None` wenn der Origin
    /// vor `first_row_offset` liegt (= geprunt) oder nach `total_rows`.
    #[inline]
    pub fn origin_to_row(&self, origin: u64) -> Option<usize> {
        let rel = origin.checked_sub(self.first_row_offset)? as usize;
        if rel >= self.total_rows {
            None
        } else {
            Some(rel)
        }
    }

    /// Pixel-Y → Row-Index, geclamped an `[0, total_rows.saturating_sub(1)]`.
    /// Hit-Testing-Pfad — leerer Block oder ungültige `cell_height`
    /// returnen `0`, das ist immer ein gültiger No-Op.
    pub fn pixel_to_row(&self, y: f32, cell_height: f32) -> usize {
        if cell_height <= 0.0 || self.total_rows == 0 {
            return 0;
        }
        let row = (y / cell_height).max(0.0) as usize;
        row.min(self.total_rows.saturating_sub(1))
    }

    /// Iteriert über alle Rows mit ihrer absoluten Adresse. Die
    /// kanonische Block-Iteration im gesamten Code.
    pub fn iter<'a>(
        &self,
        pages: &'a PageList,
    ) -> impl Iterator<Item = (RowAddr, &'a [Cell])> + 'a {
        self.iter_range(pages, self.full_range())
    }

    /// Range-bounded Iteration. Range wird auf `[0, total_rows)`
    /// geclamped; reversed/out-of-bounds Inputs liefern einen leeren
    /// Iterator statt zu paniken.
    pub fn iter_range<'a>(
        &self,
        pages: &'a PageList,
        range: Range<usize>,
    ) -> impl Iterator<Item = (RowAddr, &'a [Cell])> + 'a {
        let clamped = self.clamped_range(range);
        let first_row_offset = self.first_row_offset;
        let start = clamped.start;
        pages
            .rows(clamped.start, clamped.end)
            .enumerate()
            .map(move |(off, row)| {
                let index = start + off;
                (
                    RowAddr {
                        index,
                        origin: first_row_offset + index as u64,
                    },
                    row,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, PageCapacity};

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
    fn empty_pages_yield_empty_bounds() {
        let pages = make_pages(80, 0);
        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.total_rows(), 0);
        assert_eq!(b.full_range(), 0..0);
        assert!(b.is_empty());
        assert_eq!(b.row_to_origin(0), None);
        assert_eq!(b.iter(&pages).count(), 0);
    }

    #[test]
    fn freshly_built_bounds_match_pages() {
        let pages = make_pages(80, 12);
        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.total_rows(), 12);
        assert_eq!(b.columns(), 80);
        assert_eq!(b.first_row_offset(), 0);
        assert_eq!(b.full_range(), 0..12);
    }

    #[test]
    fn row_to_origin_round_trips() {
        let pages = make_pages(40, 5);
        let b = GridBounds::from_pages(&pages);
        for row in 0..5 {
            let origin = b.row_to_origin(row).expect("in range");
            assert_eq!(b.origin_to_row(origin), Some(row));
        }
        assert_eq!(b.row_to_origin(5), None);
        assert_eq!(b.row_to_origin(99), None);
    }

    #[test]
    fn origin_to_row_after_prune() {
        let cap = PageCapacity::new(20, 4096);
        let rows_per_page = cap.rows_cap as usize;
        let total_rows = rows_per_page * 2 + 5;
        let blank: Vec<Cell> = vec![Cell::default(); 20];
        let mut pages = PageList::new(cap);
        for _ in 0..total_rows {
            pages.append_row(&blank);
        }

        let pruned = pages.prune_head().expect("head page existed");
        let pruned_rows = pruned as u64;

        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.first_row_offset(), pruned_rows);
        assert_eq!(b.total_rows(), total_rows - pruned_rows as usize);
        assert_eq!(b.origin_to_row(0), None);
        assert_eq!(b.origin_to_row(pruned_rows - 1), None);
        assert_eq!(b.origin_to_row(pruned_rows), Some(0));
        let last_origin = (total_rows as u64) - 1;
        assert_eq!(b.origin_to_row(last_origin), Some(b.total_rows() - 1));
        assert_eq!(b.origin_to_row(total_rows as u64), None);
    }

    #[test]
    fn clamped_range_never_panics() {
        let pages = make_pages(20, 5);
        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.clamped_range(0..5), 0..5);
        assert_eq!(b.clamped_range(2..3), 2..3);
        assert_eq!(b.clamped_range(0..100), 0..5);
        assert_eq!(b.clamped_range(99..200), 5..5);
        // Reversed range — built via field syntax so the empty-range
        // lint doesn't trip on a literal. The point of this assertion
        // is that the API stays defensive against malformed input.
        assert_eq!(b.clamped_range(Range { start: 8, end: 2 }), 0..0);
    }

    #[test]
    fn pixel_to_row_clamps_to_last_row() {
        let pages = make_pages(40, 8);
        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.pixel_to_row(0.0, 16.0), 0);
        assert_eq!(b.pixel_to_row(15.9, 16.0), 0);
        assert_eq!(b.pixel_to_row(16.0, 16.0), 1);
        assert_eq!(b.pixel_to_row(112.0, 16.0), 7);
        assert_eq!(b.pixel_to_row(9999.0, 16.0), 7);
        assert_eq!(b.pixel_to_row(-50.0, 16.0), 0);
    }

    #[test]
    fn pixel_to_row_handles_empty_block_and_zero_height() {
        let pages_empty = make_pages(40, 0);
        let b_empty = GridBounds::from_pages(&pages_empty);
        assert_eq!(b_empty.pixel_to_row(120.0, 16.0), 0);

        let pages = make_pages(40, 4);
        let b = GridBounds::from_pages(&pages);
        assert_eq!(b.pixel_to_row(50.0, 0.0), 0);
        assert_eq!(b.pixel_to_row(50.0, -1.0), 0);
    }

    #[test]
    fn iter_yields_address_plus_row_data() {
        let pages = make_pages(8, 4);
        let b = GridBounds::from_pages(&pages);
        let collected: Vec<(usize, u64, usize)> = b
            .iter(&pages)
            .map(|(addr, row)| (addr.index, addr.origin, row.len()))
            .collect();
        assert_eq!(collected, vec![(0, 0, 8), (1, 1, 8), (2, 2, 8), (3, 3, 8)]);
    }

    #[test]
    fn iter_address_origins_survive_prune() {
        let cap = PageCapacity::new(8, 4096);
        let rows_per_page = cap.rows_cap as usize;
        let total = rows_per_page * 2;
        let blank: Vec<Cell> = vec![Cell::default(); 8];
        let mut pages = PageList::new(cap);
        for _ in 0..total {
            pages.append_row(&blank);
        }
        let pruned = pages.prune_head().expect("head page existed") as u64;
        let b = GridBounds::from_pages(&pages);

        let mut count = 0u64;
        for (addr, _) in b.iter(&pages) {
            assert_eq!(addr.origin, pruned + count);
            assert_eq!(addr.index, count as usize);
            count += 1;
        }
        assert_eq!(count, b.total_rows() as u64);
    }

    #[test]
    fn iter_range_clamps_silently() {
        let pages = make_pages(4, 6);
        let b = GridBounds::from_pages(&pages);

        let in_range: Vec<usize> = b.iter_range(&pages, 1..4).map(|(a, _)| a.index).collect();
        assert_eq!(in_range, vec![1, 2, 3]);

        let over: Vec<usize> = b.iter_range(&pages, 4..99).map(|(a, _)| a.index).collect();
        assert_eq!(over, vec![4, 5]);

        let empty: Vec<usize> = b
            .iter_range(&pages, 99..200)
            .map(|(a, _)| a.index)
            .collect();
        assert!(empty.is_empty());

        // Same defensive-API check as above; field syntax avoids the
        // empty-range lint on the literal form.
        let reversed: Vec<usize> = b
            .iter_range(&pages, Range { start: 5, end: 2 })
            .map(|(a, _)| a.index)
            .collect();
        assert!(reversed.is_empty());
    }

    #[test]
    fn bounds_remain_consistent_after_simulated_resize() {
        let mut pages = make_pages(40, 5);
        let before = GridBounds::from_pages(&pages);
        assert_eq!(before.total_rows(), 5);

        let blank: Vec<Cell> = vec![Cell::default(); 40];
        for _ in 0..3 {
            pages.append_row(&blank);
        }

        let after = GridBounds::from_pages(&pages);
        assert_eq!(after.total_rows(), 8);
        assert_eq!(after.full_range(), 0..8);
        // The earlier instance is unchanged — but no consumer should be
        // reusing it after a mutation.
        assert_eq!(before.total_rows(), 5);
    }
}
