//! OSC-7777-transported agent hook events.
//!
//! The agent's plugin hex-encodes a JSON envelope and emits it as an
//! OSC 7777 sequence. The `carrot-terminal` OSC parser decodes the hex
//! and hands us the raw envelope string; `parse_envelope` here turns it
//! into a typed `CliAgentHookEvent`.
//!
//! Envelope shape (from the plugin's `emit-event.sh`):
//! ```json
//! {
//!   "type": "cli_agent_event",
//!   "agent": "claude_code",
//!   "protocol_version": 1,
//!   "event": "Stop",
//!   "payload": { ...event-specific fields... }
//! }
//! ```
//!
//! We version-negotiate by taking `min(plugin_version, carrot_version)`:
//! a newer plugin talking to an older Carrot downgrades cleanly; an
//! older plugin talking to a newer Carrot still works as long as the
//! event schemas for its version remain supported (they are, until the
//! protocol explicitly deprecates a variant).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::session::{NotificationType, PermissionMode, SessionSource};

/// Current protocol version this build of Carrot speaks. Plugins emit
/// their own `protocol_version` in the envelope; we negotiate via
/// `min(plugin, self)` and parse against the resulting version.
///
/// Increment this number only when a breaking envelope or payload
/// schema change lands. Additive fields do not require a bump.
pub const CARROT_PROTOCOL_VERSION: u32 = 1;

/// Literal value the envelope `type` field must carry. Other future
/// OSC-7777 sub-schemas (e.g. a future structured shell-context v2)
/// would pick different `type` strings, letting the router discriminate.
pub const ENVELOPE_TYPE_CLI_AGENT_EVENT: &str = "cli_agent_event";

/// Outer envelope carried in the OSC 7777 payload. Always emitted by
/// the Carrot plugin; `event` + `payload` are the discriminating
/// fields for the inner `CliAgentHookEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookEventEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    pub agent: String,
    pub protocol_version: u32,
    pub event: String,
    #[serde(default)]
    pub payload: JsonValue,
}

/// Task-list item status as emitted by `TaskCreated` hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// File-change kind emitted by `FileChanged` hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// One permission-suggestion option as presented by Claude Code next to
/// a permission prompt (e.g. `Allow once`, `Allow always`, `Deny`). The
/// shape mirrors what the hook emits; we store it as-is since the UI
/// layer only ever displays it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionSuggestion {
    pub label: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// All hook events Carrot understands — one variant per event name
/// emitted by the bundled plugin. Tag + content keys mirror the
/// envelope wire format (`event` discriminates, `payload` carries
/// the variant data).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", content = "payload")]
pub enum CliAgentHookEvent {
    SessionStart {
        session_id: String,
        transcript_path: PathBuf,
        cwd: PathBuf,
        source: SessionSource,
        model: String,
        permission_mode: PermissionMode,
        agent_id: String,
        plugin_version: String,
    },
    SessionEnd {
        session_id: String,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    UserPromptSubmit {
        session_id: String,
        prompt: String,
    },
    Stop {
        session_id: String,
        #[serde(default)]
        last_assistant_message: String,
    },
    Notification {
        session_id: String,
        title: String,
        message: String,
        notification_type: NotificationType,
    },
    PermissionRequest {
        session_id: String,
        tool_name: String,
        tool_input: JsonValue,
        #[serde(default)]
        permission_suggestions: Vec<PermissionSuggestion>,
    },
    PreToolUse {
        session_id: String,
        tool_name: String,
        tool_input: JsonValue,
        tool_use_id: String,
    },
    PostToolUse {
        session_id: String,
        tool_name: String,
        tool_input: JsonValue,
        tool_response: JsonValue,
        tool_use_id: String,
    },
    TaskCreated {
        session_id: String,
        task_id: String,
        content: String,
        status: TaskStatus,
    },
    TaskCompleted {
        session_id: String,
        task_id: String,
    },
    FileChanged {
        session_id: String,
        path: PathBuf,
        change_type: ChangeType,
    },
    CwdChanged {
        session_id: String,
        cwd: PathBuf,
    },
    PreCompact {
        session_id: String,
        tokens_used: u64,
        tokens_max: u64,
    },
    PostCompact {
        session_id: String,
        tokens_used: u64,
        tokens_max: u64,
    },
    InstructionsLoaded {
        session_id: String,
        paths: Vec<PathBuf>,
    },
    SubagentStart {
        session_id: String,
        agent_id: String,
        agent_type: String,
        #[serde(default)]
        prompt: String,
    },
    SubagentStop {
        session_id: String,
        agent_id: String,
    },
    WorktreeCreate {
        session_id: String,
        path: PathBuf,
        branch: String,
    },
    WorktreeRemove {
        session_id: String,
        path: PathBuf,
    },
    Elicitation {
        session_id: String,
        mcp_server: String,
        schema: JsonValue,
    },
    ElicitationResult {
        session_id: String,
        response: JsonValue,
    },
}

impl CliAgentHookEvent {
    /// Session id is shared by every variant — expose a uniform
    /// accessor so the session manager can route without matching
    /// every time.
    pub fn session_id(&self) -> &str {
        match self {
            Self::SessionStart { session_id, .. }
            | Self::SessionEnd { session_id, .. }
            | Self::UserPromptSubmit { session_id, .. }
            | Self::Stop { session_id, .. }
            | Self::Notification { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::PreToolUse { session_id, .. }
            | Self::PostToolUse { session_id, .. }
            | Self::TaskCreated { session_id, .. }
            | Self::TaskCompleted { session_id, .. }
            | Self::FileChanged { session_id, .. }
            | Self::CwdChanged { session_id, .. }
            | Self::PreCompact { session_id, .. }
            | Self::PostCompact { session_id, .. }
            | Self::InstructionsLoaded { session_id, .. }
            | Self::SubagentStart { session_id, .. }
            | Self::SubagentStop { session_id, .. }
            | Self::WorktreeCreate { session_id, .. }
            | Self::WorktreeRemove { session_id, .. }
            | Self::Elicitation { session_id, .. }
            | Self::ElicitationResult { session_id, .. } => session_id,
        }
    }
}

/// Parse the raw envelope JSON emitted by a plugin. Returns the typed
/// event alongside the agent-id and negotiated protocol version.
///
/// Errors from this function are terminal — malformed envelopes are
/// dropped, since the PTY stream may also carry unrelated OSC 7777
/// payloads (legacy shell metadata or future sub-schemas).
pub fn parse_envelope(json: &str) -> Result<ParsedHookEvent, EnvelopeError> {
    let envelope: HookEventEnvelope = serde_json::from_str(json).map_err(EnvelopeError::Json)?;

    if envelope.event_type != ENVELOPE_TYPE_CLI_AGENT_EVENT {
        return Err(EnvelopeError::WrongType(envelope.event_type));
    }

    let negotiated_version = envelope.protocol_version.min(CARROT_PROTOCOL_VERSION);

    // Re-assemble the inner event from the tagged shape. We cannot
    // deserialise HookEventEnvelope directly into CliAgentHookEvent
    // because the envelope carries extra outer fields; instead we
    // construct a minimal tagged-shape JSON and let serde route it.
    let tagged = serde_json::json!({
        "event": envelope.event,
        "payload": envelope.payload,
    });
    let event: CliAgentHookEvent = serde_json::from_value(tagged).map_err(EnvelopeError::Json)?;

    Ok(ParsedHookEvent {
        agent: envelope.agent,
        protocol_version: negotiated_version,
        event,
    })
}

/// Successful decode of an envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedHookEvent {
    pub agent: String,
    pub protocol_version: u32,
    pub event: CliAgentHookEvent,
}

/// Reasons an envelope decode can fail. `WrongType` is not an error in
/// the strict sense — it just means the OSC 7777 sequence belongs to a
/// different sub-schema (e.g. legacy shell metadata). Callers treat it
/// as "not for us, ignore".
#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("envelope JSON could not be parsed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("envelope type is not a cli_agent_event (was {0:?})")]
    WrongType(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(event: &str, payload: JsonValue) -> String {
        serde_json::json!({
            "type": "cli_agent_event",
            "agent": "claude_code",
            "protocol_version": 1,
            "event": event,
            "payload": payload,
        })
        .to_string()
    }

    fn roundtrip(event: CliAgentHookEvent, expected_name: &str, expected_session: &str) {
        // Derive the `event` string from the tagged enum shape.
        let tagged = serde_json::to_value(&event).unwrap();
        let payload = tagged.get("payload").cloned().unwrap_or(JsonValue::Null);
        let envelope = wrap(expected_name, payload);
        let parsed = parse_envelope(&envelope).expect("envelope must parse");
        assert_eq!(parsed.agent, "claude_code");
        assert_eq!(parsed.protocol_version, 1);
        assert_eq!(parsed.event, event);
        assert_eq!(parsed.event.session_id(), expected_session);
    }

    #[test]
    fn session_start_roundtrips() {
        roundtrip(
            CliAgentHookEvent::SessionStart {
                session_id: "s1".into(),
                transcript_path: PathBuf::from("/tmp/t.jsonl"),
                cwd: PathBuf::from("/repo"),
                source: SessionSource::Startup,
                model: "claude-opus-4-7".into(),
                permission_mode: PermissionMode::Default,
                agent_id: "claude_code".into(),
                plugin_version: "1.0.0".into(),
            },
            "SessionStart",
            "s1",
        );
    }

    #[test]
    fn session_end_roundtrips_with_exit_code() {
        roundtrip(
            CliAgentHookEvent::SessionEnd {
                session_id: "s1".into(),
                exit_code: Some(0),
            },
            "SessionEnd",
            "s1",
        );
    }

    #[test]
    fn session_end_accepts_missing_exit_code() {
        let env = wrap("SessionEnd", serde_json::json!({ "session_id": "s1" }));
        let parsed = parse_envelope(&env).unwrap();
        match parsed.event {
            CliAgentHookEvent::SessionEnd { exit_code, .. } => assert_eq!(exit_code, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn user_prompt_submit_roundtrips() {
        roundtrip(
            CliAgentHookEvent::UserPromptSubmit {
                session_id: "s1".into(),
                prompt: "hello".into(),
            },
            "UserPromptSubmit",
            "s1",
        );
    }

    #[test]
    fn stop_roundtrips() {
        roundtrip(
            CliAgentHookEvent::Stop {
                session_id: "s1".into(),
                last_assistant_message: "done".into(),
            },
            "Stop",
            "s1",
        );
    }

    #[test]
    fn notification_roundtrips() {
        roundtrip(
            CliAgentHookEvent::Notification {
                session_id: "s1".into(),
                title: "Waiting".into(),
                message: "Need input".into(),
                notification_type: NotificationType::IdlePrompt,
            },
            "Notification",
            "s1",
        );
    }

    #[test]
    fn permission_request_roundtrips() {
        roundtrip(
            CliAgentHookEvent::PermissionRequest {
                session_id: "s1".into(),
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({"cmd": "ls"}),
                permission_suggestions: vec![PermissionSuggestion {
                    label: "Allow once".into(),
                    value: Some("allow_once".into()),
                    description: None,
                }],
            },
            "PermissionRequest",
            "s1",
        );
    }

    #[test]
    fn pre_tool_use_roundtrips() {
        roundtrip(
            CliAgentHookEvent::PreToolUse {
                session_id: "s1".into(),
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({"file_path": "/a"}),
                tool_use_id: "tu1".into(),
            },
            "PreToolUse",
            "s1",
        );
    }

    #[test]
    fn post_tool_use_roundtrips() {
        roundtrip(
            CliAgentHookEvent::PostToolUse {
                session_id: "s1".into(),
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({"file_path": "/a"}),
                tool_response: serde_json::json!({"ok": true}),
                tool_use_id: "tu1".into(),
            },
            "PostToolUse",
            "s1",
        );
    }

    #[test]
    fn task_created_roundtrips() {
        roundtrip(
            CliAgentHookEvent::TaskCreated {
                session_id: "s1".into(),
                task_id: "t1".into(),
                content: "Refactor".into(),
                status: TaskStatus::Pending,
            },
            "TaskCreated",
            "s1",
        );
    }

    #[test]
    fn task_completed_roundtrips() {
        roundtrip(
            CliAgentHookEvent::TaskCompleted {
                session_id: "s1".into(),
                task_id: "t1".into(),
            },
            "TaskCompleted",
            "s1",
        );
    }

    #[test]
    fn file_changed_roundtrips() {
        roundtrip(
            CliAgentHookEvent::FileChanged {
                session_id: "s1".into(),
                path: PathBuf::from("/a.rs"),
                change_type: ChangeType::Modified,
            },
            "FileChanged",
            "s1",
        );
    }

    #[test]
    fn cwd_changed_roundtrips() {
        roundtrip(
            CliAgentHookEvent::CwdChanged {
                session_id: "s1".into(),
                cwd: PathBuf::from("/other"),
            },
            "CwdChanged",
            "s1",
        );
    }

    #[test]
    fn pre_compact_roundtrips() {
        roundtrip(
            CliAgentHookEvent::PreCompact {
                session_id: "s1".into(),
                tokens_used: 190_000,
                tokens_max: 200_000,
            },
            "PreCompact",
            "s1",
        );
    }

    #[test]
    fn post_compact_roundtrips() {
        roundtrip(
            CliAgentHookEvent::PostCompact {
                session_id: "s1".into(),
                tokens_used: 20_000,
                tokens_max: 200_000,
            },
            "PostCompact",
            "s1",
        );
    }

    #[test]
    fn instructions_loaded_roundtrips() {
        roundtrip(
            CliAgentHookEvent::InstructionsLoaded {
                session_id: "s1".into(),
                paths: vec![
                    PathBuf::from("CLAUDE.md"),
                    PathBuf::from(".claude/rules/a.md"),
                ],
            },
            "InstructionsLoaded",
            "s1",
        );
    }

    #[test]
    fn subagent_start_roundtrips() {
        roundtrip(
            CliAgentHookEvent::SubagentStart {
                session_id: "s1".into(),
                agent_id: "sub1".into(),
                agent_type: "Explore".into(),
                prompt: "find the bug".into(),
            },
            "SubagentStart",
            "s1",
        );
    }

    #[test]
    fn subagent_stop_roundtrips() {
        roundtrip(
            CliAgentHookEvent::SubagentStop {
                session_id: "s1".into(),
                agent_id: "sub1".into(),
            },
            "SubagentStop",
            "s1",
        );
    }

    #[test]
    fn worktree_create_roundtrips() {
        roundtrip(
            CliAgentHookEvent::WorktreeCreate {
                session_id: "s1".into(),
                path: PathBuf::from("/tmp/wt"),
                branch: "feature-x".into(),
            },
            "WorktreeCreate",
            "s1",
        );
    }

    #[test]
    fn worktree_remove_roundtrips() {
        roundtrip(
            CliAgentHookEvent::WorktreeRemove {
                session_id: "s1".into(),
                path: PathBuf::from("/tmp/wt"),
            },
            "WorktreeRemove",
            "s1",
        );
    }

    #[test]
    fn elicitation_roundtrips() {
        roundtrip(
            CliAgentHookEvent::Elicitation {
                session_id: "s1".into(),
                mcp_server: "my-mcp".into(),
                schema: serde_json::json!({"type": "string"}),
            },
            "Elicitation",
            "s1",
        );
    }

    #[test]
    fn elicitation_result_roundtrips() {
        roundtrip(
            CliAgentHookEvent::ElicitationResult {
                session_id: "s1".into(),
                response: serde_json::json!({"answer": "yes"}),
            },
            "ElicitationResult",
            "s1",
        );
    }

    #[test]
    fn rejects_non_cli_agent_envelope() {
        let env = serde_json::json!({
            "type": "something_else",
            "agent": "x",
            "protocol_version": 1,
            "event": "Stop",
            "payload": { "session_id": "s" }
        })
        .to_string();
        match parse_envelope(&env) {
            Err(EnvelopeError::WrongType(t)) => assert_eq!(t, "something_else"),
            other => panic!("expected WrongType, got {:?}", other),
        }
    }

    #[test]
    fn rejects_unknown_event_name() {
        let env = wrap("FutureEventName", serde_json::json!({ "session_id": "s" }));
        assert!(matches!(parse_envelope(&env), Err(EnvelopeError::Json(_))));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_envelope("not json"),
            Err(EnvelopeError::Json(_))
        ));
    }

    #[test]
    fn protocol_version_negotiation_clamps_to_carrot_max() {
        // Plugin advertises a newer protocol — Carrot negotiates down.
        let env = serde_json::json!({
            "type": "cli_agent_event",
            "agent": "claude_code",
            "protocol_version": 9_999,
            "event": "Stop",
            "payload": { "session_id": "s1", "last_assistant_message": "" }
        })
        .to_string();
        let parsed = parse_envelope(&env).unwrap();
        assert_eq!(parsed.protocol_version, CARROT_PROTOCOL_VERSION);
    }

    /// Mirrors the hex encoder the OSC parser uses; kept inline so
    /// this end-to-end test does not pull in carrot-terminal as a
    /// dependency (which would break layer isolation).
    fn hex_encode(input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() * 2);
        for &b in input {
            out.push(b"0123456789abcdef"[(b >> 4) as usize]);
            out.push(b"0123456789abcdef"[(b & 0x0f) as usize]);
        }
        out
    }

    fn hex_decode(input: &[u8]) -> Option<Vec<u8>> {
        if !input.len().is_multiple_of(2) {
            return None;
        }
        let mut out = Vec::with_capacity(input.len() / 2);
        for chunk in input.chunks_exact(2) {
            let hi = match chunk[0] {
                b'0'..=b'9' => chunk[0] - b'0',
                b'a'..=b'f' => chunk[0] - b'a' + 10,
                b'A'..=b'F' => chunk[0] - b'A' + 10,
                _ => return None,
            };
            let lo = match chunk[1] {
                b'0'..=b'9' => chunk[1] - b'0',
                b'a'..=b'f' => chunk[1] - b'a' + 10,
                b'A'..=b'F' => chunk[1] - b'A' + 10,
                _ => return None,
            };
            out.push((hi << 4) | lo);
        }
        Some(out)
    }

    #[test]
    fn wire_roundtrip_session_start() {
        // End-to-end: build the envelope the same way `emit-event.sh`
        // will, hex-encode it, decode, and parse. This is the shape the
        // terminal's OSC scanner will hand us.
        let original = CliAgentHookEvent::SessionStart {
            session_id: "abc".into(),
            transcript_path: PathBuf::from("/tmp/a.jsonl"),
            cwd: PathBuf::from("/repo"),
            source: SessionSource::Startup,
            model: "claude-opus-4-7".into(),
            permission_mode: PermissionMode::Default,
            agent_id: "claude_code".into(),
            plugin_version: "1.0.0".into(),
        };
        let tagged = serde_json::to_value(&original).unwrap();
        let payload = tagged.get("payload").cloned().unwrap();
        let wrapper = serde_json::json!({
            "type": "cli_agent_event",
            "agent": "claude_code",
            "protocol_version": 1,
            "event": "SessionStart",
            "payload": payload,
        })
        .to_string();

        let hex = hex_encode(wrapper.as_bytes());
        let decoded = hex_decode(&hex).unwrap();
        let json = std::str::from_utf8(&decoded).unwrap();
        let parsed = parse_envelope(json).unwrap();
        assert_eq!(parsed.event, original);
    }

    /// Run the bundled `on-session-start.sh` forwarder against a
    /// realistic SessionStart payload, then parse the emitted OSC
    /// 7777 sequence back to a typed event. This is our end-to-end
    /// check that the plugin scripts and the envelope parser agree
    /// on the wire format.
    #[cfg(unix)]
    #[test]
    fn plugin_script_emits_parseable_envelope() {
        use std::process::{Command, Stdio};

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let script = std::path::Path::new(manifest_dir)
            .join("../../assets/plugins/claude-code-carrot/scripts/on-session-start.sh");
        assert!(
            script.exists(),
            "plugin forwarder missing at {}",
            script.display()
        );

        let payload = r#"{"session_id":"rust-e2e","transcript_path":"/tmp/t.jsonl","cwd":"/repo","source":"startup","model":"claude-opus-4-7","permission_mode":"default","agent_id":"claude_code","plugin_version":"1.0.0"}"#;

        let mut child = Command::new(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn forwarder");

        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin handle")
            .write_all(payload.as_bytes())
            .expect("write payload");

        let output = child.wait_with_output().expect("forwarder output");
        assert!(
            output.status.success(),
            "forwarder exited with {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        // Extract hex from OSC sequence: ESC ] 7777 ; <hex> BEL
        let osc = output.stdout;
        assert_eq!(osc.first().copied(), Some(0x1b), "must start with ESC");
        assert_eq!(osc.get(1).copied(), Some(b']'), "must be OSC prefix");
        assert_eq!(osc.last().copied(), Some(0x07), "must end with BEL");
        let body = &osc[2..osc.len() - 1];
        let prefix = b"7777;";
        assert!(
            body.starts_with(prefix),
            "expected 7777; prefix, got {:?}",
            std::str::from_utf8(&body[..prefix.len().min(body.len())])
        );
        let hex_bytes = &body[prefix.len()..];
        let decoded = super::tests::hex_decode(hex_bytes).expect("valid hex");
        let json = std::str::from_utf8(&decoded).expect("valid utf-8");

        let parsed = parse_envelope(json).expect("envelope parses");
        assert_eq!(parsed.agent, "claude_code");
        assert_eq!(parsed.protocol_version, 1);
        match parsed.event {
            CliAgentHookEvent::SessionStart {
                session_id,
                model,
                agent_id,
                ..
            } => {
                assert_eq!(session_id, "rust-e2e");
                assert_eq!(model, "claude-opus-4-7");
                assert_eq!(agent_id, "claude_code");
            }
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn protocol_version_negotiation_keeps_older_plugin_version() {
        let env = serde_json::json!({
            "type": "cli_agent_event",
            "agent": "claude_code",
            "protocol_version": 0,
            "event": "Stop",
            "payload": { "session_id": "s1", "last_assistant_message": "" }
        })
        .to_string();
        let parsed = parse_envelope(&env).unwrap();
        assert_eq!(parsed.protocol_version, 0);
    }
}
