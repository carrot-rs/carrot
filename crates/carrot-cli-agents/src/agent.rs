use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bitflags::bitflags;
use inazuma::App;
use serde::{Deserialize, Serialize};

use crate::hook_events::CliAgentHookEvent;
use crate::session::CliAgentSessionState;

bitflags! {
    /// Bit-flags describing what capabilities an agent exposes. The registry
    /// and UI layers use these to decide which affordances to render — e.g.
    /// the voice-input button only appears for agents that advertise
    /// `VOICE`, and the MCP selector only for those with `MCP_TOOLS`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CliAgentCapabilities: u32 {
        const HOOKS             = 1 << 0;
        const RICH_INPUT        = 1 << 1;
        const NOTIFICATIONS     = 1 << 2;
        const DIFF_REVIEW       = 1 << 3;
        const VOICE             = 1 << 4;
        const IMAGES            = 1 << 5;
        const TASK_PANEL        = 1 << 6;
        const RESUME_SESSIONS   = 1 << 7;
        const MCP_TOOLS         = 1 << 8;
        const WORKTREES         = 1 << 9;
        const SUBAGENTS         = 1 << 10;
        const ELICITATION       = 1 << 11;
        const PERMISSION_MODES  = 1 << 12;
        const CONTEXT_WINDOW    = 1 << 13;
        const RULES             = 1 << 14;
    }
}

/// Cross-platform snapshot of a single process discovered in a PTY's
/// descendant tree. Populated by `detection::scan_pty_descendants` and
/// consumed by `CliAgent::classify`.
///
/// `exe` may be empty on macOS sandboxed processes and on Linux where
/// `/proc/<pid>/exe` is unreadable from unprivileged contexts — callers
/// must treat it as a best-effort hint and fall back to `name` for
/// binary matching.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub exe: PathBuf,
    pub cmdline: Vec<String>,
    /// Environment block of the process if sysinfo could read it.
    /// None on platforms where the environ is unreadable (common in
    /// containers, restricted /proc, sandboxed macOS apps).
    pub env: Option<HashMap<String, String>>,
}

/// Opaque snapshot of the output currently visible in an agent's block.
/// The heuristic `state_from_output` path (used when the Carrot plugin is
/// not installed) inspects this to guess state. Concrete agents define
/// the representation; here we treat it as opaque data.
#[derive(Debug, Clone, Default)]
pub struct BlockOutputSnapshot {
    pub text: String,
}

/// What `classify` returns when it matched a process to an agent. Carries
/// the parsed cmdline flags downstream consumers need (session name,
/// resume id, worktree path, PR number, permission mode, model).
#[derive(Debug, Clone, Default)]
pub struct CliAgentMatch {
    pub pid: u32,
    pub cmdline: Vec<String>,
    pub name: Option<String>,
    pub resume_session_name: Option<String>,
    pub continue_latest: bool,
    pub worktree: Option<PathBuf>,
    pub from_pr: Option<u64>,
    pub permission_mode: Option<PermissionModeFlag>,
    pub model: Option<String>,
}

/// String-typed permission-mode tag as seen on the command line. The
/// richer `PermissionMode` enum in `session.rs` models the semantic state;
/// this variant is only what we can extract from cmdline text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionModeFlag(pub String);

/// Bundled plugin assets for an agent (path inside `assets/plugins/` and
/// a content-hash used by the First-Run-Installer).
#[derive(Debug, Clone, Copy)]
pub struct PluginAssets {
    pub bundle_dir: &'static str,
    pub version: &'static str,
}

/// The core trait every CLI agent implements. Agents live in
/// `src/agents/<agent>.rs` and register themselves via
/// `CliAgentRegistry::register` during `init`.
pub trait CliAgent: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn binary_names(&self) -> &'static [&'static str];
    fn icon_path(&self) -> &'static str;

    /// Primary brand colour pair — `(background, foreground)` as hex
    /// strings such as `"#C15F3C"`. The vertical-tabs row uses this
    /// pair to compose a brand circle: the bg fills the circle, the
    /// fg tints the monochrome sparkle SVG at [`CliAgent::icon_path`]
    /// painted on top. Returning `None` makes the row fall back to
    /// the generic terminal/editor icon path (no brand circle).
    ///
    /// Hex strings (rather than an `Oklch` type) keep this crate free
    /// of a `carrot-theme` dependency — brand colours are identity
    /// data, not themed UI tokens, so the UI layer converts them
    /// locally via `inazuma::rgb`.
    fn brand_colors(&self) -> Option<(&'static str, &'static str)> {
        None
    }

    fn capabilities(&self) -> CliAgentCapabilities;

    /// Decide whether a discovered process belongs to this agent and, if
    /// so, extract launch-flag metadata. Called on every child-process
    /// change of a terminal pane.
    fn classify(&self, process: &ProcessInfo, cx: &App) -> Option<CliAgentMatch>;

    /// Map a hook event to a concrete session-state transition. Agents
    /// that advertise `HOOKS` must implement this; returning `None` means
    /// "this event does not affect state".
    fn state_from_hook(&self, event: &CliAgentHookEvent) -> Option<CliAgentSessionState>;

    /// Fallback state-detection from terminal output when the agent's
    /// plugin is not installed. Returning `None` means "output was
    /// inconclusive — leave state unchanged".
    fn state_from_output(&self, snapshot: &BlockOutputSnapshot) -> Option<CliAgentSessionState>;

    /// Bundled plugin assets, if this agent ships one.
    fn plugin_assets(&self) -> Option<PluginAssets> {
        None
    }

    /// Path to the most recent session transcript for a given CWD, or
    /// `None` if the agent does not persist transcripts.
    fn session_transcript_path(&self, cwd: &Path) -> Option<PathBuf> {
        let _ = cwd;
        None
    }
}

/// An agent registered in the registry is always an `Arc<dyn CliAgent>`.
pub type SharedCliAgent = Arc<dyn CliAgent>;
