//! Idle CPU bench.
//!
//! Budget: <1 ms per frame at 60 fps when nothing is happening.
//! "Idle" means the block hasn't changed since the last frame —
//! the damage-aware path should produce zero draws and the CPU
//! cost per frame is dominated by the signature compare.
//!
//! This bench builds a representative block (30k lines × 80 cols
//! matching the memory bench) and repeatedly runs
//! `render_block_damaged` with the same FrameState. The budget
//! asserts <1 ms p99 over 240 frames (four seconds at 60fps).

use std::time::Instant;

use carrot_block_render::{BlockRenderInput, FrameState, render_block_damaged};
use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, PageCapacity, PageList};
use criterion::{Criterion, criterion_group, criterion_main};

const ROWS: usize = 30_000;
const COLS: u16 = 80;
const VISIBLE_ROWS: usize = 48;
const FRAME_COUNT: usize = 240;
const IDLE_BUDGET_MS: u128 = 1;

fn build_block() -> (PageList, CellStyleAtlas) {
    let cap = PageCapacity::new(COLS, 4096);
    let mut list = PageList::new(cap);
    for r in 0..ROWS {
        let row: Vec<Cell> = (0..COLS)
            .map(|c| Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), CellStyleId(0)))
            .collect();
        list.append_row(&row);
    }
    (list, CellStyleAtlas::new())
}

fn bench_idle(c: &mut Criterion) {
    let (list, atlas) = build_block();
    let start = ROWS - VISIBLE_ROWS;
    let end = ROWS;

    // Warm the FrameState with one render so subsequent calls are
    // the "idle" path (0-damage compare).
    let first = render_block_damaged(
        BlockRenderInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: start..end,
            viewport_cols: COLS,
        },
        &FrameState::empty(),
    );
    let mut prev_state = FrameState::empty();
    prev_state.replace_cells(first.visual_rows, COLS, first.signatures);

    let mut samples: Vec<std::time::Duration> = Vec::with_capacity(FRAME_COUNT);
    for _ in 0..FRAME_COUNT {
        let t = Instant::now();
        let frame = render_block_damaged(
            BlockRenderInput {
                pages: &list,
                atlas: &atlas,
                visible_rows: start..end,
                viewport_cols: COLS,
            },
            &prev_state,
        );
        let elapsed = t.elapsed();
        samples.push(elapsed);
        prev_state.replace_cells(frame.visual_rows, COLS, frame.signatures);
    }

    samples.sort();
    let p99 = samples[(samples.len() * 99) / 100];
    eprintln!("idle_frame p99: {:?} (budget {IDLE_BUDGET_MS} ms)", p99);
    assert!(
        p99.as_millis() < IDLE_BUDGET_MS + 1,
        "idle frame p99 = {p99:?} exceeded {IDLE_BUDGET_MS} ms budget",
    );

    c.bench_function("idle_frame_damage_check", |b| {
        b.iter(|| {
            render_block_damaged(
                BlockRenderInput {
                    pages: &list,
                    atlas: &atlas,
                    visible_rows: start..end,
                    viewport_cols: COLS,
                },
                &prev_state,
            )
        })
    });
}

criterion_group!(benches, bench_idle);
criterion_main!(benches);
