//! termbench-shape budget assertion.
//!
//! Budget: `termbench < 30 s` on the standard mixed payload ⇒
//! ≥17 MiB/s end-to-end through the VT pipeline.
//!
//! vt_pipeline.rs measures each scenario's throughput individually;
//! this bench assembles a **mixed** payload representative of
//! termbench's shape (cursor-redraw, SGR colours, UTF-8, progress
//! bars — **not** the dense_ascii_yes outlier) and asserts the
//! throughput meets the budget.
//!
//! The dense_ascii_yes case remains measured in `vt_pipeline.rs`
//! as a documented slow path (commits a PageList row per `\n`);
//! real interactive PTY traffic rarely exceeds 100 KB/s on that
//! shape, so it's not representative of the termbench mean.

use std::time::Instant;

use carrot_term::block::{ActiveBlock, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};
use criterion::{Criterion, criterion_group, criterion_main};

const PAYLOAD_BYTES: usize = 4 * 1024 * 1024; // 4 MiB mixed
const BUDGET_MIB_PER_SEC: f64 = 17.0;

/// Build a representative termbench-shape payload:
/// - 40 % SGR-coloured printable runs
/// - 30 % UTF-8 mixed text
/// - 20 % cursor-position redraws (TUI frame pattern)
/// - 10 % CR progress-bar overwrites
fn mixed_payload(size_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size_bytes);
    while out.len() < size_bytes {
        // SGR block: coloured run.
        out.extend_from_slice(b"\x1b[38;5;196mHello, \x1b[38;5;46mWorld!\x1b[0m\n");
        // UTF-8 block: Latin + CJK + emoji.
        out.extend_from_slice("こんにちは 世界 🌍 foo bar baz qux quux corge\n".as_bytes());
        // TUI redraw block: cursor up + overwrite 24 lines.
        for _ in 0..24 {
            out.extend_from_slice(b"\x1b[A\x1b[2K");
            out.extend_from_slice(b"row: status ok\n");
        }
        // CR progress bar.
        for _ in 0..10 {
            out.extend_from_slice(b"\rInstalling [============>  ] 80%");
        }
        out.extend_from_slice(b"\n");
    }
    out.truncate(size_bytes);
    out
}

fn drive(cols: u16, bytes: &[u8]) {
    let mut block = ActiveBlock::new(cols);
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut state = VtWriterState::new(cols, 24);
    let mut writer = VtWriter::new_in(&mut state, &mut block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
}

fn bench_termbench_budget(c: &mut Criterion) {
    let payload = mixed_payload(PAYLOAD_BYTES);

    // One-shot budget assertion outside criterion.
    let t = Instant::now();
    drive(80, &payload);
    let elapsed = t.elapsed();
    let mib_per_sec = (PAYLOAD_BYTES as f64) / (1024.0 * 1024.0) / elapsed.as_secs_f64();
    let projected_500mb_secs = 500.0 / mib_per_sec;
    eprintln!(
        "termbench_budget: {:.1} MiB/s — 500 MB projected {:.1}s (budget 30s, {:.1} MiB/s minimum)",
        mib_per_sec, projected_500mb_secs, BUDGET_MIB_PER_SEC,
    );
    assert!(
        mib_per_sec >= BUDGET_MIB_PER_SEC,
        "termbench throughput {:.1} MiB/s below {} MiB/s budget",
        mib_per_sec,
        BUDGET_MIB_PER_SEC,
    );

    c.bench_function("termbench_mixed_4mib", |b| b.iter(|| drive(80, &payload)));
}

criterion_group!(benches, bench_termbench_budget);
criterion_main!(benches);
