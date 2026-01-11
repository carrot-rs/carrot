//! Cross-layer integration: carrot-grid (Layer 1) feeds into
//! carrot-term ActiveBlock (Layer 2) which flows through
//! carrot-block-render (Layer 4). No intermediate snapshot copy —
//! the renderer reads the PageList directly via `ActiveBlock::grid()`.
//!
//! This test exercises the full "data path" of the Ultimate Block System
//! as designed in the plan: every layer's public API composes cleanly
//! with its neighbors without adapter structs.

use carrot_block_render::{BlockRenderInput, render_block};
use carrot_grid::{Cell, CellStyle, CellStyleFlags, CellStyleId};
use carrot_term::{ActiveBlock, BlockState};

#[test]
fn active_block_feeds_directly_into_renderer() {
    let mut active = ActiveBlock::new(80);

    // Intern a handful of styles as the VT state machine would.
    let red = active.intern_style(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Red),
        bg: carrot_grid::Color::Default,
        underline_color: None,
        flags: CellStyleFlags::BOLD,
        hyperlink: carrot_grid::HyperlinkId::NONE,
    });
    let green = active.intern_style(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Green),
        bg: carrot_grid::Color::Default,
        underline_color: None,
        flags: CellStyleFlags::empty(),
        hyperlink: carrot_grid::HyperlinkId::NONE,
    });

    // Simulate a `seq 1 60` command: 60 rows of varying-style ASCII.
    for r in 0..60u32 {
        let style = if r % 7 == 0 {
            red
        } else if r % 5 == 0 {
            green
        } else {
            CellStyleId(0)
        };
        let row: Vec<Cell> = (0..80u8)
            .map(|c| Cell::ascii(b'0' + (c % 10), style))
            .collect();
        active.append_row(&row);
    }

    assert_eq!(active.total_rows(), 60);

    // Render a 24-row window from the middle — no snapshot, direct read.
    let draws = render_block(BlockRenderInput {
        pages: active.grid(),
        atlas: active.atlas(),
        visible_rows: 20..44,
        viewport_cols: 80,
    });

    // 24 rows × 80 cols = 1920 draws.
    assert_eq!(draws.len(), 24 * 80);
    // r=20 → r%7=6, r%5=0 → green.
    let first_draw = &draws[0];
    assert_eq!(
        first_draw.style.fg,
        carrot_grid::Color::Named(carrot_grid::NamedColor::Green)
    );
    // r=21 → r%7=0 → red (bold).
    let row_21_start = &draws[80];
    assert!(row_21_start.style.flags.contains(CellStyleFlags::BOLD));
    // r=22 → neither condition hits → default.
    let row_22_start = &draws[160];
    assert_eq!(row_22_start.style.fg, CellStyle::DEFAULT.fg);
}

#[test]
fn finish_then_render_from_frozen() {
    let mut active = ActiveBlock::new(40);
    for _ in 0..10 {
        let row: Vec<Cell> = (0..40u8)
            .map(|c| Cell::ascii(b'a' + (c % 26), CellStyleId(0)))
            .collect();
        active.append_row(&row);
    }
    let frozen = active.finish(Some(0), None);

    // Frozen block renders the same way — its grid is still a PageList.
    let atlas_arc = frozen.atlas().clone();
    // Build a lightweight CellStyleAtlas facade on the frozen Arc<[CellStyle]>:
    // for this cross-layer test we just verify the render function is
    // callable with the frozen block's grid and a fresh atlas resolved
    // from the Arc snapshot — the full frozen-path API (arc_swap + GPU
    // upload) is production work for the downstream pipeline.
    let mut reconstructed_atlas = carrot_grid::CellStyleAtlas::new();
    for style in atlas_arc.iter().skip(1) {
        reconstructed_atlas.intern(*style);
    }

    let draws = render_block(BlockRenderInput {
        pages: frozen.grid(),
        atlas: &reconstructed_atlas,
        visible_rows: 0..10,
        viewport_cols: 40,
    });
    assert_eq!(draws.len(), 10 * 40);
}

#[test]
fn block_state_lifecycle_then_render() {
    let mut state = BlockState::new_active(20);

    // VT phase: write via the active variant.
    if let Some(active) = state.variant_mut().as_active_mut() {
        for _ in 0..5 {
            let row: Vec<Cell> = (0..20u8)
                .map(|_| Cell::ascii(b'X', CellStyleId(0)))
                .collect();
            active.append_row(&row);
        }
    }

    // Render BEFORE finish — still active.
    let fresh_atlas = carrot_grid::CellStyleAtlas::new();
    let active_ref = state.variant().as_active().expect("active");
    let draws_live = render_block(BlockRenderInput {
        pages: active_ref.grid(),
        atlas: active_ref.atlas(),
        visible_rows: 0..5,
        viewport_cols: 20,
    });
    assert_eq!(draws_live.len(), 5 * 20);

    // Finish, then render from the frozen state.
    let frozen_arc = state.finish(Some(0), None).expect("first finish");
    assert_eq!(frozen_arc.total_rows(), 5);
    let draws_frozen = render_block(BlockRenderInput {
        pages: frozen_arc.grid(),
        atlas: &fresh_atlas,
        visible_rows: 0..5,
        viewport_cols: 20,
    });
    assert_eq!(draws_frozen.len(), 5 * 20);
}
