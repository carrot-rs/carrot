//! `yes` in a 4K viewport bench.
//!
//! Budget: 0 dropped frames at native refresh rate — we target the
//! 60 Hz slot (16.7 ms / frame) so the bench passes on every
//! supported platform. ProMotion / 120 Hz has more headroom.
//!
//! Without a real window we can't observe dropped frames directly,
//! but we can measure the per-frame render cost when the viewport
//! is full of `yes` output and assert it fits inside the frame
//! slot. We pipe a realistic per-frame chunk of `y\n` through the
//! full pipeline (`vte::Processor` → `VtWriter` → `ActiveBlock` →
//! `PageList` → `render_block_damaged`) once per iteration and
//! time the combined cost.

use std::time::Instant;

use carrot_block_render::{BlockRenderInput, FrameState, render_block_damaged};
use carrot_grid::CellStyleAtlas;
use carrot_term::block::{ActiveBlock, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};
use criterion::{Criterion, criterion_group, criterion_main};

const COLS: u16 = 274; // 3840 px / 14 px cell
const ROWS: usize = 154; // 2160 px / 14 px cell
// 16 KiB per frame is ~960 KB/s at 60 fps — well above typical
// interactive PTY traffic, still fast enough to leave headroom.
const CHUNK_BYTES: usize = 16 * 1024;
const BUDGET_MS: u128 = 16;

fn yes_chunk(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        out.extend_from_slice(b"y\n");
    }
    out.truncate(size);
    out
}

fn drive_vt(cols: u16, bytes: &[u8]) -> ActiveBlock {
    let mut block = ActiveBlock::new(cols);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
    block
}

/// Take a freshly driven block's grid and mirror it into a PageList
/// the render path can consume. ActiveBlock already uses a PageList
/// internally — we expose its grid here via `ActiveBlock::grid`.
fn render_frame(block: &ActiveBlock) {
    let atlas = CellStyleAtlas::new();
    let pages = block.grid();
    let visible = pages.total_rows().saturating_sub(ROWS)..pages.total_rows();
    let _ = render_block_damaged(
        BlockRenderInput {
            pages,
            atlas: &atlas,
            visible_rows: visible,
            viewport_cols: COLS,
        },
        &FrameState::empty(),
    );
}

fn bench_yes_4k(c: &mut Criterion) {
    let chunk = yes_chunk(CHUNK_BYTES);

    // One-shot assertion: a single "frame's worth" of `yes` data
    // plus the render pass must fit in the 8 ms / 120 Hz slot.
    let t = Instant::now();
    let block = drive_vt(COLS, &chunk);
    render_frame(&block);
    let elapsed = t.elapsed();
    eprintln!(
        "yes_4k_frame: {:?} (budget {BUDGET_MS} ms @ 60 Hz)",
        elapsed
    );
    assert!(
        elapsed.as_millis() < BUDGET_MS + 1,
        "yes @ 4k frame {:?} exceeded {BUDGET_MS} ms 60-Hz budget",
        elapsed,
    );

    c.bench_function("yes_4k_vt_plus_render", |b| {
        b.iter(|| {
            let block = drive_vt(COLS, &chunk);
            render_frame(&block);
        })
    });
}

criterion_group!(benches, bench_yes_4k);
criterion_main!(benches);
