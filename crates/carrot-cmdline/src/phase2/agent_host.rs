//! Agent-edit APIs.
//!
//! Carrot is an ADE; the cmdline is an explicit handoff surface
//! between human and agent. The [`CmdlineAgentHost`] trait defines
//! the four operations:
//!
//! ```text
//! fn propose(&self, text: &str, reason: Option<&str>);
//! fn watch(&self, on_change: Box<dyn Fn(&str)>);
//! fn current_text(&self) -> String;
//! fn current_ast(&self) -> CommandAst;
//! ```
//!
//! This module owns the trait plus an in-memory reference
//! implementation ([`InMemoryAgentHost`]) that the cmdline uses by
//! default. Real agent integration wraps its own handle around the
//! same trait signature — no call-site changes when the handler
//! swap happens.
//!
//! # Permission boundary
//!
//! Every accessor goes through [`AgentPermission`]. The default is
//! `Denied` and the host returns empty strings / empty ASTs until
//! the user explicitly grants permission for the session. That
//! invariant is verified by negative tests — the agent cannot
//! read `current_text` without the flag set.

use std::sync::Mutex;

use crate::ast::CommandAst;

/// Per-session permission the user grants to the agent. Default is
/// `Denied`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPermission {
    /// Agent cannot read the buffer or propose edits.
    #[default]
    Denied,
    /// Agent can read `current_text` + `current_ast` but cannot
    /// propose edits.
    ReadOnly,
    /// Agent can read and propose edits. `propose()` presents the
    /// text as a pending fill for the user to accept / modify /
    /// reject.
    ReadWrite,
}

impl AgentPermission {
    pub fn allows_read(self) -> bool {
        matches!(self, AgentPermission::ReadOnly | AgentPermission::ReadWrite)
    }

    pub fn allows_write(self) -> bool {
        matches!(self, AgentPermission::ReadWrite)
    }
}

/// Shape of an agent-proposed edit sitting in the buffer as a
/// pending fill. The user accepts with Enter, modifies inline, or
/// rejects with Escape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProposal {
    pub text: String,
    pub reason: Option<String>,
}

/// Host trait every implementation fulfils.
pub trait CmdlineAgentHost {
    /// Propose a candidate for the cmdline buffer. No-op when the
    /// session's permission is lower than `ReadWrite`.
    fn propose(&self, text: &str, reason: Option<&str>);

    /// Register a callback that fires whenever the buffer text
    /// changes. Ignored when the session permission is `Denied`.
    fn watch(&self, on_change: Box<dyn Fn(&str) + Send + Sync>);

    /// Current buffer text as the agent sees it. Returns `""` when
    /// the permission is `Denied` — the agent does not get a
    /// partial read.
    fn current_text(&self) -> String;

    /// Current semantic AST. Empty AST when permission is `Denied`.
    fn current_ast(&self) -> CommandAst;
}

/// In-memory reference implementation. Wraps a plain String + the
/// derived AST plus a permission flag and a list of watchers.
///
/// Tests + the fallback path use this; the real carrot-agent host
/// wraps a live editor handle behind the same trait without
/// changing the caller's code.
pub struct InMemoryAgentHost {
    state: Mutex<InMemoryState>,
}

struct InMemoryState {
    permission: AgentPermission,
    text: String,
    ast: CommandAst,
    pending: Option<PendingProposal>,
    watchers: Vec<Box<dyn Fn(&str) + Send + Sync>>,
}

impl InMemoryAgentHost {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(InMemoryState {
                permission: AgentPermission::default(),
                text: String::new(),
                ast: CommandAst::empty(),
                pending: None,
                watchers: Vec::new(),
            }),
        }
    }

    /// Update the underlying buffer + derived AST. Also notifies
    /// registered watchers.
    pub fn set_buffer(&self, text: impl Into<String>) {
        let text = text.into();
        let ast = crate::parse::parse_simple(&text);
        let mut state = self.state.lock().expect("agent host state");
        state.text = text.clone();
        state.ast = ast;
        let watchers_snapshot: Vec<_> = state.watchers.iter().map(|_| ()).collect();
        drop(watchers_snapshot);
        // Fire watchers with the mutex released so the callback can
        // mutate the host without deadlocking.
        let text_for_watchers = text.clone();
        for watcher in &state.watchers {
            watcher(&text_for_watchers);
        }
    }

    /// Grant or revoke agent permission for this session.
    pub fn set_permission(&self, permission: AgentPermission) {
        self.state.lock().expect("agent host state").permission = permission;
    }

    /// Current permission.
    pub fn permission(&self) -> AgentPermission {
        self.state.lock().expect("agent host state").permission
    }

    /// Pending agent proposal, if any.
    pub fn pending_proposal(&self) -> Option<PendingProposal> {
        self.state.lock().expect("agent host state").pending.clone()
    }

    /// Clear the pending proposal. Used when the user accepts, modifies,
    /// or rejects the proposal.
    pub fn clear_pending(&self) {
        self.state.lock().expect("agent host state").pending = None;
    }
}

impl Default for InMemoryAgentHost {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdlineAgentHost for InMemoryAgentHost {
    fn propose(&self, text: &str, reason: Option<&str>) {
        let mut state = self.state.lock().expect("agent host state");
        if !state.permission.allows_write() {
            return;
        }
        state.pending = Some(PendingProposal {
            text: text.to_string(),
            reason: reason.map(str::to_string),
        });
    }

    fn watch(&self, on_change: Box<dyn Fn(&str) + Send + Sync>) {
        let mut state = self.state.lock().expect("agent host state");
        if !matches!(state.permission, AgentPermission::Denied) {
            state.watchers.push(on_change);
        }
    }

    fn current_text(&self) -> String {
        let state = self.state.lock().expect("agent host state");
        if state.permission.allows_read() {
            state.text.clone()
        } else {
            String::new()
        }
    }

    fn current_ast(&self) -> CommandAst {
        let state = self.state.lock().expect("agent host state");
        if state.permission.allows_read() {
            state.ast.clone()
        } else {
            CommandAst::empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn default_permission_is_denied() {
        let h = InMemoryAgentHost::new();
        assert_eq!(h.permission(), AgentPermission::Denied);
        assert!(!AgentPermission::Denied.allows_read());
        assert!(!AgentPermission::Denied.allows_write());
    }

    #[test]
    fn read_permission_allows_read_only() {
        assert!(AgentPermission::ReadOnly.allows_read());
        assert!(!AgentPermission::ReadOnly.allows_write());
    }

    #[test]
    fn readwrite_permission_allows_both() {
        assert!(AgentPermission::ReadWrite.allows_read());
        assert!(AgentPermission::ReadWrite.allows_write());
    }

    #[test]
    fn denied_host_hides_current_text() {
        let h = InMemoryAgentHost::new();
        h.set_buffer("super secret password");
        // Permission is default-denied.
        assert_eq!(h.current_text(), "");
        assert!(!h.current_ast().has_command());
    }

    #[test]
    fn readonly_host_returns_text() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadOnly);
        h.set_buffer("git status");
        assert_eq!(h.current_text(), "git status");
        assert!(h.current_ast().has_command());
    }

    #[test]
    fn propose_ignored_without_write_permission() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadOnly);
        h.propose("git checkout main", Some("switch branch"));
        assert!(h.pending_proposal().is_none());
    }

    #[test]
    fn propose_stored_under_readwrite() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadWrite);
        h.propose("git checkout main", Some("switch branch"));
        let pending = h.pending_proposal().unwrap();
        assert_eq!(pending.text, "git checkout main");
        assert_eq!(pending.reason.as_deref(), Some("switch branch"));
    }

    #[test]
    fn clear_pending_drops_the_proposal() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadWrite);
        h.propose("ls", None);
        assert!(h.pending_proposal().is_some());
        h.clear_pending();
        assert!(h.pending_proposal().is_none());
    }

    #[test]
    fn watch_ignored_when_denied() {
        let h = InMemoryAgentHost::new();
        // Default Denied.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        h.watch(Box::new(move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        }));
        h.set_buffer("hello");
        // Denied → watcher was never registered → counter stays 0.
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn watch_fires_on_buffer_change_when_readonly() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadOnly);
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        h.watch(Box::new(move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        }));
        h.set_buffer("a");
        h.set_buffer("ab");
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn propose_carries_optional_reason() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadWrite);
        h.propose("ls", None);
        assert!(h.pending_proposal().unwrap().reason.is_none());
    }

    #[test]
    fn set_permission_is_reversible() {
        let h = InMemoryAgentHost::new();
        h.set_permission(AgentPermission::ReadWrite);
        h.set_buffer("visible");
        assert_eq!(h.current_text(), "visible");
        h.set_permission(AgentPermission::Denied);
        assert_eq!(h.current_text(), "");
    }
}
