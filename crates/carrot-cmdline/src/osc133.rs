//! OSC 133 (FTCS — Final Term Command Shell) state-machine glue.
//!
//! OSC 133 is the de-facto shell-integration protocol: the shell
//! emits `ESC ] 133 ; A|B|C|D ; … BEL` at key moments and the
//! terminal interprets them to drive the prompt lifecycle.
//!
//! | Sub-command | Meaning |
//! |-------------|---------|
//! | `A` | Prompt start — shell is about to render its prompt. |
//! | `B` | Prompt end / input start — user input begins here. |
//! | `C` | Command start — user hit Enter, command is running. |
//! | `D ; <exit>` | Command end — with optional exit status. |
//! | `L` | (Carrot extension) Agent-edit in progress — disable ghost-text. |
//!
//! # Layer note
//!
//! The **event type** ([`ShellMarker`]) lives in
//! `carrot-shell-integration`. `carrot-terminal` owns the parser
//! (scanning OSC sequences out of the PTY byte stream); this module
//! owns the state-machine glue — mapping a marker onto a
//! [`PromptState`] transition, plus an emitter that writes the
//! marker back out as bytes for hook scripts.
//!
//! The name `ShellEvent` is kept as a type alias so existing
//! call-sites compile untouched; new code can use `ShellMarker`
//! directly.

pub use carrot_shell_integration::ShellMarker;

use crate::mount::BlockHandle;
use crate::prompt_state::{InteractivePromptState, PromptState};

/// Alias preserved for the cmdline's existing call-sites and tests.
/// Prefer [`ShellMarker`] in new code.
pub type ShellEvent = ShellMarker;

/// Target [`PromptState`] when `event` arrives while we're in
/// `current`. Returns `None` when the event doesn't move the state
/// machine.
///
/// The mapping is deliberately conservative — the state machine is
/// the authority, this function just proposes a target. The caller
/// (typically [`crate::session::CmdlineSession`]) owns the
/// [`BlockHandle`] for the currently-attached block; when we propose
/// an `Executing` target we attach `block` for a fresh execution.
pub fn transition_target(
    event: &ShellMarker,
    current: &PromptState,
    block: BlockHandle,
) -> Option<PromptState> {
    use ShellMarker::*;
    match (current, event) {
        (_, PromptStart) | (_, InputStart) => Some(PromptState::Active),
        (PromptState::Active, CommandStart) => Some(PromptState::Executing {
            block,
            inner: InteractivePromptState::Hidden,
        }),
        (PromptState::Executing { .. }, CommandEnd { .. }) => Some(PromptState::Active),
        // AgentEditActive / PromptKind / Metadata / TuiHint /
        // AgentEvent are hints, not state transitions.
        _ => None,
    }
}

/// `true` when the event ends the current block (Active → Frozen).
pub fn ends_block(event: &ShellMarker) -> bool {
    matches!(event, ShellMarker::CommandEnd { .. })
}

/// `true` when the event opens a new block. `PromptStart` is the
/// canonical "new block" marker.
pub fn opens_block(event: &ShellMarker) -> bool {
    matches!(event, ShellMarker::PromptStart)
}

/// Exit code if the event carries one.
pub fn exit_code(event: &ShellMarker) -> Option<i32> {
    match event {
        ShellMarker::CommandEnd { exit_code } => Some(*exit_code),
        _ => None,
    }
}

// ─── Emitter ─────────────────────────────────────────────────────

/// Emit the OSC 133 byte sequence for `event`. The returned bytes
/// are suitable for writing directly to the shell's stdin or into a
/// shell-integration hook script.
///
/// Format per FTCS: `ESC ] 133 ; <sub> BEL`.
///
/// Variants outside the OSC-133 taxonomy (`PromptKind`, `Metadata`,
/// `TuiHint`, `AgentEvent`) return an empty byte vector — they are
/// consumer-only markers, the terminal never needs to re-emit them.
pub fn emit(event: &ShellMarker) -> Vec<u8> {
    const ESC: u8 = 0x1b;
    const BEL: u8 = 0x07;
    let mut out = vec![ESC, b']', b'1', b'3', b'3', b';'];
    match event {
        ShellMarker::PromptStart => out.push(b'A'),
        ShellMarker::InputStart => out.push(b'B'),
        ShellMarker::CommandStart => out.push(b'C'),
        ShellMarker::CommandEnd { exit_code } => {
            out.push(b'D');
            out.push(b';');
            out.extend_from_slice(exit_code.to_string().as_bytes());
        }
        ShellMarker::AgentEditActive => out.push(b'L'),
        ShellMarker::PromptKind { .. }
        | ShellMarker::Metadata(_)
        | ShellMarker::TuiHint(_)
        | ShellMarker::AgentEvent(_) => return Vec::new(),
    }
    out.push(BEL);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn h(n: u64) -> BlockHandle {
        BlockHandle(NonZeroU64::new(n).expect("non-zero"))
    }

    #[test]
    fn prompt_start_always_returns_to_active() {
        let block = h(1);
        for state in [
            PromptState::Active,
            PromptState::Executing {
                block,
                inner: InteractivePromptState::Hidden,
            },
        ] {
            assert_eq!(
                transition_target(&ShellMarker::PromptStart, &state, block),
                Some(PromptState::Active)
            );
        }
    }

    #[test]
    fn input_start_also_returns_active() {
        let block = h(1);
        assert_eq!(
            transition_target(&ShellMarker::InputStart, &PromptState::Active, block),
            Some(PromptState::Active)
        );
    }

    #[test]
    fn command_start_only_from_active() {
        let block = h(5);
        let target = transition_target(&ShellMarker::CommandStart, &PromptState::Active, block);
        assert_eq!(
            target,
            Some(PromptState::Executing {
                block,
                inner: InteractivePromptState::Hidden,
            })
        );
        assert!(
            transition_target(
                &ShellMarker::CommandStart,
                &PromptState::Executing {
                    block,
                    inner: InteractivePromptState::Hidden,
                },
                block,
            )
            .is_none()
        );
    }

    #[test]
    fn command_end_collapses_to_active() {
        let block = h(3);
        let ev = ShellMarker::CommandEnd { exit_code: 0 };
        let from_hidden = PromptState::Executing {
            block,
            inner: InteractivePromptState::Hidden,
        };
        assert_eq!(
            transition_target(&ev, &from_hidden, block),
            Some(PromptState::Active)
        );
        let from_transient = PromptState::Executing {
            block,
            inner: InteractivePromptState::Transient {
                detected: crate::prompt_state::InteractionKind::FreeText,
                buffer: String::new(),
                masked: false,
            },
        };
        assert_eq!(
            transition_target(&ev, &from_transient, block),
            Some(PromptState::Active)
        );
    }

    #[test]
    fn agent_edit_is_hint_only() {
        assert!(
            transition_target(&ShellMarker::AgentEditActive, &PromptState::Active, h(1)).is_none()
        );
    }

    #[test]
    fn ends_and_opens_block_flags() {
        assert!(ends_block(&ShellMarker::CommandEnd { exit_code: 0 }));
        assert!(opens_block(&ShellMarker::PromptStart));
        assert!(!ends_block(&ShellMarker::CommandStart));
        assert!(!opens_block(&ShellMarker::CommandEnd { exit_code: 1 }));
    }

    #[test]
    fn exit_code_extraction() {
        assert_eq!(
            exit_code(&ShellMarker::CommandEnd { exit_code: 42 }),
            Some(42)
        );
        assert_eq!(exit_code(&ShellMarker::PromptStart), None);
    }

    #[test]
    fn emitter_roundtrip_shape() {
        assert_eq!(emit(&ShellMarker::PromptStart), b"\x1b]133;A\x07");
        assert_eq!(emit(&ShellMarker::InputStart), b"\x1b]133;B\x07");
        assert_eq!(emit(&ShellMarker::CommandStart), b"\x1b]133;C\x07");
        assert_eq!(
            emit(&ShellMarker::CommandEnd { exit_code: 0 }),
            b"\x1b]133;D;0\x07"
        );
        assert_eq!(emit(&ShellMarker::AgentEditActive), b"\x1b]133;L\x07");
    }

    #[test]
    fn hint_markers_do_not_emit_bytes() {
        assert!(emit(&ShellMarker::Metadata("{}".into())).is_empty());
        assert!(emit(&ShellMarker::TuiHint("{}".into())).is_empty());
        assert!(emit(&ShellMarker::AgentEvent("{}".into())).is_empty());
        assert!(
            emit(&ShellMarker::PromptKind {
                kind: carrot_shell_integration::PromptKindType::Initial
            })
            .is_empty()
        );
    }
}
