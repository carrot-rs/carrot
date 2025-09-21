//! Micro-benchmarks for the `inazuma::block` state operations.
//!
//! These run against `BlockState` directly — no window/app context — and
//! exercise the SumTree-backed data structure the element relies on. Per-op
//! cost grows logarithmically with the entry count, which is what we want
//! out of a virtualised list for terminal output that can grow without bound.
//!
//! Sample run on Apple Silicon (2026-04):
//!
//! ```text
//! block_push/100            ~154 µs  (bulk-push of N entries, so N·log N work)
//! block_push/1000           ~1.96 ms
//! block_push/10000          ~32 ms
//!
//! block_update_size/100     ~2.47 µs
//! block_update_size/1000    ~3.60 µs
//! block_update_size/10000   ~4.92 µs
//!
//! block_scrollbar_offset/100    ~12 ns
//! block_scrollbar_offset/1000   ~30 ns
//! block_scrollbar_offset/10000  ~44 ns
//! ```
//!
//! Absolute numbers differ across hardware — the point is the scaling:
//! `update_size` and `set_offset_from_scrollbar` each grow by ~2× when the
//! entry count grows 100×, confirming the log-shaped curve.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use inazuma::{
    BlockConfig, BlockMetadata, BlockState, Point, ScrollBehavior, VisualAnchor, px, size,
};

const SIZES: &[usize] = &[100, 1000, 10_000];

fn make_state(n: usize) -> BlockState {
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Bottom)
            .scroll_behavior(ScrollBehavior::FollowTail),
    );
    for _ in 0..n {
        let id = state.push(BlockMetadata::default(), None);
        state.update_size(id, size(px(100.0), px(20.0)));
    }
    state
}

fn bench_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_push");
    for &n in SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let state = BlockState::new(BlockConfig::default());
                for _ in 0..n {
                    let id = state.push(BlockMetadata::default(), None);
                    state.update_size(id, size(px(100.0), px(20.0)));
                }
                black_box(state.entry_count());
            });
        });
    }
    group.finish();
}

fn bench_update_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_update_size");
    for &n in SIZES {
        let state = make_state(n);
        // make_state always pushes n entries, so id_at_index(n/2) is Some.
        let id = match state.id_at_index(n / 2) {
            Some(id) => id,
            None => continue,
        };
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            let mut h = 20.0_f32;
            b.iter(|| {
                h = if h < 400.0 { h + 1.0 } else { 20.0 };
                state.update_size(black_box(id), size(px(100.0), px(h)));
            });
        });
    }
    group.finish();
}

fn bench_scrollbar_offset(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_scrollbar_offset");
    for &n in SIZES {
        let state = make_state(n);
        let total_px = state.max_offset_for_scrollbar().y;
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            let mut t = px(0.0);
            b.iter(|| {
                t = if t < total_px { t + px(10.0) } else { px(0.0) };
                state.set_offset_from_scrollbar(Point { x: px(0.0), y: -t });
                black_box(state.logical_scroll_top());
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_push,
    bench_update_size,
    bench_scrollbar_offset
);
criterion_main!(benches);
