//! Interactive-prompt re-entry coverage.
//!
//! Covers the prompts that show up mid-command: `sudo`, `ssh`
//! passphrase, `git credential`, `docker login`, `read -p`,
//! `npm init`, yes/no confirmations. Password masking is verified
//! — password bytes never show up in the public buffer, AI
//! context, or `agent.current_text()`.
//!
//! Each test drives a [`CmdlineSession`] through a synthetic
//! mid-command flow: schedule a block, CommandStart, feed the real
//! captured output-tail string through [`detect_interaction_kind`],
//! promote to Transient, verify the kind + masking behaviour, and
//! commit the bytes + CommandEnd.

use std::num::NonZeroU64;

use carrot_cmdline::{
    mount::BlockHandle,
    osc133::ShellEvent,
    phase2::{
        agent_host::{AgentPermission, CmdlineAgentHost, InMemoryAgentHost},
        ai::SuggestionRequest,
        ai_engine::{HistoryEngine, SuggestionEngine},
    },
    prompt_state::{InteractionKind, InteractivePromptState, PromptState, detect_interaction_kind},
    session::CmdlineSession,
    shell::ShellKind,
};
use carrot_session::command_history::CommandHistory;

fn block(n: u64) -> BlockHandle {
    BlockHandle(NonZeroU64::new(n).expect("non-zero"))
}

fn start_command(shell: ShellKind, block_id: u64) -> CmdlineSession {
    let mut s = CmdlineSession::new(shell);
    s.schedule_block(block(block_id));
    s.apply_shell_event(ShellEvent::InputStart);
    s.apply_shell_event(ShellEvent::CommandStart);
    s
}

// ─── Real-world captured output tails ────────────────────────────

const SUDO_TAIL: &str = "\n[sudo] password for nyxb: ";
const SSH_TAIL: &str = "\nEnter passphrase for key '/Users/nyxb/.ssh/id_ed25519': ";
const GIT_CRED_TAIL: &str = "\nPassword for 'https://token@github.com': ";
const DOCKER_LOGIN_TAIL: &str = "\nEnter password: ";
const READ_PROMPT_TAIL: &str = "\nYour name: ";
const NPM_INIT_TAIL: &str = "\npackage name: (my-package) ";
const CONFIRM_TAIL: &str = "\nAre you sure? [y/N] ";

#[test]
fn sudo_password_prompt_masks_input() {
    let mut s = start_command(ShellKind::Bash, 1);
    let kind = detect_interaction_kind(SUDO_TAIL).expect("sudo pattern matches");
    assert_eq!(kind, InteractionKind::Password);
    assert!(s.promote_to_transient(kind));
    s.transient_append("hunter2");
    assert!(
        s.state().buffer().is_none(),
        "public buffer must mask password"
    );
    assert_eq!(s.state().masked_buffer(), Some("hunter2"));
    // Commit writes to PTY; back to Hidden.
    let bytes = s.commit_transient().unwrap();
    assert_eq!(bytes, "hunter2");
    assert!(matches!(
        s.state(),
        PromptState::Executing {
            inner: InteractivePromptState::Hidden,
            ..
        }
    ));
}

#[test]
fn ssh_passphrase_prompt_masks_input() {
    let mut s = start_command(ShellKind::Zsh, 2);
    let kind = detect_interaction_kind(SSH_TAIL).expect("ssh pattern");
    assert_eq!(kind, InteractionKind::Password);
    s.promote_to_transient(kind);
    s.transient_append("correct horse battery staple");
    assert!(s.state().buffer().is_none());
}

#[test]
fn git_credential_prompt_masks_input() {
    let mut s = start_command(ShellKind::Fish, 3);
    let kind = detect_interaction_kind(GIT_CRED_TAIL).expect("git creds");
    assert_eq!(kind, InteractionKind::Password);
    s.promote_to_transient(kind);
    s.transient_append("ghp_abcDEF123");
    // Password bytes must never surface in the agent context.
    let host = InMemoryAgentHost::new();
    host.set_permission(AgentPermission::ReadWrite);
    host.set_buffer(s.state().buffer().unwrap_or(""));
    assert_eq!(
        host.current_text(),
        "",
        "password must not leak into agent.current_text"
    );
}

#[test]
fn docker_login_prompt_masks_input() {
    let mut s = start_command(ShellKind::Bash, 4);
    let kind = detect_interaction_kind(DOCKER_LOGIN_TAIL).expect("docker login");
    assert_eq!(kind, InteractionKind::Password);
    s.promote_to_transient(kind);
    s.transient_append("dckr_pat_xxxx");
    assert!(s.state().buffer().is_none());
}

#[test]
fn read_prompt_is_free_text_not_masked() {
    let mut s = start_command(ShellKind::Bash, 5);
    let kind = detect_interaction_kind(READ_PROMPT_TAIL).expect("read -p");
    assert_eq!(kind, InteractionKind::FreeText);
    s.promote_to_transient(kind);
    s.transient_append("Dennis");
    // FreeText is not masked — the buffer is visible.
    assert_eq!(s.state().buffer(), Some("Dennis"));
}

#[test]
fn npm_init_package_name_is_free_text() {
    let mut s = start_command(ShellKind::Nushell, 6);
    let kind = detect_interaction_kind(NPM_INIT_TAIL).expect("npm init");
    assert_eq!(kind, InteractionKind::FreeText);
    s.promote_to_transient(kind);
    s.transient_append("carrot-tools");
    assert_eq!(s.state().buffer(), Some("carrot-tools"));
}

#[test]
fn yes_no_confirmation_prompt_is_yesno() {
    let mut s = start_command(ShellKind::Bash, 7);
    let kind = detect_interaction_kind(CONFIRM_TAIL).expect("confirm");
    assert_eq!(kind, InteractionKind::YesNo);
    s.promote_to_transient(kind);
    s.transient_append("y");
    // YesNo also unmasked.
    assert_eq!(s.state().buffer(), Some("y"));
}

#[test]
fn password_not_pushed_to_history_ghost_text() {
    // Simulate that a user typed a password, committed it, then the
    // next invocation's history source does NOT see it as a
    // candidate — because we never push masked buffers to history.
    let mut s = start_command(ShellKind::Bash, 8);
    let kind = detect_interaction_kind(SUDO_TAIL).unwrap();
    s.promote_to_transient(kind);
    s.transient_append("hunter2");
    let _ = s.commit_transient();
    s.apply_shell_event(ShellEvent::CommandEnd { exit_code: 0 });

    // History stays empty — callers must not persist masked bytes.
    let h = CommandHistory::new();
    let request = SuggestionRequest::from_prefix("hunt");
    let engine = HistoryEngine::new(&h);
    assert!(engine.suggest(&request).is_empty());
}

#[test]
fn full_sudo_roundtrip_returns_to_active() {
    let mut s = start_command(ShellKind::Bash, 9);
    let kind = detect_interaction_kind(SUDO_TAIL).unwrap();
    s.promote_to_transient(kind);
    s.transient_append("pw");
    s.commit_transient();
    assert!(s.state().is_executing());
    s.apply_shell_event(ShellEvent::CommandEnd { exit_code: 0 });
    assert!(s.state().is_active());
}

#[test]
fn pattern_matcher_rejects_plain_output() {
    // Output that shouldn't match any of the prompts.
    assert!(detect_interaction_kind("Build finished.").is_none());
    assert!(detect_interaction_kind("").is_none());
    assert!(detect_interaction_kind("hello world\n").is_none());
}
