//! Prompt-lifecycle state machine.
//!
//! The cmdline surface moves between three top-level phases: the
//! user is typing (`Active`), a command is running with no input
//! expected (`Executing::Hidden`), or a running command has asked
//! for input mid-flight (`Executing::Transient`). The nested
//! `Transient` carries a detected [`InteractionKind`] (password,
//! yes/no, free-text) and an optional masking flag so screen
//! recordings, AI ghost-text, and the agent panel never see
//! password bytes.
//!
//! Mid-command interactive prompts — `sudo`, `ssh`, `git credential
//! helper`, `npm init`, `docker login`, `read -p`, interactive
//! installers — cannot be modelled as a flat 4-state enum because the
//! cmdline has to re-mount inside the active block while that block
//! is still running.
//!
//! # States
//!
//! - [`PromptState::Active`] — default "busy editing"; buffer + cursor
//!   live in the cmdline editor.
//! - [`PromptState::Executing`] — a command is in flight. Carries the
//!   attached [`BlockHandle`] plus one of:
//!   - [`InteractivePromptState::Hidden`]: pure output stream, cmdline
//!     unmounted from the bottom, keystrokes route to the PTY.
//!   - [`InteractivePromptState::Transient`]: running block asked for
//!     input. Cmdline is re-mounted inside the block with a minimal
//!     editor configuration (single cursor, no AI, no syntax
//!     highlight). On submit, the bytes are written to the PTY's
//!     stdin, not parsed as a new command.
//!
//! # Detection
//!
//! The `Hidden → Transient` transition is driven by two signals:
//!
//! 1. **Primary:** the shell re-emits `OSC 133;B` mid-block (bash,
//!    zsh, fish with hooks, nu natively).
//! 2. **Fallback:** pattern-match the block's output tail for
//!    well-known prompts (`password:`, `passphrase:`, `[y/N]`,
//!    `Continue?`, `(yes/no)?`, known credential-helper prompts).
//!    See [`detect_interaction_kind`].
//!
//! # Masking
//!
//! When [`InteractionKind::Password`] is detected, `masked == true`
//! so consumers know to hide the buffer in ghost-text, skip it in
//! AI context, and omit it from `agent.current_text()`. Enforced by
//! the `buffer()` accessor returning `None` when masked — callers
//! get the full text only when explicitly asking for the masked
//! content.

use crate::mount::BlockHandle;

/// Top-level lifecycle state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PromptState {
    /// User is typing; buffer owned by the cmdline editor.
    #[default]
    Active,
    /// A command is running; further state carried by `inner`.
    Executing {
        block: BlockHandle,
        inner: InteractivePromptState,
    },
}

/// Fine-grained state while a command is executing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractivePromptState {
    /// Pure output stream, cmdline unmounted from the bottom. Keys
    /// route to the PTY.
    Hidden,
    /// Running block asked for input. Cmdline re-mounts inside the
    /// block with a minimal editor.
    Transient {
        detected: InteractionKind,
        /// Current input buffer for the interactive prompt. Empty
        /// string when the user hasn't typed anything yet. Stored
        /// in-crate as a plain `String` so we don't require the
        /// carrot-editor dep at this layer; the real `Buffer` type
        /// attaches in the feature-gated integration.
        buffer: String,
        /// When true, the buffer is considered sensitive — see
        /// module-level docs.
        masked: bool,
    },
}

/// Shape of the detected interactive prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InteractionKind {
    /// Password / passphrase / PIN / secret.
    Password,
    /// Binary yes-or-no choice (`[y/N]`, `(yes/no)?`).
    YesNo,
    /// Free-text input (`Name:`, `Project name:`, …).
    FreeText,
}

impl InteractionKind {
    /// Whether the kind warrants input masking by default.
    pub fn default_masked(self) -> bool {
        matches!(self, InteractionKind::Password)
    }
}

impl PromptState {
    /// Whether the user is actively editing a command in the cmdline.
    pub fn is_active(&self) -> bool {
        matches!(self, PromptState::Active)
    }

    /// Whether a command is running.
    pub fn is_executing(&self) -> bool {
        matches!(self, PromptState::Executing { .. })
    }

    /// Whether keystrokes should be routed to the cmdline editor
    /// (true) or straight to the PTY (false).
    pub fn routes_input_to_cmdline(&self) -> bool {
        matches!(
            self,
            PromptState::Active
                | PromptState::Executing {
                    inner: InteractivePromptState::Transient { .. },
                    ..
                }
        )
    }

    /// Attached block, if any.
    pub fn block(&self) -> Option<BlockHandle> {
        match self {
            PromptState::Executing { block, .. } => Some(*block),
            _ => None,
        }
    }

    /// Accessor for the detected interaction kind of the current
    /// `Transient` state, if any.
    pub fn interaction_kind(&self) -> Option<InteractionKind> {
        match self {
            PromptState::Executing {
                inner: InteractivePromptState::Transient { detected, .. },
                ..
            } => Some(*detected),
            _ => None,
        }
    }

    /// Current transient-prompt buffer. Returns `None` when the
    /// buffer is masked (consumers must call `masked_buffer` to
    /// access the sensitive bytes explicitly). Also returns `None`
    /// when we're not in a Transient state.
    pub fn buffer(&self) -> Option<&str> {
        match self {
            PromptState::Executing {
                inner:
                    InteractivePromptState::Transient {
                        buffer,
                        masked: false,
                        ..
                    },
                ..
            } => Some(buffer.as_str()),
            _ => None,
        }
    }

    /// Explicit accessor for masked buffers. Used only by the PTY
    /// write path; never by the ghost-text / AI / agent surfaces.
    pub fn masked_buffer(&self) -> Option<&str> {
        match self {
            PromptState::Executing {
                inner: InteractivePromptState::Transient { buffer, .. },
                ..
            } => Some(buffer.as_str()),
            _ => None,
        }
    }
}

/// Pattern-match the running block's output tail to detect a
/// mid-command interactive prompt. Returns the kind when a known
/// pattern matches, `None` otherwise.
///
/// Used as the fallback detector when the shell doesn't re-emit
/// OSC-133 mid-block. Pattern list is intentionally conservative;
/// false positives here would steer user keystrokes into the
/// Transient buffer instead of the PTY.
pub fn detect_interaction_kind(output_tail: &str) -> Option<InteractionKind> {
    let lower = output_tail.to_ascii_lowercase();
    let trimmed = lower.trim_end();

    // Password-style prompts.
    for needle in [
        "password:",
        "password for",
        "passphrase:",
        "passphrase for",
        "sudo password",
        "enter pin",
        "secret:",
    ] {
        if trimmed.ends_with(needle) || trimmed.contains(needle) {
            return Some(InteractionKind::Password);
        }
    }

    // Yes / No prompts.
    for needle in [
        "[y/n]",
        "[Y/n]",
        "[y/N]",
        "(yes/no)?",
        "(yes/no)",
        "continue?",
        "are you sure?",
        "proceed?",
    ] {
        if trimmed.ends_with(&needle.to_ascii_lowercase())
            || trimmed.contains(&needle.to_ascii_lowercase())
        {
            return Some(InteractionKind::YesNo);
        }
    }

    // Free-text prompt heuristic: ends with ":" / "?" / ">" or
    // with a "(default)" suffix — npm init / cargo new write
    // `package name: (my-package) ` and similar.
    if trimmed.ends_with(':')
        || trimmed.ends_with('?')
        || trimmed.ends_with('>')
        || trimmed.ends_with(')')
    {
        return Some(InteractionKind::FreeText);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn h(n: u64) -> BlockHandle {
        BlockHandle(NonZeroU64::new(n).expect("non-zero"))
    }

    #[test]
    fn default_is_active() {
        assert_eq!(PromptState::default(), PromptState::Active);
    }

    #[test]
    fn active_state_predicates() {
        let s = PromptState::Active;
        assert!(s.is_active());
        assert!(!s.is_executing());
        assert!(s.routes_input_to_cmdline());
        assert!(s.block().is_none());
    }

    #[test]
    fn executing_hidden_predicates() {
        let s = PromptState::Executing {
            block: h(1),
            inner: InteractivePromptState::Hidden,
        };
        assert!(!s.is_active());
        assert!(s.is_executing());
        assert!(!s.routes_input_to_cmdline());
        assert_eq!(s.block(), Some(h(1)));
        assert!(s.interaction_kind().is_none());
    }

    #[test]
    fn executing_transient_routes_to_cmdline() {
        let s = PromptState::Executing {
            block: h(2),
            inner: InteractivePromptState::Transient {
                detected: InteractionKind::FreeText,
                buffer: "hello".into(),
                masked: false,
            },
        };
        assert!(s.routes_input_to_cmdline());
        assert_eq!(s.interaction_kind(), Some(InteractionKind::FreeText));
        assert_eq!(s.buffer(), Some("hello"));
        assert_eq!(s.masked_buffer(), Some("hello"));
    }

    #[test]
    fn masked_transient_hides_buffer() {
        let s = PromptState::Executing {
            block: h(3),
            inner: InteractivePromptState::Transient {
                detected: InteractionKind::Password,
                buffer: "super-secret".into(),
                masked: true,
            },
        };
        // Public `buffer()` hides the masked bytes.
        assert!(s.buffer().is_none());
        // Explicit `masked_buffer()` returns them for the PTY write.
        assert_eq!(s.masked_buffer(), Some("super-secret"));
    }

    #[test]
    fn password_kind_defaults_to_masked() {
        assert!(InteractionKind::Password.default_masked());
        assert!(!InteractionKind::YesNo.default_masked());
        assert!(!InteractionKind::FreeText.default_masked());
    }

    #[test]
    fn detect_password_prompts() {
        assert_eq!(
            detect_interaction_kind("Password: "),
            Some(InteractionKind::Password)
        );
        assert_eq!(
            detect_interaction_kind("[sudo] password for nyxb: "),
            Some(InteractionKind::Password)
        );
        assert_eq!(
            detect_interaction_kind("Enter passphrase for /Users/nyxb/.ssh/id_ed25519: "),
            Some(InteractionKind::Password)
        );
    }

    #[test]
    fn detect_yes_no_prompts() {
        assert_eq!(
            detect_interaction_kind("Continue? [y/N] "),
            Some(InteractionKind::YesNo)
        );
        assert_eq!(
            detect_interaction_kind("Are you sure?"),
            Some(InteractionKind::YesNo)
        );
        assert_eq!(
            detect_interaction_kind("Proceed?"),
            Some(InteractionKind::YesNo)
        );
    }

    #[test]
    fn detect_free_text_prompts() {
        assert_eq!(
            detect_interaction_kind("Name: "),
            Some(InteractionKind::FreeText)
        );
        assert_eq!(
            detect_interaction_kind("git> "),
            Some(InteractionKind::FreeText)
        );
    }

    #[test]
    fn detect_none_on_plain_output() {
        assert!(detect_interaction_kind("").is_none());
        assert!(detect_interaction_kind("regular output line").is_none());
        assert!(detect_interaction_kind("Done.").is_none());
    }

    #[test]
    fn interaction_kind_is_copy_and_hash() {
        use std::collections::HashSet;
        let mut s: HashSet<InteractionKind> = HashSet::new();
        s.insert(InteractionKind::Password);
        s.insert(InteractionKind::Password);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn executing_without_block_not_constructible() {
        // Compile-time guarantee: PromptState::Executing requires block.
        // This test just documents the invariant.
        let s = PromptState::Executing {
            block: h(42),
            inner: InteractivePromptState::Hidden,
        };
        assert_eq!(s.block().unwrap().get(), 42);
    }
}
