//! End-to-end integration tests for the cmdline session.
//!
//! These exercise the full data-flow without any external deps:
//! keystroke-like mutations, OSC-133 shell events, AST refresh,
//! agent handoff, ghost-text suggestion injection, and history
//! round-tripping.

use std::num::NonZeroU64;

use carrot_cmdline::{
    ast::CommandAst,
    mount::BlockHandle,
    osc133::ShellEvent,
    phase2::{
        agent::{AgentChip, AgentHandoff, ChipIntent},
        ai::{Suggestion, SuggestionSet, SuggestionSource},
    },
    prompt_state::{InteractionKind, PromptState},
    session::CmdlineSession,
    shell::ShellKind,
};
use carrot_session::command_history::CommandHistory;

fn block(n: u64) -> BlockHandle {
    BlockHandle(NonZeroU64::new(n).expect("non-zero"))
}

#[test]
fn typing_a_command_produces_a_live_ast() {
    let mut s = CmdlineSession::new(ShellKind::Bash);
    for piece in ["git", " ", "checkout", " ", "main"] {
        s.insert(piece);
    }
    assert_eq!(s.buffer(), "git checkout main");
    let ast = s.ast();
    let first = ast.first().expect("stage");
    assert_eq!(first.command.as_ref().unwrap().name, "git");
    assert_eq!(first.subcommand.as_ref().unwrap().name, "checkout");
    assert_eq!(first.positionals.len(), 1);
    assert_eq!(first.positionals[0].value, "main");
}

#[test]
fn full_shell_lifecycle_via_osc133_events() {
    let mut s = CmdlineSession::new(ShellKind::Nushell);
    s.schedule_block(block(1));
    assert!(s.state().is_active());
    // PromptStart / InputStart keep us Active.
    s.apply_shell_event(ShellEvent::InputStart);
    assert!(s.state().is_active());
    s.insert("ls");
    s.apply_shell_event(ShellEvent::CommandStart);
    assert!(s.state().is_executing());
    assert_eq!(s.state().block(), Some(block(1)));
    s.apply_shell_event(ShellEvent::CommandEnd { exit_code: 0 });
    assert!(s.state().is_active());
}

#[test]
fn interactive_prompt_promotes_to_transient() {
    let mut s = CmdlineSession::new(ShellKind::Zsh);
    s.schedule_block(block(7));
    s.apply_shell_event(ShellEvent::InputStart);
    s.apply_shell_event(ShellEvent::CommandStart);
    assert!(s.state().is_executing());
    // Mount controller detected an interactive prompt, promotes to
    // Transient with a FreeText interaction.
    assert!(s.promote_to_transient(InteractionKind::FreeText));
    assert_eq!(
        s.state().interaction_kind(),
        Some(InteractionKind::FreeText)
    );
    // The user types their reply.
    s.transient_append("yes");
    // Submit — writes the bytes to PTY, drops back to Hidden.
    let bytes = s.commit_transient().unwrap();
    assert_eq!(bytes, "yes");
    // CommandEnd returns to Active.
    s.apply_shell_event(ShellEvent::CommandEnd { exit_code: 0 });
    assert!(s.state().is_active());
}

#[test]
fn ghost_text_suggestion_overrides_session() {
    let mut s = CmdlineSession::new(ShellKind::Fish);
    s.insert("git chec");
    let mut set = SuggestionSet::new();
    set.candidates
        .push(Suggestion::new("kout main", SuggestionSource::Cloud).with_confidence(80));
    set.candidates
        .push(Suggestion::new("kout main", SuggestionSource::History).with_confidence(60));
    s.set_suggestions(set);
    let best = s.active_suggestion().unwrap();
    // History wins over Cloud at same completion length (priority bucket).
    assert_eq!(best.source, SuggestionSource::History);
    assert_eq!(best.completion, "kout main");
}

#[test]
fn hash_handoff_strips_sigil_and_keeps_context() {
    let mut s = CmdlineSession::new(ShellKind::Bash);
    s.insert("# explain the failure");
    let handoff = AgentHandoff::from_hash_line(s.buffer()).unwrap();
    assert_eq!(handoff.message, "explain the failure");
    assert!(!handoff.from_chip);
    let with_context = handoff
        .with_cwd("/workspaces/carrot")
        .with_context_ast(s.ast().clone());
    assert_eq!(with_context.cwd.as_deref(), Some("/workspaces/carrot"));
    assert!(with_context.context_ast.is_some());
}

#[test]
fn chip_seed_flows_into_handoff() {
    let chip = AgentChip {
        label: "Ask AI to fix".into(),
        seed: "# why did this fail".into(),
        intent: ChipIntent::Fix,
    };
    let handoff = chip.into_handoff();
    assert!(handoff.from_chip);
    assert_eq!(handoff.message, "why did this fail");
}

#[test]
fn history_dedup_and_order_preserved() {
    let mut h = CommandHistory::new();
    h.push("ls".to_string());
    h.push("pwd".to_string());
    h.push("ls".to_string());
    // `ls` is deduplicated; entries stay in insertion order (oldest
    // first) but the repeated `ls` bumps its timestamp + frequency.
    let cmds: Vec<_> = h.entries().iter().map(|e| e.command.as_str()).collect();
    assert_eq!(cmds, vec!["ls", "pwd"]);
    assert_eq!(h.entries()[0].frequency, 2);
}

#[test]
fn empty_session_ast_is_empty() {
    let s = CmdlineSession::new(ShellKind::Nushell);
    assert_eq!(s.ast(), &CommandAst::empty());
    assert!(matches!(s.state(), PromptState::Active));
}

#[test]
fn password_prompt_masks_the_buffer() {
    let mut s = CmdlineSession::new(ShellKind::Bash);
    s.schedule_block(block(2));
    s.apply_shell_event(ShellEvent::CommandStart);
    s.promote_to_transient(InteractionKind::Password);
    s.transient_append("correct horse battery staple");
    // Public buffer is hidden.
    assert!(s.state().buffer().is_none());
    // PTY-write accessor still sees the bytes.
    assert!(s.state().masked_buffer().is_some());
}
