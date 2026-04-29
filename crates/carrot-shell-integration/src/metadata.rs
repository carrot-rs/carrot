/// Shell metadata received via OSC 7777;carrot-precmd.
///
/// Deserialized from hex-encoded JSON sent by the shell precmd hook AND
/// the preexec hook. Two emits per command-cycle: the precmd-time emit
/// carries cwd / git / user / host / exit / duration; the preexec-time
/// emit carries the about-to-execute `command` line.
///
/// Every field is `Option`-typed so a partial emit (e.g. preexec with
/// only `command`) does not clobber the values captured at precmd.
/// `ShellContext::update_from_metadata` performs field-by-field merge.
///
/// Extensible with `#[serde(default)]` — new fields can be added without
/// breaking shells that don't send them yet.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ShellMetadataPayload {
    pub cwd: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub git_root: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_duration_ms: Option<u64>,
    pub shell: Option<String>,
    /// Command line about to be executed. Set by the preexec-time
    /// emit; consumed by the OSC 133;C dispatch into the new block's
    /// `RouterBlockMetadata.command`. Authoritative source — wins
    /// over the cmdline-side `pending_command` slot, since the shell
    /// sees what actually runs (handles paste-and-execute, scripts,
    /// agent-PTY-writes that bypass the cmdline).
    pub command: Option<String>,
}

#[cfg(test)]
mod metadata_payload_tests {
    use super::*;

    #[test]
    fn payload_with_only_command_parses() {
        // Preexec-time emit carries only the command field. cwd /
        // git / user fields stay None — the precmd-time emit owns
        // those, and field-by-field merge in update_from_metadata
        // ensures they don't get clobbered.
        let json = r#"{"command":"cd Projects"}"#;
        let payload: ShellMetadataPayload =
            serde_json::from_str(json).expect("partial payload should parse");
        assert_eq!(payload.command.as_deref(), Some("cd Projects"));
        assert!(payload.cwd.is_none());
        assert!(payload.git_branch.is_none());
    }

    #[test]
    fn payload_with_only_cwd_parses_without_command() {
        // Precmd-time emit (existing path) — no command field, must
        // still parse.
        let json = r#"{"cwd":"/home/x","username":"x","hostname":"y","shell":"zsh"}"#;
        let payload: ShellMetadataPayload =
            serde_json::from_str(json).expect("legacy payload should parse");
        assert_eq!(payload.cwd.as_deref(), Some("/home/x"));
        assert!(payload.command.is_none());
    }

    #[test]
    fn payload_with_escaped_command_roundtrips() {
        // JSON escapes for the kinds of characters real shells emit
        // — quotes, backslashes, newlines for multiline commands.
        let json = r#"{"command":"echo \"hi\\nthere\""}"#;
        let payload: ShellMetadataPayload =
            serde_json::from_str(json).expect("escaped payload should parse");
        assert_eq!(payload.command.as_deref(), Some("echo \"hi\\nthere\""));
    }

    #[test]
    fn unknown_fields_are_skipped() {
        // Forward-compat: future shells may add fields the current
        // build doesn't know yet. `#[serde(default)]` on the struct
        // skips them without erroring.
        let json = r#"{"command":"ls","future_field":42,"cwd":"/x"}"#;
        let payload: ShellMetadataPayload =
            serde_json::from_str(json).expect("unknown fields should be skipped");
        assert_eq!(payload.command.as_deref(), Some("ls"));
        assert_eq!(payload.cwd.as_deref(), Some("/x"));
    }
}

/// TUI-mode hint received via OSC 7777;carrot-tui-hint.
///
/// Emitted by the shell preexec hook immediately before a known TUI
/// command starts, so the terminal can activate a live-frame region on
/// the upcoming block before the first output byte arrives. Avoids the
/// "first frame stacks once before the heuristic arms" window.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct TuiHintPayload {
    /// Whether this command should be treated as a TUI redraw target.
    pub tui_mode: Option<bool>,
    /// Reserved for future use: whether the shell hints the TUI would
    /// prefer alt-screen rendering. Currently informational only.
    pub prefer_alt_screen: Option<bool>,
}
