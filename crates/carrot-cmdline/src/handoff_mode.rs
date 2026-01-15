//! `#` agent handoff mode.
//!
//! When the first non-whitespace character in the buffer is `#`,
//! the cmdline switches from shell input to agent input:
//!
//! - Accent colour switches to the active theme's accent token.
//! - Enter routes to the agent instead of the PTY.
//! - The agent streams its response as an inline, collapsible block
//!   above the prompt.
//! - Escape cancels and returns to shell mode, keeping typed
//!   content so the user can edit the `#` off.
//!
//! This module owns the mode state and the detector. Actual rendering
//! (accent glyph tint, streamed response block) lives in `element.rs`
//! when it lands and reads the accent colour from the theme; routing
//! Enter to the agent vs. the PTY lives in `pty_route.rs`.

/// Cmdline operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HandoffMode {
    /// Normal shell input — Enter routes to the PTY.
    #[default]
    Shell,
    /// `#` handoff active — Enter routes to the agent, accent colour
    /// is applied, responses render inline.
    Agent,
}

impl HandoffMode {
    pub fn is_agent(self) -> bool {
        matches!(self, HandoffMode::Agent)
    }

    pub fn is_shell(self) -> bool {
        matches!(self, HandoffMode::Shell)
    }
}

/// Detect the current mode from a buffer snapshot. Returns
/// [`HandoffMode::Agent`] when the first non-whitespace byte is
/// `#`, [`HandoffMode::Shell`] otherwise.
pub fn detect_mode(buffer: &str) -> HandoffMode {
    let first = buffer.chars().find(|c| !c.is_whitespace());
    match first {
        Some('#') => HandoffMode::Agent,
        _ => HandoffMode::Shell,
    }
}

/// Mode transition produced by a buffer edit. The caller drives
/// this from the keystroke handler: before + after the buffer
/// mutation, compute the mode and compare. The `entered` /
/// `exited` flags tell the renderer whether a mode flash effect
/// should play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffTransition {
    pub before: HandoffMode,
    pub after: HandoffMode,
}

impl HandoffTransition {
    pub fn entered(self) -> bool {
        matches!(
            (self.before, self.after),
            (HandoffMode::Shell, HandoffMode::Agent)
        )
    }

    pub fn exited(self) -> bool {
        matches!(
            (self.before, self.after),
            (HandoffMode::Agent, HandoffMode::Shell)
        )
    }

    pub fn changed(self) -> bool {
        self.before != self.after
    }
}

/// Strip the leading `#` (and any whitespace between it and the
/// message body) from a buffer. Used when the user hits Enter in
/// Agent mode so the bytes sent to the agent don't include the
/// sigil. Returns `None` when the buffer isn't in Agent mode.
///
/// Whitespace outside the leading sigil is preserved.
pub fn strip_sigil(buffer: &str) -> Option<&str> {
    // Walk past leading whitespace.
    let trimmed = buffer.trim_start();
    let rest = trimmed.strip_prefix('#')?;
    // Leave the user's own content intact: strip only the `#` itself
    // and a single separator space (common idiom: `# explain this`).
    let remainder = rest.strip_prefix(' ').unwrap_or(rest);
    Some(remainder)
}

/// Keystroke produced by Escape in Agent mode: drop the leading
/// `#` + separator so the buffer becomes pure shell input again.
/// No-op when not in Agent mode.
pub fn cancel_to_shell(buffer: &str) -> String {
    match detect_mode(buffer) {
        HandoffMode::Agent => strip_sigil(buffer).unwrap_or(buffer).to_string(),
        HandoffMode::Shell => buffer.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_agent_on_hash_first_char() {
        assert_eq!(detect_mode("# why"), HandoffMode::Agent);
        assert_eq!(detect_mode("#why"), HandoffMode::Agent);
        assert_eq!(detect_mode("   # why"), HandoffMode::Agent);
    }

    #[test]
    fn detect_shell_otherwise() {
        assert_eq!(detect_mode(""), HandoffMode::Shell);
        assert_eq!(detect_mode("ls"), HandoffMode::Shell);
        assert_eq!(detect_mode("echo #"), HandoffMode::Shell);
    }

    #[test]
    fn transition_flags_entry_and_exit() {
        let t_in = HandoffTransition {
            before: HandoffMode::Shell,
            after: HandoffMode::Agent,
        };
        assert!(t_in.entered());
        assert!(!t_in.exited());
        assert!(t_in.changed());

        let t_out = HandoffTransition {
            before: HandoffMode::Agent,
            after: HandoffMode::Shell,
        };
        assert!(!t_out.entered());
        assert!(t_out.exited());
        assert!(t_out.changed());

        let t_none = HandoffTransition {
            before: HandoffMode::Shell,
            after: HandoffMode::Shell,
        };
        assert!(!t_none.changed());
    }

    #[test]
    fn strip_sigil_removes_hash_and_one_space() {
        assert_eq!(strip_sigil("# explain"), Some("explain"));
        assert_eq!(strip_sigil("#explain"), Some("explain"));
        assert_eq!(
            strip_sigil("  # explain this please"),
            Some("explain this please")
        );
    }

    #[test]
    fn strip_sigil_returns_none_when_no_sigil() {
        assert!(strip_sigil("explain").is_none());
        assert!(strip_sigil("").is_none());
    }

    #[test]
    fn strip_preserves_inner_whitespace() {
        assert_eq!(
            strip_sigil("#  two leading spaces"),
            Some(" two leading spaces"),
        );
    }

    #[test]
    fn cancel_to_shell_drops_sigil_in_agent_mode() {
        assert_eq!(cancel_to_shell("# explain"), "explain");
        assert_eq!(cancel_to_shell("#explain"), "explain");
    }

    #[test]
    fn cancel_to_shell_noop_in_shell_mode() {
        assert_eq!(cancel_to_shell("ls -la"), "ls -la");
        assert_eq!(cancel_to_shell(""), "");
    }

    #[test]
    fn handoff_mode_is_agent_flag() {
        assert!(HandoffMode::Agent.is_agent());
        assert!(!HandoffMode::Agent.is_shell());
        assert!(HandoffMode::Shell.is_shell());
        assert!(!HandoffMode::Shell.is_agent());
    }

    #[test]
    fn default_is_shell() {
        assert_eq!(HandoffMode::default(), HandoffMode::Shell);
    }
}
