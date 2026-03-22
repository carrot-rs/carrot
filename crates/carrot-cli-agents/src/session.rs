use std::path::PathBuf;
use std::time::Instant;

use inazuma::EventEmitter;
use serde::{Deserialize, Serialize};

use crate::agent::{PermissionModeFlag, SharedCliAgent};

/// Logical state of a single CLI agent session. The state machine is
/// advance-only per lifecycle: once a session hits `Completed` or
/// `Errored`, it stays there until it is dropped.
///
/// Events update state via one of two paths:
///   * `CliAgent::state_from_hook` — primary, plugin-driven.
///   * `CliAgent::state_from_output` — heuristic fallback when no plugin
///     is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAgentSessionState {
    Starting,
    Idle,
    Working { since: Instant },
    WaitingForInput { notification_type: NotificationType },
    Compacting,
    Completed { exit_code: Option<i32> },
    Errored { reason: String },
}

impl CliAgentSessionState {
    /// Returns true if this state is terminal — no further transitions
    /// will happen and the session can be dropped after its TTL.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Errored { .. })
    }
}

/// What kind of attention the agent needs when it transitions to
/// `WaitingForInput`. Drives notification sound choice and, later,
/// UI badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    /// Agent paused waiting for the next user prompt — ambient signal.
    IdlePrompt,
    /// Agent is blocking on a tool-permission decision — louder signal,
    /// since the run cannot make progress.
    PermissionPrompt,
    /// MCP server requested elicitation input.
    Elicitation,
    /// Generic custom notification from the agent plugin.
    Custom,
}

/// Source of a `SessionStart` event — mirrors Claude Code's own enum so
/// the plugin can forward it unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Startup,
    Resume,
    Clear,
    Compact,
}

/// Permission-mode enum used throughout the UI layer. Covers Claude
/// Code's six modes plus `Unknown` for forward compatibility with agents
/// that add more modes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    Plan,
    AcceptEdits,
    Auto,
    DontAsk,
    BypassPermissions,
    Unknown,
}

impl From<&PermissionModeFlag> for PermissionMode {
    fn from(flag: &PermissionModeFlag) -> Self {
        match flag.0.as_str() {
            "default" => Self::Default,
            "plan" => Self::Plan,
            "acceptEdits" => Self::AcceptEdits,
            "auto" => Self::Auto,
            "dontAsk" => Self::DontAsk,
            "bypassPermissions" => Self::BypassPermissions,
            _ => Self::Unknown,
        }
    }
}

/// Token-usage snapshot carried on `PreCompact` / `PostCompact` events
/// and rendered by the Context-Usage-Chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub tokens_used: u64,
    pub tokens_max: u64,
}

impl ContextUsage {
    pub fn percent(&self) -> f32 {
        if self.tokens_max == 0 {
            0.0
        } else {
            (self.tokens_used as f32 / self.tokens_max as f32) * 100.0
        }
    }
}

/// The runtime handle for a single agent session — one per agent process
/// that Carrot is tracking. Attached to a `WorkspaceSession` via
/// `CliAgentSessionManager`.
///
/// Lives as an `Entity<CliAgentSession>` in Inazuma's ownership model.
#[derive(Clone)]
pub struct CliAgentSession {
    pub agent: SharedCliAgent,
    pub state: CliAgentSessionState,
    pub pid: u32,
    pub started_at: Instant,
    pub cmdline: Vec<String>,

    /// Opaque session id handed out by the agent via its first
    /// `SessionStart` hook. `None` until the first hook arrives (or
    /// permanently `None` when the plugin is not installed).
    pub session_id: Option<String>,

    /// Path to the agent's own JSONL transcript, if any.
    pub transcript_path: Option<PathBuf>,

    /// Human-friendly name — set via `--name` or derived from `--resume`.
    pub name: Option<String>,

    /// If the session was launched with `--resume`/`-r`, the name or id
    /// that was resumed.
    pub resume_session_name: Option<String>,

    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub worktree: Option<PathBuf>,
    pub from_pr: Option<u64>,

    /// Rule files loaded by the agent (CLAUDE.md, .claude/rules/*.md,
    /// AGENTS.md, …). Populated via the `InstructionsLoaded` hook.
    pub rules_loaded: Vec<PathBuf>,

    pub context_usage: Option<ContextUsage>,

    /// Counter driving the Vertical-Tabs Unread-Dot. Incremented by the
    /// session manager on every inbound `CliAgentEvent`; reset to 0 when
    /// the panel's `activate_session` grabs focus.
    pub unread_events_since_focus: u32,
}

impl CliAgentSession {
    /// Build a freshly-starting session. State-priority rules say the
    /// session starts in `Starting` and the first `SessionStart` hook
    /// (or, failing that, the first heuristic output chunk) promotes it.
    pub fn new(agent: SharedCliAgent, pid: u32, cmdline: Vec<String>) -> Self {
        Self {
            agent,
            state: CliAgentSessionState::Starting,
            pid,
            started_at: Instant::now(),
            cmdline,
            session_id: None,
            transcript_path: None,
            name: None,
            resume_session_name: None,
            permission_mode: None,
            model: None,
            worktree: None,
            from_pr: None,
            rules_loaded: Vec::new(),
            context_usage: None,
            unread_events_since_focus: 0,
        }
    }
}

impl EventEmitter<CliAgentEvent> for CliAgentSession {}

/// High-level events the session manager emits to subscribers (UI
/// layer, notifications, task panel, review panel, etc.). This is a
/// semantic layer on top of raw hook events.
#[derive(Debug, Clone)]
pub enum CliAgentEvent {
    SessionStarted,
    SessionEnded {
        exit_code: Option<i32>,
    },
    StateChanged {
        old: CliAgentSessionState,
        new: CliAgentSessionState,
    },
    FileChanged {
        path: PathBuf,
    },
    PermissionRequested {
        tool_name: String,
    },
    TaskCreated {
        task_id: String,
        content: String,
    },
    TaskCompleted {
        task_id: String,
    },
    ContextUsageUpdated {
        usage: ContextUsage,
    },
    RulesLoaded {
        paths: Vec<PathBuf>,
    },
    WorktreeCreated {
        path: PathBuf,
        branch: String,
    },
    WorktreeRemoved {
        path: PathBuf,
    },
    SubagentStarted {
        agent_id: String,
        agent_type: String,
    },
    SubagentStopped {
        agent_id: String,
    },
    ElicitationRequested {
        mcp_server: String,
    },
    PromptSubmitted {
        prompt: String,
    },
    AssistantResponded {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_terminal() {
        assert!(
            CliAgentSessionState::Completed { exit_code: Some(0) }.is_terminal(),
            "Completed{{0}} should be terminal"
        );
        assert!(
            CliAgentSessionState::Completed { exit_code: Some(1) }.is_terminal(),
            "Completed{{nonzero}} should be terminal"
        );
        assert!(
            CliAgentSessionState::Errored { reason: "x".into() }.is_terminal(),
            "Errored should be terminal"
        );
    }

    #[test]
    fn working_states_are_not_terminal() {
        assert!(!CliAgentSessionState::Starting.is_terminal());
        assert!(!CliAgentSessionState::Idle.is_terminal());
        assert!(
            !CliAgentSessionState::Working {
                since: Instant::now()
            }
            .is_terminal()
        );
        assert!(
            !CliAgentSessionState::WaitingForInput {
                notification_type: NotificationType::IdlePrompt,
            }
            .is_terminal()
        );
        assert!(!CliAgentSessionState::Compacting.is_terminal());
    }

    #[test]
    fn permission_mode_parses_known_variants() {
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("default".into())),
            PermissionMode::Default
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("plan".into())),
            PermissionMode::Plan
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("acceptEdits".into())),
            PermissionMode::AcceptEdits
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("auto".into())),
            PermissionMode::Auto
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("dontAsk".into())),
            PermissionMode::DontAsk
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("bypassPermissions".into())),
            PermissionMode::BypassPermissions
        );
    }

    #[test]
    fn permission_mode_falls_back_to_unknown() {
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag("future-mode".into())),
            PermissionMode::Unknown
        );
        assert_eq!(
            PermissionMode::from(&PermissionModeFlag(String::new())),
            PermissionMode::Unknown
        );
    }

    #[test]
    fn context_usage_percent_matches_ratio() {
        let usage = ContextUsage {
            tokens_used: 50,
            tokens_max: 200,
        };
        assert!((usage.percent() - 25.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_usage_handles_zero_max() {
        let usage = ContextUsage {
            tokens_used: 0,
            tokens_max: 0,
        };
        assert_eq!(usage.percent(), 0.0);
    }
}
