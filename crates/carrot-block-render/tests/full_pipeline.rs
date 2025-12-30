//! Hero-test: every carrot-block-render primitive composing with
//! every carrot-term / carrot-grid primitive in one realistic scenario.
//!
//! Flow:
//! 1. PTY bytes → `VtWriter` → `ActiveBlock` (carrot-grid PageList
//!    backed). Simulates a shell block that prints styled output.
//! 2. Intern a styled cell with UNDERLINE so the decoration pass
//!    fires.
//! 3. Attach an image to the block's `ImageStore` so the image pass
//!    fires.
//! 4. Call `build_frame` with cursor in view.
//! 5. Assert: cells + decorations + cursor + images all present.
//! 6. Capture `prev` state, re-call `build_frame` on identical input
//!    → damage reports clean, zero cell draws, cursor still present.
//! 7. `ActiveBlock::finish` → `FrozenBlock`, verify replay bytes are
//!    intact.
//! 8. Rebuild a fresh `ActiveBlock` from the frozen replay bytes,
//!    diff the grids — they must match cell-for-cell (replay
//!    round-trip).
//!
//! When this test passes, every render primitive is wired to every
//! terminal primitive correctly. Regressions that break a single
//! seam surface immediately.

use std::sync::Arc;

use carrot_block_render::{
    CursorShape, CursorState, FrameInput, FrameState, build_frame, diff_grids,
};
use carrot_grid::{
    Cell, CellStyle, CellStyleFlags, CellStyleId, CellTag, DecodedImage, ImageFormat, Placement,
};
use carrot_term::block::{ActiveBlock, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};

fn drive(block: &mut ActiveBlock, cols: u16, bytes: &[u8]) {
    block.record_bytes(bytes);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
}

#[test]
fn end_to_end_pipeline_composes() {
    const COLS: u16 = 20;

    // ─── 1. VT bytes → ActiveBlock ────────────────────────────────
    let mut block = ActiveBlock::new(COLS);
    drive(&mut block, COLS, b"hello\nworld\n");
    assert_eq!(block.total_rows(), 2);

    // ─── 2. Inject a styled (underlined) cell into the live grid ──
    let style_id = block.intern_style(CellStyle {
        flags: CellStyleFlags::UNDERLINE,
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Red),
        bg: carrot_grid::Color::Default,
        ..CellStyle::DEFAULT
    });
    let row: Vec<Cell> = (0..COLS)
        .map(|c| {
            if c < 5 {
                Cell::ascii(b'U', style_id)
            } else {
                Cell::ascii(b' ', CellStyleId(0))
            }
        })
        .collect();
    block.append_row(&row);
    assert_eq!(block.total_rows(), 3);

    // ─── 3. Attach an image to the block ──────────────────────────
    let image = Arc::new(DecodedImage::new(
        16,
        16,
        ImageFormat::Rgba8,
        vec![0u8; 16 * 16 * 4],
    ));
    block.images_mut().push(image, Placement::at(1, 2, 2, 3));
    assert_eq!(block.images().len(), 1);

    // ─── 4. Build the first frame ─────────────────────────────────
    let cursor = CursorState {
        row: 0,
        col: 0,
        shape: CursorShape::Block,
        blink_phase_on: true,
        visible: true,
        cell_tag: CellTag::Ascii,
    };

    let frame = build_frame(FrameInput {
        pages: block.grid(),
        atlas: block.atlas(),
        visible_rows: 0..block.total_rows(),
        viewport_cols: COLS,
        cursor,
        prev: &FrameState::empty(),
        palette: &carrot_block_render::TerminalPalette::CARROT_DARK,
        underline_color_override: None,
        images: Some(block.images()),
        cell_pixel_width: 8.0,
        cell_pixel_height: 16.0,
    });

    // ─── 5. Every pass fired ──────────────────────────────────────
    assert!(
        !frame.cells.is_empty(),
        "cells must be emitted on first paint"
    );
    assert!(
        !frame.decorations.is_empty(),
        "underline must produce a decoration"
    );
    assert_eq!(frame.decorations.len(), 5); // 5 'U' cells on the styled row
    assert!(frame.cursor.is_some(), "visible cursor must be emitted");
    assert_eq!(frame.images.len(), 1, "attached image must be projected");

    // ─── 6. Second call with identical input → clean damage ───────
    let mut prev = FrameState::with_viewport(frame.visual_rows, COLS);
    prev.replace_cells(frame.visual_rows, COLS, frame.signatures);

    let second = build_frame(FrameInput {
        pages: block.grid(),
        atlas: block.atlas(),
        visible_rows: 0..block.total_rows(),
        viewport_cols: COLS,
        cursor,
        prev: &prev,
        palette: &carrot_block_render::TerminalPalette::CARROT_DARK,
        underline_color_override: None,
        images: Some(block.images()),
        cell_pixel_width: 8.0,
        cell_pixel_height: 16.0,
    });
    assert!(second.damage.is_clean());
    assert!(second.cells.is_empty(), "steady state emits no cell draws");
    assert!(
        second.decorations.is_empty(),
        "clean cells emit no decorations"
    );
    assert!(second.cursor.is_some(), "cursor always emits when visible");
    assert_eq!(
        second.images.len(),
        1,
        "images always emit for consumer compositor"
    );

    // ─── 7. Finalize → FrozenBlock carries replay + data ──────────
    let replay_bytes = block.replay().as_slice().to_vec();
    let frozen = block.finish(Some(0), None);
    assert_eq!(frozen.exit_code(), Some(0));
    assert_eq!(frozen.replay().as_slice(), replay_bytes.as_slice());
    assert_eq!(frozen.total_rows(), 3);

    // ─── 8. Replay round-trip matches the frozen grid ─────────────
    //
    // We only replay the initial "hello\nworld\n" portion that went
    // through the parser — the styled row and the image were injected
    // directly, not via a byte stream, so they wouldn't be in the
    // replay by design. Verify the parser-driven rows match though.
    let mut replayed = ActiveBlock::new(COLS);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(COLS, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut replayed);
    processor.advance(&mut writer, &replay_bytes);
    writer.commit_row();

    writer.finalize();

    // Compare the first two rows (the parser-driven ones) cell-wise.
    let frozen_rows: Vec<Vec<Cell>> = (0..2)
        .map(|r| frozen.grid().row(r).expect("frozen row").to_vec())
        .collect();
    let replayed_rows: Vec<Vec<Cell>> = (0..2)
        .map(|r| replayed.grid().row(r).expect("replayed row").to_vec())
        .collect();

    let diff = diff_grids(&frozen_rows, &replayed_rows);
    // Replay reproduces the parser-driven content byte-for-byte.
    assert!(
        diff.is_empty(),
        "replay diff should be empty, got {} entries",
        diff.len()
    );
}

#[test]
fn cross_layer_smoke_seq_output() {
    // Smaller, tighter smoke: drive `seq 1 20`-style output, build
    // one frame, verify cells shape matches expectations.
    const COLS: u16 = 8;
    let mut block = ActiveBlock::new(COLS);
    let mut input = String::new();
    for i in 1..=20u32 {
        input.push_str(&format!("{i}\n"));
    }
    drive(&mut block, COLS, input.as_bytes());
    assert_eq!(block.total_rows(), 20);

    let frame = build_frame(FrameInput {
        pages: block.grid(),
        atlas: block.atlas(),
        visible_rows: 0..20,
        viewport_cols: COLS,
        cursor: CursorState {
            visible: false,
            ..CursorState {
                row: 0,
                col: 0,
                shape: CursorShape::Block,
                blink_phase_on: true,
                visible: true,
                cell_tag: CellTag::Ascii,
            }
        },
        prev: &FrameState::empty(),
        palette: &carrot_block_render::TerminalPalette::CARROT_DARK,
        underline_color_override: None,
        images: None,
        cell_pixel_width: 8.0,
        cell_pixel_height: 16.0,
    });

    assert_eq!(frame.visual_rows, 20);
    assert_eq!(frame.cells.len(), 20 * 8); // dense full-paint
    assert!(frame.cursor.is_none()); // hidden cursor
    assert!(frame.images.is_empty());
    assert!(frame.decorations.is_empty());
}
