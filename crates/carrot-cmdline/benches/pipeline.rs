//! Every-keystroke pipeline bench.
//!
//! Measures the work the cmdline does on each keystroke:
//!   parse_simple → highlight_ast → validate.
//!
//! Budget: the UI loop repaints at 120 FPS (≈ 8 ms per frame), so
//! the full parse-highlight-validate cycle should complete well
//! under 500 µs to leave headroom for rendering + agent IO. On an
//! M-series Mac this bench typically clocks the full pipeline
//! under 5 µs — two orders of magnitude below budget.

use std::collections::HashSet;

use carrot_cmdline::{
    ast::{CommandAst, Range, SubcommandNode},
    highlight::highlight_ast,
    parse::parse_simple,
    validation::{ValidationContext, validate},
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

const LIGHT_INPUT: &str = "ls";
const TYPICAL_INPUT: &str = "git checkout main";
const HEAVY_INPUT: &str = "git log --oneline --graph --decorate --abbrev-commit HEAD~20..HEAD";

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_simple");
    group.bench_function("light", |b| b.iter(|| parse_simple(black_box(LIGHT_INPUT))));
    group.bench_function("typical", |b| {
        b.iter(|| parse_simple(black_box(TYPICAL_INPUT)))
    });
    group.bench_function("heavy", |b| b.iter(|| parse_simple(black_box(HEAVY_INPUT))));
    group.finish();
}

fn bench_highlight(c: &mut Criterion) {
    let typical = parse_simple(TYPICAL_INPUT);
    let heavy = parse_simple(HEAVY_INPUT);
    let mut group = c.benchmark_group("highlight_ast");
    group.bench_function("typical", |b| b.iter(|| highlight_ast(black_box(&typical))));
    group.bench_function("heavy", |b| b.iter(|| highlight_ast(black_box(&heavy))));
    group.finish();
}

fn bench_validate(c: &mut Criterion) {
    let mut typical_ast = parse_simple(TYPICAL_INPUT);
    // Mutate the first pipeline stage in place — the fallback parser
    // already put `checkout` in the subcommand slot; we pretend the
    // schema typed `main` as a GitRef branch.
    if let Some(stage) = typical_ast.elements.first_mut() {
        stage.subcommand = Some(SubcommandNode {
            name: "checkout".into(),
            depth: 0,
            range: Range::new(4, 12),
        });
        if let Some(pos) = stage.positionals.first_mut() {
            pos.kind = carrot_cmdline::ast::ArgKind::GitRef {
                scope: carrot_cmdline::ast::GitScope::Branch,
            };
        }
    }

    let known_refs: HashSet<&str> = ["main", "develop", "release/2026"].into_iter().collect();
    let known_commands: HashSet<&str> = ["git", "ls", "cargo", "cat"].into_iter().collect();

    let ctx = ValidationContext {
        known_refs: Some(known_refs),
        known_commands: Some(known_commands),
        ..Default::default()
    };

    c.bench_function("validate_typical", |b| {
        b.iter(|| validate(black_box(&typical_ast), black_box(&ctx)))
    });
}

fn bench_full_pipeline(c: &mut Criterion) {
    let known_refs: HashSet<&str> = ["main", "develop"].into_iter().collect();
    let known_commands: HashSet<&str> = ["git", "ls", "cat"].into_iter().collect();
    let ctx = ValidationContext {
        known_refs: Some(known_refs),
        known_commands: Some(known_commands),
        ..Default::default()
    };

    let mut group = c.benchmark_group("full_pipeline");
    group.bench_function("typical", |b| {
        b.iter(|| {
            let ast: CommandAst = parse_simple(black_box(TYPICAL_INPUT));
            let _spans = highlight_ast(&ast);
            let _errors = validate(&ast, &ctx);
        })
    });
    group.bench_function("heavy", |b| {
        b.iter(|| {
            let ast = parse_simple(black_box(HEAVY_INPUT));
            let _spans = highlight_ast(&ast);
            let _errors = validate(&ast, &ctx);
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_parse,
    bench_highlight,
    bench_validate,
    bench_full_pipeline,
);
criterion_main!(benches);
