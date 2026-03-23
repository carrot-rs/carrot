//! Pure, effectful-free finite state machine for agent-session
//! lifecycle.
//!
//! ## Why Core + Effect
//!
//! Session-manager logic touches four concerns:
//!   1. Data — which terminals and sessions exist, how they relate.
//!   2. Inazuma entities — watcher handles, emitter subscriptions.
//!   3. Timers — the 30-second drop TTL after a session completes.
//!   4. Event dispatch — hooks that arrive over OSC 7777.
//!
//! Colocating all four in one struct (the naive approach) makes the
//! data-and-FSM part impossible to test without a live Inazuma app
//! and turns map-consistency into per-call-site discipline. This
//! module isolates concern #1 + #4 into a pure state machine; the
//! thin runner in `session_manager.rs` realises the effects on the
//! Inazuma side.
//!
//! ## Contract
//!
//! * Every mutation goes through [`SessionCore::handle`], which
//!   takes a [`SessionInput`] and returns `Vec<SessionEffect>`.
//! * The returned effects are the ONLY instructions the runner ever
//!   executes. Any data change not reflected in the returned
//!   vector is a bug — the runner would drift out of sync.
//! * Invariants (no dangling index entry, session key belongs to
//!   exactly one terminal, …) are asserted in debug builds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::agent::{CliAgentMatch, ProcessInfo, SharedCliAgent};
use crate::child_process_watcher::WatcherMode;
use crate::hook_events::{CliAgentHookEvent, ParsedHookEvent};
use crate::session::{
    CliAgentEvent, CliAgentSession, CliAgentSessionState, ContextUsage, PermissionMode,
};

/// Opaque pane identifier; the runner chooses what it means (we
/// use the Inazuma entity id of the pane).
pub type PaneId = u64;

/// PID of the shell the PTY is attached to. Stable for the
/// lifetime of the terminal.
pub type PtyPid = u32;

/// Monotonic session identifier issued by the core. Never reused —
/// once a session is dropped, its key becomes invalid. The public
/// API of the manager hands these out to UI code that then uses
/// them to look sessions up.
pub type SessionKey = u64;

/// How long a terminated session lingers in the core before it is
/// dropped. The runner arms a timer via
/// [`SessionEffect::ScheduleDropTtl`] and then calls
/// [`SessionInput::DropTtlExpired`] when the timer fires.
pub const SESSION_DROP_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Record for one live terminal being tracked. Purely data.
#[derive(Debug, Clone)]
pub struct TerminalRecord {
    pub pty_pid: PtyPid,
    pub pane_id: PaneId,
    pub cwd: PathBuf,
    pub session: Option<SessionKey>,
}

/// Internal per-session record; we store [`CliAgentSession`] as-is
/// and add runtime-only fields next to it so session.rs stays the
/// single source of truth for agent-facing data.
#[derive(Clone)]
struct SessionEntry {
    session: CliAgentSession,
    /// When `Some`, the session has completed and the runner has
    /// scheduled a drop at `Instant`. Set by the FSM when a hook or
    /// process-tree transition lands the session in a terminal
    /// state.
    terminated_at: Option<Instant>,
}

/// Pure data model for the session manager.
///
/// Invariants (enforced by [`SessionCore::handle`] — violation is a
/// bug in the FSM):
///
/// * Every `SessionKey` stored in `terminals[pty].session` exists in
///   `sessions`.
/// * Every `session_id` in `session_id_index` maps to an entry in
///   `sessions`.
/// * Every `pane_id` in `pane_index` maps to an entry in
///   `terminals`.
/// * A session key is assigned to at most one terminal.
pub struct SessionCore {
    terminals: BTreeMap<PtyPid, TerminalRecord>,
    sessions: BTreeMap<SessionKey, SessionEntry>,
    session_id_index: BTreeMap<String, SessionKey>,
    pane_index: BTreeMap<PaneId, PtyPid>,
    next_session_key: SessionKey,
}

impl SessionCore {
    pub fn new() -> Self {
        Self {
            terminals: BTreeMap::new(),
            sessions: BTreeMap::new(),
            session_id_index: BTreeMap::new(),
            pane_index: BTreeMap::new(),
            next_session_key: 1,
        }
    }

    // ---------- Read-side API (pure, non-mutating) ----------

    pub fn terminal(&self, pty_pid: PtyPid) -> Option<&TerminalRecord> {
        self.terminals.get(&pty_pid)
    }

    pub fn terminal_for_pane(&self, pane_id: PaneId) -> Option<&TerminalRecord> {
        let pty = self.pane_index.get(&pane_id).copied()?;
        self.terminals.get(&pty)
    }

    pub fn session(&self, key: SessionKey) -> Option<&CliAgentSession> {
        self.sessions.get(&key).map(|entry| &entry.session)
    }

    pub fn session_for_pane(&self, pane_id: PaneId) -> Option<&CliAgentSession> {
        let key = self.terminal_for_pane(pane_id)?.session?;
        self.session(key)
    }

    pub fn session_by_session_id(&self, session_id: &str) -> Option<&CliAgentSession> {
        let key = self.session_id_index.get(session_id).copied()?;
        self.session(key)
    }

    pub fn active_session_keys(&self) -> Vec<SessionKey> {
        self.sessions.keys().copied().collect()
    }

    pub fn terminals_count(&self) -> usize {
        self.terminals.len()
    }

    pub fn sessions_count(&self) -> usize {
        self.sessions.len()
    }

    // ---------- FSM entry point ----------

    /// Drive one input event through the FSM and return the
    /// effects the runner must apply. `now` is passed explicitly
    /// so tests can inject deterministic timestamps.
    pub fn handle(&mut self, input: SessionInput, now: Instant) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        match input {
            SessionInput::RegisterTerminal {
                pty_pid,
                pane_id,
                cwd,
            } => self.on_register_terminal(pty_pid, pane_id, cwd, &mut effects),
            SessionInput::UnregisterTerminal { pty_pid } => {
                self.on_unregister_terminal(pty_pid, &mut effects)
            }
            SessionInput::ChildProcessesDetected {
                pty_pid,
                classification,
            } => self.on_child_processes(pty_pid, classification, now, &mut effects),
            SessionInput::HookEnvelopeParsed { envelope } => {
                self.on_hook_envelope(envelope, now, &mut effects)
            }
            SessionInput::DropTtlExpired { key } => {
                self.on_drop_ttl_expired(key, now, &mut effects)
            }
            SessionInput::PaneFocused { pane_id } => self.on_pane_focused(pane_id, &mut effects),
        }
        self.assert_invariants();
        effects
    }

    // ---------- Transition handlers ----------

    fn on_register_terminal(
        &mut self,
        pty_pid: PtyPid,
        pane_id: PaneId,
        cwd: PathBuf,
        effects: &mut Vec<SessionEffect>,
    ) {
        // Idempotent: re-registering the same pty pid is a no-op
        // beyond updating the cwd if it changed (which should never
        // happen under the current caller contract, but we handle
        // it defensively).
        if let Some(terminal) = self.terminals.get_mut(&pty_pid) {
            terminal.cwd = cwd;
            return;
        }
        self.terminals.insert(
            pty_pid,
            TerminalRecord {
                pty_pid,
                pane_id,
                cwd,
                session: None,
            },
        );
        self.pane_index.insert(pane_id, pty_pid);
        effects.push(SessionEffect::StartWatcher { pty_pid });
    }

    fn on_unregister_terminal(&mut self, pty_pid: PtyPid, effects: &mut Vec<SessionEffect>) {
        let Some(terminal) = self.terminals.remove(&pty_pid) else {
            return;
        };
        self.pane_index.remove(&terminal.pane_id);
        effects.push(SessionEffect::StopWatcher { pty_pid });

        if let Some(key) = terminal.session
            && let Some(entry) = self.sessions.remove(&key)
        {
            if let Some(sid) = &entry.session.session_id {
                self.session_id_index.remove(sid);
            }
            effects.push(SessionEffect::EmitEvent {
                key,
                event: ManagerEvent::SessionEnded { exit_code: None },
            });
            effects.push(SessionEffect::DropSession { key });
        }
    }

    fn on_child_processes(
        &mut self,
        pty_pid: PtyPid,
        classification: Option<Classification>,
        now: Instant,
        effects: &mut Vec<SessionEffect>,
    ) {
        let Some(terminal) = self.terminals.get(&pty_pid) else {
            return;
        };
        let current_session = terminal.session;

        match (classification, current_session) {
            (Some(class), None) => {
                // New agent detected under this terminal.
                let key = self.allocate_session_key();
                let session = build_session_from_match(&class);
                self.sessions.insert(
                    key,
                    SessionEntry {
                        session,
                        terminated_at: None,
                    },
                );
                if let Some(terminal) = self.terminals.get_mut(&pty_pid) {
                    terminal.session = Some(key);
                }
                effects.push(SessionEffect::SetWatcherMode {
                    pty_pid,
                    mode: WatcherMode::Warm,
                });
                effects.push(SessionEffect::EmitEvent {
                    key,
                    event: ManagerEvent::SessionStarted,
                });
            }
            (Some(_), Some(_existing)) => {
                // Agent still present; a subsequent hook event will
                // advance its state. The process-poll path only
                // creates and destroys sessions.
            }
            (None, Some(key)) => {
                // All agent descendants are gone; session transitions
                // to Completed (exit code unknown without a hook).
                let prev_state = self.sessions.get(&key).map(|e| e.session.state.clone());
                if let Some(entry) = self.sessions.get_mut(&key)
                    && !entry.session.state.is_terminal()
                {
                    let old = entry.session.state.clone();
                    entry.session.state = CliAgentSessionState::Completed { exit_code: None };
                    entry.terminated_at = Some(now);
                    let new = entry.session.state.clone();
                    effects.push(SessionEffect::EmitEvent {
                        key,
                        event: ManagerEvent::StateChanged {
                            old,
                            new: new.clone(),
                        },
                    });
                    effects.push(SessionEffect::EmitEvent {
                        key,
                        event: ManagerEvent::SessionEnded { exit_code: None },
                    });
                    effects.push(SessionEffect::ScheduleDropTtl {
                        key,
                        at: now + SESSION_DROP_TTL,
                    });
                    effects.push(SessionEffect::SetWatcherMode {
                        pty_pid,
                        mode: WatcherMode::Hot,
                    });
                }
                let _ = prev_state;
            }
            (None, None) => {
                // Shell running but no agent — poll noise, ignore.
            }
        }
    }

    fn on_hook_envelope(
        &mut self,
        envelope: ParsedHookEvent,
        now: Instant,
        effects: &mut Vec<SessionEffect>,
    ) {
        let session_id = envelope.event.session_id().to_string();

        // Resolve session: existing session-id mapping first, then
        // fall back to CWD matching for hooks that carry one.
        let key = self
            .session_id_index
            .get(&session_id)
            .copied()
            .or_else(|| self.match_session_by_cwd(&envelope.event));

        let Some(key) = key else {
            // Unroutable: the session is not yet materialised. The
            // next ChildProcessesDetected tick will create it and
            // the plugin will resend its SessionStart then.
            return;
        };

        // Update CWD index before we mutate the session: if this is
        // a CwdChanged event, the new cwd replaces the old so
        // future cwd-matches find this terminal.
        if let CliAgentHookEvent::CwdChanged { cwd, .. } = &envelope.event {
            if let Some(pty) = self.find_pty_for_session(key)
                && let Some(terminal) = self.terminals.get_mut(&pty)
            {
                terminal.cwd = cwd.clone();
            }
        }

        let Some(entry) = self.sessions.get_mut(&key) else {
            debug_assert!(
                false,
                "session_id_index points to non-existent session {key}"
            );
            return;
        };

        // Record session_id on first encounter.
        if entry.session.session_id.is_none() {
            entry.session.session_id = Some(session_id.clone());
            self.session_id_index.insert(session_id.clone(), key);
        }

        entry.session.unread_events_since_focus =
            entry.session.unread_events_since_focus.saturating_add(1);

        let agent = entry.session.agent.clone();
        let new_state = agent.state_from_hook(&envelope.event);

        apply_event_metadata(&mut entry.session, &envelope.event);

        if let Some(new_state) = new_state {
            let old = entry.session.state.clone();
            if old != new_state {
                entry.session.state = new_state.clone();
                effects.push(SessionEffect::EmitEvent {
                    key,
                    event: ManagerEvent::StateChanged {
                        old: old.clone(),
                        new: new_state.clone(),
                    },
                });
                if matches!(&new_state, CliAgentSessionState::Completed { .. }) {
                    entry.terminated_at = Some(now);
                    let exit_code = match &new_state {
                        CliAgentSessionState::Completed { exit_code } => *exit_code,
                        _ => None,
                    };
                    effects.push(SessionEffect::EmitEvent {
                        key,
                        event: ManagerEvent::SessionEnded { exit_code },
                    });
                    effects.push(SessionEffect::ScheduleDropTtl {
                        key,
                        at: now + SESSION_DROP_TTL,
                    });
                }
            }
        }

        for ev in semantic_events_for(&envelope.event) {
            effects.push(SessionEffect::EmitEvent { key, event: ev });
        }
    }

    fn on_drop_ttl_expired(
        &mut self,
        key: SessionKey,
        now: Instant,
        effects: &mut Vec<SessionEffect>,
    ) {
        let Some(entry) = self.sessions.get(&key) else {
            return;
        };
        let Some(terminated_at) = entry.terminated_at else {
            // The session re-started while the timer was pending —
            // terminated_at was cleared. Ignore.
            return;
        };
        if now.duration_since(terminated_at) < SESSION_DROP_TTL {
            // Timer landed early (e.g. coarse scheduler); no-op and
            // let the runner re-schedule if it wants to.
            return;
        }

        // Remove from all indices.
        let session_id_to_remove = entry.session.session_id.clone();
        self.sessions.remove(&key);
        if let Some(sid) = session_id_to_remove {
            self.session_id_index.remove(&sid);
        }
        // Detach from whatever terminal had it.
        if let Some(pty) = self.find_pty_for_session_in_terminals(key) {
            if let Some(terminal) = self.terminals.get_mut(&pty) {
                terminal.session = None;
            }
        }
        effects.push(SessionEffect::DropSession { key });
    }

    fn on_pane_focused(&mut self, pane_id: PaneId, effects: &mut Vec<SessionEffect>) {
        let Some(pty) = self.pane_index.get(&pane_id).copied() else {
            return;
        };
        let Some(terminal) = self.terminals.get(&pty) else {
            return;
        };
        let Some(key) = terminal.session else { return };
        let Some(entry) = self.sessions.get_mut(&key) else {
            return;
        };
        if entry.session.unread_events_since_focus > 0 {
            entry.session.unread_events_since_focus = 0;
            effects.push(SessionEffect::EmitEvent {
                key,
                event: ManagerEvent::UnreadReset,
            });
        }
    }

    // ---------- Internal helpers ----------

    fn allocate_session_key(&mut self) -> SessionKey {
        let key = self.next_session_key;
        self.next_session_key = self.next_session_key.saturating_add(1);
        key
    }

    /// Walk terminals and return the pty whose session field is
    /// this key. Used by the hook path that needs to update the
    /// terminal's cwd when a `CwdChanged` arrives.
    fn find_pty_for_session(&self, key: SessionKey) -> Option<PtyPid> {
        self.terminals
            .iter()
            .find(|(_, t)| t.session == Some(key))
            .map(|(pty, _)| *pty)
    }

    fn find_pty_for_session_in_terminals(&self, key: SessionKey) -> Option<PtyPid> {
        self.find_pty_for_session(key)
    }

    /// Route an unknown-session hook by matching the event's cwd
    /// field (only populated for SessionStart and CwdChanged)
    /// against every registered terminal.
    fn match_session_by_cwd(&self, event: &CliAgentHookEvent) -> Option<SessionKey> {
        let event_cwd = match event {
            CliAgentHookEvent::SessionStart { cwd, .. } => cwd,
            CliAgentHookEvent::CwdChanged { cwd, .. } => cwd,
            _ => return None,
        };
        let terminal = self
            .terminals
            .values()
            .find(|t| &t.cwd == event_cwd && t.session.is_some())?;
        terminal.session
    }

    /// Enforce the structural invariants documented on
    /// [`SessionCore`]. Runs in debug builds only; production
    /// hot-paths stay lean.
    fn assert_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            for (pty, terminal) in &self.terminals {
                debug_assert_eq!(*pty, terminal.pty_pid, "terminal key/pid mismatch");
                if let Some(key) = terminal.session {
                    debug_assert!(
                        self.sessions.contains_key(&key),
                        "terminal {} references non-existent session {}",
                        pty,
                        key
                    );
                }
                debug_assert_eq!(
                    self.pane_index.get(&terminal.pane_id).copied(),
                    Some(*pty),
                    "pane_index/terminal desync for pane {}",
                    terminal.pane_id
                );
            }
            for (sid, key) in &self.session_id_index {
                debug_assert!(
                    self.sessions.contains_key(key),
                    "session_id_index key {} points to non-existent session {}",
                    sid,
                    key
                );
                let entry_sid = self
                    .sessions
                    .get(key)
                    .and_then(|e| e.session.session_id.as_ref());
                debug_assert_eq!(
                    entry_sid,
                    Some(sid),
                    "session_id_index key {} disagrees with session record",
                    sid
                );
            }
            // Each session key belongs to at most one terminal.
            let mut seen: BTreeMap<SessionKey, PtyPid> = BTreeMap::new();
            for (pty, terminal) in &self.terminals {
                if let Some(key) = terminal.session {
                    debug_assert!(
                        seen.insert(key, *pty).is_none(),
                        "session {} is attached to multiple terminals",
                        key
                    );
                }
            }
        }
    }
}

impl Default for SessionCore {
    fn default() -> Self {
        Self::new()
    }
}

/// Classification payload produced by the runner from a
/// [`ChildProcessesDetected`] watcher event. Carries everything the
/// core needs to materialise a fresh session without touching the
/// agent registry or App context.
#[derive(Clone)]
pub struct Classification {
    pub agent: SharedCliAgent,
    pub process: ProcessInfo,
    pub cmdline_match: CliAgentMatch,
}

/// Every mutation the core accepts.
pub enum SessionInput {
    RegisterTerminal {
        pty_pid: PtyPid,
        pane_id: PaneId,
        cwd: PathBuf,
    },
    UnregisterTerminal {
        pty_pid: PtyPid,
    },
    ChildProcessesDetected {
        pty_pid: PtyPid,
        /// `None` means "no agent found under this PTY". The core
        /// uses this to transition an existing session to
        /// Completed.
        classification: Option<Classification>,
    },
    HookEnvelopeParsed {
        envelope: ParsedHookEvent,
    },
    DropTtlExpired {
        key: SessionKey,
    },
    PaneFocused {
        pane_id: PaneId,
    },
}

/// Instructions the runner must execute on the Inazuma side.
#[derive(Debug, Clone)]
pub enum SessionEffect {
    StartWatcher {
        pty_pid: PtyPid,
    },
    StopWatcher {
        pty_pid: PtyPid,
    },
    SetWatcherMode {
        pty_pid: PtyPid,
        mode: WatcherMode,
    },
    EmitEvent {
        key: SessionKey,
        event: ManagerEvent,
    },
    /// Arm a drop timer; the runner calls
    /// [`SessionInput::DropTtlExpired`] once it fires.
    ScheduleDropTtl {
        key: SessionKey,
        at: Instant,
    },
    /// Final drop — the session has already been removed from the
    /// core. The runner should tear down any per-session Inazuma
    /// state it was keeping.
    DropSession {
        key: SessionKey,
    },
}

/// Semantic events emitted by the manager. The runner
/// forwards each one via `cx.emit`; consumers (UI layer, Vertical
/// Tabs, notifications) subscribe to the manager entity.
#[derive(Debug, Clone)]
pub enum ManagerEvent {
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
    UnreadReset,
}

impl ManagerEvent {
    /// Map to the old [`CliAgentEvent`] enum for consumers that
    /// already subscribe to it. Kept so the public `CliAgentEvent`
    /// surface does not need to change when the runner is
    /// introduced.
    pub fn to_cli_agent_event(&self) -> Option<CliAgentEvent> {
        Some(match self {
            Self::SessionStarted => CliAgentEvent::SessionStarted,
            Self::SessionEnded { exit_code } => CliAgentEvent::SessionEnded {
                exit_code: *exit_code,
            },
            Self::StateChanged { old, new } => CliAgentEvent::StateChanged {
                old: old.clone(),
                new: new.clone(),
            },
            Self::FileChanged { path } => CliAgentEvent::FileChanged { path: path.clone() },
            Self::PermissionRequested { tool_name } => CliAgentEvent::PermissionRequested {
                tool_name: tool_name.clone(),
            },
            Self::TaskCreated { task_id, content } => CliAgentEvent::TaskCreated {
                task_id: task_id.clone(),
                content: content.clone(),
            },
            Self::TaskCompleted { task_id } => CliAgentEvent::TaskCompleted {
                task_id: task_id.clone(),
            },
            Self::ContextUsageUpdated { usage } => {
                CliAgentEvent::ContextUsageUpdated { usage: *usage }
            }
            Self::RulesLoaded { paths } => CliAgentEvent::RulesLoaded {
                paths: paths.clone(),
            },
            Self::WorktreeCreated { path, branch } => CliAgentEvent::WorktreeCreated {
                path: path.clone(),
                branch: branch.clone(),
            },
            Self::WorktreeRemoved { path } => CliAgentEvent::WorktreeRemoved { path: path.clone() },
            Self::SubagentStarted {
                agent_id,
                agent_type,
            } => CliAgentEvent::SubagentStarted {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
            },
            Self::SubagentStopped { agent_id } => CliAgentEvent::SubagentStopped {
                agent_id: agent_id.clone(),
            },
            Self::ElicitationRequested { mcp_server } => CliAgentEvent::ElicitationRequested {
                mcp_server: mcp_server.clone(),
            },
            Self::PromptSubmitted { prompt } => CliAgentEvent::PromptSubmitted {
                prompt: prompt.clone(),
            },
            Self::AssistantResponded { message } => CliAgentEvent::AssistantResponded {
                message: message.clone(),
            },
            Self::UnreadReset => return None,
        })
    }
}

// ---------- Free helpers ----------

fn build_session_from_match(class: &Classification) -> CliAgentSession {
    let mut session = CliAgentSession::new(
        class.agent.clone(),
        class.process.pid,
        class.process.cmdline.clone(),
    );
    session.name = class.cmdline_match.name.clone();
    session.resume_session_name = class.cmdline_match.resume_session_name.clone();
    session.worktree = class.cmdline_match.worktree.clone();
    session.from_pr = class.cmdline_match.from_pr;
    session.permission_mode = class
        .cmdline_match
        .permission_mode
        .as_ref()
        .map(PermissionMode::from);
    session.model = class.cmdline_match.model.clone();
    // Transcript path is agent-specific and best-effort; non-fatal
    // if the directory does not yet exist.
    let cwd = class
        .process
        .exe
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    session.transcript_path = class.agent.session_transcript_path(&cwd);
    session
}

fn apply_event_metadata(session: &mut CliAgentSession, event: &CliAgentHookEvent) {
    match event {
        CliAgentHookEvent::SessionStart {
            transcript_path,
            permission_mode,
            model,
            ..
        } => {
            session.transcript_path = Some(transcript_path.clone());
            session.permission_mode = Some(*permission_mode);
            session.model = Some(model.clone());
        }
        CliAgentHookEvent::InstructionsLoaded { paths, .. } => {
            session.rules_loaded = paths.clone();
        }
        CliAgentHookEvent::PreCompact {
            tokens_used,
            tokens_max,
            ..
        }
        | CliAgentHookEvent::PostCompact {
            tokens_used,
            tokens_max,
            ..
        } => {
            session.context_usage = Some(ContextUsage {
                tokens_used: *tokens_used,
                tokens_max: *tokens_max,
            });
        }
        CliAgentHookEvent::WorktreeCreate { path, .. } => {
            session.worktree = Some(path.clone());
        }
        _ => {}
    }
}

fn semantic_events_for(event: &CliAgentHookEvent) -> Vec<ManagerEvent> {
    match event {
        CliAgentHookEvent::FileChanged { path, .. } => {
            vec![ManagerEvent::FileChanged { path: path.clone() }]
        }
        CliAgentHookEvent::PermissionRequest { tool_name, .. } => {
            vec![ManagerEvent::PermissionRequested {
                tool_name: tool_name.clone(),
            }]
        }
        CliAgentHookEvent::TaskCreated {
            task_id, content, ..
        } => vec![ManagerEvent::TaskCreated {
            task_id: task_id.clone(),
            content: content.clone(),
        }],
        CliAgentHookEvent::TaskCompleted { task_id, .. } => {
            vec![ManagerEvent::TaskCompleted {
                task_id: task_id.clone(),
            }]
        }
        CliAgentHookEvent::PreCompact {
            tokens_used,
            tokens_max,
            ..
        }
        | CliAgentHookEvent::PostCompact {
            tokens_used,
            tokens_max,
            ..
        } => vec![ManagerEvent::ContextUsageUpdated {
            usage: ContextUsage {
                tokens_used: *tokens_used,
                tokens_max: *tokens_max,
            },
        }],
        CliAgentHookEvent::InstructionsLoaded { paths, .. } => {
            vec![ManagerEvent::RulesLoaded {
                paths: paths.clone(),
            }]
        }
        CliAgentHookEvent::WorktreeCreate { path, branch, .. } => {
            vec![ManagerEvent::WorktreeCreated {
                path: path.clone(),
                branch: branch.clone(),
            }]
        }
        CliAgentHookEvent::WorktreeRemove { path, .. } => {
            vec![ManagerEvent::WorktreeRemoved { path: path.clone() }]
        }
        CliAgentHookEvent::SubagentStart {
            agent_id,
            agent_type,
            ..
        } => vec![ManagerEvent::SubagentStarted {
            agent_id: agent_id.clone(),
            agent_type: agent_type.clone(),
        }],
        CliAgentHookEvent::SubagentStop { agent_id, .. } => {
            vec![ManagerEvent::SubagentStopped {
                agent_id: agent_id.clone(),
            }]
        }
        CliAgentHookEvent::Elicitation { mcp_server, .. } => {
            vec![ManagerEvent::ElicitationRequested {
                mcp_server: mcp_server.clone(),
            }]
        }
        CliAgentHookEvent::UserPromptSubmit { prompt, .. } => {
            vec![ManagerEvent::PromptSubmitted {
                prompt: prompt.clone(),
            }]
        }
        CliAgentHookEvent::Stop {
            last_assistant_message,
            ..
        } => vec![ManagerEvent::AssistantResponded {
            message: last_assistant_message.clone(),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{BlockOutputSnapshot, CliAgent, CliAgentCapabilities, PermissionModeFlag};
    use crate::hook_events::HookEventEnvelope;
    use crate::session::NotificationType;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    // A dummy agent for pure FSM testing — never dispatches to the
    // real Inazuma App.
    struct TestAgent {
        id: &'static str,
    }

    impl CliAgent for TestAgent {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            "Test"
        }
        fn binary_names(&self) -> &'static [&'static str] {
            &["testagent"]
        }
        fn icon_path(&self) -> &'static str {
            "icons/test.svg"
        }
        fn capabilities(&self) -> CliAgentCapabilities {
            CliAgentCapabilities::HOOKS
        }
        fn classify(&self, _process: &ProcessInfo, _cx: &inazuma::App) -> Option<CliAgentMatch> {
            None
        }
        fn state_from_hook(&self, event: &CliAgentHookEvent) -> Option<CliAgentSessionState> {
            match event {
                CliAgentHookEvent::SessionStart { .. } => Some(CliAgentSessionState::Starting),
                CliAgentHookEvent::UserPromptSubmit { .. } => Some(CliAgentSessionState::Working {
                    since: Instant::now(),
                }),
                CliAgentHookEvent::Stop { .. } => Some(CliAgentSessionState::Idle),
                CliAgentHookEvent::SessionEnd { exit_code, .. } => {
                    Some(CliAgentSessionState::Completed {
                        exit_code: *exit_code,
                    })
                }
                CliAgentHookEvent::PermissionRequest { .. } => {
                    Some(CliAgentSessionState::WaitingForInput {
                        notification_type: NotificationType::PermissionPrompt,
                    })
                }
                _ => None,
            }
        }
        fn state_from_output(
            &self,
            _snapshot: &BlockOutputSnapshot,
        ) -> Option<CliAgentSessionState> {
            None
        }
    }

    fn test_agent() -> SharedCliAgent {
        Arc::new(TestAgent { id: "test" })
    }

    fn test_classification(pid: u32, cwd: &str) -> Classification {
        Classification {
            agent: test_agent(),
            process: ProcessInfo {
                pid,
                ppid: 1,
                name: "testagent".into(),
                exe: PathBuf::from(format!("{}/testagent", cwd)),
                cmdline: vec!["testagent".into()],
                env: None,
            },
            cmdline_match: CliAgentMatch {
                pid,
                cmdline: vec!["testagent".into()],
                name: Some("demo".into()),
                ..CliAgentMatch::default()
            },
        }
    }

    fn envelope(name: &str, payload: serde_json::Value) -> ParsedHookEvent {
        let raw = serde_json::json!({
            "type": "cli_agent_event",
            "agent": "test",
            "protocol_version": 1,
            "event": name,
            "payload": payload,
        })
        .to_string();
        // Route through the real parser so our tests pin the wire
        // format at the same time.
        crate::hook_events::parse_envelope(&raw).expect("envelope parses")
    }

    fn stub_envelope_json() -> HookEventEnvelope {
        HookEventEnvelope {
            event_type: "cli_agent_event".into(),
            agent: "test".into(),
            protocol_version: 1,
            event: "Stop".into(),
            payload: serde_json::json!({"session_id": "x", "last_assistant_message": ""}),
        }
    }

    // ---- Transition tests ----

    #[test]
    fn register_terminal_emits_start_watcher() {
        let mut core = SessionCore::new();
        let now = Instant::now();
        let effects = core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            now,
        );
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            effects[0],
            SessionEffect::StartWatcher { pty_pid: 100 }
        ));
        assert_eq!(core.terminals_count(), 1);
        assert_eq!(core.sessions_count(), 0);
    }

    #[test]
    fn register_terminal_idempotent() {
        let mut core = SessionCore::new();
        let now = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/a"),
            },
            now,
        );
        let effects = core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/b"),
            },
            now,
        );
        assert!(effects.is_empty(), "re-register must produce no effects");
        assert_eq!(
            core.terminal(100).unwrap().cwd,
            PathBuf::from("/b"),
            "cwd update should still apply"
        );
    }

    #[test]
    fn classification_creates_session() {
        let mut core = SessionCore::new();
        let now = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            now,
        );
        let effects = core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            now,
        );

        assert_eq!(core.sessions_count(), 1);
        assert!(matches!(
            effects.iter().find(|e| matches!(
                e,
                SessionEffect::SetWatcherMode {
                    mode: WatcherMode::Warm,
                    ..
                }
            )),
            Some(_)
        ));
        assert!(matches!(
            effects.iter().find(|e| matches!(
                e,
                SessionEffect::EmitEvent {
                    event: ManagerEvent::SessionStarted,
                    ..
                }
            )),
            Some(_)
        ));

        let session = core.session_for_pane(1).expect("session reachable by pane");
        assert_eq!(session.pid, 42);
        assert_eq!(session.name.as_deref(), Some("demo"));
    }

    #[test]
    fn classification_gone_moves_session_to_completed_and_schedules_drop() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );

        let t1 = t0 + Duration::from_secs(10);
        let effects = core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: None,
            },
            t1,
        );

        let completed = effects.iter().any(|e| {
            matches!(
                e,
                SessionEffect::EmitEvent {
                    event: ManagerEvent::StateChanged {
                        new: CliAgentSessionState::Completed { .. },
                        ..
                    },
                    ..
                }
            )
        });
        assert!(completed, "state must transition to Completed");

        let scheduled_at = effects.iter().find_map(|e| match e {
            SessionEffect::ScheduleDropTtl { at, .. } => Some(*at),
            _ => None,
        });
        assert_eq!(scheduled_at, Some(t1 + SESSION_DROP_TTL));
    }

    #[test]
    fn hook_envelope_updates_session_state() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );

        // SessionStart hook records session_id and refreshes
        // metadata. State stays Starting (classification already
        // put the session there), so no StateChanged effect fires
        // — equal-state transitions are idempotent by design.
        let envelope_ss = envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": "sess-1",
                "transcript_path": "/t.jsonl",
                "cwd": "/repo",
                "source": "startup",
                "model": "claude-opus-4-7",
                "permission_mode": "default",
                "agent_id": "test",
                "plugin_version": "1.0.0",
            }),
        );
        let _effects = core.handle(
            SessionInput::HookEnvelopeParsed {
                envelope: envelope_ss,
            },
            t0,
        );

        let session = core.session_by_session_id("sess-1").unwrap();
        assert_eq!(session.session_id.as_deref(), Some("sess-1"));
        assert_eq!(session.state, CliAgentSessionState::Starting);
        assert_eq!(
            session.transcript_path.as_deref(),
            Some(Path::new("/t.jsonl"))
        );
        assert_eq!(session.model.as_deref(), Some("claude-opus-4-7"));

        // A follow-up UserPromptSubmit *does* change state, so it
        // should emit StateChanged.
        let ups = envelope(
            "UserPromptSubmit",
            serde_json::json!({"session_id": "sess-1", "prompt": "hello"}),
        );
        let effects = core.handle(SessionInput::HookEnvelopeParsed { envelope: ups }, t0);
        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::EmitEvent {
                event: ManagerEvent::StateChanged {
                    old: CliAgentSessionState::Starting,
                    new: CliAgentSessionState::Working { .. },
                },
                ..
            }
        )));
    }

    #[test]
    fn hook_cwd_match_routes_pre_session_id_events() {
        // Simulate the case where a hook envelope arrives *before*
        // process-tree detection had a chance to link the session
        // by id. The CWD should still route it.
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );

        // No session_id yet on the session record. Fire a
        // CwdChanged with a session_id the core has never seen —
        // the matcher falls back to CWD.
        let new_cwd = envelope(
            "CwdChanged",
            serde_json::json!({"session_id": "first-hook", "cwd": "/repo"}),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: new_cwd }, t0);

        let session = core
            .session_by_session_id("first-hook")
            .expect("session_id now mapped by cwd-matcher");
        assert_eq!(session.session_id.as_deref(), Some("first-hook"));
    }

    #[test]
    fn hook_envelope_without_routable_session_is_dropped() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        let unroutable = envelope(
            "Stop",
            serde_json::json!({"session_id": "nowhere", "last_assistant_message": ""}),
        );
        let effects = core.handle(
            SessionInput::HookEnvelopeParsed {
                envelope: unroutable,
            },
            t0,
        );
        assert!(
            effects.is_empty(),
            "unroutable hook must not produce effects"
        );
    }

    #[test]
    fn drop_ttl_expired_removes_session_and_cleans_indexes() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        let ss = envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": "sess-1",
                "transcript_path": "/t.jsonl",
                "cwd": "/repo",
                "source": "startup",
                "model": "m",
                "permission_mode": "default",
                "agent_id": "test",
                "plugin_version": "1.0.0",
            }),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: ss }, t0);

        // Agent exits.
        let t1 = t0 + Duration::from_secs(10);
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: None,
            },
            t1,
        );
        assert!(
            core.session_by_session_id("sess-1").is_some(),
            "session still present during TTL window"
        );

        // TTL fires.
        let key = core.active_session_keys()[0];
        let t2 = t1 + SESSION_DROP_TTL + Duration::from_secs(1);
        let effects = core.handle(SessionInput::DropTtlExpired { key }, t2);

        assert!(matches!(
            effects.last(),
            Some(SessionEffect::DropSession { .. })
        ));
        assert!(core.session_by_session_id("sess-1").is_none());
        assert_eq!(core.sessions_count(), 0);
        assert!(core.terminal(100).unwrap().session.is_none());
    }

    #[test]
    fn drop_ttl_expired_before_ttl_is_noop() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: None,
            },
            t0,
        );
        let key = core.active_session_keys()[0];
        let effects = core.handle(
            SessionInput::DropTtlExpired { key },
            t0 + Duration::from_secs(1),
        );
        assert!(effects.is_empty(), "early TTL must not drop the session");
        assert!(core.session(key).is_some());
    }

    #[test]
    fn unregister_terminal_drops_session_immediately() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        let key_before = core.active_session_keys()[0];
        let effects = core.handle(SessionInput::UnregisterTerminal { pty_pid: 100 }, t0);

        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::EmitEvent {
                event: ManagerEvent::SessionEnded { .. },
                ..
            }
        )));
        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::DropSession { key } if *key == key_before
        )));
        assert_eq!(core.sessions_count(), 0);
        assert_eq!(core.terminals_count(), 0);
    }

    #[test]
    fn pane_focused_resets_unread_counter() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        // Establish session_id mapping via SessionStart first —
        // otherwise UserPromptSubmit would be unroutable.
        let ss = envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": "sess",
                "transcript_path": "/t.jsonl",
                "cwd": "/repo",
                "source": "startup",
                "model": "m",
                "permission_mode": "default",
                "agent_id": "test",
                "plugin_version": "1.0.0",
            }),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: ss }, t0);

        let ups = envelope(
            "UserPromptSubmit",
            serde_json::json!({"session_id": "sess", "prompt": "hi"}),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: ups }, t0);
        assert!(core.session_for_pane(1).unwrap().unread_events_since_focus >= 1);

        let effects = core.handle(SessionInput::PaneFocused { pane_id: 1 }, t0);
        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::EmitEvent {
                event: ManagerEvent::UnreadReset,
                ..
            }
        )));
        assert_eq!(
            core.session_for_pane(1).unwrap().unread_events_since_focus,
            0
        );
    }

    #[test]
    fn pane_focused_without_session_is_noop() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        // No classification yet, so no session.
        let effects = core.handle(SessionInput::PaneFocused { pane_id: 1 }, t0);
        assert!(effects.is_empty());
    }

    #[test]
    fn session_end_hook_schedules_drop() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        let ss = envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": "sess-1",
                "transcript_path": "/t.jsonl",
                "cwd": "/repo",
                "source": "startup",
                "model": "m",
                "permission_mode": "default",
                "agent_id": "test",
                "plugin_version": "1.0.0",
            }),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: ss }, t0);

        let se = envelope(
            "SessionEnd",
            serde_json::json!({"session_id": "sess-1", "exit_code": 0}),
        );
        let effects = core.handle(SessionInput::HookEnvelopeParsed { envelope: se }, t0);

        assert!(effects.iter().any(|e| matches!(
            e,
            SessionEffect::EmitEvent {
                event: ManagerEvent::SessionEnded { exit_code: Some(0) },
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, SessionEffect::ScheduleDropTtl { .. }))
        );
    }

    #[test]
    fn session_id_index_rebuilds_from_empty() {
        // Compile-time-ish guard for a future refactor.
        let core = SessionCore::new();
        assert!(core.session_by_session_id("x").is_none());
        let stub = stub_envelope_json();
        assert_eq!(stub.event, "Stop");
        core.assert_invariants();
    }

    #[test]
    fn hook_increments_unread_counter() {
        let mut core = SessionCore::new();
        let t0 = Instant::now();
        core.handle(
            SessionInput::RegisterTerminal {
                pty_pid: 100,
                pane_id: 1,
                cwd: PathBuf::from("/repo"),
            },
            t0,
        );
        core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid: 100,
                classification: Some(test_classification(42, "/repo")),
            },
            t0,
        );
        // Establish session_id mapping first.
        let ss = envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": "s",
                "transcript_path": "/t.jsonl",
                "cwd": "/repo",
                "source": "startup",
                "model": "m",
                "permission_mode": "default",
                "agent_id": "test",
                "plugin_version": "1.0.0",
            }),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: ss }, t0);
        let before_ups = core.session_for_pane(1).unwrap().unread_events_since_focus;

        let e1 = envelope(
            "UserPromptSubmit",
            serde_json::json!({"session_id": "s", "prompt": "a"}),
        );
        let e2 = envelope(
            "UserPromptSubmit",
            serde_json::json!({"session_id": "s", "prompt": "b"}),
        );
        core.handle(SessionInput::HookEnvelopeParsed { envelope: e1 }, t0);
        core.handle(SessionInput::HookEnvelopeParsed { envelope: e2 }, t0);

        // Each UPS increments unread by 1 on top of the SessionStart
        // we already counted.
        assert_eq!(
            core.session_for_pane(1).unwrap().unread_events_since_focus,
            before_ups + 2
        );
    }

    #[test]
    fn allocate_session_key_is_monotonic() {
        let mut core = SessionCore::new();
        assert_eq!(core.allocate_session_key(), 1);
        assert_eq!(core.allocate_session_key(), 2);
        assert_eq!(core.allocate_session_key(), 3);
    }

    #[test]
    fn permission_mode_from_flag_matches_session_mirror() {
        // Contract between detection's PermissionModeFlag and the
        // session's PermissionMode. If this ever drifts, the
        // classification path would silently store Unknown.
        let flag = PermissionModeFlag("acceptEdits".into());
        assert_eq!(PermissionMode::from(&flag), PermissionMode::AcceptEdits);
    }

    #[test]
    fn manager_event_round_trips_to_cli_agent_event_except_unread() {
        assert!(ManagerEvent::UnreadReset.to_cli_agent_event().is_none());
        assert!(ManagerEvent::SessionStarted.to_cli_agent_event().is_some());
    }
}
