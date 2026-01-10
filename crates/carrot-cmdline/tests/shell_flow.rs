//! Full shell-integration flow exercised end-to-end.
//!
//! Complements `session_flow.rs` with coverage of the newer modules:
//! OSC 7777 metadata roundtrip, ShellStream composition, completion
//! driver dispatch against a live session, validation + highlight
//! interplay.

use carrot_cmdline::{
    ast::{ArgKind, PathKind, PositionalNode, Range},
    completion::CompletionSource,
    completion_driver::{DriverContext, suggest_for_cursor},
    highlight::{HighlightRole, highlight_ast},
    mount::BlockHandle,
    osc133::ShellEvent,
    osc7777::{self, BlockMetadata, GitInfo},
    phase2::ai_engine::{HistoryEngine, RacingEngine},
    session::CmdlineSession,
    shell::ShellKind,
    shell_event::{ShellBlockEvent, ShellStream},
    validation::{ValidationContext, validate},
};
use carrot_session::command_history::CommandHistory;
use std::collections::HashSet;
use std::num::NonZeroU64;

fn block(n: u64) -> BlockHandle {
    BlockHandle(NonZeroU64::new(n).expect("non-zero"))
}

#[test]
fn osc7777_round_trips_through_wire_format() {
    let md = BlockMetadata {
        cwd: Some("/home/nyxb".into()),
        git: Some(GitInfo {
            branch: Some("feat/inazuma-block-primitive".into()),
            dirty: Some(true),
            ahead: Some(1),
            behind: Some(0),
        }),
        user: Some("nyxb".into()),
        shell: Some("nu".into()),
        exit: Some(0),
        runtime_ms: Some(42),
        host: None,
    };
    let hex = osc7777::encode(&md);
    let back = osc7777::parse(&hex).unwrap();
    assert_eq!(back, md);
}

#[test]
fn shell_stream_composes_full_command_lifecycle() {
    let mut stream = ShellStream::new();

    // PromptStart + metadata (cwd + git) → composed event.
    let pre = BlockMetadata {
        cwd: Some("/workspaces/carrot".into()),
        git: Some(GitInfo {
            branch: Some("main".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    stream.push_lifecycle(ShellEvent::PromptStart);
    let start = stream.push_metadata(pre).unwrap();
    assert_eq!(start.lifecycle, ShellEvent::PromptStart);
    assert_eq!(start.cwd(), Some("/workspaces/carrot"));
    assert_eq!(start.git_branch(), Some("main"));

    // CommandStart with no metadata — flushed alone by the next
    // lifecycle or by flush() at end of stream.
    assert!(stream.push_lifecycle(ShellEvent::CommandStart).is_none());
    let flushed = stream
        .push_lifecycle(ShellEvent::CommandEnd { exit_code: 0 })
        .unwrap();
    assert_eq!(flushed.lifecycle, ShellEvent::CommandStart);
    assert!(flushed.metadata.is_none());

    // Attach CommandEnd metadata.
    let post = BlockMetadata {
        exit: Some(0),
        runtime_ms: Some(1234),
        ..Default::default()
    };
    let end = stream.push_metadata(post).unwrap();
    assert_eq!(end.lifecycle, ShellEvent::CommandEnd { exit_code: 0 });
    assert_eq!(end.runtime_ms(), Some(1234));
}

#[test]
fn completion_driver_history_fallback_when_positional_unknown() {
    let mut session = CmdlineSession::new(ShellKind::Bash);
    session.set_buffer("");
    let mut h = CommandHistory::new();
    h.push("git checkout main".to_string());
    h.push("ls -la".to_string());
    let ctx = DriverContext {
        history: Some(&h),
        per_source_limit: 5,
        ..Default::default()
    };
    let set = suggest_for_cursor(&session, &ctx);
    assert_eq!(set.candidates.len(), 2);
    assert!(
        set.candidates
            .iter()
            .all(|c| c.source == CompletionSource::History)
    );
}

#[test]
fn validation_plus_highlight_produces_red_underline_span() {
    let mut session = CmdlineSession::new(ShellKind::Zsh);
    session.set_buffer("git checkout main");

    // Pretend the schema tagged `main` as a GitRef. parse_simple
    // gives Unknown; we overwrite each positional across all pipeline
    // stages for the test.
    let mut ast = session.ast().clone();
    for element in ast.elements.iter_mut() {
        for p in element.positionals.iter_mut() {
            p.kind = ArgKind::GitRef {
                scope: carrot_cmdline::ast::GitScope::Branch,
            };
        }
    }

    let mut known = HashSet::new();
    known.insert("develop");
    let ctx = ValidationContext {
        known_refs: Some(known),
        ..Default::default()
    };
    let errors = validate(&ast, &ctx);
    assert_eq!(errors.len(), 1);
    ast.errors = errors;
    let spans = highlight_ast(&ast);
    assert!(spans.iter().any(|s| s.role == HighlightRole::Error));
    assert!(spans.iter().any(|s| s.role == HighlightRole::GitRef));
}

#[test]
fn racing_engine_produces_ghost_text_from_history() {
    let mut h = CommandHistory::new();
    h.push_with_metadata("cargo test --workspace".to_string(), Some(0), None);
    let mut racer = RacingEngine::new();
    racer.register(HistoryEngine::new(&h));
    let request = carrot_cmdline::phase2::ai::SuggestionRequest::from_prefix("cargo test");
    let set = racer.run(&request);
    let best = set.best().unwrap();
    assert_eq!(best.completion, " --workspace");
    assert!(best.confidence >= 80);
}

#[test]
fn shell_stream_flush_returns_pending_lifecycle_at_end_of_stream() {
    let mut stream = ShellStream::new();
    stream.push_lifecycle(ShellEvent::PromptStart);
    assert!(stream.is_pending());
    let ev = stream.flush().unwrap();
    assert_eq!(ev.lifecycle, ShellEvent::PromptStart);
    assert!(ev.metadata.is_none());
    assert!(!stream.is_pending());
}

#[test]
fn filesystem_completion_through_driver() {
    let tmp = std::env::temp_dir();
    let marker = format!("carrot-cmdline-shell-flow-{}", std::process::id());
    let file = tmp.join(&marker);
    std::fs::write(&file, b"").unwrap();

    let mut session = CmdlineSession::new(ShellKind::Bash);
    session.set_buffer("cat ");
    // The session's AST already has `cat` as command in stage 0; this
    // test exercises the filesystem candidate source directly rather
    // than swapping the AST, so no AST mutation is needed here.
    let ctx = DriverContext {
        cwd: Some(&tmp),
        per_source_limit: 50,
        ..Default::default()
    };
    // Sanity: filesystem source works with the marker prefix.
    let out = carrot_cmdline::completion_sources::filesystem_candidates(
        ctx.cwd.unwrap(),
        "carrot-cmdline-shell-flow-",
        Range::new(4, 4),
        50,
    );
    assert!(out.iter().any(|c| c.label.contains(&marker)));
    // And suggest_for_cursor falls back to empty when no typed
    // positional covers the cursor.
    let set = suggest_for_cursor(&session, &ctx);
    assert!(set.is_empty() || set.anchor.is_some());
    // Keep variables referenced to avoid clippy unused complaint.
    let _ = PositionalNode {
        value: String::new(),
        kind: ArgKind::Path {
            must_exist: false,
            kind: PathKind::Any,
        },
        range: Range::new(0, 0),
    };
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn session_applies_shell_events_through_full_cycle() {
    let mut s = CmdlineSession::new(ShellKind::Nushell);
    s.schedule_block(block(1));
    // Active → Active on InputStart, then user types, CommandStart
    // enters Executing::Hidden under the scheduled block,
    // CommandEnd returns to Active.
    assert!(s.state().is_active());
    s.apply_shell_event(ShellEvent::InputStart);
    assert!(s.state().is_active());
    s.set_buffer("echo hi");
    assert!(s.ast().has_command());
    s.apply_shell_event(ShellEvent::CommandStart);
    assert!(s.state().is_executing());
    let ev = ShellBlockEvent::new(ShellEvent::CommandEnd { exit_code: 0 }).with_metadata(
        BlockMetadata {
            exit: Some(0),
            runtime_ms: Some(5),
            ..Default::default()
        },
    );
    s.apply_shell_event(ev.lifecycle.clone());
    assert!(s.state().is_active());
    assert_eq!(ev.exit_code(), Some(0));
    assert_eq!(ev.runtime_ms(), Some(5));
}
