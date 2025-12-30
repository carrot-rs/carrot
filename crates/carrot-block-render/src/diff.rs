//! Cell-for-cell grid diff (Diff-Rerun).
//!
//! When a user re-runs a command (often via Ctrl-R + Enter or a
//! dedicated "rerun this block" action), the natural question is
//! *what changed vs last time?* This module compares two grids
//! cell-by-cell and surfaces the delta for the consumer to render
//! as inline highlights — green for new, red for removed, yellow
//! for changed.
//!
//! The "before" grid is reconstructed from the previous block's
//! `ReplayBuffer` bytes (in `carrot-term`), the "after" grid is
//! the freshly-collected new output. Both are just `Vec<Vec<Cell>>`,
//! which is what this diffs.
//!
//! The diff is **cell-level**, not line-level. Two identical lines
//! that move up by one row (because `seq` emitted one extra line)
//! would appear as 80-cells-changed in a naive diff; real-world
//! rerun-diffs are almost always append-only (new lines at the
//! bottom, same content above) so this simple diff does the right
//! thing for the common case. Line-diff (Myers / patience) is a
//! later refinement; the primitive here is the groundwork.

use carrot_grid::Cell;

/// One delta entry in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffEntry {
    /// Cell exists in both grids and is different.
    Changed {
        row: usize,
        col: usize,
        before: Cell,
        after: Cell,
    },
    /// Cell exists in "after" but not in "before" (new row / new
    /// trailing cell).
    Added { row: usize, col: usize, after: Cell },
    /// Cell exists in "before" but not in "after" (truncated).
    Removed {
        row: usize,
        col: usize,
        before: Cell,
    },
}

impl DiffEntry {
    pub fn row(&self) -> usize {
        match self {
            DiffEntry::Changed { row, .. } => *row,
            DiffEntry::Added { row, .. } => *row,
            DiffEntry::Removed { row, .. } => *row,
        }
    }

    pub fn col(&self) -> usize {
        match self {
            DiffEntry::Changed { col, .. } => *col,
            DiffEntry::Added { col, .. } => *col,
            DiffEntry::Removed { col, .. } => *col,
        }
    }
}

/// Full diff between two grids. Stores entries in row-major order
/// for easy render-pass iteration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridDiff {
    pub entries: Vec<DiffEntry>,
}

impl GridDiff {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Count of Changed entries.
    pub fn changed_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, DiffEntry::Changed { .. }))
            .count()
    }

    /// Count of Added entries.
    pub fn added_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, DiffEntry::Added { .. }))
            .count()
    }

    /// Count of Removed entries.
    pub fn removed_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, DiffEntry::Removed { .. }))
            .count()
    }
}

/// Diff two grids. Both `before` and `after` are row-major
/// `Vec<Vec<Cell>>`. Rows can have different widths per row; the
/// diff does column-wise pairing up to `max(a.len(), b.len())`.
pub fn diff_grids(before: &[Vec<Cell>], after: &[Vec<Cell>]) -> GridDiff {
    let mut entries = Vec::new();
    let max_rows = before.len().max(after.len());
    for r in 0..max_rows {
        match (before.get(r), after.get(r)) {
            (Some(b), Some(a)) => diff_row(r, b, a, &mut entries),
            (None, Some(a)) => {
                for (c, &cell) in a.iter().enumerate() {
                    entries.push(DiffEntry::Added {
                        row: r,
                        col: c,
                        after: cell,
                    });
                }
            }
            (Some(b), None) => {
                for (c, &cell) in b.iter().enumerate() {
                    entries.push(DiffEntry::Removed {
                        row: r,
                        col: c,
                        before: cell,
                    });
                }
            }
            (None, None) => {} // unreachable given max_rows bound
        }
    }
    GridDiff { entries }
}

fn diff_row(row: usize, before: &[Cell], after: &[Cell], out: &mut Vec<DiffEntry>) {
    let max_cols = before.len().max(after.len());
    for c in 0..max_cols {
        match (before.get(c), after.get(c)) {
            (Some(&b), Some(&a)) => {
                if b != a {
                    out.push(DiffEntry::Changed {
                        row,
                        col: c,
                        before: b,
                        after: a,
                    });
                }
            }
            (None, Some(&a)) => out.push(DiffEntry::Added {
                row,
                col: c,
                after: a,
            }),
            (Some(&b), None) => out.push(DiffEntry::Removed {
                row,
                col: c,
                before: b,
            }),
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::CellStyleId;

    fn ascii_row(s: &[u8]) -> Vec<Cell> {
        s.iter().map(|&c| Cell::ascii(c, CellStyleId(0))).collect()
    }

    #[test]
    fn identical_grids_produce_empty_diff() {
        let g = vec![ascii_row(b"hello"), ascii_row(b"world")];
        let diff = diff_grids(&g, &g);
        assert!(diff.is_empty());
    }

    #[test]
    fn single_cell_change_yields_single_changed_entry() {
        let before = vec![ascii_row(b"hello")];
        let after = vec![ascii_row(b"hEllo")];
        let diff = diff_grids(&before, &after);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff.changed_count(), 1);
        let DiffEntry::Changed {
            row,
            col,
            before: b,
            after: a,
        } = diff.entries[0]
        else {
            panic!("expected Changed entry");
        };
        assert_eq!((row, col), (0, 1));
        assert_eq!(b.content(), b'e' as u32);
        assert_eq!(a.content(), b'E' as u32);
    }

    #[test]
    fn trailing_row_appended_is_all_added() {
        let before = vec![ascii_row(b"foo")];
        let after = vec![ascii_row(b"foo"), ascii_row(b"bar")];
        let diff = diff_grids(&before, &after);
        assert_eq!(diff.len(), 3); // 3 cells in "bar"
        assert_eq!(diff.added_count(), 3);
        assert_eq!(diff.removed_count(), 0);
    }

    #[test]
    fn trailing_row_removed_is_all_removed() {
        let before = vec![ascii_row(b"foo"), ascii_row(b"bar")];
        let after = vec![ascii_row(b"foo")];
        let diff = diff_grids(&before, &after);
        assert_eq!(diff.len(), 3);
        assert_eq!(diff.removed_count(), 3);
        assert_eq!(diff.added_count(), 0);
    }

    #[test]
    fn rows_of_different_widths_diff_cell_wise() {
        let before = vec![ascii_row(b"hi")];
        let after = vec![ascii_row(b"hi there")];
        let diff = diff_grids(&before, &after);
        // "hi" is unchanged; the trailing " there" is added.
        assert_eq!(diff.len(), 6);
        assert_eq!(diff.added_count(), 6);
        let expected_cols: Vec<_> = diff.entries.iter().map(|e| e.col()).collect();
        assert_eq!(expected_cols, vec![2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn empty_before_all_added() {
        let after = vec![ascii_row(b"hello")];
        let diff = diff_grids(&[], &after);
        assert_eq!(diff.len(), 5);
        assert_eq!(diff.added_count(), 5);
    }

    #[test]
    fn empty_after_all_removed() {
        let before = vec![ascii_row(b"hello")];
        let diff = diff_grids(&before, &[]);
        assert_eq!(diff.len(), 5);
        assert_eq!(diff.removed_count(), 5);
    }

    #[test]
    fn both_empty_is_empty_diff() {
        let diff = diff_grids(&[], &[]);
        assert!(diff.is_empty());
    }

    #[test]
    fn changed_plus_added_counts_separately() {
        let before = vec![ascii_row(b"abc")];
        let after = vec![ascii_row(b"aXcdef")];
        let diff = diff_grids(&before, &after);
        assert_eq!(diff.changed_count(), 1); // 'b' → 'X'
        assert_eq!(diff.added_count(), 3); // 'd', 'e', 'f'
        assert_eq!(diff.len(), 4);
    }

    #[test]
    fn style_change_counts_as_changed() {
        // Same content, different style id ⇒ different cell ⇒ Changed.
        let a = vec![vec![Cell::ascii(b'x', CellStyleId(1))]];
        let b = vec![vec![Cell::ascii(b'x', CellStyleId(2))]];
        let diff = diff_grids(&a, &b);
        assert_eq!(diff.changed_count(), 1);
    }

    #[test]
    fn diff_entries_preserve_row_order() {
        let before = vec![ascii_row(b"a"), ascii_row(b"b"), ascii_row(b"c")];
        let after = vec![ascii_row(b"A"), ascii_row(b"B"), ascii_row(b"C")];
        let diff = diff_grids(&before, &after);
        let rows: Vec<_> = diff.entries.iter().map(|e| e.row()).collect();
        assert_eq!(rows, vec![0, 1, 2]);
    }
}
