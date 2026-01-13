//! MCP-native completion source.
//!
//! Any MCP-connected tool can register completion providers by
//! glob pattern (`git *`, `kubectl *`, `npm run *`). Providers
//! receive completion requests over MCP and return typed
//! candidates.
//!
//! This module owns the **glob-dispatch** contract between
//! `carrot-cmdline` and the MCP bridge in `carrot-terminal`. When
//! the user types a command whose first token matches a registered
//! glob, the driver consults the matching provider for completions.
//!
//! The MCP transport (JSON-RPC over stdio) lives in the extension
//! host. This module only owns:
//!
//! - The glob-matching algorithm (`*` is a single-level wildcard).
//! - The [`McpProvider`] trait that the bridge implements.
//! - An in-process [`McpRegistry`] that the driver calls into.
//!
//! When the real `carrot-mcp` / `carrot-context-server` bridge lands,
//! its provider wrapper implements [`McpProvider`] and registers
//! with the same registry. No call-site changes.

use crate::ast::Range;
use crate::completion::{CompletionCandidate, CompletionSource};

/// A provider the registry dispatches to when the user's command
/// matches `pattern`.
pub trait McpProvider: Send + Sync {
    /// Human-readable name (for debug / settings UI).
    fn name(&self) -> &str;

    /// Return completion candidates for `command_line`. Called only
    /// after the pattern matched, so the provider can assume it's
    /// the right backend for this command.
    fn complete(&self, command_line: &str, anchor: Range) -> Vec<CompletionCandidate>;
}

/// Registration entry — glob pattern paired with the provider.
pub struct McpRegistration {
    pub pattern: String,
    pub provider: Box<dyn McpProvider>,
}

/// Registry of MCP completion providers. The cmdline driver keeps
/// a single instance and consults it for every completion request.
#[derive(Default)]
pub struct McpRegistry {
    entries: Vec<McpRegistration>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Register a provider under a glob pattern. Multiple providers
    /// can share the same pattern; all matching ones contribute.
    pub fn register(&mut self, pattern: impl Into<String>, provider: Box<dyn McpProvider>) {
        self.entries.push(McpRegistration {
            pattern: pattern.into(),
            provider,
        });
    }

    /// Query every provider whose pattern matches `command_line`.
    /// Candidates from all matching providers are concatenated;
    /// the driver applies ranking + dedup downstream.
    pub fn dispatch(&self, command_line: &str, anchor: Range) -> Vec<CompletionCandidate> {
        let mut out = Vec::new();
        for entry in &self.entries {
            if glob_matches(&entry.pattern, command_line) {
                out.extend(entry.provider.complete(command_line, anchor));
            }
        }
        out
    }
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let patterns: Vec<&str> = self.entries.iter().map(|e| e.pattern.as_str()).collect();
        f.debug_struct("McpRegistry")
            .field("patterns", &patterns)
            .finish()
    }
}

/// Glob-match `input` against `pattern`. The pattern grammar is
/// single-level only — `*` matches the remainder of the string but
/// `/` etc. have no special meaning. Examples:
///
/// - `"git *"` matches `"git status"`, `"git commit -m hi"`.
/// - `"kubectl *"` matches `"kubectl get pods"`.
/// - `"npm run *"` matches `"npm run test --watch"`.
/// - `"ls"` matches `"ls"` (exact) and nothing else.
///
/// Implemented by hand (no regex dep) — the grammar is trivial
/// enough that a loop beats a crate.
pub fn glob_matches(pattern: &str, input: &str) -> bool {
    let pat = pattern.trim_end();
    // Trailing `*` means "starts with this prefix". Use strip_prefix
    // so the match is exact on the leading portion.
    if let Some(prefix) = pat.strip_suffix('*') {
        let prefix = prefix.trim_end();
        if prefix.is_empty() {
            return true;
        }
        // Must match at a word boundary — otherwise "git*" matches
        // "gitoxide" which we don't want.
        if let Some(tail) = input.strip_prefix(prefix) {
            return tail.is_empty() || tail.starts_with(char::is_whitespace);
        }
        return false;
    }
    pat == input
}

/// Adapter: produce `CompletionSource::Mcp`-sourced candidates
/// from a plain list of labels. Used by MCP providers that have a
/// simple string-only completion set.
pub fn mcp_candidates_from_labels(labels: &[&str], anchor: Range) -> Vec<CompletionCandidate> {
    labels
        .iter()
        .map(|label| {
            CompletionCandidate::replace(CompletionSource::Mcp, (*label).to_string(), anchor)
                .with_icon("mcp")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LiteralProvider {
        name: &'static str,
        labels: Vec<&'static str>,
    }

    impl McpProvider for LiteralProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn complete(&self, _line: &str, anchor: Range) -> Vec<CompletionCandidate> {
            self.labels
                .iter()
                .map(|l| {
                    CompletionCandidate::replace(CompletionSource::Mcp, *l, anchor).with_icon("mcp")
                })
                .collect()
        }
    }

    #[test]
    fn glob_star_matches_word_boundary() {
        assert!(glob_matches("git *", "git status"));
        assert!(glob_matches("git *", "git"));
        assert!(!glob_matches("git *", "gitoxide"));
        assert!(glob_matches("kubectl *", "kubectl get pods"));
        assert!(!glob_matches("kubectl *", "kube"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_matches("ls", "ls"));
        assert!(!glob_matches("ls", "ls -la"));
        assert!(!glob_matches("ls", "lsof"));
    }

    #[test]
    fn glob_bare_star_matches_everything() {
        assert!(glob_matches("*", ""));
        assert!(glob_matches("*", "anything"));
    }

    #[test]
    fn registry_starts_empty() {
        let r = McpRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn dispatch_invokes_matching_provider_only() {
        let mut r = McpRegistry::new();
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "git-mcp",
                labels: vec!["status", "checkout"],
            }),
        );
        r.register(
            "kubectl *",
            Box::new(LiteralProvider {
                name: "k8s-mcp",
                labels: vec!["get", "apply"],
            }),
        );
        let out = r.dispatch("git ", Range::new(0, 0));
        let labels: Vec<_> = out.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["status", "checkout"]);
    }

    #[test]
    fn dispatch_returns_empty_when_no_match() {
        let mut r = McpRegistry::new();
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "git-mcp",
                labels: vec!["status"],
            }),
        );
        let out = r.dispatch("cargo build", Range::new(0, 0));
        assert!(out.is_empty());
    }

    #[test]
    fn multiple_providers_on_same_pattern_concatenate() {
        let mut r = McpRegistry::new();
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "a",
                labels: vec!["one"],
            }),
        );
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "b",
                labels: vec!["two"],
            }),
        );
        let out = r.dispatch("git checkout", Range::new(0, 0));
        let labels: Vec<_> = out.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["one", "two"]);
    }

    #[test]
    fn candidates_carry_mcp_source() {
        let mut r = McpRegistry::new();
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "g",
                labels: vec!["status"],
            }),
        );
        let out = r.dispatch("git ", Range::new(0, 0));
        assert!(out.iter().all(|c| c.source == CompletionSource::Mcp));
    }

    #[test]
    fn mcp_candidates_from_labels_builds_set() {
        let out = mcp_candidates_from_labels(&["a", "b", "c"], Range::new(0, 0));
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|c| c.source == CompletionSource::Mcp));
    }

    #[test]
    fn debug_output_lists_patterns() {
        let mut r = McpRegistry::new();
        r.register(
            "git *",
            Box::new(LiteralProvider {
                name: "g",
                labels: vec![],
            }),
        );
        let s = format!("{r:?}");
        assert!(s.contains("git *"));
    }
}
