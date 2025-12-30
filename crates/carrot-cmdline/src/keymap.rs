//! Shell-aware cmdline keymap primitives.
//!
//! Each `ShellKind` has slightly different "native" keybindings —
//! fish's `Alt+L` list-directory, zsh's `Ctrl+X Ctrl+E` edit-command-in-editor,
//! bash's `Ctrl+R` reverse-search, nu's Emacs-style defaults.
//!
//! The cmdline's own action set overlays those defaults and the
//! user's carrot keymap overlays everything. This module owns the
//! **action enum** + the **default resolver**; the keymap file
//! parser that sits above lives in `carrot-actions`.
//!
//! # What this module is NOT
//!
//! - Not a key-event dispatcher. That is `inazuma`'s job.
//! - Not a keymap-file parser. That is `carrot-actions`'s.
//! - Not a renderer of key-hint popovers. That is the element layer.
//!
//! This module just defines the action vocabulary that every other
//! layer imports.

use crate::shell::ShellKind;

/// An action the cmdline can perform in response to a keystroke.
///
/// Grouped into four families:
/// - **Edit**: modify the buffer (cursor moves, history walk, …)
/// - **Completion**: accept / reject / cycle completions
/// - **History**: move through the history buffer
/// - **Agent**: AI / agent handoff actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmdlineAction {
    // ─── Edit / movement ────────────────────────────────────────
    SubmitCommand,
    CancelCommand,
    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveStart,
    MoveEnd,
    DeleteLeft,
    DeleteRight,
    DeleteWordLeft,
    DeleteWordRight,
    ClearLine,

    // ─── Completion ─────────────────────────────────────────────
    AcceptCompletion,
    CycleNextCompletion,
    CyclePrevCompletion,
    DismissCompletion,
    /// Accept only the *common prefix* among current candidates —
    /// zsh / fish's "complete to the shared start".
    AcceptCommonPrefix,

    // ─── History ────────────────────────────────────────────────
    HistoryPrev,
    HistoryNext,
    HistorySearchIncremental,

    // ─── Agent ──────────────────────────────────────────────────
    /// `#` prefix hand-off — send the AST to the agent.
    AgentHandoff,
    /// Accept the visible AI ghost-text suggestion.
    AcceptGhostText,
    /// Accept one word of the ghost-text (fish-style partial accept).
    AcceptGhostTextWord,

    // ─── Shell integration ─────────────────────────────────────
    /// Re-run the last command from history.
    RerunLast,
    /// Open the current input in `$EDITOR` (zsh `Ctrl+X Ctrl+E`).
    EditInExternalEditor,
}

impl CmdlineAction {
    /// Whether the action requires an active completion set. UI
    /// layers use this to grey out irrelevant keybindings.
    pub fn requires_completion(self) -> bool {
        matches!(
            self,
            CmdlineAction::AcceptCompletion
                | CmdlineAction::CycleNextCompletion
                | CmdlineAction::CyclePrevCompletion
                | CmdlineAction::DismissCompletion
                | CmdlineAction::AcceptCommonPrefix
        )
    }

    /// Whether the action is "destructive" in the sense that it
    /// mutates / discards unsaved buffer text. Used by the "confirm
    /// before discard" safety net for long commands.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            CmdlineAction::CancelCommand
                | CmdlineAction::ClearLine
                | CmdlineAction::DeleteWordLeft
                | CmdlineAction::DeleteWordRight
        )
    }
}

/// Default bindings baked into each shell. The cmdline registers
/// these at startup; user keymaps overlay them.
///
/// Returning a `&'static` slice keeps this allocation-free — every
/// runtime just gets a handle into a static table.
pub fn default_bindings(shell: ShellKind) -> &'static [(&'static str, CmdlineAction)] {
    use CmdlineAction::*;
    match shell {
        ShellKind::Nushell => &[
            ("Enter", SubmitCommand),
            ("Ctrl+C", CancelCommand),
            ("Tab", AcceptCompletion),
            ("Shift+Tab", CyclePrevCompletion),
            ("Ctrl+R", HistorySearchIncremental),
            ("Up", HistoryPrev),
            ("Down", HistoryNext),
            ("Ctrl+A", MoveStart),
            ("Ctrl+E", MoveEnd),
            ("Alt+Left", MoveWordLeft),
            ("Alt+Right", MoveWordRight),
            ("Right", AcceptGhostText),
        ],
        ShellKind::Bash => &[
            ("Enter", SubmitCommand),
            ("Ctrl+C", CancelCommand),
            ("Tab", AcceptCommonPrefix),
            ("Ctrl+R", HistorySearchIncremental),
            ("Up", HistoryPrev),
            ("Down", HistoryNext),
            ("Ctrl+A", MoveStart),
            ("Ctrl+E", MoveEnd),
            ("Ctrl+W", DeleteWordLeft),
            ("Ctrl+U", ClearLine),
            ("Ctrl+X Ctrl+E", EditInExternalEditor),
        ],
        ShellKind::Zsh => &[
            ("Enter", SubmitCommand),
            ("Ctrl+C", CancelCommand),
            ("Tab", AcceptCommonPrefix),
            ("Shift+Tab", CyclePrevCompletion),
            ("Ctrl+R", HistorySearchIncremental),
            ("Up", HistoryPrev),
            ("Down", HistoryNext),
            ("Ctrl+A", MoveStart),
            ("Ctrl+E", MoveEnd),
            ("Ctrl+W", DeleteWordLeft),
            ("Ctrl+U", ClearLine),
            ("Ctrl+X Ctrl+E", EditInExternalEditor),
        ],
        ShellKind::Fish => &[
            ("Enter", SubmitCommand),
            ("Ctrl+C", CancelCommand),
            ("Tab", AcceptCompletion),
            ("Shift+Tab", CyclePrevCompletion),
            ("Ctrl+R", HistorySearchIncremental),
            ("Up", HistoryPrev),
            ("Down", HistoryNext),
            ("Ctrl+A", MoveStart),
            ("Ctrl+E", MoveEnd),
            ("Right", AcceptGhostText),
            ("Alt+Right", AcceptGhostTextWord),
        ],
        // Every other shell (Posix sh/dash/ksh, Csh, Tcsh, Rc,
        // PowerShell, Pwsh, Cmd, Xonsh, Elvish) gets the bash-style
        // defaults — closest common denominator for POSIX-like UX.
        ShellKind::Posix
        | ShellKind::Csh
        | ShellKind::Tcsh
        | ShellKind::Rc
        | ShellKind::PowerShell
        | ShellKind::Pwsh
        | ShellKind::Cmd
        | ShellKind::Xonsh
        | ShellKind::Elvish => default_bindings(ShellKind::Bash),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_has_submit_binding() {
        for shell in [
            ShellKind::Nushell,
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Fish,
        ] {
            let has_submit = default_bindings(shell)
                .iter()
                .any(|(_, a)| *a == CmdlineAction::SubmitCommand);
            assert!(has_submit, "{shell:?} lacks SubmitCommand");
        }
    }

    #[test]
    fn every_shell_has_cancel_binding() {
        for shell in [
            ShellKind::Nushell,
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Fish,
        ] {
            let has_cancel = default_bindings(shell)
                .iter()
                .any(|(_, a)| *a == CmdlineAction::CancelCommand);
            assert!(has_cancel, "{shell:?} lacks CancelCommand");
        }
    }

    #[test]
    fn every_shell_has_history_walking() {
        for shell in [
            ShellKind::Nushell,
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Fish,
        ] {
            let table = default_bindings(shell);
            assert!(
                table.iter().any(|(_, a)| *a == CmdlineAction::HistoryPrev),
                "{shell:?} lacks HistoryPrev"
            );
            assert!(
                table.iter().any(|(_, a)| *a == CmdlineAction::HistoryNext),
                "{shell:?} lacks HistoryNext"
            );
        }
    }

    #[test]
    fn fish_has_ghost_text_word_accept() {
        let table = default_bindings(ShellKind::Fish);
        assert!(
            table
                .iter()
                .any(|(_, a)| *a == CmdlineAction::AcceptGhostTextWord),
        );
    }

    #[test]
    fn bash_and_zsh_expose_external_editor_shortcut() {
        for shell in [ShellKind::Bash, ShellKind::Zsh] {
            let table = default_bindings(shell);
            assert!(
                table
                    .iter()
                    .any(|(_, a)| *a == CmdlineAction::EditInExternalEditor),
                "{shell:?} missing EditInExternalEditor"
            );
        }
    }

    #[test]
    fn requires_completion_classification() {
        assert!(CmdlineAction::AcceptCompletion.requires_completion());
        assert!(CmdlineAction::CycleNextCompletion.requires_completion());
        assert!(!CmdlineAction::SubmitCommand.requires_completion());
        assert!(!CmdlineAction::HistoryPrev.requires_completion());
    }

    #[test]
    fn destructive_actions_flagged() {
        assert!(CmdlineAction::ClearLine.is_destructive());
        assert!(CmdlineAction::CancelCommand.is_destructive());
        assert!(!CmdlineAction::MoveLeft.is_destructive());
    }

    #[test]
    fn no_shell_has_duplicate_keystroke_mappings() {
        for shell in [
            ShellKind::Nushell,
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Fish,
        ] {
            let table = default_bindings(shell);
            for i in 0..table.len() {
                for j in (i + 1)..table.len() {
                    assert_ne!(
                        table[i].0, table[j].0,
                        "{shell:?} has duplicate binding for {}",
                        table[i].0
                    );
                }
            }
        }
    }
}
