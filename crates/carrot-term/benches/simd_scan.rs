//! Throughput benchmark for the SIMD control-byte scanner.
//!
//! Perf budget (plan A6): ≥ 2 GB/s reiner ASCII-Durchsatz on M-class
//! and modern x86. We measure:
//!
//! - `all_printable_4kb` — 4 KiB of pure ASCII, no control bytes. Pure
//!   fast-path throughput.
//! - `all_printable_64kb` — 64 KiB, same shape, larger input.
//! - `esc_at_far_end` — worst case for the fast path: scan a whole
//!   chunk only to find the one control byte at the last position.
//! - `esc_early` — best case for the fast path: quickly bail out.
//! - `scalar_vs_simd` — both implementations timed for comparison.
//!
//! Sample numbers on Apple M-class NEON (2026-04-20):
//!
//! ```text
//! SIMD (NEON):
//! all_printable/1024_bytes        ≈ 66 ns         thrpt ≈ 14.3 GiB/s
//! all_printable/4096_bytes        ≈ 436 ns        thrpt ≈  8.7 GiB/s
//! all_printable/65536_bytes       ≈ 5.8 µs        thrpt ≈ 10.5 GiB/s
//! all_printable/1048576_bytes     ≈ 57 µs         thrpt ≈ 17.0 GiB/s  ← peak
//!
//! Scalar fallback:
//! all_printable/1024_bytes                        thrpt ≈  1.65 GiB/s
//! all_printable/4096_bytes                        thrpt ≈  1.54 GiB/s
//! all_printable/65536_bytes                       thrpt ≈  2.01 GiB/s
//!
//! SIMD bail-out latency:
//! esc_at_0                        ≈ 1.8 ns
//! esc_at_16                       ≈ 2.6 ns
//! esc_at_256                      ≈ 18 ns
//! esc_at_4095 (full scan)         ≈ 288 ns
//! ```
//!
//! SIMD path is 5–10× faster than scalar across all sizes. Plan budget
//! (A6): ≥ 2 GB/s on M-class → we hit **17 GB/s**, ~8× over budget.

use carrot_term::simd_scan::{scan_control, scan_control_scalar};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

fn bench_all_printable(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_scan/all_printable");
    for &size in &[1024usize, 4096, 65_536, 1 << 20] {
        let input: Vec<u8> = (0..size).map(|i| b'a' + ((i as u8) % 26)).collect();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{size}_bytes")),
            &input,
            |b, input| {
                b.iter(|| {
                    black_box(scan_control(black_box(input)));
                });
            },
        );
    }
    group.finish();
}

fn bench_scalar_all_printable(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_scan/scalar_all_printable");
    for &size in &[1024usize, 4096, 65_536] {
        let input: Vec<u8> = (0..size).map(|i| b'a' + ((i as u8) % 26)).collect();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{size}_bytes")),
            &input,
            |b, input| {
                b.iter(|| {
                    black_box(scan_control_scalar(black_box(input)));
                });
            },
        );
    }
    group.finish();
}

fn bench_esc_position(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_scan/esc_position");
    for pos in [0usize, 16, 256, 4095] {
        let mut input = vec![b'x'; 4096];
        input[pos] = 0x1B;
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("esc_at_{pos}")),
            &input,
            |b, input| {
                b.iter(|| {
                    black_box(scan_control(black_box(input)));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_all_printable,
    bench_scalar_all_printable,
    bench_esc_position
);
criterion_main!(benches);
