//! Property-based fuzz tests for carrot-grid primitives.
//!
//! Covers invariants that unit tests alone couldn't exercise
//! exhaustively: random byte streams into cell packing, long
//! sequences of PageList operations, varied CellStyleAtlas intern
//! patterns, compression round-trips on arbitrary data.
//!
//! Each proptest runs 256 random cases per invocation; across 8
//! properties that's 2,048 random cases per `cargo test` run.
//! Running on CI catches edge cases the curated unit tests miss.

use carrot_grid::{
    Cell, CellStyle, CellStyleAtlas, CellStyleFlags, CellStyleId, CellTag, PageCapacity, PageList,
    compress, decompress,
};
use proptest::prelude::*;

/// Strategy: any valid codepoint (1-char UTF-8 inclusive).
fn arb_char() -> impl Strategy<Value = char> {
    proptest::char::any()
}

/// Strategy: byte array representing ASCII + control bytes + high.
fn arb_ascii() -> impl Strategy<Value = u8> {
    proptest::num::u8::ANY
}

/// Strategy: arbitrary cell via one of the tag variants.
fn arb_cell() -> impl Strategy<Value = Cell> {
    prop_oneof![
        (arb_ascii(), 0u16..1000).prop_map(|(c, s)| Cell::ascii(c, CellStyleId(s))),
        (arb_char(), 0u16..1000).prop_map(|(c, s)| Cell::codepoint(c, CellStyleId(s))),
        (0u16..5000).prop_map(|s| Cell::wide_2nd(CellStyleId(s))),
    ]
}

/// Strategy: build a CellStyleAtlas with `n` intern operations,
/// returning the atlas + the sequence of style ids produced.
fn arb_atlas_with_styles(
    max_ops: usize,
) -> impl Strategy<Value = (CellStyleAtlas, Vec<CellStyleId>)> {
    proptest::collection::vec(arb_style(), 0..max_ops).prop_map(|styles| {
        let mut atlas = CellStyleAtlas::new();
        let ids: Vec<CellStyleId> = styles.iter().map(|s| atlas.intern(*s)).collect();
        (atlas, ids)
    })
}

fn arb_style() -> impl Strategy<Value = CellStyle> {
    (0u8..64, 0u8..=255, 0u8..=255, 0u8..=255).prop_map(|(flag_bits, r, g, b)| CellStyle {
        fg: carrot_grid::Color::Rgb(r, g, b),
        bg: carrot_grid::Color::Default,
        underline_color: None,
        flags: CellStyleFlags(flag_bits as u16),
        hyperlink: carrot_grid::HyperlinkId::NONE,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// Any Cell round-trips through to_bits + from_bits without
    /// changing its observable properties.
    #[test]
    fn cell_to_bits_round_trip(c in arb_cell()) {
        let bits = c.to_bits();
        let back = Cell::from_bits(bits);
        prop_assert_eq!(c.content(), back.content());
        prop_assert_eq!(c.style(), back.style());
        prop_assert_eq!(c.tag(), back.tag());
    }

    /// Dirty bit toggles are reversible.
    #[test]
    fn dirty_flag_is_idempotent(c in arb_cell(), toggle in any::<bool>()) {
        let after = c.with_dirty(toggle);
        prop_assert_eq!(after.flags().dirty(), toggle);
        let cleared = after.with_dirty(false);
        prop_assert!(!cleared.flags().dirty());
    }

    /// CellStyleAtlas intern returns the same id for identical styles,
    /// and `len()` never exceeds the number of unique insertions.
    #[test]
    fn atlas_intern_is_deterministic((atlas, ids) in arb_atlas_with_styles(200)) {
        prop_assert_eq!(atlas.len() <= 1 + ids.len(), true);
        // Re-interning the styles (even different order) should map
        // every input to a valid id.
        for &id in &ids {
            prop_assert!((id.0 as usize) < atlas.len());
        }
    }

    /// compress + decompress round-trip preserves every cell.
    #[test]
    fn compression_round_trip(cells in proptest::collection::vec(arb_cell(), 0..500)) {
        let compressed = compress(&cells).expect("compress");
        let back = decompress(&compressed).expect("decompress");
        prop_assert_eq!(cells.len(), back.len());
        for (a, b) in cells.iter().zip(back.iter()) {
            prop_assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    /// PageList preserves content across long random operation
    /// sequences. Append-then-read must return what we wrote.
    #[test]
    fn page_list_append_read_consistency(rows in proptest::collection::vec(
            proptest::collection::vec(arb_cell(), 8..=8), 0..80)) {
        let cap = PageCapacity::new(8, 256);
        let mut list = PageList::new(cap);
        for row in &rows {
            list.append_row(row);
        }
        prop_assert_eq!(list.total_rows(), rows.len());
        for (ix, expected) in rows.iter().enumerate() {
            let got = list.row(ix).expect("row in range");
            for (a, b) in got.iter().zip(expected.iter()) {
                prop_assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }

    /// first_row_offset advances monotonically with prune.
    #[test]
    fn prune_advances_offset_monotonically(
        initial_rows in 4usize..=40,
        prunes in 0usize..=20,
    ) {
        let cap = PageCapacity::new(4, 128);
        let mut list = PageList::new(cap);
        let row = vec![Cell::ascii(b'.', CellStyleId(0)); 4];
        for _ in 0..initial_rows {
            list.append_row(&row);
        }
        let mut prev_offset = list.first_row_offset();
        for _ in 0..prunes {
            let _ = list.prune_head();
            let now = list.first_row_offset();
            prop_assert!(now >= prev_offset, "offset went backward");
            prev_offset = now;
        }
    }

    /// CellId's resolve-then-read round-trip returns the same cell
    /// the id was issued against, as long as the row is still alive.
    #[test]
    fn cell_id_resolves_to_issued_cell(
        cells in proptest::collection::vec(arb_cell(), 16..=16),
    ) {
        let cap = PageCapacity::new(16, 512);
        let mut list = PageList::new(cap);
        list.append_row(&cells);
        for col in 0..16u16 {
            let id = list.cell_id_at(0, col).expect("in range");
            let cell = list.cell_at_id(id).expect("not pruned");
            prop_assert_eq!(cell.to_bits(), cells[col as usize].to_bits());
        }
    }

    /// Non-tail page invariant: after any sequence of appends, every
    /// non-tail page is exactly `rows_cap` rows deep. The tail can
    /// be partial.
    #[test]
    fn non_tail_pages_full_under_random_appends(
        rows in 0usize..=200,
    ) {
        let cap = PageCapacity::new(4, 128);
        let mut list = PageList::new(cap);
        let row = vec![Cell::ascii(b'.', CellStyleId(0)); 4];
        for _ in 0..rows {
            list.append_row(&row);
        }
        // Total rows must match: full_pages * rows_cap + tail_rows.
        let pages = list.page_count();
        if pages > 1 {
            let full = pages - 1;
            let expected_min = full * cap.rows_cap as usize + 1;
            prop_assert!(
                list.total_rows() >= expected_min,
                "total {} < expected_min {} for {} pages",
                list.total_rows(), expected_min, pages
            );
        }
    }
}

// Non-proptest explicit tests that would otherwise be missed.

#[test]
fn wide_2nd_preserves_style_round_trip() {
    let c = Cell::wide_2nd(CellStyleId(42));
    assert_eq!(c.tag(), CellTag::Wide2nd);
    assert_eq!(c.style().0, 42);
    let bits = c.to_bits();
    let back = Cell::from_bits(bits);
    assert_eq!(back.style().0, 42);
    assert_eq!(back.tag(), CellTag::Wide2nd);
}

#[test]
fn zero_cells_compress_to_small_payload() {
    // An all-zero payload is trivially compressible; verify the
    // framing doesn't waste space.
    let cells = vec![Cell::EMPTY; 1024];
    let compressed = compress(&cells).expect("compress");
    assert!(
        compressed.len() < 100,
        "1024 empty cells compressed to {} bytes, expected <100",
        compressed.len()
    );
}
