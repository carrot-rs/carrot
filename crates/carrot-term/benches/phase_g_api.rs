//! Phase-G API-gate benches.
//!
//! Validates the new block primitives hit their Phase-G perf
//! budgets on a realistic corpus:
//!
//! - `select_and_copy`: drive a 30k-row scrollback buffer, open a
//!   full-buffer [`carrot_term::block::BlockSelection`], and
//!   materialise it via `to_string`. Budget: under 150 ms.
//! - `search_regex`: build a 30k-row router-backed block containing
//!   mixed output, then count `"error"` (case-insensitive) matches
//!   via [`carrot_term::block::BlockRouter::search`]. Budget:
//!   under 100 ms.
//!
//! Run with `cargo bench -p carrot-term --bench phase_g_api`. The
//! criterion harness emits a JSON report that the CI perf gate can
//! compare against the budgets.

use std::time::Duration;

use carrot_grid::search::SearchOptions;
use carrot_grid::{Cell, CellStyle, CellStyleAtlas};
use carrot_term::block::{ActiveBlock, BlockRouter, BlockSelection, SelectionKind, Side};
use criterion::{Criterion, criterion_group, criterion_main};

const COLS: u16 = 80;
const ROWS: usize = 30_000;

fn make_styled_block() -> ActiveBlock {
    let mut block = ActiveBlock::new(COLS);
    let style_id = block.intern_style(CellStyle::DEFAULT);
    let mut row = vec![Cell::EMPTY; COLS as usize];
    for i in 0..ROWS {
        // Fill with some actual content so selection has something
        // to materialise — mix `error` into every 17th row so the
        // search bench has realistic hit density.
        let include_error = i % 17 == 0;
        let text: String = if include_error {
            format!("row {:05} error: something failed ", i)
        } else {
            format!("row {:05} normal shell output text here", i)
        };
        for (col, byte) in text.bytes().take(COLS as usize).enumerate() {
            row[col] = Cell::ascii(byte, style_id);
        }
        for cell in row.iter_mut().skip(text.len().min(COLS as usize)) {
            *cell = Cell::EMPTY;
        }
        block.append_row(&row);
    }
    // Force the atlas to look realistic (single shared style is fine —
    // selection + search both walk cells regardless).
    let _ = block.atlas();
    let _ = CellStyleAtlas::new();
    block
}

fn bench_select_and_copy(c: &mut Criterion) {
    let block = make_styled_block();
    let mut group = c.benchmark_group("phase_g_prep");
    group.measurement_time(Duration::from_secs(5));
    group.bench_function("select_and_copy", |b| {
        b.iter(|| {
            let start = carrot_grid::CellId::new(0, 0);
            let end = carrot_grid::CellId::new(ROWS as u64 - 1, COLS - 1);
            let mut sel = BlockSelection::new(start, SelectionKind::Simple, Side::Left);
            sel.update(end, Side::Right);
            let s = sel.to_string(block.grid(), block.graphemes());
            criterion::black_box(s);
        });
    });
    group.finish();
}

fn bench_search_regex(c: &mut Criterion) {
    // Router-level: one live block with the corpus. Mirrors how
    // Layer 5 will drive search at runtime.
    let mut router = BlockRouter::new(COLS);
    router.on_command_start();
    if let carrot_term::block::ActiveTarget::Block { block, .. } = router.active() {
        let style = block.intern_style(CellStyle::DEFAULT);
        let mut row = vec![Cell::EMPTY; COLS as usize];
        for i in 0..ROWS {
            let include_error = i % 17 == 0;
            let text: String = if include_error {
                format!("row {:05} error: something failed ", i)
            } else {
                format!("row {:05} normal shell output text here", i)
            };
            for (col, byte) in text.bytes().take(COLS as usize).enumerate() {
                row[col] = Cell::ascii(byte, style);
            }
            for cell in row.iter_mut().skip(text.len().min(COLS as usize)) {
                *cell = Cell::EMPTY;
            }
            block.append_row(&row);
        }
    }
    let mut group = c.benchmark_group("phase_g_prep");
    group.measurement_time(Duration::from_secs(5));
    group.bench_function("search_regex_like", |b| {
        let opt = SearchOptions::default().case_insensitive(true);
        b.iter(|| {
            let n = router.search_count("error", opt);
            criterion::black_box(n);
        });
    });
    group.finish();
}

criterion_group!(phase_g_benches, bench_select_and_copy, bench_search_regex);
criterion_main!(phase_g_benches);
