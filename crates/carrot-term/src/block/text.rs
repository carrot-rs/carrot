//! Block text extraction — Grid → `String`, the canonical conversion.
//!
//! Three callers, one implementation:
//!
//! - **Selection materialisation** (`super::selection::BlockSelection::
//!   to_string`) — picks a sub-range, walks rows, joins with newlines.
//! - **Search export / Copy-as-text / AI context** — full-block flat
//!   string for ranking, prompts, and clipboard payloads.
//! - **Per-line export** (Markdown blocks, JSON streaming, screenshot
//!   alt-text) — wants a `Vec<String>` so each line is addressable.
//!
//! Operates on a [`BlockSnapshot`] + [`GraphemeStore`]: rows, atlas and
//! bounds travel with the snapshot, the grapheme store resolves
//! multi-codepoint cluster cells. No `PageList` access needed — the
//! snapshot already iterated the live grid via `GridBounds`, so this
//! module's contract is "pure cell rendering, no terminal-lock work".
//!
//! Cells with no textual contribution (Wide2nd ghost cells, Image
//! cells, ShapedRun cache cells, CustomRender plugin cells) are
//! skipped — they're rendered as visuals, not characters. Trailing
//! whitespace within a row is preserved; trailing-empty-row trimming
//! is the caller's policy.

use carrot_grid::{BlockSnapshot, Cell, CellTag, GraphemeIndex, GraphemeStore};

/// Concatenate every row of a snapshot into one `String`, separated by
/// `\n`. The terminating newline is **not** added — leave that to the
/// caller, since copy-to-clipboard and AI-prompt callers disagree.
pub fn extract_block_text(snapshot: &BlockSnapshot, graphemes: &GraphemeStore) -> String {
    let mut out = String::new();
    for (i, row) in snapshot.rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        append_row(row, graphemes, &mut out);
    }
    out
}

/// Materialise each row of a snapshot as a separate `String`. Empty
/// rows produce empty strings — callers that want trimming filter
/// downstream.
pub fn extract_block_lines(snapshot: &BlockSnapshot, graphemes: &GraphemeStore) -> Vec<String> {
    snapshot
        .rows
        .iter()
        .map(|row| {
            let mut s = String::new();
            append_row(row, graphemes, &mut s);
            s
        })
        .collect()
}

/// Append a `[first_col, last_col]` slice of a single row to `out`.
/// Used by selection materialisation when the selection geometry
/// pins the column range. Both endpoints are inclusive — callers
/// pass `(row.len() - 1)` to mean "to the end of the row".
pub fn append_row_range(
    row: &[Cell],
    graphemes: &GraphemeStore,
    first_col: u16,
    last_col: u16,
    out: &mut String,
) {
    let end = (last_col as usize + 1).min(row.len());
    let start = (first_col as usize).min(end);
    for cell in &row[start..end] {
        append_cell(*cell, graphemes, out);
    }
}

/// Append every textual cell of `row` to `out`. Wide2nd ghosts /
/// images / shaped-run cache / custom-render cells are skipped.
pub fn append_row(row: &[Cell], graphemes: &GraphemeStore, out: &mut String) {
    for cell in row {
        append_cell(*cell, graphemes, out);
    }
}

/// Append the textual contribution of a single cell to `out`. Shared
/// by the row-and-range helpers above and by selection extraction.
pub fn append_cell(cell: Cell, graphemes: &GraphemeStore, out: &mut String) {
    match cell.tag() {
        CellTag::Ascii => {
            let b = cell.content() as u8;
            if b != 0 {
                out.push(b as char);
            } else {
                // Null content cell renders as a space — keeps row
                // widths aligned for diff / column-extraction callers.
                out.push(' ');
            }
        }
        CellTag::Codepoint => {
            if let Some(c) = char::from_u32(cell.content()) {
                out.push(c);
            }
        }
        CellTag::Grapheme => {
            let id = GraphemeIndex(cell.content());
            if let Some(s) = graphemes.get(id) {
                out.push_str(s);
            }
        }
        // Ghost cells (Wide2nd) are covered by the primary preceding
        // cell. Image / ShapedRun / CustomRender cells have no
        // textual contribution by design.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{Cell, CellStyle, PageCapacity, PageList};

    fn ascii_cell(b: u8) -> Cell {
        Cell::ascii(b, carrot_grid::CellStyleId(0))
    }

    fn populate(text: &str, cols: u16) -> BlockSnapshot {
        let cap = PageCapacity::new(cols, 4096);
        let mut pages = PageList::new(cap);
        let mut row: Vec<Cell> = vec![Cell::default(); cols as usize];
        for (i, byte) in text.bytes().enumerate() {
            row[i] = ascii_cell(byte);
        }
        pages.append_row(&row);
        BlockSnapshot::from_pages(&pages, &[CellStyle::DEFAULT])
    }

    #[test]
    fn extract_block_text_joins_rows_with_newline() {
        let cap = PageCapacity::new(8, 4096);
        let mut pages = PageList::new(cap);
        let mut row: Vec<Cell> = vec![Cell::default(); 8];
        for (i, byte) in b"hello".iter().enumerate() {
            row[i] = ascii_cell(*byte);
        }
        pages.append_row(&row);
        let mut row2: Vec<Cell> = vec![Cell::default(); 8];
        for (i, byte) in b"world".iter().enumerate() {
            row2[i] = ascii_cell(*byte);
        }
        pages.append_row(&row2);
        let snap = BlockSnapshot::from_pages(&pages, &[CellStyle::DEFAULT]);
        let text = extract_block_text(&snap, &GraphemeStore::new());
        assert_eq!(text, "hello   \nworld   ");
    }

    #[test]
    fn extract_block_lines_yields_one_string_per_row() {
        let snap = populate("hi", 4);
        let lines = extract_block_lines(&snap, &GraphemeStore::new());
        assert_eq!(lines, vec!["hi  ".to_string()]);
    }

    #[test]
    fn append_row_range_respects_inclusive_bounds() {
        let cap = PageCapacity::new(10, 4096);
        let mut pages = PageList::new(cap);
        let mut row: Vec<Cell> = vec![Cell::default(); 10];
        for (i, byte) in b"abcdefghij".iter().enumerate() {
            row[i] = ascii_cell(*byte);
        }
        pages.append_row(&row);
        let snap = BlockSnapshot::from_pages(&pages, &[CellStyle::DEFAULT]);

        let mut out = String::new();
        append_row_range(&snap.rows[0], &GraphemeStore::new(), 2, 5, &mut out);
        assert_eq!(out, "cdef");
    }

    #[test]
    fn null_ascii_renders_as_space() {
        let snap = populate("", 4);
        let lines = extract_block_lines(&snap, &GraphemeStore::new());
        assert_eq!(lines, vec!["    ".to_string()]);
    }
}
