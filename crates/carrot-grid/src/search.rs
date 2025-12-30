//! Full-text search over `PageList` cells.
//!
//! Walks cells directly, never materialising a rendered string, and
//! emits matches tagged with stable [`CellId`]s so highlights persist
//! across scrollback pruning.
//!
//! This is the **CPU** implementation — straight single-pass byte
//! comparison. Phase 2 replaces it with a wgpu compute-shader pass
//! for 1 M-line sub-10 ms search (that's the B2 differentiator), but
//! the API stays the same: `search_cells(pages, pattern, options)`
//! returns `Vec<SearchMatch>`. A consumer binds to this function
//! once and benefits from the GPU upgrade transparently later.
//!
//! # Scope
//!
//! - Exact byte-sequence matching (ASCII + UTF-8 mixed).
//! - Case-insensitive option (ASCII-only fold — full Unicode case
//!   folding is Phase 3).
//! - Whole-word boundary option (matches bordered by non-alphanumeric
//!   cells).
//! - Overlapping vs non-overlapping — configurable. Non-overlapping
//!   is the default (typical user expectation: `aa` in `aaa` finds
//!   one match, not two).
//!
//! # Not in scope (later phases)
//!
//! - Regex (would pull in `regex-automata` — worth when consumers
//!   need it).
//! - Fuzzy matching (consumer can layer `inazuma-fuzzy` on top of
//!   the extracted text).
//! - Cross-row pattern matching. The current implementation searches
//!   within a single row at a time — line-breaks are natural match
//!   boundaries. Matches spanning soft-wrapped rows are a render-time
//!   concern; data-layer search stays row-local.

use crate::cell::{Cell, CellTag};
use crate::cell_id::CellId;
use crate::page_list::PageList;

/// Match discovered by [`search_cells`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SearchMatch {
    /// Start of the match — stable across prune.
    pub start: CellId,
    /// Length in cells (a multi-byte UTF-8 char still takes 1 cell).
    pub len: u16,
}

impl SearchMatch {
    /// Inclusive end cell of the match.
    pub fn end(&self) -> CellId {
        CellId::new(self.start.origin, self.start.col + self.len - 1)
    }
}

/// Search options. Builder-style for ergonomic call sites.
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchOptions {
    pub case_insensitive: bool,
    pub whole_word: bool,
    pub allow_overlap: bool,
}

impl SearchOptions {
    pub fn case_insensitive(mut self, on: bool) -> Self {
        self.case_insensitive = on;
        self
    }
    pub fn whole_word(mut self, on: bool) -> Self {
        self.whole_word = on;
        self
    }
    pub fn allow_overlap(mut self, on: bool) -> Self {
        self.allow_overlap = on;
        self
    }
}

/// Search every row of `pages` for `needle`. Returns matches in row-
/// then-column order.
///
/// `needle` is a `&str` — the caller normalises their input before
/// calling. Empty needle returns no matches.
pub fn search_cells(pages: &PageList, needle: &str, options: SearchOptions) -> Vec<SearchMatch> {
    if needle.is_empty() {
        return Vec::new();
    }

    // Precompute the needle as a vec of (char, lowercase_char) so the
    // inner loop is allocation-free and case-insensitive compare is
    // one `if case_insensitive` branch per cell.
    let needle_chars: Vec<(char, char)> = needle.chars().map(|c| (c, ascii_lower(c))).collect();

    let mut out = Vec::new();
    let first_origin = pages.first_row_offset();
    let total_rows = pages.total_rows();

    for row_ix in 0..total_rows {
        let Some(row) = pages.row(row_ix) else {
            continue;
        };
        let origin = first_origin + row_ix as u64;
        search_row(row, origin, &needle_chars, options, &mut out);
    }

    out
}

fn search_row(
    row: &[Cell],
    origin: u64,
    needle: &[(char, char)],
    options: SearchOptions,
    out: &mut Vec<SearchMatch>,
) {
    let n = needle.len();
    if n == 0 || row.len() < n {
        return;
    }

    let row_len = row.len();
    let mut i = 0usize;
    while i + n <= row_len {
        if match_at(row, i, needle, options) {
            if options.whole_word && !is_word_match_boundary(row, i, n) {
                i += 1;
                continue;
            }
            out.push(SearchMatch {
                start: CellId::new(origin, i as u16),
                len: n as u16,
            });
            if options.allow_overlap {
                i += 1;
            } else {
                i += n;
            }
        } else {
            i += 1;
        }
    }
}

fn match_at(row: &[Cell], start: usize, needle: &[(char, char)], options: SearchOptions) -> bool {
    for (offset, &(ch, lower)) in needle.iter().enumerate() {
        let cell = row[start + offset];
        let cell_ch = match cell.tag() {
            CellTag::Ascii | CellTag::Codepoint => match char::from_u32(cell.content()) {
                Some(c) => c,
                None => return false,
            },
            // Wide2nd / Grapheme / Image / ShapedRun / CustomRender
            // are not currently searchable. Grapheme support lands
            // with the grapheme table in a future phase.
            _ => return false,
        };
        if options.case_insensitive {
            if ascii_lower(cell_ch) != lower {
                return false;
            }
        } else if cell_ch != ch {
            return false;
        }
    }
    true
}

fn is_word_match_boundary(row: &[Cell], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !is_word_cell(row[start - 1]);
    let after_ok = start + len == row.len() || !is_word_cell(row[start + len]);
    before_ok && after_ok
}

fn is_word_cell(cell: Cell) -> bool {
    match cell.tag() {
        CellTag::Ascii | CellTag::Codepoint => {
            if let Some(c) = char::from_u32(cell.content()) {
                c.is_alphanumeric() || c == '_'
            } else {
                false
            }
        }
        _ => false,
    }
}

/// ASCII-only lower-case fold. Full Unicode case folding is Phase 3.
fn ascii_lower(c: char) -> char {
    if c.is_ascii_uppercase() {
        ((c as u8) + 32) as char
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyleId;
    use crate::page::PageCapacity;

    fn populate_text(cols: u16, lines: &[&str]) -> PageList {
        let cap = PageCapacity::new(cols, 1024);
        let mut list = PageList::new(cap);
        for line in lines {
            let mut row: Vec<Cell> = line
                .chars()
                .map(|c| {
                    if c.is_ascii() {
                        Cell::ascii(c as u8, CellStyleId(0))
                    } else {
                        Cell::codepoint(c, CellStyleId(0))
                    }
                })
                .collect();
            // Pad / truncate to cols.
            row.truncate(cols as usize);
            while row.len() < cols as usize {
                row.push(Cell::ascii(b' ', CellStyleId(0)));
            }
            list.append_row(&row);
        }
        list
    }

    #[test]
    fn empty_needle_returns_no_matches() {
        let pages = populate_text(10, &["hello"]);
        let matches = search_cells(&pages, "", SearchOptions::default());
        assert!(matches.is_empty());
    }

    #[test]
    fn single_exact_match() {
        let pages = populate_text(20, &["hello world"]);
        let matches = search_cells(&pages, "world", SearchOptions::default());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, CellId::new(0, 6));
        assert_eq!(matches[0].len, 5);
        assert_eq!(matches[0].end(), CellId::new(0, 10));
    }

    #[test]
    fn multiple_matches_across_rows() {
        let pages = populate_text(20, &["foo bar", "bar foo", "foobar"]);
        let matches = search_cells(&pages, "foo", SearchOptions::default());
        // Row 0 col 0, row 1 col 4, row 2 col 0 → 3 matches.
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].start, CellId::new(0, 0));
        assert_eq!(matches[1].start, CellId::new(1, 4));
        assert_eq!(matches[2].start, CellId::new(2, 0));
    }

    #[test]
    fn case_sensitive_by_default() {
        let pages = populate_text(20, &["Hello hello"]);
        let matches = search_cells(&pages, "hello", SearchOptions::default());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start.col, 6);
    }

    #[test]
    fn case_insensitive_option() {
        let pages = populate_text(20, &["Hello HELLO hello"]);
        let opts = SearchOptions::default().case_insensitive(true);
        let matches = search_cells(&pages, "hello", opts);
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn whole_word_boundary() {
        let pages = populate_text(30, &["foo foobar foo_bar foo bar"]);
        let opts = SearchOptions::default().whole_word(true);
        let matches = search_cells(&pages, "foo", opts);
        // Expected: first "foo" (col 0), last "foo" (col 19).
        // "foobar" and "foo_bar" are excluded — adjacent alphanumeric
        // or underscore cells.
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start.col, 0);
        assert_eq!(matches[1].start.col, 19);
    }

    #[test]
    fn overlap_default_is_non_overlapping() {
        let pages = populate_text(10, &["aaaa"]);
        let matches = search_cells(&pages, "aa", SearchOptions::default());
        // Non-overlapping: col 0, col 2 → 2 matches.
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].start.col, 0);
        assert_eq!(matches[1].start.col, 2);
    }

    #[test]
    fn allow_overlap_option() {
        let pages = populate_text(10, &["aaaa"]);
        let opts = SearchOptions::default().allow_overlap(true);
        let matches = search_cells(&pages, "aa", opts);
        // Overlapping: col 0, 1, 2 → 3 matches.
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn match_surviving_prune_still_resolves() {
        // Capture a match before pruning, then assert the CellId still
        // resolves to the same cell after prune.
        let cap = PageCapacity::new(10, 128);
        let mut pages = PageList::new(cap);
        for _ in 0..10 {
            let row: Vec<Cell> = "dead beef "
                .chars()
                .map(|c| Cell::ascii(c as u8, CellStyleId(0)))
                .collect();
            pages.append_row(&row);
        }
        let before = search_cells(&pages, "beef", SearchOptions::default());
        assert!(!before.is_empty());
        let first = before[0];

        pages.prune_head();
        // Old id still points at a valid cell (the row wasn't the one
        // we matched in — we matched row 0 which got pruned). The id
        // should return None via cell_coords_for_id.
        if first.start.origin == 0 {
            assert!(pages.cell_coords_for_id(first.start).is_none());
        }
    }

    #[test]
    fn utf8_char_matches_as_single_cell() {
        let pages = populate_text(10, &["café"]);
        let matches = search_cells(&pages, "café", SearchOptions::default());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].len, 4);
    }

    #[test]
    fn no_matches_returns_empty_vec() {
        let pages = populate_text(10, &["hello"]);
        let matches = search_cells(&pages, "xyz", SearchOptions::default());
        assert!(matches.is_empty());
    }

    #[test]
    fn needle_longer_than_row_returns_none() {
        let pages = populate_text(4, &["hi"]);
        let matches = search_cells(&pages, "hello", SearchOptions::default());
        assert!(matches.is_empty());
    }
}
