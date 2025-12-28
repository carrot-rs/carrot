//! Agent handoff + Active-AI chip data model.
//!
//! Two interaction patterns, one data contract:
//!
//! 1. **`#` prefix handoff** — the user prefixes a line with `#`,
//!    the cmdline lifts the remaining text + the parsed
//!    [`CommandAst`] + contextual snapshot (cwd, last error,
//!    recent history) and hands the `AgentHandoff` to the agent
//!    panel. The agent responds with [`AgentResponse`] which may
//!    replay text or rewrite the buffer via [`AgentEdit`].
//! 2. **Active-AI chips** — when a command ends with non-zero
//!    exit, the previous block emits `AgentChip`s ("Ask AI to
//!    fix", "Explain this error") which the user can click to
//!    materialise a pre-seeded handoff.
//!
//! # What this module is NOT
//!
//! - Not an LLM transport. That's the agent panel's concern.
//! - Not a MCP client. That is `carrot-context-server`.
//! - Not a tool-use schema. This is the input **into** the agent,
//!   not the agent's own tool vocabulary.

use carrot_session::command_history::HistoryEntry;

use crate::ast::{CommandAst, Range};

/// Self-contained handoff payload.
///
/// The cmdline owns this struct for the brief moment between
/// "user pressed Enter on `#...` line" and "agent panel accepted
/// the handoff". After accept, the agent panel owns the data —
/// the cmdline returns to its own prompt state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHandoff {
    /// The user's message with the `#` prefix stripped. Whitespace
    /// trimmed on both sides.
    pub message: String,
    /// Parsed AST of the **previous** command if one is relevant
    /// (e.g. user typed `# why did it fail` after a failed command).
    /// `None` for stand-alone questions.
    pub context_ast: Option<CommandAst>,
    /// Working directory when the handoff was issued. Always
    /// populated for Carrot-native sessions.
    pub cwd: Option<String>,
    /// Bounded tail of the user's recent commands — the agent
    /// reads this to ground its answer.
    pub recent_history: Vec<HistoryEntry>,
    /// Whether the handoff was seeded from a chip (true) vs typed
    /// directly (false). Telemetry + UX cue distinguishes "user
    /// asked" from "user clicked the AI suggestion".
    pub from_chip: bool,
}

impl AgentHandoff {
    /// Construct a fresh handoff from a `#`-prefixed line, stripping
    /// the sigil and trimming whitespace. Returns `None` if the
    /// stripped message is empty.
    pub fn from_hash_line(line: &str) -> Option<Self> {
        let rest = line.strip_prefix('#')?;
        let trimmed = rest.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self {
            message: trimmed.to_string(),
            context_ast: None,
            cwd: None,
            recent_history: Vec::new(),
            from_chip: false,
        })
    }

    /// Attach the preceding command's AST as context.
    pub fn with_context_ast(mut self, ast: CommandAst) -> Self {
        self.context_ast = Some(ast);
        self
    }

    /// Attach the current working directory.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Attach recent-history entries (caller decides the window size).
    pub fn with_recent_history(mut self, entries: Vec<HistoryEntry>) -> Self {
        self.recent_history = entries;
        self
    }

    /// Mark the handoff as "seeded from a chip" — set when the user
    /// clicks an Active-AI chip instead of typing.
    pub fn from_chip(mut self) -> Self {
        self.from_chip = true;
        self
    }
}

/// A clickable Active-AI chip rendered inside / below a block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentChip {
    /// Chip label — rendered verbatim.
    pub label: String,
    /// Seed text inserted into the cmdline when clicked. Leading `#`
    /// is added automatically if absent.
    pub seed: String,
    /// Chip intent — drives icon choice + telemetry bucket.
    pub intent: ChipIntent,
}

/// Coarse categorisation of a chip's purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChipIntent {
    /// "Explain this output / error."
    Explain,
    /// "Fix this error" — agent proposes a corrected command.
    Fix,
    /// "Show me a related / next command."
    Suggest,
    /// Free-form chip — user-configured.
    Custom,
}

impl AgentChip {
    /// Materialise this chip into an `AgentHandoff`. Empty seeds
    /// degenerate to the chip label.
    pub fn into_handoff(self) -> AgentHandoff {
        let seed = if self.seed.is_empty() {
            self.label.clone()
        } else {
            self.seed.clone()
        };
        let message = seed.trim_start_matches('#').trim().to_string();
        AgentHandoff {
            message,
            context_ast: None,
            cwd: None,
            recent_history: Vec::new(),
            from_chip: true,
        }
    }

    /// Canonical "Ask AI to fix this" chip for a failed block.
    ///
    /// After every `BlockLifecycle::CommandEnd` with a non-zero
    /// exit code, the cmdline shows a dismissible chip inline
    /// above the prompt: "Ask Claude to fix this (Cmd+.)".
    pub fn ask_ai_to_fix(command: &str, exit_code: i32) -> Self {
        let seed = format!("# fix: {} (exit {})", command.trim(), exit_code);
        Self {
            label: "Ask AI to fix this (Cmd+.)".into(),
            seed,
            intent: ChipIntent::Fix,
        }
    }

    /// Canonical "Explain this error" chip.
    pub fn explain_error(command: &str) -> Self {
        let seed = format!("# explain the output of: {}", command.trim());
        Self {
            label: "Explain this error".into(),
            seed,
            intent: ChipIntent::Explain,
        }
    }
}

/// Produce the default chip set for a block that just ended. Returns
/// an empty vec for successful blocks (exit 0) — success doesn't
/// need an AI chip.
///
/// Callers hook this into [`crate::osc133::ShellEvent::CommandEnd`]
/// handling: when the event carries a non-zero exit, the returned
/// chips attach to the block's `extra_chips` metadata.
pub fn default_chips_for_failed_block(command: &str, exit_code: Option<i32>) -> Vec<AgentChip> {
    match exit_code {
        Some(code) if code != 0 => {
            vec![
                AgentChip::ask_ai_to_fix(command, code),
                AgentChip::explain_error(command),
            ]
        }
        _ => Vec::new(),
    }
}

/// Agent's reply to a handoff.
///
/// The cmdline dispatches these back to itself to apply the
/// requested effect. A single response may carry multiple effects
/// (e.g. a message AND an edit) — consumers loop over the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentResponse {
    /// Plain text reply — shown in the agent panel, cmdline
    /// itself stays untouched.
    Message(String),
    /// Agent proposes an edit to the cmdline buffer. Applied
    /// verbatim on accept; shown as a preview otherwise.
    Edit(AgentEdit),
    /// Agent emits a new chip to render on the triggering block.
    Chip(AgentChip),
}

/// An agent-proposed buffer edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentEdit {
    /// Byte range in the cmdline input to replace.
    pub range: Range,
    /// Replacement text.
    pub replacement: String,
    /// Why the edit was proposed — shown in the preview tooltip.
    pub rationale: Option<String>,
}

impl AgentEdit {
    /// Whether applying the edit would leave the buffer unchanged.
    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.replacement.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_line_strips_prefix_and_trims() {
        let h = AgentHandoff::from_hash_line("#  why did it fail  ").unwrap();
        assert_eq!(h.message, "why did it fail");
        assert!(!h.from_chip);
    }

    #[test]
    fn hash_line_rejects_non_hash_prefix() {
        assert!(AgentHandoff::from_hash_line("why did it fail").is_none());
    }

    #[test]
    fn hash_line_rejects_empty_body() {
        assert!(AgentHandoff::from_hash_line("#").is_none());
        assert!(AgentHandoff::from_hash_line("#   ").is_none());
    }

    #[test]
    fn builder_chain_fills_context() {
        let h = AgentHandoff::from_hash_line("#fix this")
            .unwrap()
            .with_cwd("/tmp")
            .with_context_ast(CommandAst::empty())
            .from_chip();
        assert_eq!(h.cwd.as_deref(), Some("/tmp"));
        assert!(h.context_ast.is_some());
        assert!(h.from_chip);
    }

    #[test]
    fn chip_into_handoff_marks_from_chip() {
        let chip = AgentChip {
            label: "Ask AI to fix".into(),
            seed: "# explain the failure".into(),
            intent: ChipIntent::Fix,
        };
        let handoff = chip.into_handoff();
        assert!(handoff.from_chip);
        assert_eq!(handoff.message, "explain the failure");
    }

    #[test]
    fn chip_with_empty_seed_falls_back_to_label() {
        let chip = AgentChip {
            label: "Explain".into(),
            seed: String::new(),
            intent: ChipIntent::Explain,
        };
        let h = chip.into_handoff();
        assert_eq!(h.message, "Explain");
    }

    #[test]
    fn agent_edit_noop_detection() {
        let edit = AgentEdit {
            range: Range::new(5, 5),
            replacement: String::new(),
            rationale: None,
        };
        assert!(edit.is_noop());
        let real = AgentEdit {
            range: Range::new(0, 3),
            replacement: "git".into(),
            rationale: None,
        };
        assert!(!real.is_noop());
    }

    #[test]
    fn default_chips_empty_for_success() {
        assert!(default_chips_for_failed_block("ls", Some(0)).is_empty());
        assert!(default_chips_for_failed_block("ls", None).is_empty());
    }

    #[test]
    fn default_chips_emit_for_failure() {
        let chips = default_chips_for_failed_block("git push", Some(1));
        assert_eq!(chips.len(), 2);
        assert!(matches!(chips[0].intent, ChipIntent::Fix));
        assert!(matches!(chips[1].intent, ChipIntent::Explain));
    }

    #[test]
    fn ask_ai_to_fix_seeds_command_and_exit() {
        let chip = AgentChip::ask_ai_to_fix("git push", 128);
        assert_eq!(chip.seed, "# fix: git push (exit 128)");
        assert_eq!(chip.label, "Ask AI to fix this (Cmd+.)");
    }

    #[test]
    fn explain_error_seeds_command() {
        let chip = AgentChip::explain_error("cargo build");
        assert_eq!(chip.seed, "# explain the output of: cargo build");
    }

    #[test]
    fn ask_ai_to_fix_chip_round_trips_into_handoff() {
        let chip = AgentChip::ask_ai_to_fix("cargo test", 101);
        let h = chip.into_handoff();
        assert!(h.from_chip);
        assert_eq!(h.message, "fix: cargo test (exit 101)");
    }
}
