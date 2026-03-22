//! CLI agent integration for Carrot.
//!
//! Detects CLI agents (Claude Code, and later Codex/Gemini/Aider) running
//! inside terminal panes and augments them with native Carrot metadata —
//! without ever replacing the agent's own TUI. Every feature in this crate
//! is either a metadata display around the terminal or a write into the
//! PTY input stream that mimics user typing.
//!
//! The foundation lives here: traits, session model, event types, and the
//! global registry. Agents themselves implement the `CliAgent` trait in
//! sibling modules.

mod agent;
mod agents;
mod child_process_watcher;
mod detection;
mod diff_stats_poller;
mod hook_events;
mod plugin_installer;
mod registry;
mod session;
mod session_core;
mod session_manager;

pub use agents::ClaudeCodeAgent;
pub use child_process_watcher::{ChildProcessWatcher, ChildProcessesDetected, WatcherMode};
pub use diff_stats_poller::{
    DiffStats, POLL_INTERVAL as DIFF_STATS_POLL_INTERVAL, has_git_worktree, parse_shortstat,
    poll_once, run_shortstat_blocking, spawn_poll_loop,
};
pub use session_core::{
    Classification, ManagerEvent, PaneId, PtyPid, SESSION_DROP_TTL, SessionCore, SessionEffect,
    SessionInput, SessionKey, TerminalRecord,
};
pub use session_manager::{
    CliAgentSessionManager, GlobalCliAgentSessionManager, ManagerEventEnvelope, focus_pane,
    forward_hook_envelope, register_terminal, unregister_terminal,
};

pub use agent::{
    BlockOutputSnapshot, CliAgent, CliAgentCapabilities, CliAgentMatch, PermissionModeFlag,
    PluginAssets, ProcessInfo, SharedCliAgent,
};
pub use detection::{
    binary_matches, classify_processes, parse_claude_cmdline, scan_pty_descendants,
    scan_pty_descendants_in,
};
pub use hook_events::{
    CARROT_PROTOCOL_VERSION, ChangeType, CliAgentHookEvent, ENVELOPE_TYPE_CLI_AGENT_EVENT,
    EnvelopeError, HookEventEnvelope, ParsedHookEvent, PermissionSuggestion, TaskStatus,
    parse_envelope,
};
pub use plugin_installer::{
    InstallError, InstallStatus, bundled_version, check_status, install, install_into,
    plugin_install_dir, plugins_root, uninstall,
};
pub use registry::CliAgentRegistry;
pub use session::{
    CliAgentEvent, CliAgentSession, CliAgentSessionState, ContextUsage, NotificationType,
    PermissionMode, SessionSource,
};

use inazuma::{App, AppContext};
use std::sync::Arc;

/// Install the `CliAgentRegistry` as a global and seed it with every
/// built-in `CliAgent`. Materialises the `CliAgentSessionManager` as
/// an entity and stores it in a `Global` wrapper. Called once from
/// `carrot-app`'s `main.rs`.
///
/// New agents are added by instantiating them in this function — the
/// registry is append-only during app lifetime, so order here
/// determines classification priority in
/// `detection::classify_processes`.
pub fn init(cx: &mut App) {
    let mut registry = CliAgentRegistry::default();
    registry.register(Arc::new(ClaudeCodeAgent::new()));
    cx.set_global(registry);

    let manager = cx.new(|_| CliAgentSessionManager::new());
    cx.set_global(GlobalCliAgentSessionManager(manager));
}
