//! End-to-end VT-pipeline throughput benchmark.
//!
//! Drives synthetic byte streams through the **full** new-world
//! pipeline — `vte::Processor` → `VtWriter` → `ActiveBlock` →
//! `carrot-grid::PageList` — and reports bytes-per-second.
//!
//! Scenarios mirror the termbench suite's shape without pulling in
//! termbench itself:
//!
//! - `dense_ascii_yes` — a long run of `y\n` as `yes` emits. Pure
//!   printable throughput, the fast-path baseline.
//! - `seq_1_to_10k` — `seq` output, numbers + newlines. Realistic
//!   scripting workload.
//! - `alternating_sgr_colors` — cells interleaved with SGR escape
//!   sequences that switch colour. Exercises the parser's CSI
//!   state machine and CellStyleAtlas interning.
//! - `utf8_text` — mixed Latin / CJK / emoji. Multibyte continuation
//!   decoding.
//! - `cr_progress_bar` — `\r` + overwrite pattern. Classic
//!   cargo / pip install progress.
//! - `cursor_tui_redraw` — `\r`, cursor-up, mass overwrite. Pattern
//!   that historically stresses terminals into dropping frames.
//!
//! Numbers from a run go into the docstring below so regressions are
//! visible in diffs.
//!
//! Sample numbers on Apple M-class (2026-04-21):
//!
//! ```text
//! dense_ascii_yes / 1 MiB              ≈ 35.6 ms    thrpt ≈  28.1 MiB/s
//! seq_1_to_10k                          ≈ 912 µs     thrpt ≈  51.1 MiB/s
//! alternating_sgr_colors / 1 MiB        ≈ 4.63 ms    thrpt ≈ 216.1 MiB/s
//! utf8_text / 100 KiB                   ≈ 469 µs     thrpt ≈ 208.4 MiB/s
//! cr_progress_bar / 1000 ticks          ≈ 91 µs      thrpt ≈ 501.9 MiB/s
//! cursor_tui_redraw / 1000 frames       ≈ 824 µs     thrpt ≈ 247.8 MiB/s
//! ```
//!
//! Budget: `termbench < 30 s` on
//! ~500 MB payload ⇒ ≥ 17 MiB/s end-to-end throughput required.
//! Worst case here (dense_ascii_yes) is 28 MiB/s — **1.65× over
//! budget**. Typical workloads hit 200–500 MiB/s.
//!
//! dense_ascii_yes is slowest because every `\n` commits a row to
//! the PageList; per-row overhead dominates. Rare in real PTY data
//! (typical interactive PTY rates < 100 KB/s). Commonly-hit
//! workloads (sgr, tui, progress) all run ≥ 200 MiB/s.

use carrot_term::block::{ActiveBlock, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

/// Run `bytes` through the full new-world pipeline. Returns the
/// block so criterion's `black_box` keeps the work alive.
fn drive(cols: u16, bytes: &[u8]) -> ActiveBlock {
    let mut block = ActiveBlock::new(cols);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
    block
}

fn dense_ascii_yes(size_bytes: usize) -> Vec<u8> {
    // `yes` output: "y\n" repeated. `size_bytes` is total payload.
    let mut out = Vec::with_capacity(size_bytes);
    while out.len() < size_bytes {
        out.extend_from_slice(b"y\n");
    }
    out.truncate(size_bytes);
    out
}

fn seq_1_to_n(n: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 1..=n {
        out.extend_from_slice(format!("{i}\n").as_bytes());
    }
    out
}

fn alternating_sgr_colors(size_bytes: usize) -> Vec<u8> {
    // 80 bytes per cycle: 40 "red" cells, 40 "green" cells.
    let mut out = Vec::with_capacity(size_bytes);
    let mut toggle = false;
    while out.len() < size_bytes {
        if toggle {
            out.extend_from_slice(b"\x1b[31m");
            out.extend_from_slice(b"hellohellohellohellohellohellohellohello");
        } else {
            out.extend_from_slice(b"\x1b[32m");
            out.extend_from_slice(b"worldworldworldworldworldworldworldworld");
        }
        out.extend_from_slice(b"\n");
        toggle = !toggle;
    }
    out.truncate(size_bytes);
    out
}

fn utf8_text(size_bytes: usize) -> Vec<u8> {
    // Mixed Latin / German umlauts / CJK / emoji. Rotates to hit
    // every multibyte-length class.
    let samples = [
        "hello world",
        "héllo wörld",
        "こんにちは世界",
        "日本語のテスト",
        "Привет мир",
        "שלום עולם",
    ];
    let mut out = Vec::with_capacity(size_bytes);
    let mut i = 0;
    while out.len() < size_bytes {
        out.extend_from_slice(samples[i % samples.len()].as_bytes());
        out.extend_from_slice(b"\n");
        i += 1;
    }
    out.truncate(size_bytes);
    out
}

fn cr_progress_bar(ticks: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for pct in 0..ticks {
        let filled = (pct * 40) / ticks.max(1);
        let mut line = Vec::with_capacity(60);
        line.push(b'\r');
        line.push(b'[');
        for _ in 0..filled {
            line.push(b'#');
        }
        for _ in filled..40 {
            line.push(b' ');
        }
        line.push(b']');
        line.push(b' ');
        line.extend_from_slice(format!("{pct:3}%").as_bytes());
        out.extend_from_slice(&line);
    }
    out.push(b'\n');
    out
}

fn cursor_tui_redraw(frames: usize) -> Vec<u8> {
    // Each frame: cursor-up 5, then overwrite 5 rows of 40 chars.
    let mut out = Vec::new();
    for _ in 0..frames {
        out.extend_from_slice(b"\x1b[5A"); // cursor up 5
        for row in 0..5u8 {
            out.push(b'\r');
            for c in 0..40u8 {
                out.push(b'a' + ((row + c) % 26));
            }
            out.push(b'\n');
        }
    }
    out
}

fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("vt_pipeline");

    let yes = dense_ascii_yes(1024 * 1024);
    group.throughput(Throughput::Bytes(yes.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("dense_ascii_yes", "1MiB"),
        &yes,
        |b, input| {
            b.iter(|| {
                black_box(drive(80, black_box(input)));
            });
        },
    );

    let seq = seq_1_to_n(10_000);
    group.throughput(Throughput::Bytes(seq.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("seq_1_to_n", "10_000"),
        &seq,
        |b, input| {
            b.iter(|| {
                black_box(drive(80, black_box(input)));
            });
        },
    );

    let sgr = alternating_sgr_colors(1024 * 1024);
    group.throughput(Throughput::Bytes(sgr.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("alternating_sgr_colors", "1MiB"),
        &sgr,
        |b, input| {
            b.iter(|| {
                black_box(drive(80, black_box(input)));
            });
        },
    );

    let utf = utf8_text(100 * 1024);
    group.throughput(Throughput::Bytes(utf.len() as u64));
    group.bench_with_input(BenchmarkId::new("utf8_text", "100KiB"), &utf, |b, input| {
        b.iter(|| {
            black_box(drive(80, black_box(input)));
        });
    });

    let progress = cr_progress_bar(1000);
    group.throughput(Throughput::Bytes(progress.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("cr_progress_bar", "1000_ticks"),
        &progress,
        |b, input| {
            b.iter(|| {
                black_box(drive(80, black_box(input)));
            });
        },
    );

    let tui = cursor_tui_redraw(1000);
    group.throughput(Throughput::Bytes(tui.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("cursor_tui_redraw", "1000_frames"),
        &tui,
        |b, input| {
            b.iter(|| {
                black_box(drive(80, black_box(input)));
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
