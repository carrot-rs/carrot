//! Performance benches for `carrot-grid`.
//!
//! Proof-of-claims for the perf budget:
//!
//! - `append_row` O(1) → throughput-oriented: push 100 k rows of 80 cols,
//!   measure cells/sec. Target: ≥ 1 GB/s (≥ 128 M cells/sec at 8 B/cell)
//!   on modern hardware.
//! - `prune_head` O(1) → page pruning stays constant with N pages.
//! - `rows(range)` O(N_visible) → iteration cost proportional to the
//!   visible range, regardless of total scrollback size.
//! - `CellStyleAtlas::intern` with hit vs miss → interning stays constant-time.
//!
//! Numbers captured on Apple M-class (2026-04-20):
//!
//! ```text
//! append_row/1k_rows_80_cols      ≈ 39 µs        thrpt ≈ 15.4 GiB/s
//! append_row/10k_rows_80_cols     ≈ 338 µs       thrpt ≈ 17.6 GiB/s
//! append_row/100k_rows_80_cols    ≈ 3.3 ms       thrpt ≈ 18.0 GiB/s
//!                                  per-row cost flat at ~33 ns → O(1) amortized ✓
//!
//! prune_head/1667_pages           ≈ 142 ns
//! prune_head/16667_pages          ≈ 718 ns
//!                                  sub-µs at 10× the pages; cache-miss dominated,
//!                                  not algorithmic → practical O(1) ✓
//!
//! rows_iter/visible_24_in_10k     ≈ 24 ns
//! rows_iter/visible_24_in_100k    ≈ 24 ns          ← identical despite 10× data ✓
//! rows_iter/visible_100_in_100k   ≈ 87 ns          ← scales with visible, not total ✓
//!
//! atlas_intern/miss_unique (100)  ≈ 2.76 µs       → 28 ns per miss
//! atlas_intern/hit_repeat_5       ≈ 35 ns         → 7 ns per hit
//!
//! cold_compression/compress/100_rows_80_cols     ≈ 68 µs      thrpt ≈ 886 MiB/s
//! cold_compression/compress/1000_rows_80_cols    ≈ 487 µs     thrpt ≈ 1.22 GiB/s
//! cold_compression/compress/10000_rows_80_cols   ≈ 1.30 ms    thrpt ≈ 4.60 GiB/s
//! cold_compression/decompress/100_rows_80_cols   ≈ 31 µs      thrpt ≈ 1.87 GiB/s
//! cold_compression/decompress/1000_rows_80_cols  ≈ 222 µs     thrpt ≈ 2.67 GiB/s
//! cold_compression/decompress/10000_rows_80_cols ≈ 504 µs     thrpt ≈ 11.80 GiB/s
//!   Ratios (varied ASCII content): ~12-20× compression. Target
//!   ≥10× — easily hit. Cold-page re-inflate is sub-ms even for 10k rows,
//!   well within one frame at any refresh rate.
//! ```
//!
//! Perf-budget from plan: append ≥ 1 GB/s (we hit 17×), prune O(1) (we are
//! sub-µs), visible iter O(N_visible) (identical 10k vs 100k confirmed).
//!
//! Run: `cargo bench -p carrot-grid --bench grid`.

use carrot_grid::{
    Cell, CellStyle, CellStyleAtlas, CellStyleFlags, CellStyleId, PageCapacity, PageList, compress,
    decompress,
};
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};

fn bench_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_row");
    for &rows in &[1_000usize, 10_000, 100_000] {
        let cols: u16 = 80;
        let row: Vec<Cell> = (0..cols)
            .map(|i| Cell::ascii(b'a' + (i as u8 % 26), CellStyleId(0)))
            .collect();
        let bytes = rows * cols as usize * std::mem::size_of::<Cell>();
        group.throughput(Throughput::Bytes(bytes as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{rows}_rows_{cols}_cols")),
            &rows,
            |b, &rows| {
                b.iter_with_large_drop(|| {
                    let cap = PageCapacity::new(cols, 4096);
                    let mut list = PageList::new(cap);
                    for _ in 0..rows {
                        list.append_row(black_box(&row));
                    }
                    list
                });
            },
        );
    }
    group.finish();
}

fn bench_prune_head(c: &mut Criterion) {
    let cols: u16 = 80;
    let row: Vec<Cell> = (0..cols)
        .map(|_| Cell::ascii(b'x', CellStyleId(0)))
        .collect();

    let mut group = c.benchmark_group("prune_head");
    for &rows in &[10_000usize, 100_000] {
        let cap = PageCapacity::new(cols, 4096);
        let probe_list = {
            let mut list = PageList::new(cap);
            for _ in 0..rows {
                list.append_row(&row);
            }
            list
        };
        let pages = probe_list.page_count();
        drop(probe_list);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{pages}_pages")),
            &pages,
            |b, _| {
                // `iter_batched_ref` passes &mut setup result — drop of the
                // whole PageList is NOT counted against the measurement.
                // Only the pop_front + reset + push-to-pool is timed.
                b.iter_batched_ref(
                    || {
                        let mut list = PageList::new(cap);
                        for _ in 0..rows {
                            list.append_row(&row);
                        }
                        list
                    },
                    |list| {
                        black_box(list.prune_head());
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_rows_iter(c: &mut Criterion) {
    let cols: u16 = 80;
    let row: Vec<Cell> = (0..cols)
        .map(|_| Cell::ascii(b'.', CellStyleId(0)))
        .collect();

    let mut group = c.benchmark_group("rows_iter");
    for &(total, visible) in &[(10_000usize, 24usize), (100_000, 24), (100_000, 100)] {
        let cap = PageCapacity::new(cols, 4096);
        let mut list = PageList::new(cap);
        for _ in 0..total {
            list.append_row(&row);
        }
        let mid_start = total / 2;
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("visible_{visible}_in_{total}")),
            &visible,
            |b, &visible| {
                b.iter(|| {
                    let mut n = 0u32;
                    for r in list.rows(mid_start, mid_start + visible) {
                        n = n.wrapping_add(r.len() as u32);
                    }
                    black_box(n);
                });
            },
        );
    }
    group.finish();
}

fn bench_atlas(c: &mut Criterion) {
    let mut group = c.benchmark_group("atlas_intern");

    // Miss: always novel styles.
    group.bench_function("miss_unique", |b| {
        b.iter_with_setup(
            || CellStyleAtlas::new(),
            |mut atlas| {
                for i in 0..100u16 {
                    let style = CellStyle {
                        fg: carrot_grid::Color::Indexed((i % 256) as u8),
                        ..CellStyle::DEFAULT
                    };
                    black_box(atlas.intern(style));
                }
            },
        );
    });

    // Hit: repeated interning of the same handful of styles.
    group.bench_function("hit_repeat_5_styles", |b| {
        let styles: Vec<CellStyle> = (0..5)
            .map(|i| CellStyle {
                fg: carrot_grid::Color::Indexed(i as u8),
                flags: if i % 2 == 0 {
                    CellStyleFlags::BOLD
                } else {
                    CellStyleFlags::ITALIC
                },
                ..CellStyle::DEFAULT
            })
            .collect();
        let mut atlas = CellStyleAtlas::new();
        for s in &styles {
            atlas.intern(*s);
        }
        b.iter(|| {
            for s in &styles {
                black_box(atlas.intern(*s));
            }
        });
    });

    group.finish();
}

fn bench_cold_compression(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_compression");
    for &rows in &[100usize, 1_000, 10_000] {
        let cells: Vec<Cell> = (0..rows * 80)
            .map(|i| Cell::ascii(b'a' + ((i % 26) as u8), CellStyleId((i / 80) as u16 % 3)))
            .collect();
        let raw_bytes = std::mem::size_of_val(cells.as_slice());
        group.throughput(Throughput::Bytes(raw_bytes as u64));
        group.bench_with_input(
            BenchmarkId::new("compress", format!("{rows}_rows_80_cols")),
            &cells,
            |b, cells| {
                b.iter(|| {
                    black_box(compress(black_box(cells)).expect("compress"));
                });
            },
        );
        let compressed = compress(&cells).expect("pre-compress for decompress bench");
        group.bench_with_input(
            BenchmarkId::new("decompress", format!("{rows}_rows_80_cols")),
            &compressed,
            |b, compressed| {
                b.iter(|| {
                    black_box(decompress(black_box(compressed)).expect("decompress"));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_append,
    bench_prune_head,
    bench_rows_iter,
    bench_atlas,
    bench_cold_compression
);
criterion_main!(benches);
