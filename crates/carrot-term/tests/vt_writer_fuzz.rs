//! Property-based fuzz tests for VtWriter.
//!
//! Runs thousands of randomly-generated VT byte streams through
//! VtWriter and asserts invariants that should hold for any input.
//! Catches regressions no curated corpus can — especially edge cases
//! around control-byte sequences, empty rows, and cap-boundary
//! behaviour.

use carrot_grid::CellTag;
use carrot_term::block::{ActiveBlock, ReplayBuffer, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};
use proptest::prelude::*;

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

fn drive_with_record(cols: u16, bytes: &[u8]) -> ActiveBlock {
    let mut block = ActiveBlock::new(cols);
    block.record_bytes(bytes);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
    block
}

/// A byte stream with some structure — more realistic than uniform
/// random bytes, which almost always devolve to "hit a printable ASCII".
/// We bias toward: printable ASCII (50 %), common control bytes
/// (LF/CR/BS/TAB/ESC) (35 %), and UTF-8 prefix bytes (15 %).
fn arb_vt_byte() -> impl Strategy<Value = u8> {
    prop_oneof![
        4 => (0x20u8..=0x7E).boxed(),       // printable ASCII
        1 => prop_oneof![
                Just(b'\n'), Just(b'\r'),
                Just(b'\t'), Just(0x08),     // BS
                Just(0x1B),                  // ESC
             ].boxed(),
        1 => (0x80u8..=0xFF).boxed(),       // high bytes (UTF-8 continuations)
    ]
}

fn arb_byte_stream(max_len: usize) -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(arb_vt_byte(), 0..max_len)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Any input stream drives without panic.
    #[test]
    fn no_panic_on_random_stream(bytes in arb_byte_stream(512), cols in 2u16..=120) {
        let _ = drive(cols, &bytes);
    }

    /// Output row count never exceeds input byte count (each row must
    /// contain at least one newline or wrapping character). Strict
    /// bound: total_rows ≤ bytes.len() + 1 (+1 for the finalize flush
    /// of a partial trailing row).
    #[test]
    fn row_count_bounded_by_input(bytes in arb_byte_stream(256), cols in 2u16..=80) {
        let block = drive(cols, &bytes);
        prop_assert!(block.total_rows() <= bytes.len() + 1);
    }

    /// Every row produced has the correct column width — the row
    /// buffer is always padded/truncated to `cols`.
    #[test]
    fn rows_have_correct_width(bytes in arb_byte_stream(256), cols in 2u16..=80) {
        let block = drive(cols, &bytes);
        for r in 0..block.total_rows() {
            let row = block.grid().row(r).expect("row in range");
            prop_assert_eq!(row.len(), cols as usize,
                "row {} width {} != cols {}", r, row.len(), cols);
        }
    }

    /// Wide2nd cells only ever appear **after** a Codepoint/Grapheme
    /// (never as the first cell in a row, never adjacent to an
    /// Ascii cell from a different wide-char run). The current
    /// VtWriter doesn't emit wide-char sequences — this test locks
    /// in that invariant so future extensions don't break it.
    #[test]
    fn wide_2nd_only_after_codepoint_or_grapheme(
        bytes in arb_byte_stream(256), cols in 2u16..=40
    ) {
        let block = drive(cols, &bytes);
        for r in 0..block.total_rows() {
            let row = block.grid().row(r).expect("row");
            for (i, cell) in row.iter().enumerate() {
                if cell.tag() == CellTag::Wide2nd {
                    prop_assert!(i > 0,
                        "Wide2nd at column 0 (row {}) — needs a preceding opener", r);
                    let prev = row[i - 1].tag();
                    prop_assert!(
                        matches!(prev, CellTag::Codepoint | CellTag::Grapheme),
                        "Wide2nd at ({}, {}) preceded by {:?}, expected Codepoint/Grapheme",
                        r, i, prev
                    );
                }
            }
        }
    }

    /// Replay round-trip: if we record the input bytes + drive them
    /// through a writer, then drive the same bytes through a fresh
    /// writer, the resulting grids are cell-for-cell identical.
    #[test]
    fn replay_roundtrip_is_deterministic(bytes in arb_byte_stream(256), cols in 2u16..=40) {
        let first = drive_with_record(cols, &bytes);
        let captured = first.replay().as_slice().to_vec();
        let second = drive(cols, &captured);

        prop_assert_eq!(first.total_rows(), second.total_rows());
        for r in 0..first.total_rows() {
            let a = first.grid().row(r).expect("a");
            let b = second.grid().row(r).expect("b");
            prop_assert_eq!(a.len(), b.len());
            for (c, (ca, cb)) in a.iter().zip(b.iter()).enumerate() {
                prop_assert_eq!(
                    (ca.content(), ca.tag()),
                    (cb.content(), cb.tag()),
                    "cell ({},{}) mismatch after replay", r, c
                );
            }
        }
    }

    /// ReplayBuffer with a generous cap preserves every byte.
    #[test]
    fn replay_preserves_input_bytes_under_cap(bytes in arb_byte_stream(256)) {
        let mut buf = ReplayBuffer::new(4 * 1024);
        buf.extend(&bytes);
        prop_assert_eq!(buf.as_slice(), bytes.as_slice());
        prop_assert!(!buf.is_truncated());
    }

    /// ReplayBuffer truncation is monotonic: once truncated, never
    /// un-truncated (short of a manual clear()).
    #[test]
    fn replay_truncation_is_monotonic(
        cap in 1usize..=16,
        chunks in proptest::collection::vec(arb_byte_stream(32), 1..8),
    ) {
        let mut buf = ReplayBuffer::new(cap);
        let mut saw_truncation = false;
        for chunk in &chunks {
            buf.extend(chunk);
            if saw_truncation {
                prop_assert!(buf.is_truncated(),
                    "truncation un-set after a later write");
            }
            if buf.is_truncated() {
                saw_truncation = true;
            }
        }
    }
}
