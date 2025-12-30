//! End-to-end spike: full data flow from raw cell input through
//! PageList → CellStyleAtlas → render_block → draw commands.
//!
//! This test proves the Layer-1 + Layer-4 datatypes compose cleanly. It
//! simulates what a VT-state-machine would do: push rows of cells as if
//! they came off a PTY, then ask the renderer for what it would send to
//! the GPU for a given visible range.

use carrot_block_render::{BlockRenderInput, CellDraw, render_block};
use carrot_grid::{
    Cell, CellStyle, CellStyleAtlas, CellStyleFlags, CellStyleId, CellTag, PageCapacity, PageList,
};

#[test]
fn end_to_end_three_blocks_scroll_window_render() {
    // Simulate three `seq 1 N` blocks totalling 1000 rows of 80 cols.
    let cap = PageCapacity::new(80, 4096);
    let mut pages = PageList::new(cap);
    let mut atlas = CellStyleAtlas::new();

    // Intern a few styles like a VT state machine would.
    let red = atlas.intern(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Red),
        bg: carrot_grid::Color::Default,
        underline_color: None,
        flags: CellStyleFlags::BOLD,
        hyperlink: carrot_grid::HyperlinkId::NONE,
    });
    let green = atlas.intern(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Green),
        bg: carrot_grid::Color::Default,
        underline_color: None,
        flags: CellStyleFlags::empty(),
        hyperlink: carrot_grid::HyperlinkId::NONE,
    });

    // Push 1000 rows. Every 7th row is "red", every 11th is "green".
    for r in 0..1000u32 {
        let style = if r % 11 == 0 {
            green
        } else if r % 7 == 0 {
            red
        } else {
            CellStyleId(0)
        };
        let row: Vec<Cell> = (0..80u8)
            .map(|c| Cell::ascii(b'0' + (c % 10), style))
            .collect();
        pages.append_row(&row);
    }

    assert_eq!(pages.total_rows(), 1000);
    // With 80 cols in 4 KB pages, each page fits 512/80 = 6 rows.
    // 1000 rows / 6 = ~167 pages.
    assert!(pages.page_count() >= 166);

    // Simulate the user scrolled to the middle: render rows 500..524 (a
    // 24-row viewport) at full width. This is the O(visible) hot path.
    let draws = render_block(BlockRenderInput {
        pages: &pages,
        atlas: &atlas,
        visible_rows: 500..524,
        viewport_cols: 80,
    });
    assert_eq!(draws.len(), 24 * 80);
    assert_eq!(draws[0].visual_row, 0);
    assert_eq!(draws.last().unwrap().visual_row, 23);

    // One of the rendered rows must carry the red style. Row 504 is the
    // first `r % 7 == 0` inside the window that isn't also `r % 11 == 0`.
    let found_red = draws
        .iter()
        .any(|d| d.style.flags.contains(CellStyleFlags::BOLD));
    assert!(
        found_red,
        "expected at least one red-styled row in viewport"
    );
}

#[test]
fn prune_head_keeps_the_renderer_consistent() {
    // Verify that after pruning oldest pages, the renderer still produces
    // sensible output for the now-shifted row indices.
    let cap = PageCapacity::new(10, 256);
    let mut pages = PageList::new(cap);
    let atlas = CellStyleAtlas::new();

    for r in 0..100u8 {
        let row: Vec<Cell> = (0..10)
            .map(|_| Cell::ascii(b'0' + (r % 10), CellStyleId(0)))
            .collect();
        pages.append_row(&row);
    }

    let before = pages.total_rows();
    assert_eq!(before, 100);

    // Prune 5 pages — that's roughly half the scrollback.
    let mut pruned_rows = 0u64;
    for _ in 0..5 {
        if let Some(n) = pages.prune_head() {
            pruned_rows += n as u64;
        }
    }
    assert_eq!(pages.first_row_offset(), pruned_rows);
    assert_eq!(pages.total_rows() as u64, before as u64 - pruned_rows);

    let draws = render_block(BlockRenderInput {
        pages: &pages,
        atlas: &atlas,
        visible_rows: 0..5,
        viewport_cols: 10,
    });
    assert_eq!(draws.len(), 5 * 10);
}

#[test]
fn tag_variants_round_trip_through_render() {
    let cap = PageCapacity::new(4, 128);
    let mut pages = PageList::new(cap);
    let atlas = CellStyleAtlas::new();

    let row = vec![
        Cell::ascii(b'A', CellStyleId(0)),
        Cell::codepoint('€', CellStyleId(0)),
        Cell::wide_2nd(CellStyleId(0)),
        Cell::ascii(b'Z', CellStyleId(0)),
    ];
    pages.append_row(&row);

    let draws: Vec<CellDraw> = render_block(BlockRenderInput {
        pages: &pages,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    assert_eq!(draws.len(), 4);
    assert_eq!(draws[0].tag, CellTag::Ascii);
    assert_eq!(draws[1].tag, CellTag::Codepoint);
    assert_eq!(draws[1].content, '€' as u32);
    assert_eq!(draws[2].tag, CellTag::Wide2nd);
    assert_eq!(draws[3].tag, CellTag::Ascii);
}
