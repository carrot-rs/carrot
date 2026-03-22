//! Claude Code implementation of the `CliAgent` trait.
//!
//! Claude Code is our first-class agent and the reference implementation
//! every future agent module (Codex/Gemini/Aider/…) mirrors.
//!
//! Two detection paths coexist:
//!
//!   * **Hook path (primary):** when the plugin is installed
//!     and emits OSC 7777 envelopes, `state_from_hook` maps each
//!     `CliAgentHookEvent` to a `CliAgentSessionState`. This is the
//!     authoritative source — `CliAgentSessionManager` always prefers
//!     it over heuristic output.
//!   * **Output path (fallback):** when the plugin is not installed,
//!     `state_from_output` watches the block's text buffer for
//!     signature phrases and spinner glyphs and guesses a state. The
//!     guesses are coarse by design — they exist only so we have
//!     *some* status UI when hooks are unavailable.
//!
//! Claude Code's transcripts live under
//! `~/.claude/projects/<encoded-cwd>/*.jsonl` where `<encoded-cwd>` is
//! the absolute CWD with every non-alphanumeric character replaced by
//! `-`. `session_transcript_path` returns the most recently modified
//! transcript in that directory, or `None` if the directory or any of
//! its entries is unreadable.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use inazuma::App;

use crate::agent::{
    BlockOutputSnapshot, CliAgent, CliAgentCapabilities, CliAgentMatch, PluginAssets, ProcessInfo,
};
use crate::detection::{binary_matches, parse_claude_cmdline};
use crate::hook_events::CliAgentHookEvent;
use crate::plugin_installer::bundled_version;
use crate::session::{CliAgentSessionState, NotificationType};

const AGENT_ID: &str = "claude_code";
const AGENT_DISPLAY_NAME: &str = "Claude Code";
const AGENT_ICON_PATH: &str = "icons/agents/claude_code.svg";
/// Claude Code brand identity — "Crail" orange circle with a white
/// sparkle. Used by the vertical-tabs rows to compose the pixel-perfect
/// reference tab icon; kept as explicit hex literals because brand
/// identity is fixed data, not a theme-tunable UI token.
const AGENT_BRAND_BG: &str = "#C15F3C";
const AGENT_BRAND_FG: &str = "#FFFFFF";

/// Binary names Claude Code can show up as. `binary_matches` in
/// `detection` lowercases and strips `.exe` on comparison, so the
/// list only needs the platform-canonical forms.
const AGENT_BINARY_NAMES: &[&str] = &["claude", "claude.exe"];

const PLUGIN_BUNDLE_DIR: &str = "claude-code-carrot";

/// Claude Code agent. Stateless — every method derives its answer from
/// the arguments, so a single registry-wide `Arc<ClaudeCodeAgent>` is
/// enough for the whole app.
pub struct ClaudeCodeAgent;

impl ClaudeCodeAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeCodeAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl CliAgent for ClaudeCodeAgent {
    fn id(&self) -> &'static str {
        AGENT_ID
    }

    fn display_name(&self) -> &'static str {
        AGENT_DISPLAY_NAME
    }

    fn binary_names(&self) -> &'static [&'static str] {
        AGENT_BINARY_NAMES
    }

    fn icon_path(&self) -> &'static str {
        AGENT_ICON_PATH
    }

    fn brand_colors(&self) -> Option<(&'static str, &'static str)> {
        Some((AGENT_BRAND_BG, AGENT_BRAND_FG))
    }

    fn capabilities(&self) -> CliAgentCapabilities {
        CliAgentCapabilities::HOOKS
            | CliAgentCapabilities::RICH_INPUT
            | CliAgentCapabilities::NOTIFICATIONS
            | CliAgentCapabilities::DIFF_REVIEW
            | CliAgentCapabilities::VOICE
            | CliAgentCapabilities::IMAGES
            | CliAgentCapabilities::TASK_PANEL
            | CliAgentCapabilities::RESUME_SESSIONS
            | CliAgentCapabilities::MCP_TOOLS
            | CliAgentCapabilities::WORKTREES
            | CliAgentCapabilities::SUBAGENTS
            | CliAgentCapabilities::ELICITATION
            | CliAgentCapabilities::PERMISSION_MODES
            | CliAgentCapabilities::CONTEXT_WINDOW
            | CliAgentCapabilities::RULES
    }

    fn classify(&self, process: &ProcessInfo, _cx: &App) -> Option<CliAgentMatch> {
        if !binary_matches(process, AGENT_BINARY_NAMES) {
            return None;
        }
        Some(parse_claude_cmdline(process))
    }

    fn state_from_hook(&self, event: &CliAgentHookEvent) -> Option<CliAgentSessionState> {
        match event {
            CliAgentHookEvent::SessionStart { .. } => Some(CliAgentSessionState::Starting),
            CliAgentHookEvent::UserPromptSubmit { .. } => Some(CliAgentSessionState::Working {
                since: Instant::now(),
            }),
            CliAgentHookEvent::PreToolUse { .. } => Some(CliAgentSessionState::Working {
                since: Instant::now(),
            }),
            CliAgentHookEvent::PostToolUse { .. } => {
                // Tool finished but assistant may keep thinking — stay
                // in Working so the status dot does not flicker. Stop
                // is the explicit transition to Idle.
                Some(CliAgentSessionState::Working {
                    since: Instant::now(),
                })
            }
            CliAgentHookEvent::PermissionRequest { .. } => {
                Some(CliAgentSessionState::WaitingForInput {
                    notification_type: NotificationType::PermissionPrompt,
                })
            }
            CliAgentHookEvent::Notification {
                notification_type, ..
            } => Some(CliAgentSessionState::WaitingForInput {
                notification_type: *notification_type,
            }),
            CliAgentHookEvent::Elicitation { .. } => Some(CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::Elicitation,
            }),
            CliAgentHookEvent::PreCompact { .. } => Some(CliAgentSessionState::Compacting),
            CliAgentHookEvent::PostCompact { .. } => Some(CliAgentSessionState::Working {
                since: Instant::now(),
            }),
            CliAgentHookEvent::Stop { .. } => Some(CliAgentSessionState::Idle),
            CliAgentHookEvent::SessionEnd { exit_code, .. } => {
                Some(CliAgentSessionState::Completed {
                    exit_code: *exit_code,
                })
            }
            // Events that carry metadata but do not move the state
            // machine. Task/FileChanged/CwdChanged/InstructionsLoaded
            // are surfaced by the session manager separately.
            CliAgentHookEvent::TaskCreated { .. }
            | CliAgentHookEvent::TaskCompleted { .. }
            | CliAgentHookEvent::FileChanged { .. }
            | CliAgentHookEvent::CwdChanged { .. }
            | CliAgentHookEvent::InstructionsLoaded { .. }
            | CliAgentHookEvent::SubagentStart { .. }
            | CliAgentHookEvent::SubagentStop { .. }
            | CliAgentHookEvent::WorktreeCreate { .. }
            | CliAgentHookEvent::WorktreeRemove { .. }
            | CliAgentHookEvent::ElicitationResult { .. } => None,
        }
    }

    fn state_from_output(&self, snapshot: &BlockOutputSnapshot) -> Option<CliAgentSessionState> {
        let text = &snapshot.text;
        if text.is_empty() {
            return None;
        }

        // Waiting-for-input patterns win over working patterns when
        // both are present — Claude Code typically prints a working
        // indicator right above its prompt, so the prompt is the
        // more recent signal.
        if looks_like_waiting_for_input(text) {
            return Some(CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::PermissionPrompt,
            });
        }

        if looks_like_working(text) {
            return Some(CliAgentSessionState::Working {
                since: Instant::now(),
            });
        }

        None
    }

    fn plugin_assets(&self) -> Option<PluginAssets> {
        Some(PluginAssets {
            bundle_dir: PLUGIN_BUNDLE_DIR,
            version: bundled_version(),
        })
    }

    fn session_transcript_path(&self, cwd: &Path) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        let projects = home.join(".claude").join("projects").join(encode_cwd(cwd));
        newest_jsonl(&projects)
    }
}

/// Encode an absolute CWD the same way Claude Code does when it picks
/// a transcript directory: every character that is not ASCII alnum
/// becomes `-`. This is a lossy hash by design — Claude Code made the
/// same choice so transcript-dir lookup stays trivial on every
/// platform.
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Pick the most recently modified `*.jsonl` inside `dir`. Returns
/// `None` if `dir` is unreadable or contains no matching file — the
/// caller treats that as "no transcript yet".
fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        match &best {
            Some((_, current)) if modified <= *current => {}
            _ => best = Some((path, modified)),
        }
    }
    best.map(|(path, _)| path)
}

/// Heuristic signatures Claude Code prints while it is thinking or
/// running a tool. Any match → Working. Kept as a free fn so the
/// `state_from_output` body reads top-to-bottom.
fn looks_like_working(text: &str) -> bool {
    // Thinking-line verbs cycle pseudo-randomly between sessions.
    // Every known one is listed here; they are short enough that a
    // linear scan is cheaper than a regex.
    const WORKING_MARKERS: &[&str] = &[
        "✻ Baked for ",
        "✻ Brewed for ",
        "✻ Cooking for ",
        "✻ Thinking",
        "✻ Pondering",
        "✻ Composing",
        "✻ Churning",
        "✻ Crafting",
        "✻ Musing",
        "* Thinking",
        "Thinking...",
    ];
    for marker in WORKING_MARKERS {
        if text.contains(marker) {
            return true;
        }
    }
    // Braille spinner glyphs — Claude Code animates between these while
    // it is running a tool.
    const SPINNER: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
    for ch in SPINNER.chars() {
        if text.contains(ch) {
            return true;
        }
    }
    false
}

/// Heuristic signatures Claude Code prints when it is waiting on the
/// user — permission prompts, yes/no menus, tool-run confirmations.
fn looks_like_waiting_for_input(text: &str) -> bool {
    const WAITING_MARKERS: &[&str] = &[
        "Do you want to proceed?",
        "Do you want to continue?",
        "❯ 1. Yes",
        "❯ 2. No",
        "❯ 1) Yes",
        "❯ 2) No",
        "Select an option",
    ];
    WAITING_MARKERS.iter().any(|m| text.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_events::{CliAgentHookEvent, TaskStatus};
    use crate::session::{PermissionMode, SessionSource};

    #[test]
    fn identity_fields_are_stable() {
        let agent = ClaudeCodeAgent::new();
        assert_eq!(agent.id(), "claude_code");
        assert_eq!(agent.display_name(), "Claude Code");
        assert_eq!(agent.icon_path(), "icons/agents/claude_code.svg");
        assert_eq!(agent.binary_names(), &["claude", "claude.exe"]);
    }

    #[test]
    fn capabilities_cover_all_documented_flags() {
        let caps = ClaudeCodeAgent::new().capabilities();
        assert!(caps.contains(CliAgentCapabilities::HOOKS));
        assert!(caps.contains(CliAgentCapabilities::RICH_INPUT));
        assert!(caps.contains(CliAgentCapabilities::NOTIFICATIONS));
        assert!(caps.contains(CliAgentCapabilities::DIFF_REVIEW));
        assert!(caps.contains(CliAgentCapabilities::VOICE));
        assert!(caps.contains(CliAgentCapabilities::IMAGES));
        assert!(caps.contains(CliAgentCapabilities::TASK_PANEL));
        assert!(caps.contains(CliAgentCapabilities::RESUME_SESSIONS));
        assert!(caps.contains(CliAgentCapabilities::MCP_TOOLS));
        assert!(caps.contains(CliAgentCapabilities::WORKTREES));
        assert!(caps.contains(CliAgentCapabilities::SUBAGENTS));
        assert!(caps.contains(CliAgentCapabilities::ELICITATION));
        assert!(caps.contains(CliAgentCapabilities::PERMISSION_MODES));
        assert!(caps.contains(CliAgentCapabilities::CONTEXT_WINDOW));
        assert!(caps.contains(CliAgentCapabilities::RULES));
    }

    // Note on classify(): the trait signature requires `&App`, which
    // would force every classify test through `TestAppContext`.
    // ClaudeCodeAgent::classify delegates to `binary_matches` (detection
    // tests cover match/reject + windows .exe stripping) and
    // `parse_claude_cmdline` (detection covers every flag combo and
    // edge case). The integration point is a two-line function; the
    // value of repeating those assertions here under `TestAppContext`
    // is low. Hook- and output-state tests below provide the
    // Claude-specific coverage that cannot live in detection.

    #[test]
    fn state_from_hook_session_start() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::SessionStart {
            session_id: "s1".into(),
            transcript_path: PathBuf::from("/t.jsonl"),
            cwd: PathBuf::from("/repo"),
            source: SessionSource::Startup,
            model: "x".into(),
            permission_mode: PermissionMode::Default,
            agent_id: "claude_code".into(),
            plugin_version: "1.0.0".into(),
        };
        assert_eq!(
            agent.state_from_hook(&event),
            Some(CliAgentSessionState::Starting)
        );
    }

    #[test]
    fn state_from_hook_user_prompt_submit_goes_working() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::UserPromptSubmit {
            session_id: "s1".into(),
            prompt: "hello".into(),
        };
        let state = agent.state_from_hook(&event).unwrap();
        assert!(matches!(state, CliAgentSessionState::Working { .. }));
    }

    #[test]
    fn state_from_hook_stop_goes_idle() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::Stop {
            session_id: "s1".into(),
            last_assistant_message: String::new(),
        };
        assert_eq!(
            agent.state_from_hook(&event),
            Some(CliAgentSessionState::Idle)
        );
    }

    #[test]
    fn state_from_hook_permission_request_goes_waiting_with_permission_type() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::PermissionRequest {
            session_id: "s1".into(),
            tool_name: "Bash".into(),
            tool_input: serde_json::Value::Null,
            permission_suggestions: vec![],
        };
        let state = agent.state_from_hook(&event).unwrap();
        assert!(matches!(
            state,
            CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::PermissionPrompt,
            }
        ));
    }

    #[test]
    fn state_from_hook_notification_preserves_notification_type() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::Notification {
            session_id: "s1".into(),
            title: String::new(),
            message: String::new(),
            notification_type: NotificationType::IdlePrompt,
        };
        assert_eq!(
            agent.state_from_hook(&event),
            Some(CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::IdlePrompt,
            })
        );
    }

    #[test]
    fn state_from_hook_elicitation_goes_waiting() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::Elicitation {
            session_id: "s1".into(),
            mcp_server: "x".into(),
            schema: serde_json::Value::Null,
        };
        let state = agent.state_from_hook(&event).unwrap();
        assert!(matches!(
            state,
            CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::Elicitation,
            }
        ));
    }

    #[test]
    fn state_from_hook_pre_compact_goes_compacting() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::PreCompact {
            session_id: "s1".into(),
            tokens_used: 0,
            tokens_max: 0,
        };
        assert_eq!(
            agent.state_from_hook(&event),
            Some(CliAgentSessionState::Compacting)
        );
    }

    #[test]
    fn state_from_hook_session_end_preserves_exit_code() {
        let agent = ClaudeCodeAgent::new();
        let event = CliAgentHookEvent::SessionEnd {
            session_id: "s1".into(),
            exit_code: Some(42),
        };
        assert_eq!(
            agent.state_from_hook(&event),
            Some(CliAgentSessionState::Completed {
                exit_code: Some(42),
            })
        );
    }

    #[test]
    fn state_from_hook_task_events_do_not_change_state() {
        let agent = ClaudeCodeAgent::new();
        let created = CliAgentHookEvent::TaskCreated {
            session_id: "s1".into(),
            task_id: "t1".into(),
            content: "x".into(),
            status: TaskStatus::Pending,
        };
        let completed = CliAgentHookEvent::TaskCompleted {
            session_id: "s1".into(),
            task_id: "t1".into(),
        };
        assert!(agent.state_from_hook(&created).is_none());
        assert!(agent.state_from_hook(&completed).is_none());
    }

    #[test]
    fn state_from_output_working_on_thinking_marker() {
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: "✻ Baked for 5s".into(),
        };
        assert!(matches!(
            agent.state_from_output(&snapshot),
            Some(CliAgentSessionState::Working { .. })
        ));
    }

    #[test]
    fn state_from_output_working_on_spinner_glyph() {
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: "running tool ⠋".into(),
        };
        assert!(matches!(
            agent.state_from_output(&snapshot),
            Some(CliAgentSessionState::Working { .. })
        ));
    }

    #[test]
    fn state_from_output_waiting_on_proceed_prompt() {
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: "Do you want to proceed?\n❯ 1. Yes\n  2. No".into(),
        };
        let state = agent.state_from_output(&snapshot).unwrap();
        assert!(matches!(
            state,
            CliAgentSessionState::WaitingForInput { .. }
        ));
    }

    #[test]
    fn state_from_output_waiting_beats_working() {
        // If both signals are present (thinking log above a prompt),
        // the prompt wins because it is the more recent signal.
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: "✻ Baked for 5s\n\nDo you want to proceed?".into(),
        };
        let state = agent.state_from_output(&snapshot).unwrap();
        assert!(matches!(
            state,
            CliAgentSessionState::WaitingForInput { .. }
        ));
    }

    #[test]
    fn state_from_output_none_for_plain_text() {
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: "hello world".into(),
        };
        assert!(agent.state_from_output(&snapshot).is_none());
    }

    #[test]
    fn state_from_output_none_for_empty_text() {
        let agent = ClaudeCodeAgent::new();
        let snapshot = BlockOutputSnapshot {
            text: String::new(),
        };
        assert!(agent.state_from_output(&snapshot).is_none());
    }

    #[test]
    fn encode_cwd_strips_path_separators() {
        let s = encode_cwd(Path::new("/Users/nyxb/Projects/carrot"));
        assert!(!s.contains('/'));
        assert!(s.starts_with("-Users-nyxb-Projects-carrot") || s.contains("nyxb"));
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn encode_cwd_normalises_dots() {
        let s = encode_cwd(Path::new("/tmp/my.project"));
        assert!(!s.contains('.'));
    }

    #[test]
    fn newest_jsonl_picks_latest_by_mtime() {
        // Integration-ish: write two JSONL files with different
        // mtimes and assert the newer one is returned.
        let tmp = std::env::temp_dir().join(format!(
            "carrot-cli-agents-claude-newest-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let old = tmp.join("a.jsonl");
        let new = tmp.join("b.jsonl");
        fs::write(&old, b"{}").unwrap();
        // Touch `new` after `old` so mtime is strictly later. We use a
        // short sleep because fs metadata resolution is fs-dependent.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&new, b"{}").unwrap();

        let picked = super::newest_jsonl(&tmp).expect("one of the two");
        assert_eq!(picked, new);

        // Non-JSONL files are ignored.
        let junk = tmp.join("c.txt");
        fs::write(&junk, b"{}").unwrap();
        let picked = super::newest_jsonl(&tmp).expect("still one of the jsonl");
        assert!(picked.ends_with("a.jsonl") || picked.ends_with("b.jsonl"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn newest_jsonl_returns_none_on_missing_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "carrot-cli-agents-claude-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        assert!(super::newest_jsonl(&tmp).is_none());
    }

    #[test]
    fn plugin_assets_carry_bundle_dir_and_current_version() {
        let agent = ClaudeCodeAgent::new();
        let assets = agent.plugin_assets().unwrap();
        assert_eq!(assets.bundle_dir, "claude-code-carrot");
        assert_eq!(assets.version, bundled_version());
    }
}
