//! Block search — search through terminal block output.
//!
//! Provides `BlockMatch` (the `SearchableItem::Match` type for `TerminalPane`)
//! and `find_matches_in_blocks()` which searches across all block grids,
//! stripping ANSI escape sequences before matching.

use std::ops::Range;

use carrot_project::search::SearchQuery;
use carrot_term::BlockId;

/// A single search match within a terminal block's output.
///
/// Stored as the `SearchableItem::Match` type for `TerminalPane`.
/// Contains enough information to locate and highlight the match.
#[derive(Clone, Debug)]
pub struct BlockMatch {
    /// Index of the block in the block list (for scrolling/navigation).
    pub block_index: usize,
    /// Block ID (stable identifier, survives block deletion of earlier blocks).
    pub block_id: BlockId,
    /// Byte range within the block's plain-text output (ANSI-stripped).
    pub byte_range: Range<usize>,
    /// Line number within the block's output (0-based, relative to block start).
    pub line: usize,
    /// Column offset within the line (0-based, in characters).
    pub col: usize,
    /// Length of the match in characters.
    pub char_len: usize,
    /// Whether this match is in the command header (not grid output).
    pub in_command: bool,
}

/// Fast-path substring search driven by [`carrot_term::block::
/// BlockRouter::search`]. Walks every block's `PageList` at the cell
/// level — no per-block text extraction, no regex engine. Call-site
/// in `SearchableItem::find_matches` routes non-regex plain-text
/// queries here; regex/whole-word queries keep going through
/// [`find_matches_in_extracted_blocks`].
pub fn find_via_router<T: carrot_term::event::EventListener>(
    term: &carrot_term::Term<T>,
    needle: &str,
    case_sensitive: bool,
) -> Vec<BlockMatch> {
    let options = carrot_grid::search::SearchOptions::default().case_insensitive(!case_sensitive);
    let router = term.block_router();
    let mut out = Vec::new();
    // Walk router entries in chronological order so `block_index`
    // lines up with the Inazuma block list.
    for (block_index, entry) in router.entries().iter().enumerate() {
        let legacy_id = BlockId(entry.id.0 as usize);
        for hit in router
            .search(needle, options)
            .filter(|m| m.block_id == entry.id)
        {
            let line = hit
                .inner
                .start
                .origin
                .saturating_sub(first_row_origin(entry)) as usize;
            let col = hit.inner.start.col as usize;
            let char_len = hit.inner.len as usize;
            out.push(BlockMatch {
                block_index,
                block_id: legacy_id,
                // byte_range is a text-relative concept; cell search
                // doesn't compute it. Consumers that care (replace,
                // select) aren't enabled for terminal output, so a
                // char-len span is a safe stub.
                byte_range: 0..char_len,
                line,
                col,
                char_len,
                in_command: false,
            });
        }
    }
    out
}

fn first_row_origin(entry: &carrot_term::block::RouterEntry) -> u64 {
    let grid = match &entry.variant {
        carrot_term::block::BlockVariant::Active(b) => b.grid(),
        carrot_term::block::BlockVariant::Frozen(b) => b.grid(),
    };
    grid.first_row_offset()
}

/// Extract plain text from a v2 RouterEntry's grid.
///
/// Walks every row in the underlying `PageList` and decodes cells
/// via the same path `carrot-grid::search` uses. No styling, no
/// colors — just the characters, one row per line.
pub fn extract_entry_text(entry: &carrot_term::block::RouterEntry) -> String {
    use carrot_grid::CellTag;
    let grid = match &entry.variant {
        carrot_term::block::BlockVariant::Active(block) => block.grid(),
        carrot_term::block::BlockVariant::Frozen(block) => block.grid(),
    };
    let graphemes = match &entry.variant {
        carrot_term::block::BlockVariant::Active(block) => block.graphemes(),
        carrot_term::block::BlockVariant::Frozen(block) => block.graphemes(),
    };
    let total = grid.total_rows();
    let mut out = String::new();
    for ix in 0..total {
        let Some(row) = grid.row(ix) else { continue };
        for cell in row {
            match cell.tag() {
                CellTag::Ascii => {
                    let b = cell.content() as u8;
                    if b != 0 {
                        out.push(b as char);
                    }
                }
                CellTag::Codepoint => {
                    if let Some(c) = char::from_u32(cell.content()) {
                        out.push(c);
                    }
                }
                CellTag::Grapheme => {
                    let id = carrot_grid::GraphemeIndex(cell.content());
                    if let Some(s) = graphemes.get(id) {
                        out.push_str(s);
                    }
                }
                _ => {}
            }
        }
        out.push('\n');
    }
    out
}

/// Search across pre-extracted block text data. Designed to be called from
/// a background task where the terminal lock is not held.
///
/// Each tuple: `(block_index, block_id, grid_text, command_text)`.
pub fn find_matches_in_extracted_blocks(
    block_data: &[(usize, BlockId, String, String)],
    query: &SearchQuery,
) -> Vec<BlockMatch> {
    if query.as_str().is_empty() {
        return Vec::new();
    }

    let mut all_matches = Vec::new();

    for &(block_index, block_id, ref text, ref command) in block_data {
        // Search grid output text
        let ranges = find_ranges_in_text(text, query);
        for byte_range in ranges {
            let (line, col, char_len) = byte_range_to_line_col(text, &byte_range);
            all_matches.push(BlockMatch {
                block_index,
                block_id,
                byte_range,
                line,
                col,
                char_len,
                in_command: false,
            });
        }

        // Also search the command text (header)
        if !command.is_empty() {
            let cmd_ranges = find_ranges_in_text(command, query);
            for byte_range in cmd_ranges {
                let (line, col, char_len) = byte_range_to_line_col(command, &byte_range);
                all_matches.push(BlockMatch {
                    block_index,
                    block_id,
                    byte_range,
                    line,
                    col,
                    char_len,
                    in_command: true,
                });
            }
        }
    }

    all_matches
}

/// Run a SearchQuery against plain text, returning byte ranges of matches.
fn find_ranges_in_text(text: &str, query: &SearchQuery) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();

    match query {
        SearchQuery::Text {
            search, whole_word, ..
        } => {
            for mat in search.find_iter(text) {
                if *whole_word && !is_word_boundary_match(text, mat.start(), mat.end()) {
                    continue;
                }
                ranges.push(mat.start()..mat.end());
            }
        }
        SearchQuery::Regex {
            regex, whole_word, ..
        } => {
            for mat in regex.find_iter(text).flatten() {
                if *whole_word && !is_word_boundary_match(text, mat.start(), mat.end()) {
                    continue;
                }
                ranges.push(mat.start()..mat.end());
            }
        }
    }

    ranges
}

/// Check if a match at the given byte offsets is at word boundaries.
fn is_word_boundary_match(text: &str, start: usize, end: usize) -> bool {
    let prev_is_word = text[..start]
        .chars()
        .next_back()
        .map_or(true, |c| !c.is_alphanumeric() && c != '_');
    let next_is_word = text[end..]
        .chars()
        .next()
        .map_or(true, |c| !c.is_alphanumeric() && c != '_');
    prev_is_word && next_is_word
}

/// Convert a byte range within text to (line, col, char_len).
fn byte_range_to_line_col(text: &str, range: &Range<usize>) -> (usize, usize, usize) {
    let before = &text[..range.start];
    let line = before.chars().filter(|&c| c == '\n').count();
    let line_start = before.rfind('\n').map_or(0, |pos| pos + 1);
    let col = text[line_start..range.start].chars().count();
    let char_len = text[range.start..range.end].chars().count();
    (line, col, char_len)
}

/// Get search highlights for a specific block (by index).
///
/// Returns (matches_for_block, active_match_line_col) for the grid element renderer.
pub fn search_highlights_for_block(
    all_matches: &[BlockMatch],
    active_highlight_index: Option<usize>,
    block_index: usize,
) -> (&[BlockMatch], Option<(usize, usize)>) {
    let start = all_matches.partition_point(|m| m.block_index < block_index);
    let end = all_matches.partition_point(|m| m.block_index <= block_index);
    let block_matches = &all_matches[start..end];

    let active_line_col = active_highlight_index.and_then(|idx| {
        all_matches.get(idx).and_then(|m| {
            if m.block_index == block_index {
                Some((m.line, m.col))
            } else {
                None
            }
        })
    });

    (block_matches, active_line_col)
}

/// Count total matches across all blocks (for status display).
pub fn match_count(matches: &[BlockMatch]) -> usize {
    matches.len()
}

/// Find the match index closest to a given block index (for active_match_index).
pub fn nearest_match_index(matches: &[BlockMatch], block_index: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }

    // Binary search for the first match at or after the given block
    let pos = matches.partition_point(|m| m.block_index < block_index);
    if pos < matches.len() {
        Some(pos)
    } else {
        Some(matches.len() - 1)
    }
}
