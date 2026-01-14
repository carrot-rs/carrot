//! End-to-end damage-aware rendering test.
//!
//! Simulates a two-frame sequence: render a block, change one cell,
//! re-render with the first frame's signatures as `prev`. Assert that
//! only the changed cell emits a CellDraw.

use carrot_block_render::{BlockRenderInput, Damage, FrameState, render_block_damaged};
use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, PageCapacity, PageList};

fn make_block(cols: u16, rows: u16) -> (PageList, CellStyleAtlas) {
    let cap = PageCapacity::new(cols, 1024);
    let mut list = PageList::new(cap);
    let atlas = CellStyleAtlas::new();
    for r in 0..rows {
        let row: Vec<Cell> = (0..cols)
            .map(|c| Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), CellStyleId(0)))
            .collect();
        list.append_row(&row);
    }
    (list, atlas)
}

#[test]
fn first_frame_emits_every_visible_cell() {
    let (pages, atlas) = make_block(8, 6);
    let prev = FrameState::empty();

    let out = render_block_damaged(
        BlockRenderInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..6,
            viewport_cols: 8,
        },
        &prev,
    );

    assert_eq!(out.damage, Damage::Full);
    assert_eq!(out.draws.len(), 6 * 8);
    assert_eq!(out.signatures.len(), 6 * 8);
    assert_eq!(out.visual_rows, 6);
}

#[test]
fn identical_frames_emit_zero_draws() {
    let (pages, atlas) = make_block(8, 4);

    // Frame 1: full render, capture signatures.
    let first = render_block_damaged(
        BlockRenderInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 8,
        },
        &FrameState::empty(),
    );
    let mut prev = FrameState::with_viewport(first.visual_rows, 8);
    prev.replace_cells(first.visual_rows, 8, first.signatures);

    // Frame 2: same data, damage-aware render.
    let second = render_block_damaged(
        BlockRenderInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 8,
        },
        &prev,
    );
    assert!(second.damage.is_clean());
    assert_eq!(second.draws.len(), 0);
}

#[test]
fn single_cell_change_emits_single_draw() {
    // Build two separate PageLists: same shape, one cell differs.
    let cols: u16 = 10;
    let (pages_a, atlas) = make_block(cols, 4);

    // Frame A
    let first = render_block_damaged(
        BlockRenderInput {
            pages: &pages_a,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: cols,
        },
        &FrameState::empty(),
    );
    let mut prev = FrameState::with_viewport(first.visual_rows, cols);
    prev.replace_cells(first.visual_rows, cols, first.signatures);

    // Frame B: rebuild pages with cell (2, 5) swapped to 'Z'.
    let cap = PageCapacity::new(cols, 1024);
    let mut pages_b = PageList::new(cap);
    for r in 0..4u16 {
        let row: Vec<Cell> = (0..cols)
            .map(|c| {
                if r == 2 && c == 5 {
                    Cell::ascii(b'Z', CellStyleId(0))
                } else {
                    Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), CellStyleId(0))
                }
            })
            .collect();
        pages_b.append_row(&row);
    }

    let second = render_block_damaged(
        BlockRenderInput {
            pages: &pages_b,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: cols,
        },
        &prev,
    );
    assert_eq!(second.draws.len(), 1);
    assert_eq!(second.draws[0].content, b'Z' as u32);
    assert_eq!(second.draws[0].visual_row, 2);
    assert_eq!(second.draws[0].visual_col, 5);
}

#[test]
fn viewport_resize_falls_back_to_full_damage() {
    let (pages, atlas) = make_block(10, 4);

    let first = render_block_damaged(
        BlockRenderInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 10,
        },
        &FrameState::empty(),
    );
    let mut prev = FrameState::with_viewport(first.visual_rows, 10);
    prev.replace_cells(first.visual_rows, 10, first.signatures);

    // Second render with different viewport_cols → Damage::Full.
    let second = render_block_damaged(
        BlockRenderInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 5,
        },
        &prev,
    );
    assert_eq!(second.damage, Damage::Full);
    // All cells emitted.
    assert!(second.draws.len() >= 4 * 10);
}
