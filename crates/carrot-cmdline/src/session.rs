//! In-memory cmdline session.
//!
//! A [`CmdlineSession`] is the composable core of a single cmdline
//! instance — one terminal block's worth of input. It owns:
//!
//! - the text buffer (today: `String`; later: `carrot-editor`'s
//!   SumTree via `ErasedEditor`)
//! - cursor byte-offset
//! - the parsed [`CommandAst`] (recomputed on every edit)
//! - the current [`PromptState`]
//! - the active [`ShellKind`]
//! - an optional active [`Suggestion`] and [`CompletionSet`]
//!
//! # Why a pure struct first
//!
//! Landing the session as a plain data struct before the
//! `carrot-editor` adapter lets every other module be tested
//! end-to-end: keystroke → buffer mutation → AST refresh →
//! completion query. When the adapter arrives, the `buffer` +
//! `cursor` fields get replaced by a trait object; the rest of
//! this struct stays.
//!
//! # What this module is NOT
//!
//! - Not a key-event dispatcher — inputs arrive as method calls.
//! - Not a renderer — consumers read the struct's fields.
//! - Not an async runtime — suggestion / completion queries are
//!   fire-and-store; this file just owns the latest results.

use crate::ast::CommandAst;
use crate::completion::CompletionSet;
use crate::mount::BlockHandle;
use crate::osc133::ShellEvent;
use crate::phase2::ai::{Suggestion, SuggestionSet};
use crate::prompt_state::{InteractionKind, InteractivePromptState, PromptState};
use crate::shell::ShellKind;
use crate::syntax::{bash::parse_bash, fish::parse_fish, nu::parse_nu, zsh::parse_zsh};

/// Snapshot of one cmdline instance.
#[derive(Debug, Clone)]
pub struct CmdlineSession {
    shell: ShellKind,
    state: PromptState,
    /// Block handle to attach when transitioning into `Executing`.
    /// Callers rotate this before submitting each command.
    next_block: Option<BlockHandle>,
    buffer: String,
    cursor: usize,
    ast: CommandAst,
    suggestions: SuggestionSet,
    completions: CompletionSet,
}

impl CmdlineSession {
    /// Empty session for a given shell, in `Active` state.
    pub fn new(shell: ShellKind) -> Self {
        Self {
            shell,
            state: PromptState::Active,
            next_block: None,
            buffer: String::new(),
            cursor: 0,
            ast: CommandAst::empty(),
            suggestions: SuggestionSet::default(),
            completions: CompletionSet::default(),
        }
    }

    // ─── Accessors ──────────────────────────────────────────────

    pub fn shell(&self) -> ShellKind {
        self.shell
    }

    pub fn state(&self) -> &PromptState {
        &self.state
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn ast(&self) -> &CommandAst {
        &self.ast
    }

    pub fn suggestions(&self) -> &SuggestionSet {
        &self.suggestions
    }

    pub fn completions(&self) -> &CompletionSet {
        &self.completions
    }

    /// Currently scheduled block for the next `CommandStart` event.
    pub fn next_block(&self) -> Option<BlockHandle> {
        self.next_block
    }

    // ─── Buffer mutations ──────────────────────────────────────

    /// Replace the full buffer and reset cursor to end. Re-parses
    /// the AST.
    pub fn set_buffer(&mut self, new_buffer: impl Into<String>) {
        self.buffer = new_buffer.into();
        self.cursor = self.buffer.len();
        self.refresh_ast();
    }

    /// Insert a string at the cursor, advancing the cursor past the
    /// insertion. Re-parses the AST.
    pub fn insert(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.refresh_ast();
    }

    /// Delete `n` bytes to the left of the cursor, clamping at 0.
    /// Re-parses the AST.
    pub fn delete_left(&mut self, n: usize) {
        let start = self.cursor.saturating_sub(n);
        self.buffer.drain(start..self.cursor);
        self.cursor = start;
        self.refresh_ast();
    }

    /// Clear the buffer and return cursor to 0. Re-parses (to empty).
    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.refresh_ast();
    }

    /// Set cursor to `offset`, clamped to buffer length.
    pub fn set_cursor(&mut self, offset: usize) {
        self.cursor = offset.min(self.buffer.len());
    }

    // ─── State transitions ────────────────────────────────────

    /// Schedule the block handle that should attach on the next
    /// `CommandStart` transition. Callers (the session manager)
    /// typically call this right before submitting the command.
    pub fn schedule_block(&mut self, block: BlockHandle) {
        self.next_block = Some(block);
    }

    /// Directly set the prompt state. Does not validate transitions
    /// — use [`CmdlineSession::apply_shell_event`] for the state-
    /// machine-driven path, this is for consumer-driven overrides
    /// (e.g. the mount controller promoting Hidden → Transient when
    /// it detects an interactive prompt).
    pub fn set_state(&mut self, target: PromptState) {
        self.state = target;
    }

    /// Apply an OSC-133 shell event, transitioning the prompt state
    /// accordingly. Returns `true` if the state machine moved.
    ///
    /// `CommandStart` consumes the `next_block` handle — the caller
    /// must schedule a block before submitting, otherwise the
    /// event is ignored.
    pub fn apply_shell_event(&mut self, event: ShellEvent) -> bool {
        let before = self.state.clone();
        let was_executing = matches!(self.state, PromptState::Executing { .. });
        // Resolve the block handle for the current transition.
        // - If we're already Executing, use the attached block.
        // - If we're Active, use the scheduled next_block. For
        //   `CommandStart` specifically this is required — without a
        //   scheduled block, the event can't move us into Executing.
        let block = match &self.state {
            PromptState::Executing { block, .. } => Some(*block),
            PromptState::Active => self.next_block,
        };
        let Some(target) = (match block {
            Some(b) => crate::osc133::transition_target(&event, &self.state, b),
            None => {
                // No block available. Informational events (which
                // keep us in Active) are still fine; `CommandStart`
                // is a no-op.
                match event {
                    ShellEvent::PromptStart | ShellEvent::InputStart => Some(PromptState::Active),
                    _ => None,
                }
            }
        }) else {
            return false;
        };
        // Only clear next_block when we actually leave Executing.
        if was_executing && matches!(target, PromptState::Active) {
            self.next_block = None;
        }
        self.state = target;
        self.state != before
    }

    /// Promote the current state from `Executing::Hidden` to
    /// `Executing::Transient` when the mount detector finds an
    /// interactive mid-command prompt. Returns `true` if the state
    /// actually changed.
    pub fn promote_to_transient(&mut self, detected: InteractionKind) -> bool {
        if let PromptState::Executing {
            block,
            inner: InteractivePromptState::Hidden,
        } = &self.state
        {
            self.state = PromptState::Executing {
                block: *block,
                inner: InteractivePromptState::Transient {
                    detected,
                    buffer: String::new(),
                    masked: detected.default_masked(),
                },
            };
            return true;
        }
        false
    }

    /// Append `text` to the transient buffer when we're in a
    /// `Transient` state. Returns `true` on success, `false` when
    /// we're not in a Transient state.
    pub fn transient_append(&mut self, text: &str) -> bool {
        if let PromptState::Executing {
            inner: InteractivePromptState::Transient { buffer, .. },
            ..
        } = &mut self.state
        {
            buffer.push_str(text);
            return true;
        }
        false
    }

    /// Drop back from `Transient` to `Hidden` after submitting the
    /// interactive input to the PTY. Returns the buffered bytes so
    /// the caller can write them.
    pub fn commit_transient(&mut self) -> Option<String> {
        if let PromptState::Executing {
            block,
            inner: InteractivePromptState::Transient { buffer, .. },
        } = &self.state
        {
            let bytes = buffer.clone();
            let block = *block;
            self.state = PromptState::Executing {
                block,
                inner: InteractivePromptState::Hidden,
            };
            return Some(bytes);
        }
        None
    }

    // ─── Suggestion / completion results ──────────────────────

    /// Replace the active suggestion set with `new`.
    pub fn set_suggestions(&mut self, new: SuggestionSet) {
        self.suggestions = new;
    }

    /// Current best ghost-text suggestion, if any.
    pub fn active_suggestion(&self) -> Option<&Suggestion> {
        self.suggestions.best()
    }

    /// Replace the active completion set.
    pub fn set_completions(&mut self, new: CompletionSet) {
        self.completions = new;
    }

    // ─── Internal ────────────────────────────────────────────

    fn refresh_ast(&mut self) {
        use crate::parse::parse_simple;
        self.ast = match self.shell {
            ShellKind::Bash => parse_bash(&self.buffer),
            ShellKind::Zsh => parse_zsh(&self.buffer),
            ShellKind::Fish => parse_fish(&self.buffer),
            ShellKind::Nushell => parse_nu(&self.buffer),
            // Other shells have no dedicated grammar — fall back to
            // the whitespace parser so the cmdline still produces a
            // usable AST structure.
            ShellKind::Posix
            | ShellKind::Csh
            | ShellKind::Tcsh
            | ShellKind::Rc
            | ShellKind::PowerShell
            | ShellKind::Pwsh
            | ShellKind::Cmd
            | ShellKind::Xonsh
            | ShellKind::Elvish => parse_simple(&self.buffer),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase2::ai::SuggestionSource;
    use std::num::NonZeroU64;

    fn block(n: u64) -> BlockHandle {
        BlockHandle(NonZeroU64::new(n).expect("non-zero"))
    }

    #[test]
    fn new_session_starts_active_and_empty() {
        let s = CmdlineSession::new(ShellKind::Nushell);
        assert_eq!(s.shell(), ShellKind::Nushell);
        assert!(s.state().is_active());
        assert_eq!(s.buffer(), "");
        assert_eq!(s.cursor(), 0);
        assert!(!s.ast().has_command());
    }

    #[test]
    fn set_buffer_reparses_ast() {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        s.set_buffer("git checkout main");
        assert_eq!(s.cursor(), 17);
        assert!(s.ast().has_command());
        let first = s.ast().first().expect("stage");
        assert_eq!(first.positionals.len(), 1);
    }

    #[test]
    fn insert_advances_cursor_and_updates_ast() {
        let mut s = CmdlineSession::new(ShellKind::Zsh);
        s.insert("ls");
        assert_eq!(s.buffer(), "ls");
        assert_eq!(s.cursor(), 2);
        assert_eq!(
            s.ast().first().unwrap().command.as_ref().unwrap().name,
            "ls",
        );

        s.insert(" -la");
        assert_eq!(s.buffer(), "ls -la");
        assert_eq!(s.cursor(), 6);
        assert_eq!(s.ast().first().unwrap().flags.len(), 1);
    }

    #[test]
    fn delete_left_clamps_at_zero() {
        let mut s = CmdlineSession::new(ShellKind::Nushell);
        s.set_buffer("abc");
        s.delete_left(10);
        assert_eq!(s.buffer(), "");
        assert_eq!(s.cursor(), 0);
    }

    #[test]
    fn clear_resets_everything() {
        let mut s = CmdlineSession::new(ShellKind::Fish);
        s.set_buffer("ls -la");
        s.clear_buffer();
        assert_eq!(s.buffer(), "");
        assert_eq!(s.cursor(), 0);
        assert!(!s.ast().has_command());
    }

    #[test]
    fn set_cursor_clamps_to_buffer_length() {
        let mut s = CmdlineSession::new(ShellKind::Nushell);
        s.set_buffer("abc");
        s.set_cursor(99);
        assert_eq!(s.cursor(), 3);
    }

    #[test]
    fn full_command_lifecycle_with_scheduled_block() {
        let mut s = CmdlineSession::new(ShellKind::Nushell);
        s.schedule_block(block(1));
        // Active → Active on PromptStart / InputStart (no-op).
        s.apply_shell_event(ShellEvent::PromptStart);
        assert!(s.state().is_active());
        // Active → Executing::Hidden on CommandStart.
        assert!(s.apply_shell_event(ShellEvent::CommandStart));
        assert_eq!(s.state().block(), Some(block(1)));
        // Executing → Active on CommandEnd.
        assert!(s.apply_shell_event(ShellEvent::CommandEnd { exit_code: 0 }));
        assert!(s.state().is_active());
        assert!(s.next_block().is_none());
    }

    #[test]
    fn command_start_without_scheduled_block_is_noop() {
        let mut s = CmdlineSession::new(ShellKind::Nushell);
        assert!(!s.apply_shell_event(ShellEvent::CommandStart));
        assert!(s.state().is_active());
    }

    #[test]
    fn promote_to_transient_preserves_block() {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        s.schedule_block(block(7));
        s.apply_shell_event(ShellEvent::CommandStart);
        assert!(s.promote_to_transient(InteractionKind::Password));
        assert_eq!(s.state().block(), Some(block(7)));
        assert_eq!(
            s.state().interaction_kind(),
            Some(InteractionKind::Password)
        );
    }

    #[test]
    fn promote_from_active_rejected() {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        assert!(!s.promote_to_transient(InteractionKind::FreeText));
    }

    #[test]
    fn transient_append_and_commit_round_trips() {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        s.schedule_block(block(1));
        s.apply_shell_event(ShellEvent::CommandStart);
        s.promote_to_transient(InteractionKind::FreeText);
        assert!(s.transient_append("hello"));
        assert!(s.transient_append(" world"));
        let bytes = s.commit_transient().unwrap();
        assert_eq!(bytes, "hello world");
        // Back in Hidden.
        assert!(matches!(
            s.state(),
            PromptState::Executing {
                inner: InteractivePromptState::Hidden,
                ..
            }
        ));
    }

    #[test]
    fn transient_password_buffer_is_masked() {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        s.schedule_block(block(1));
        s.apply_shell_event(ShellEvent::CommandStart);
        s.promote_to_transient(InteractionKind::Password);
        s.transient_append("correct horse battery staple");
        // Public buffer() returns None when masked.
        assert!(s.state().buffer().is_none());
        // But the PTY write path still sees the bytes.
        assert_eq!(
            s.state().masked_buffer().unwrap(),
            "correct horse battery staple"
        );
    }

    #[test]
    fn active_suggestion_follows_set() {
        let mut s = CmdlineSession::new(ShellKind::Nushell);
        assert!(s.active_suggestion().is_none());
        let mut set = SuggestionSet::new();
        set.candidates
            .push(Suggestion::new("ls", SuggestionSource::History));
        s.set_suggestions(set);
        assert_eq!(s.active_suggestion().unwrap().completion, "ls");
    }

    #[test]
    fn full_replacement_works() {
        let mut s = CmdlineSession::new(ShellKind::Zsh);
        s.set_buffer("git cehckout main");
        s.set_buffer("git checkout main");
        assert!(s.ast().has_command());
        let first = s.ast().first().expect("stage");
        assert_eq!(first.subcommand.as_ref().unwrap().name, "checkout");
    }
}
