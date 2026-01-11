//! End-to-end coverage of wide-char-aware soft wrap.
//!
//! The main render pipeline must never split a wide-char pair across
//! a visual-row boundary. This file arranges pages containing
//! CJK-width glyphs and asserts the resulting CellDraws keep the
//! pair together on the same visual row.

use carrot_block_render::{BlockRenderInput, CellDraw, render_block};
use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, CellTag, PageCapacity, PageList};

fn ascii(ch: u8) -> Cell {
    Cell::ascii(ch, CellStyleId(0))
}

fn wide_first() -> Cell {
    Cell::codepoint('漢', CellStyleId(0))
}

fn wide_second() -> Cell {
    Cell::wide_2nd(CellStyleId(0))
}

fn build_list(cols: u16, rows: &[Vec<Cell>]) -> (PageList, CellStyleAtlas) {
    let cap = PageCapacity::new(cols, 1024);
    let mut list = PageList::new(cap);
    for row in rows {
        list.append_row(row);
    }
    (list, CellStyleAtlas::new())
}

/// Helper: collect `(visual_row, visual_col, tag)` triples for easy
/// assertion on the layout the renderer produced.
fn layout(draws: &[CellDraw]) -> Vec<(u32, u16, CellTag)> {
    draws
        .iter()
        .map(|d| (d.visual_row, d.visual_col, d.tag))
        .collect()
}

#[test]
fn wide_pair_at_viewport_boundary_wraps_together() {
    // Row: [a b c W1 W2 d]   viewport=4   data_cols=6
    // Expect 2 visual rows:
    //   row 0: a b c
    //   row 1: W1 W2 d
    let row = vec![
        ascii(b'a'),
        ascii(b'b'),
        ascii(b'c'),
        wide_first(),
        wide_second(),
        ascii(b'd'),
    ];
    let (list, atlas) = build_list(6, &[row]);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    let layout = layout(&draws);

    // Row 0: 3 ASCII cells at columns 0, 1, 2.
    assert_eq!(
        layout[0..3],
        [
            (0, 0, CellTag::Ascii),
            (0, 1, CellTag::Ascii),
            (0, 2, CellTag::Ascii),
        ]
    );
    // Row 1: W1 + W2 + d at columns 0, 1, 2.
    assert_eq!(layout[3], (1, 0, CellTag::Codepoint));
    assert_eq!(layout[4], (1, 1, CellTag::Wide2nd));
    assert_eq!(layout[5], (1, 2, CellTag::Ascii));
}

#[test]
fn wide_pair_fits_at_end_of_row_no_wrap() {
    // Row: [a b W1 W2]   viewport=4
    // Pair fits exactly, no wrap.
    let row = vec![ascii(b'a'), ascii(b'b'), wide_first(), wide_second()];
    let (list, atlas) = build_list(4, &[row]);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    let rows: std::collections::BTreeSet<u32> = draws.iter().map(|d| d.visual_row).collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(draws.len(), 4);
}

#[test]
fn multiple_wide_pairs_produce_stable_layout() {
    // Row: [W1a W2a W1b W2b a b]   viewport=4
    // Expected:
    //   row 0: W1a W2a W1b W2b  (2 pairs fit in 4 cols)
    //   row 1: a b
    let row = vec![
        wide_first(),
        wide_second(),
        wide_first(),
        wide_second(),
        ascii(b'a'),
        ascii(b'b'),
    ];
    let (list, atlas) = build_list(6, &[row]);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    assert_eq!(draws.len(), 6);
    // First 4 cells on row 0, last 2 on row 1.
    assert!(draws[0..4].iter().all(|d| d.visual_row == 0));
    assert!(draws[4..6].iter().all(|d| d.visual_row == 1));
}

#[test]
fn no_orphaned_wide2nd_cell_appears_alone_on_a_row() {
    // Assemble a row likely to trigger the naive bug: pair straddles
    // the boundary at every wrap width from 2 to 9.
    let row: Vec<Cell> = (0..10)
        .flat_map(|i| {
            if i % 3 == 0 {
                vec![wide_first(), wide_second()]
            } else {
                vec![ascii(b'x')]
            }
        })
        .collect();
    let data_cols = row.len() as u16;
    let (list, atlas) = build_list(data_cols, &[row]);

    for viewport in 2..=9u16 {
        let draws = render_block(BlockRenderInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..1,
            viewport_cols: viewport,
        });
        // Group draws by visual_row and ensure no row starts with Wide2nd.
        let mut by_row: std::collections::BTreeMap<u32, Vec<CellTag>> =
            std::collections::BTreeMap::new();
        for d in &draws {
            by_row.entry(d.visual_row).or_default().push(d.tag);
        }
        for (row_idx, tags) in &by_row {
            assert_ne!(
                tags.first(),
                Some(&CellTag::Wide2nd),
                "visual row {row_idx} starts with orphaned Wide2nd at viewport={viewport}",
            );
        }
    }
}

#[test]
fn ascii_only_rows_layout_unchanged_after_wrap_refactor() {
    // Regression guard: the previous behaviour for pure-ASCII rows
    // must be preserved byte-for-byte.
    let row: Vec<Cell> = (0..8).map(|i| ascii(b'a' + i)).collect();
    let (list, atlas) = build_list(8, &[row]);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    // 8 draws, 2 visual rows of 4 each.
    assert_eq!(draws.len(), 8);
    let rows: std::collections::BTreeSet<u32> = draws.iter().map(|d| d.visual_row).collect();
    assert_eq!(rows.len(), 2);
    assert!(draws[0..4].iter().all(|d| d.visual_row == 0));
    assert!(draws[4..8].iter().all(|d| d.visual_row == 1));
    // Columns within each row reset to 0..3.
    for i in 0..4u16 {
        assert_eq!(draws[i as usize].visual_col, i);
        assert_eq!(draws[(i + 4) as usize].visual_col, i);
    }
}

#[test]
fn wide_pair_with_narrow_viewport_still_completes() {
    // Viewport=1 can never fit a wide pair; the soft-wrap safety net
    // clips rather than loop forever. Make sure we still produce
    // exactly one draw per cell.
    let row = vec![wide_first(), wide_second()];
    let (list, atlas) = build_list(2, &[row]);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 1,
    });
    assert_eq!(draws.len(), 2);
}
