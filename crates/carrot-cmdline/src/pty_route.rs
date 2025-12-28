//! Keystroke → route decision.
//!
//! When a command is running, keystrokes route straight to the PTY
//! so interactive TUIs (vim, top, claude-inside-Carrot) work.
//! When the cmdline is mounted and the user types `#`-first,
//! keystrokes go to the agent instead. This module owns the
//! dispatch switch that the keystroke handler consults.
//!
//! The actual PTY handle lives in `carrot-terminal`; this module
//! doesn't import it. It produces a [`KeystrokeRoute`] decision
//! from the current [`PromptState`] + [`HandoffMode`], and the
//! consumer (terminal-view) dispatches the bytes accordingly.

use crate::handoff_mode::{HandoffMode, detect_mode};
use crate::prompt_state::{InteractivePromptState, PromptState};

/// Where a keystroke should go. The renderer picks the consumer
/// based on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeystrokeRoute {
    /// Keystroke is consumed by the cmdline editor (buffer edit,
    /// cursor move, completion accept, …).
    Cmdline,
    /// Keystroke is written to the PTY's stdin — the running
    /// command (vim / top / ssh / …) gets it.
    Pty,
    /// Keystroke is interpreted by the agent surface (ghost-text
    /// accept, chip keystroke, `#` handoff).
    Agent,
    /// Keystroke is written to the PTY but as **transient input**
    /// — the Transient editor buffers it before sending. Callers
    /// treat it like `Cmdline` for keystroke handling and invoke
    /// `commit_transient` on Enter.
    TransientInput,
}

/// Decide where a keystroke should go for the given state pair.
///
/// Decision table:
/// - `PromptState::Active` + `HandoffMode::Shell`     → Cmdline
/// - `PromptState::Active` + `HandoffMode::Agent`     → Agent
/// - `PromptState::Executing::Hidden`                 → Pty (pass-through)
/// - `PromptState::Executing::Transient { .. }`       → TransientInput
pub fn route_keystroke(state: &PromptState, mode: HandoffMode) -> KeystrokeRoute {
    match state {
        PromptState::Active => match mode {
            HandoffMode::Shell => KeystrokeRoute::Cmdline,
            HandoffMode::Agent => KeystrokeRoute::Agent,
        },
        PromptState::Executing {
            inner: InteractivePromptState::Hidden,
            ..
        } => KeystrokeRoute::Pty,
        PromptState::Executing {
            inner: InteractivePromptState::Transient { .. },
            ..
        } => KeystrokeRoute::TransientInput,
    }
}

/// Higher-level: route a keystroke whose buffer context is `buffer`.
/// Infers `HandoffMode` from the buffer via `detect_mode`.
pub fn route_with_buffer(state: &PromptState, buffer: &str) -> KeystrokeRoute {
    route_keystroke(state, detect_mode(buffer))
}

/// Whether the agent surface should receive the keystroke. Includes
/// ghost-text accept + chip activation, not just `#` handoff.
pub fn agent_consumes(route: KeystrokeRoute) -> bool {
    matches!(route, KeystrokeRoute::Agent)
}

/// Whether the PTY should receive the raw byte. Includes both the
/// straight pass-through (`Pty`) and the Transient-buffered path
/// (on Enter commit).
pub fn pty_consumes(route: KeystrokeRoute) -> bool {
    matches!(route, KeystrokeRoute::Pty | KeystrokeRoute::TransientInput)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::BlockHandle;
    use crate::prompt_state::InteractionKind;
    use std::num::NonZeroU64;

    fn block(n: u64) -> BlockHandle {
        BlockHandle(NonZeroU64::new(n).expect("non-zero"))
    }

    #[test]
    fn active_shell_routes_to_cmdline() {
        assert_eq!(
            route_keystroke(&PromptState::Active, HandoffMode::Shell),
            KeystrokeRoute::Cmdline
        );
    }

    #[test]
    fn active_agent_routes_to_agent() {
        assert_eq!(
            route_keystroke(&PromptState::Active, HandoffMode::Agent),
            KeystrokeRoute::Agent
        );
    }

    #[test]
    fn executing_hidden_routes_to_pty() {
        let state = PromptState::Executing {
            block: block(1),
            inner: InteractivePromptState::Hidden,
        };
        // HandoffMode is ignored in Executing.
        assert_eq!(
            route_keystroke(&state, HandoffMode::Shell),
            KeystrokeRoute::Pty
        );
        assert_eq!(
            route_keystroke(&state, HandoffMode::Agent),
            KeystrokeRoute::Pty
        );
    }

    #[test]
    fn executing_transient_routes_to_transient_input() {
        let state = PromptState::Executing {
            block: block(1),
            inner: InteractivePromptState::Transient {
                detected: InteractionKind::Password,
                buffer: String::new(),
                masked: true,
            },
        };
        assert_eq!(
            route_keystroke(&state, HandoffMode::Shell),
            KeystrokeRoute::TransientInput
        );
    }

    #[test]
    fn route_with_buffer_detects_hash_prefix() {
        let r = route_with_buffer(&PromptState::Active, "# why did this fail");
        assert_eq!(r, KeystrokeRoute::Agent);
        let r = route_with_buffer(&PromptState::Active, "ls -la");
        assert_eq!(r, KeystrokeRoute::Cmdline);
    }

    #[test]
    fn agent_consumes_flag() {
        assert!(agent_consumes(KeystrokeRoute::Agent));
        assert!(!agent_consumes(KeystrokeRoute::Cmdline));
        assert!(!agent_consumes(KeystrokeRoute::Pty));
        assert!(!agent_consumes(KeystrokeRoute::TransientInput));
    }

    #[test]
    fn pty_consumes_flag() {
        assert!(pty_consumes(KeystrokeRoute::Pty));
        assert!(pty_consumes(KeystrokeRoute::TransientInput));
        assert!(!pty_consumes(KeystrokeRoute::Cmdline));
        assert!(!pty_consumes(KeystrokeRoute::Agent));
    }

    #[test]
    fn route_is_stable_across_executing_regardless_of_buffer() {
        // The buffer's leading `#` means nothing when a command is
        // running — keystrokes go to the PTY.
        let state = PromptState::Executing {
            block: block(1),
            inner: InteractivePromptState::Hidden,
        };
        assert_eq!(
            route_with_buffer(&state, "# this should not matter"),
            KeystrokeRoute::Pty
        );
    }
}
