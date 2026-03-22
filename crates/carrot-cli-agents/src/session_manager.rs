//! Inazuma-side effect runner for [`SessionCore`].
//!
//! The FSM and all session-lifecycle logic lives in
//! [`crate::session_core`]; this module contains the glue that:
//!
//!   1. Materialises the [`SessionEffect`] stream into real Inazuma
//!      side effects — watcher entities, subscriptions, timers, and
//!      event emission.
//!   2. Bridges the two external event sources (child-process
//!      watchers and OSC 7777 hook envelopes) into the FSM's
//!      [`SessionInput`] stream.
//!
//! Consumers subscribe to this entity to receive
//! [`ManagerEvent`]s; individual session data is read via
//! [`CliAgentSessionManager::session_for_pane`] and friends, which
//! forward to the [`SessionCore`] snapshot.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use inazuma::{App, AsyncApp, Context, Entity, EventEmitter, Global, Subscription, WeakEntity};

use crate::agent::SharedCliAgent;
use crate::child_process_watcher::{
    ChildProcessWatcher, ChildProcessesDetected, WatcherMode, spawn_watcher,
};
use crate::detection::classify_processes;
use crate::hook_events::{EnvelopeError, parse_envelope};
use crate::registry::CliAgentRegistry;
use crate::session::CliAgentSession;
use crate::session_core::{
    Classification, ManagerEvent, PaneId, PtyPid, SessionCore, SessionEffect, SessionInput,
    SessionKey,
};

pub struct CliAgentSessionManager {
    core: SessionCore,
    /// One watcher entity per registered PTY. The runner holds the
    /// strong reference so the entity (and its poll task) live as
    /// long as the terminal is tracked.
    watchers: HashMap<PtyPid, Entity<ChildProcessWatcher>>,
    /// Subscriptions the runner owns for incoming watcher events.
    _subscriptions: Vec<Subscription>,
}

impl CliAgentSessionManager {
    /// Build an empty manager. Called by `cli_agents::init`.
    pub fn new() -> Self {
        Self {
            core: SessionCore::new(),
            watchers: HashMap::new(),
            _subscriptions: Vec::new(),
        }
    }

    // ---------- Read-side API (pure forwards) ----------

    pub fn session_for_pane(&self, pane_id: PaneId) -> Option<&CliAgentSession> {
        self.core.session_for_pane(pane_id)
    }

    pub fn session_by_session_id(&self, session_id: &str) -> Option<&CliAgentSession> {
        self.core.session_by_session_id(session_id)
    }

    pub fn session(&self, key: SessionKey) -> Option<&CliAgentSession> {
        self.core.session(key)
    }

    pub fn active_session_keys(&self) -> Vec<SessionKey> {
        self.core.active_session_keys()
    }

    pub fn terminal(&self, pty_pid: PtyPid) -> Option<&crate::session_core::TerminalRecord> {
        self.core.terminal(pty_pid)
    }

    pub fn watcher_for_pty(&self, pty_pid: PtyPid) -> Option<Entity<ChildProcessWatcher>> {
        self.watchers.get(&pty_pid).cloned()
    }

    // ---------- Write-side API (forwards to the FSM) ----------

    /// Start tracking a terminal. The `manager_entity` weak handle
    /// is used to keep the subscription callbacks self-contained;
    /// callers pass their own weak entity so the runner does not
    /// have to store a reference to itself.
    pub fn register_terminal(
        &mut self,
        manager: WeakEntity<Self>,
        pty_pid: PtyPid,
        pane_id: PaneId,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let effects = self.core.handle(
            SessionInput::RegisterTerminal {
                pty_pid,
                pane_id,
                cwd,
            },
            Instant::now(),
        );
        self.apply_effects(manager, effects, cx);
    }

    pub fn unregister_terminal(
        &mut self,
        manager: WeakEntity<Self>,
        pty_pid: PtyPid,
        cx: &mut Context<Self>,
    ) {
        let effects = self
            .core
            .handle(SessionInput::UnregisterTerminal { pty_pid }, Instant::now());
        self.apply_effects(manager, effects, cx);
    }

    /// Parse and dispatch a raw OSC 7777 agent-event envelope as
    /// emitted by the terminal scanner. Invalid envelopes are
    /// logged and dropped.
    pub fn handle_hook_envelope(
        &mut self,
        manager: WeakEntity<Self>,
        json: &str,
        cx: &mut Context<Self>,
    ) {
        let parsed = match parse_envelope(json) {
            Ok(p) => p,
            Err(EnvelopeError::WrongType(_)) => return,
            Err(err) => {
                log::debug!("dropped malformed cli_agent_event envelope: {err}");
                return;
            }
        };
        let effects = self.core.handle(
            SessionInput::HookEnvelopeParsed { envelope: parsed },
            Instant::now(),
        );
        self.apply_effects(manager, effects, cx);
    }

    pub fn focus_pane(
        &mut self,
        manager: WeakEntity<Self>,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) {
        let effects = self
            .core
            .handle(SessionInput::PaneFocused { pane_id }, Instant::now());
        self.apply_effects(manager, effects, cx);
    }

    // ---------- Effect application ----------

    fn apply_effects(
        &mut self,
        manager: WeakEntity<Self>,
        effects: Vec<SessionEffect>,
        cx: &mut Context<Self>,
    ) {
        for effect in effects {
            self.apply_effect(manager.clone(), effect, cx);
        }
    }

    fn apply_effect(
        &mut self,
        manager: WeakEntity<Self>,
        effect: SessionEffect,
        cx: &mut Context<Self>,
    ) {
        match effect {
            SessionEffect::StartWatcher { pty_pid } => {
                self.start_watcher(manager, pty_pid, cx);
            }
            SessionEffect::StopWatcher { pty_pid } => {
                self.watchers.remove(&pty_pid);
            }
            SessionEffect::SetWatcherMode { pty_pid, mode } => {
                if let Some(watcher) = self.watchers.get(&pty_pid) {
                    watcher.update(cx, |w, _| match mode {
                        WatcherMode::Warm => w.notify_agent_active(),
                        WatcherMode::Hot => w.notify_agent_gone(),
                        WatcherMode::Cold => w.notify_agent_gone(),
                    });
                }
            }
            SessionEffect::EmitEvent { key, event } => {
                cx.emit(ManagerEventEnvelope { key, event });
            }
            SessionEffect::ScheduleDropTtl { key, at } => {
                self.schedule_drop(manager, key, at, cx);
            }
            SessionEffect::DropSession { key } => {
                // Nothing for the runner to clean up — the core
                // already removed the record. Emitting a final
                // SessionDropped event keeps consumers in sync.
                cx.emit(ManagerEventEnvelope {
                    key,
                    event: ManagerEvent::SessionEnded { exit_code: None },
                });
            }
        }
    }

    fn start_watcher(
        &mut self,
        _manager: WeakEntity<Self>,
        pty_pid: PtyPid,
        cx: &mut Context<Self>,
    ) {
        if self.watchers.contains_key(&pty_pid) {
            return;
        }
        let watcher = spawn_watcher(pty_pid, cx);
        // Subscribe from `self`'s context. The closure receives a
        // `&mut Self` (the manager), the watcher entity that
        // emitted, the event, and a `Context<Self>` for further
        // work.
        let sub = cx.subscribe(
            &watcher,
            |this, watcher, event: &ChildProcessesDetected, cx| {
                let pty = watcher.read(cx).pty_pid();
                let weak = cx.weak_entity();
                this.on_watcher_event(weak, pty, &event.children, cx);
            },
        );
        self._subscriptions.push(sub);
        self.watchers.insert(pty_pid, watcher);
    }

    fn on_watcher_event(
        &mut self,
        manager: WeakEntity<Self>,
        pty_pid: PtyPid,
        children: &[crate::agent::ProcessInfo],
        cx: &mut Context<Self>,
    ) {
        // Classify here where we have access to the App/registry,
        // then hand a pre-classified input to the core.
        let registry = cx.global::<CliAgentRegistry>();
        let classification = classify_processes(children, registry, cx).map(|(agent, process)| {
            let cmdline_match = match_cmdline(&agent, &process, cx);
            Classification {
                agent,
                process,
                cmdline_match,
            }
        });

        let effects = self.core.handle(
            SessionInput::ChildProcessesDetected {
                pty_pid,
                classification,
            },
            Instant::now(),
        );
        self.apply_effects(manager, effects, cx);
    }

    fn schedule_drop(
        &mut self,
        _manager: WeakEntity<Self>,
        key: SessionKey,
        at: Instant,
        cx: &mut Context<Self>,
    ) {
        let delay = at.saturating_duration_since(Instant::now());
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                let weak = cx.weak_entity();
                let effects = this
                    .core
                    .handle(SessionInput::DropTtlExpired { key }, Instant::now());
                this.apply_effects(weak, effects, cx);
            })
            .ok();
        })
        .detach();
    }
}

impl Default for CliAgentSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrapper that pairs a [`ManagerEvent`] with the [`SessionKey`] it
/// refers to. The runner emits this; subscribers match on both
/// fields.
#[derive(Debug, Clone)]
pub struct ManagerEventEnvelope {
    pub key: SessionKey,
    pub event: ManagerEvent,
}

impl EventEmitter<ManagerEventEnvelope> for CliAgentSessionManager {}

/// Global wrapper for the session-manager entity.
///
/// Consumers access it via `cx.global::<GlobalCliAgentSessionManager>().0.read(cx)` (read) or `.0.update(cx, |m, cx| …)` (write). Kept as a
/// separate newtype so other globals that hold Inazuma entities
/// follow the same pattern.
pub struct GlobalCliAgentSessionManager(pub Entity<CliAgentSessionManager>);

impl Global for GlobalCliAgentSessionManager {}

// ---------- Integration helpers ----------
//
// The three helpers below hide the global-lookup + weak-entity dance
// from integration call-sites. They are the only functions the rest
// of the app should use when wiring terminals, hooks, and focus
// events into the manager. Using them instead of hand-rolling the
// lookup keeps every wiring point one line long and makes the
// integration surface auditable.

/// Register a terminal PTY with the session manager. Spawns a
/// watcher under the hood. Safe to call on any platform; if the
/// manager global is not installed yet (e.g. during very early
/// startup or in tests) the call is a no-op and a debug log is
/// emitted — the terminal simply will not be tracked, which is
/// better than panicking.
pub fn register_terminal(pty_pid: PtyPid, pane_id: PaneId, cwd: std::path::PathBuf, cx: &mut App) {
    let Some(wrapper) = cx.try_global::<GlobalCliAgentSessionManager>() else {
        log::debug!(
            "CliAgentSessionManager global not installed; skipping register_terminal(pty={}, pane={})",
            pty_pid,
            pane_id
        );
        return;
    };
    let manager = wrapper.0.clone();
    manager.update(cx, |m, cx| {
        let weak = cx.weak_entity();
        m.register_terminal(weak, pty_pid, pane_id, cwd, cx);
    });
}

/// Unregister a terminal PTY. Called from the terminal pane's
/// `cx.on_release` hook when the pane is dropped.
pub fn unregister_terminal(pty_pid: PtyPid, cx: &mut App) {
    let Some(wrapper) = cx.try_global::<GlobalCliAgentSessionManager>() else {
        return;
    };
    let manager = wrapper.0.clone();
    manager.update(cx, |m, cx| {
        let weak = cx.weak_entity();
        m.unregister_terminal(weak, pty_pid, cx);
    });
}

/// Forward a raw OSC 7777 agent-event envelope into the manager.
/// The envelope JSON comes from the terminal's OSC scanner as
/// `ShellMarker::AgentEvent(String)`.
pub fn forward_hook_envelope(json: &str, cx: &mut App) {
    let Some(wrapper) = cx.try_global::<GlobalCliAgentSessionManager>() else {
        return;
    };
    let manager = wrapper.0.clone();
    manager.update(cx, |m, cx| {
        let weak = cx.weak_entity();
        m.handle_hook_envelope(weak, json, cx);
    });
}

/// Notify the manager that a pane received focus. Resets its
/// session's unread-event counter so the Vertical-Tabs unread dot
/// disappears.
pub fn focus_pane(pane_id: PaneId, cx: &mut App) {
    let Some(wrapper) = cx.try_global::<GlobalCliAgentSessionManager>() else {
        return;
    };
    let manager = wrapper.0.clone();
    manager.update(cx, |m, cx| {
        let weak = cx.weak_entity();
        m.focus_pane(weak, pane_id, cx);
    });
}

fn match_cmdline(
    agent: &SharedCliAgent,
    process: &crate::agent::ProcessInfo,
    cx: &App,
) -> crate::agent::CliAgentMatch {
    // `classify` is guaranteed non-None here (the registry already
    // matched this agent against the process). Unwrap falling back
    // to an empty CliAgentMatch keeps the failure surface tight —
    // if an agent ever implements `classify` to return None after a
    // registry-level match, the session still gets created with
    // default metadata rather than disappearing silently.
    agent
        .classify(process, cx)
        .unwrap_or_else(crate::agent::CliAgentMatch::default)
}

// The `Global` impl lets the manager be stored as an `Entity<Self>`
// via `cx.global::<Entity<CliAgentSessionManager>>()` style use,
// but our canonical access is through the initializer in
// `cli_agents::init` which stores the entity and then reads it on
// demand. No `Global` here — consumers hold a weak entity instead.

// Silence: `Arc` is used via `SharedCliAgent` through the core path
// but re-checked by the compiler in `match_cmdline`. This line keeps
// the static import used in debug builds.
const _: fn() = || {
    let _: Option<Arc<()>> = None;
};
