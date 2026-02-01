//! Smoke test proving the render stack compiles + runs inside
//! `carrot-terminal-view`.
//!
//! Exercises the path that replaces the legacy `grid_snapshot` /
//! `grid_element` split with `carrot_block_render::{SharedTerminal,
//! BlockElement}` backed by `carrot_grid::{PageList, CellStyleAtlas}`.
//! End-to-end in miniature:
//!
//! 1. Build a PageList + CellStyleAtlas in the terminal-view crate.
//! 2. Feed it to `carrot_block_render::render_block`.
//! 3. Assert per-cell draws come out with the expected structure.
//!
//! When the migration proper lands, this file is deleted in favour
//! of the real integration in `terminal_pane.rs`. Until then it
//! guards the dep wiring — a broken workspace-edition bump or a
//! rename in the new stack shows up as a test failure here instead
//! of at runtime.

use carrot_block_render::{BlockRenderInput, render_block};
use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, PageCapacity, PageList};

#[test]
fn new_stack_emits_expected_draws() {
    let cap = PageCapacity::new(4, 64);
    let mut list = PageList::new(cap);
    let atlas = CellStyleAtlas::new();
    for r in 0..3u8 {
        let row: Vec<Cell> = (0..4u8)
            .map(|c| Cell::ascii(b'a' + ((r + c) % 26), CellStyleId(0)))
            .collect();
        list.append_row(&row);
    }
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..3,
        viewport_cols: 4,
    });
    assert_eq!(draws.len(), 12);
    assert_eq!(draws[0].visual_row, 0);
    assert_eq!(draws[0].visual_col, 0);
    assert_eq!(draws[11].visual_row, 2);
    assert_eq!(draws[11].visual_col, 3);
}

#[test]
fn soft_wrap_on_narrow_viewport_produces_expected_visual_rows() {
    let cap = PageCapacity::new(8, 64);
    let mut list = PageList::new(cap);
    let atlas = CellStyleAtlas::new();
    let row: Vec<Cell> = (0..8u8)
        .map(|c| Cell::ascii(b'a' + c, CellStyleId(0)))
        .collect();
    list.append_row(&row);
    let draws = render_block(BlockRenderInput {
        pages: &list,
        atlas: &atlas,
        visible_rows: 0..1,
        viewport_cols: 4,
    });
    assert_eq!(draws.len(), 8);
    // Two visual rows of 4 cells.
    let rows: std::collections::BTreeSet<u32> = draws.iter().map(|d| d.visual_row).collect();
    assert_eq!(rows.len(), 2);
}
