//! Resize bench.
//!
//! Budget: resize 4K → 1080p and back with 0 dropped frames at
//! native refresh rate. Without a real window we can't measure
//! frame drops, but we can measure the render cost at each
//! viewport size — the budget is met when every viewport fits
//! inside the 16.7 ms (60 Hz) / 8.3 ms (120 Hz) / 6.9 ms (144 Hz)
//! frame slot.
//!
//! Scenarios:
//! - 4K (3840×2160) at 14px cell → ~274×154 cells
//! - 1440p (2560×1440) at 14px cell → ~183×103 cells
//! - 1080p (1920×1080) at 14px cell → ~137×77 cells
//!
//! For each viewport we build a matching PageList full of content
//! and measure `render_block_damaged` from an empty FrameState
//! (worst-case: every cell is "damaged").

use std::time::Instant;

use carrot_block_render::{BlockRenderInput, FrameState, render_block_damaged};
use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, PageCapacity, PageList};
use criterion::{Criterion, criterion_group, criterion_main};

const BUDGET_MS_120HZ: u128 = 8;

struct Viewport {
    name: &'static str,
    cols: u16,
    rows: usize,
}

const VIEWPORTS: &[Viewport] = &[
    Viewport {
        name: "1080p",
        cols: 137,
        rows: 77,
    },
    Viewport {
        name: "1440p",
        cols: 183,
        rows: 103,
    },
    Viewport {
        name: "4k",
        cols: 274,
        rows: 154,
    },
];

fn build_list(cols: u16, rows: usize) -> (PageList, CellStyleAtlas) {
    let cap = PageCapacity::new(cols, 4096);
    let mut list = PageList::new(cap);
    for r in 0..rows {
        let row: Vec<Cell> = (0..cols)
            .map(|c| Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), CellStyleId(0)))
            .collect();
        list.append_row(&row);
    }
    (list, CellStyleAtlas::new())
}

fn bench_resize(c: &mut Criterion) {
    for viewport in VIEWPORTS {
        let (list, atlas) = build_list(viewport.cols, viewport.rows);

        // One-shot worst-case measurement (empty FrameState →
        // Damage::Full → every cell re-drawn).
        let t = Instant::now();
        let _ = render_block_damaged(
            BlockRenderInput {
                pages: &list,
                atlas: &atlas,
                visible_rows: 0..viewport.rows,
                viewport_cols: viewport.cols,
            },
            &FrameState::empty(),
        );
        let elapsed = t.elapsed();
        eprintln!(
            "resize_full_frame/{}: {:?} (budget {} ms @ 120 Hz)",
            viewport.name, elapsed, BUDGET_MS_120HZ
        );
        assert!(
            elapsed.as_millis() < BUDGET_MS_120HZ + 1,
            "{} full-frame render {:?} exceeded {} ms 120-Hz budget",
            viewport.name,
            elapsed,
            BUDGET_MS_120HZ,
        );

        c.bench_function(&format!("resize_full_frame_{}", viewport.name), |b| {
            b.iter(|| {
                render_block_damaged(
                    BlockRenderInput {
                        pages: &list,
                        atlas: &atlas,
                        visible_rows: 0..viewport.rows,
                        viewport_cols: viewport.cols,
                    },
                    &FrameState::empty(),
                )
            })
        });
    }
}

criterion_group!(benches, bench_resize);
criterion_main!(benches);
