//! Parity harness — regression gate for the VT writer pipeline.
//!
//! Feeds curated VT byte streams through the `block::VtWriter` +
//! `carrot-grid` path and asserts the rendered cells match
//! deterministic expectations. This is the seed of the eventual
//! 1000-session corpus: we add captures over time, and every capture
//! must produce the same rendered grid on every platform + architecture.

use carrot_grid::CellTag;
use carrot_term::block::{ActiveBlock, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};

/// Drive a byte stream through `VtWriter` and return the resulting block.
/// Explicitly commits any pending row so tests that assert against the
/// grid see the trailing content — the production `finalize()` is a
/// no-op since row_buf is carried across chunks on `VtWriterState`.
fn drive(cols: u16, bytes: &[u8]) -> ActiveBlock {
    let mut block = ActiveBlock::new(cols);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();
    writer.finalize();
    block
}

/// Build the UTF-8 decoded string for a given row.
fn row_text(block: &ActiveBlock, row: usize) -> String {
    let row = block.grid().row(row).expect("row in range");
    let mut out = String::with_capacity(row.len());
    for cell in row {
        match cell.tag() {
            CellTag::Ascii => {
                let c = cell.content() as u8;
                out.push(if c == 0 { ' ' } else { c as char });
            }
            CellTag::Codepoint => {
                let c = char::from_u32(cell.content()).unwrap_or('?');
                out.push(c);
            }
            _ => out.push('?'),
        }
    }
    out
}

#[test]
fn seq_1_to_10_produces_ten_rows() {
    let mut input = String::new();
    for i in 1u32..=10 {
        input.push_str(&format!("{i}\n"));
    }
    let block = drive(8, input.as_bytes());
    assert_eq!(block.total_rows(), 10);
    for i in 0..10 {
        assert!(row_text(&block, i).starts_with(&format!("{}", i + 1)));
    }
}

#[test]
fn shell_prompt_and_output_pattern() {
    // Simulated shell session: prompt, command echo, output.
    // Keep col=80 so the prompt + echo fit comfortably on one row.
    let bytes = b"$ echo hi\nhi\n$ ";
    let block = drive(80, bytes);
    // Row 0: prompt + echoed command.
    assert!(row_text(&block, 0).starts_with("$ echo hi"));
    // Row 1: command output.
    assert!(row_text(&block, 1).starts_with("hi"));
    // Row 2: trailing prompt (partial, committed by finalize).
    assert!(row_text(&block, 2).starts_with("$ "));
}

#[test]
fn long_line_wraps_to_next_row() {
    // 5 cols, 12-char input: expect 3 rows (5 + 5 + 2).
    let block = drive(5, b"ABCDEFGHIJKL");
    assert_eq!(block.total_rows(), 3);
    assert_eq!(row_text(&block, 0).trim_end_matches('\0'), "ABCDE");
    assert_eq!(row_text(&block, 1).trim_end_matches('\0'), "FGHIJ");
    assert!(row_text(&block, 2).starts_with("KL"));
}

#[test]
fn carriage_return_overwrite_is_idempotent() {
    // `foo\r__` writes "foo", returns to col 0, overwrites with "__o".
    let block = drive(10, b"foo\r__");
    assert_eq!(block.total_rows(), 1);
    let text = row_text(&block, 0);
    assert!(text.starts_with("__o"));
}

#[test]
fn progress_bar_pattern_keeps_last_state() {
    // Classic `progress\r[####  ]\r[#####]\n` — each \r resets col 0 and
    // the next write overwrites. Final row should contain the last frame.
    let bytes = b"progress\r[####  ]\r[#####]\n";
    let block = drive(20, bytes);
    let text = row_text(&block, 0);
    // The final content should be the last write, padded by trailing
    // content from prior writes in cells the last write didn't reach.
    assert!(text.starts_with("[#####]"));
}

#[test]
fn utf8_multibyte_preserved() {
    let block = drive(20, "héllo → wörld".as_bytes());
    let text = row_text(&block, 0);
    assert!(text.starts_with("héllo → wörld"));
}

#[test]
fn bulk_ascii_throughput_shape_is_correct() {
    // 1000 'x's and one newline → 1000 cols / row_width = rows used.
    let mut bytes = vec![b'x'; 1000];
    bytes.push(b'\n');
    let block = drive(80, &bytes);
    // 1000 / 80 = 12 full rows, 40 leftover = 13 rows total, last is partial.
    assert_eq!(block.total_rows(), 13);
    let last = row_text(&block, 12);
    assert!(last.starts_with(&"x".repeat(40)));
    // Cell 40 onwards should be empty.
    let row = block.grid().row(12).expect("row 12");
    assert_eq!(row[40].content(), 0);
}

#[test]
fn empty_input_produces_empty_block() {
    let block = drive(20, b"");
    assert_eq!(block.total_rows(), 0);
}

#[test]
fn only_newlines_produces_no_committed_rows() {
    // linefeed on a clean buffer is no-op: row_dirty=false, nothing committed.
    // This is intentional — blank-row commits shouldn't advance scrollback.
    let block = drive(20, b"\n\n\n");
    assert_eq!(block.total_rows(), 0);
}
