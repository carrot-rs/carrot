//! p99 latency measurement for the every-keystroke pipeline.
//!
//! Budgets:
//!   keystroke latency       <1 ms  p99  (1.000 random keypresses)
//!   semantic AST reparse    <2 ms  p99  (keystroke bench)
//!
//! Criterion's default bench reports p50; this file measures the
//! full sorted latency distribution over N samples and fails the
//! run when p99 busts the budget. Keeps the gate automated so a
//! regression shows up immediately instead of buried in a summary.

use std::time::Instant;

use carrot_cmdline::{
    ast::CommandAst, highlight::highlight_ast, parse::parse_simple, validation::ValidationContext,
    validation::validate,
};
use criterion::{Criterion, criterion_group, criterion_main};

/// Realistic keystroke corpus: 1.000 lines from actual shell use,
/// generated procedurally from a small set of templates so the
/// bench is reproducible.
fn corpus() -> Vec<String> {
    const VERBS: &[&str] = &[
        "ls", "cd", "git", "cargo", "npm", "docker", "kubectl", "curl", "grep", "find", "rg",
        "ssh", "sudo", "make", "pwd",
    ];
    const TAILS: &[&str] = &[
        "",
        " -la",
        " status",
        " checkout main",
        " build --release",
        " install foo",
        " run -p carrot-cmdline",
        " push origin main",
        " log --oneline",
        " -rf ./target",
    ];
    let mut out = Vec::with_capacity(1_000);
    let mut seed = 0u64;
    for _ in 0..1_000 {
        // Tiny deterministic LCG so the bench is reproducible.
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let verb = VERBS[(seed as usize) % VERBS.len()];
        let tail = TAILS[((seed >> 32) as usize) % TAILS.len()];
        out.push(format!("{verb}{tail}"));
    }
    out
}

/// Run the full parse → highlight → validate pipeline once, return
/// elapsed time.
fn one_keystroke(input: &str, ctx: &ValidationContext<'_>) -> std::time::Duration {
    let t = Instant::now();
    let ast: CommandAst = parse_simple(input);
    let _spans = highlight_ast(&ast);
    let _errors = validate(&ast, ctx);
    t.elapsed()
}

fn bench_p99(c: &mut Criterion) {
    let corpus = corpus();
    let ctx = ValidationContext::default();
    c.bench_function("keystroke_pipeline_p99", |b| {
        b.iter(|| {
            let mut samples: Vec<std::time::Duration> = Vec::with_capacity(corpus.len());
            for line in &corpus {
                samples.push(one_keystroke(line, &ctx));
            }
            samples.sort();
            let p99 = samples[(samples.len() * 99) / 100];
            // Budget: p99 <1 ms. Assert in bench so a regression
            // surfaces immediately in the output.
            assert!(
                p99 < std::time::Duration::from_millis(1),
                "p99 = {p99:?} exceeded 1ms budget",
            );
            p99
        })
    });
}

fn bench_reparse_only_p99(c: &mut Criterion) {
    let corpus = corpus();
    c.bench_function("ast_reparse_p99", |b| {
        b.iter(|| {
            let mut samples: Vec<std::time::Duration> = Vec::with_capacity(corpus.len());
            for line in &corpus {
                let t = Instant::now();
                let _ast = parse_simple(line);
                samples.push(t.elapsed());
            }
            samples.sort();
            let p99 = samples[(samples.len() * 99) / 100];
            // Budget: AST reparse p99 <2 ms.
            assert!(
                p99 < std::time::Duration::from_millis(2),
                "AST reparse p99 = {p99:?} exceeded 2ms budget",
            );
            p99
        })
    });
}

criterion_group!(benches, bench_p99, bench_reparse_only_p99);
criterion_main!(benches);
