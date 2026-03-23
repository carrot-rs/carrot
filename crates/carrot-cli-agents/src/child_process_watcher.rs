//! Poll a PTY's descendant process tree and emit an event when it
//! changes.
//!
//! Runs as its own Inazuma entity so the polling loop stays cleanly
//! isolated from the session manager's event handling. One watcher
//! per terminal PTY; when the PTY exits, the watcher is dropped and
//! its background task falls out of scope.
//!
//! ## Cadence strategy
//!
//! Polling the OS process table is cheap but not free; we pick a
//! cadence based on what we last observed:
//!
//!   * **Hot (500 ms):** we have not yet classified any agent under
//!     this PTY. We poll aggressively so the first `claude` launch
//!     appears in the UI within half a second.
//!   * **Warm (2 s):** an agent is registered and its plugin is
//!     sending hook events. Hooks are the authoritative state
//!     source; the poll is a belt-and-braces safety net.
//!   * **Cold (5 s):** the PTY has been idle for a long time and no
//!     agent is in flight. Mostly keeps us alive for belated
//!     launches.
//!
//! Mode transitions happen inline in the loop based on
//! `notify_agent_active` / `notify_agent_gone` calls from the
//! session manager.

use std::collections::HashSet;
use std::time::Duration;

use inazuma::{AppContext, Context, Entity, EventEmitter, Task};

use crate::agent::ProcessInfo;
use crate::detection::scan_pty_descendants;

const HOT_POLL: Duration = Duration::from_millis(500);
const WARM_POLL: Duration = Duration::from_secs(2);
const COLD_POLL: Duration = Duration::from_secs(5);

/// How long the watcher sits in `Hot` before falling back to `Cold`
/// when no agent was ever classified. Keeps CPU low on long-lived
/// shells where the user never launches an agent.
const HOT_TO_COLD_AFTER: Duration = Duration::from_secs(30);

/// Watcher polling mode. Mode-to-interval mapping lives on the enum
/// rather than sprinkled through the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherMode {
    Hot,
    Warm,
    Cold,
}

impl WatcherMode {
    fn interval(self) -> Duration {
        match self {
            Self::Hot => HOT_POLL,
            Self::Warm => WARM_POLL,
            Self::Cold => COLD_POLL,
        }
    }
}

/// Event emitted when the set of descendant PIDs changes. The payload
/// carries every currently-alive descendant process; subscribers
/// compare against their own last-known set to detect added/removed
/// entries.
#[derive(Debug, Clone)]
pub struct ChildProcessesDetected {
    pub children: Vec<ProcessInfo>,
}

/// Watcher entity. Owns exactly one background poll task that stops
/// when the entity drops.
pub struct ChildProcessWatcher {
    pty_pid: u32,
    mode: WatcherMode,
    last_pids: HashSet<u32>,
    polls_in_current_mode: u32,
    _task: Task<()>,
}

impl ChildProcessWatcher {
    /// Create and start a watcher for the given PTY root pid. The
    /// polling task is kicked off immediately; callers typically
    /// subscribe to `ChildProcessesDetected` via
    /// `cx.subscribe(&watcher, …)` right after spawn.
    pub fn new(pty_pid: u32, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| {
            // Small initial delay lets the shell spawn fully before
            // our first poll — avoids a noise event reporting only
            // the shell itself.
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;

            loop {
                let mode = this.update(cx, |this, cx| this.tick(cx)).ok().flatten();
                let Some(mode) = mode else {
                    // Entity dropped; exit loop.
                    break;
                };
                cx.background_executor().timer(mode.interval()).await;
            }
        });

        Self {
            pty_pid,
            mode: WatcherMode::Hot,
            last_pids: HashSet::new(),
            polls_in_current_mode: 0,
            _task: task,
        }
    }

    /// One poll. Returns the next mode (which determines how long
    /// the outer loop sleeps) or `None` when the caller should stop.
    /// Public for unit tests; production callers go through the
    /// background task set up in `new`.
    pub fn tick(&mut self, cx: &mut Context<Self>) -> Option<WatcherMode> {
        let children = scan_pty_descendants(self.pty_pid);
        let pid_set: HashSet<u32> = children.iter().map(|p| p.pid).collect();

        if pid_set != self.last_pids {
            self.last_pids = pid_set;
            cx.emit(ChildProcessesDetected {
                children: children.clone(),
            });
        }

        // Auto-demote Hot → Cold after HOT_TO_COLD_AFTER with no
        // upgrade to Warm. Warm is only reached via
        // `notify_agent_active`, so a Hot watcher that never hears
        // about an agent is wasting CPU.
        self.polls_in_current_mode = self.polls_in_current_mode.saturating_add(1);
        if self.mode == WatcherMode::Hot {
            let elapsed = self.mode.interval() * self.polls_in_current_mode;
            if elapsed >= HOT_TO_COLD_AFTER {
                self.mode = WatcherMode::Cold;
                self.polls_in_current_mode = 0;
            }
        }

        Some(self.mode)
    }

    /// Called by the session manager when it classifies an agent
    /// process under this PTY. Switches to `Warm` cadence because
    /// hooks become the primary signal from now on.
    pub fn notify_agent_active(&mut self) {
        self.mode = WatcherMode::Warm;
        self.polls_in_current_mode = 0;
    }

    /// Called by the session manager when every agent under this
    /// PTY has exited. Returns to `Hot` so the next launch is
    /// discovered quickly.
    pub fn notify_agent_gone(&mut self) {
        self.mode = WatcherMode::Hot;
        self.polls_in_current_mode = 0;
    }

    pub fn pty_pid(&self) -> u32 {
        self.pty_pid
    }

    pub fn mode(&self) -> WatcherMode {
        self.mode
    }

    pub fn last_children_count(&self) -> usize {
        self.last_pids.len()
    }
}

impl EventEmitter<ChildProcessesDetected> for ChildProcessWatcher {}

/// Helper for the session manager: spawn a watcher entity and return
/// its handle. Kept as a free fn rather than an `impl` method so the
/// manager does not need to know about `Context<ChildProcessWatcher>`.
pub fn spawn_watcher(pty_pid: u32, cx: &mut inazuma::App) -> Entity<ChildProcessWatcher> {
    cx.new(|cx| ChildProcessWatcher::new(pty_pid, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_interval_matches_constants() {
        assert_eq!(WatcherMode::Hot.interval(), HOT_POLL);
        assert_eq!(WatcherMode::Warm.interval(), WARM_POLL);
        assert_eq!(WatcherMode::Cold.interval(), COLD_POLL);
    }

    #[test]
    fn intervals_are_strictly_ordered() {
        // Hot must poll fastest, Cold slowest. Flipping any pair
        // would defeat the cadence strategy.
        assert!(HOT_POLL < WARM_POLL);
        assert!(WARM_POLL < COLD_POLL);
    }

    #[test]
    fn hot_to_cold_timeout_is_meaningful() {
        // Must allow at least a few Hot polls; otherwise we demote
        // so fast the user never sees responsive first-detection.
        assert!(HOT_TO_COLD_AFTER >= HOT_POLL * 10);
    }
}
