//! Per-grid tabstop table.
//!
//! Terminal tabstops are a per-grid concept (HT / CSI W / ESC H all
//! consult or mutate the same table), so the natural home is next to
//! `PageList`. Pure data — the interpretation of `HT` lives in the
//! VT writer.
//!
//! Default stride follows terminfo `it`: one stop every 8 columns.

use std::ops::{Index, IndexMut};

/// Default columns between initial tabstops — matches terminfo `it`.
pub const INITIAL_TABSTOPS: usize = 8;

/// Tabstop table backing a single grid.
///
/// `tabs[col] == true` means there is a stop at `col`. Columns are
/// 0-based. Resizing the grid resizes the table; columns added at
/// the right inherit the default stride.
#[derive(Debug, Clone)]
pub struct TabStops {
    tabs: Vec<bool>,
}

impl TabStops {
    /// Fresh table of `columns` wide with default tabstops every
    /// `INITIAL_TABSTOPS` (8) columns. Column 0 is a stop.
    #[inline]
    pub fn new(columns: usize) -> TabStops {
        TabStops {
            tabs: (0..columns).map(|i| i % INITIAL_TABSTOPS == 0).collect(),
        }
    }

    /// Remove every stop. `HT` after this is effectively a no-op
    /// until stops are re-added by the writer.
    #[inline]
    pub fn clear_all(&mut self) {
        for tab in &mut self.tabs {
            *tab = false;
        }
    }

    /// Resize the table to `columns`. New columns at the right get
    /// the default stride; columns removed at the right drop their
    /// stops without affecting earlier columns.
    #[inline]
    pub fn resize(&mut self, columns: usize) {
        let mut index = self.tabs.len();
        self.tabs.resize_with(columns, || {
            let is_tabstop = index.is_multiple_of(INITIAL_TABSTOPS);
            index += 1;
            is_tabstop
        });
    }

    /// Number of columns the table covers.
    #[inline]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// `true` when the table is zero-wide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Iterate columns (`col`, `is_stop`).
    pub fn iter(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.tabs.iter().copied().enumerate()
    }
}

impl Index<usize> for TabStops {
    type Output = bool;
    fn index(&self, index: usize) -> &bool {
        &self.tabs[index]
    }
}

impl IndexMut<usize> for TabStops {
    fn index_mut(&mut self, index: usize) -> &mut bool {
        &mut self.tabs[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_table_has_stops_every_eight() {
        let t = TabStops::new(24);
        assert!(t[0]);
        assert!(t[8]);
        assert!(t[16]);
        assert!(!t[1]);
        assert!(!t[7]);
    }

    #[test]
    fn clear_all_empties_stops_but_keeps_width() {
        let mut t = TabStops::new(32);
        t.clear_all();
        assert_eq!(t.len(), 32);
        assert!(!t[0]);
        assert!(!t[8]);
    }

    #[test]
    fn resize_grows_with_default_stride() {
        let mut t = TabStops::new(8);
        t.resize(24);
        assert!(t[8]);
        assert!(t[16]);
    }

    #[test]
    fn resize_shrink_drops_stops_at_tail() {
        let mut t = TabStops::new(24);
        t.resize(8);
        assert_eq!(t.len(), 8);
    }

    #[test]
    fn index_mut_toggles_stop() {
        let mut t = TabStops::new(8);
        t[3] = true;
        assert!(t[3]);
        t[3] = false;
        assert!(!t[3]);
    }

    #[test]
    fn iter_yields_col_and_flag_pairs() {
        let t = TabStops::new(10);
        let stops: Vec<usize> = t.iter().filter_map(|(c, s)| s.then_some(c)).collect();
        assert_eq!(stops, vec![0, 8]);
    }
}
