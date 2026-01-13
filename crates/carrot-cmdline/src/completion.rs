//! Completion candidate types.
//!
//! The cmdline asks different backends ("sources") for completions
//! based on the AST's understanding of the token under the cursor:
//!
//! - `ArgKind::Path { … }` → filesystem source.
//! - `ArgKind::GitRef { scope }` → git source, scoped to branches/tags/remotes/commits.
//! - `ArgKind::Url` → history URL source.
//! - `ArgKind::EnvVar` → environment variable names.
//! - `ArgKind::Enum(variants)` → static dropdown, source is the AST itself.
//! - Subcommand / flag positions → schema lookup (715+ `carrot-completions` specs).
//! - MCP-connected shell → MCP tool completion source.
//!
//! This module defines the **data model** every source returns. The
//! source registry, ranking, and UI binding land in follow-ups.
//!
//! # Scoring
//!
//! Candidates carry a `score: u32` (higher = better). Ordering is
//! left to the `rank` step — this module only guarantees the fields
//! are present. The fuzzy matcher that consumes these is the shared
//! `inazuma-fuzzy` crate (`CharBag` / `match_strings`), not a local
//! re-implementation.
//!
//! # Icons
//!
//! Icons are referenced by logical name, not path. Renderer picks
//! the concrete SVG based on the active theme.

use crate::ast::Range;

/// Where a candidate came from. Drives the icon, the sort bucket,
/// and whether hitting Tab accepts-and-closes or accepts-and-continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionSource {
    /// Filesystem — files and directories under `cwd` or an
    /// explicit prefix.
    Filesystem,
    /// Git — branches, tags, remotes, commit hashes.
    Git,
    /// Command spec — subcommand / flag / value from the 715+
    /// `carrot-completions` JSON specs.
    Spec,
    /// Shell history — commands the user ran before.
    History,
    /// Environment variable names.
    EnvVar,
    /// Running processes (pids / names), for `kill`, `renice`, …
    Process,
    /// MCP server tool — the connected MCP tool advertises a
    /// completion endpoint.
    Mcp,
    /// AI ghost-text — suggestion from local or cloud model. Always
    /// sorted last unless the user has opted in to AI-first ranking.
    Ai,
}

impl CompletionSource {
    /// Default display-priority bucket (lower = higher priority). The
    /// ranker may override per user preference.
    pub fn default_bucket(self) -> u8 {
        match self {
            CompletionSource::Spec => 0,
            CompletionSource::Filesystem => 1,
            CompletionSource::Git => 1,
            CompletionSource::EnvVar => 2,
            CompletionSource::Process => 2,
            CompletionSource::History => 3,
            CompletionSource::Mcp => 3,
            CompletionSource::Ai => 4,
        }
    }
}

/// What a candidate inserts when the user accepts it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InsertAction {
    /// Replace the range in the input with `replacement`.
    Replace {
        /// Byte range in the input to replace.
        range: Range,
        /// Literal text to insert.
        replacement: String,
    },
    /// Insert `snippet` at `offset`, then position the cursor at the
    /// first `$0` placeholder (simplified LSP-style snippet).
    Snippet { offset: usize, snippet: String },
}

impl InsertAction {
    /// Length of the replacement / snippet text. Used by renderers to
    /// preview the effect of acceptance.
    pub fn text_len(&self) -> usize {
        match self {
            InsertAction::Replace { replacement, .. } => replacement.len(),
            InsertAction::Snippet { snippet, .. } => snippet.len(),
        }
    }
}

/// A completion candidate as returned by one source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompletionCandidate {
    /// Shown as the primary line in the popup.
    pub label: String,
    /// Secondary / dim text — e.g. flag description, file size.
    pub detail: Option<String>,
    /// Icon key. Renderer resolves to SVG / glyph.
    pub icon: Option<&'static str>,
    /// Where the candidate came from.
    pub source: CompletionSource,
    /// Insertion behaviour on accept.
    pub action: InsertAction,
    /// Ranker hint (higher = better). Source-local; the ranker blends
    /// across sources using `CompletionSource::default_bucket`.
    pub score: u32,
}

impl CompletionCandidate {
    /// Convenience constructor for the common Replace-with-label case.
    pub fn replace(source: CompletionSource, label: impl Into<String>, range: Range) -> Self {
        let label = label.into();
        Self {
            action: InsertAction::Replace {
                range,
                replacement: label.clone(),
            },
            label,
            detail: None,
            icon: None,
            source,
            score: 0,
        }
    }

    /// Builder: attach a `detail` string.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Builder: attach a score.
    pub fn with_score(mut self, score: u32) -> Self {
        self.score = score;
        self
    }

    /// Builder: attach an icon.
    pub fn with_icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A ranked list of candidates the popup renders. Owned by the
/// completion session — invalidated on every keystroke by design, so
/// pooling is intentionally **not** provided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionSet {
    pub candidates: Vec<CompletionCandidate>,
    /// Byte range in the input the candidates are completing. Shared
    /// across candidates — renderer uses it for underline / preview.
    pub anchor: Option<Range>,
}

impl CompletionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Sort in-place by `(source bucket, -score, label)`. Stable, so
    /// equal-score entries keep source-local ordering.
    pub fn sort(&mut self) {
        self.candidates.sort_by(|a, b| {
            a.source
                .default_bucket()
                .cmp(&b.source.default_bucket())
                .then_with(|| b.score.cmp(&a.score))
                .then_with(|| a.label.cmp(&b.label))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bucket_prioritises_spec_over_ai() {
        assert!(CompletionSource::Spec.default_bucket() < CompletionSource::Ai.default_bucket());
    }

    #[test]
    fn replace_constructor_sets_action_and_label() {
        let c = CompletionCandidate::replace(
            CompletionSource::Filesystem,
            "Cargo.toml",
            Range::new(0, 5),
        );
        assert_eq!(c.label, "Cargo.toml");
        match c.action {
            InsertAction::Replace { range, replacement } => {
                assert_eq!(range, Range::new(0, 5));
                assert_eq!(replacement, "Cargo.toml");
            }
            _ => panic!("expected Replace"),
        }
        assert_eq!(c.source, CompletionSource::Filesystem);
        assert_eq!(c.score, 0);
    }

    #[test]
    fn builder_chain_sets_fields() {
        let c = CompletionCandidate::replace(CompletionSource::Git, "main", Range::new(13, 17))
            .with_detail("last updated 2 days ago")
            .with_score(42)
            .with_icon("git-branch");
        assert_eq!(c.detail.as_deref(), Some("last updated 2 days ago"));
        assert_eq!(c.score, 42);
        assert_eq!(c.icon, Some("git-branch"));
    }

    #[test]
    fn insert_action_text_len_matches() {
        let r = InsertAction::Replace {
            range: Range::new(0, 0),
            replacement: "abc".into(),
        };
        assert_eq!(r.text_len(), 3);
        let s = InsertAction::Snippet {
            offset: 0,
            snippet: "fn $0()".into(),
        };
        assert_eq!(s.text_len(), 7);
    }

    #[test]
    fn sort_orders_by_bucket_then_score_then_label() {
        let mut set = CompletionSet::new();
        set.candidates.push(
            CompletionCandidate::replace(CompletionSource::Ai, "b-low", Range::new(0, 0))
                .with_score(100),
        );
        set.candidates.push(
            CompletionCandidate::replace(CompletionSource::Spec, "z", Range::new(0, 0))
                .with_score(1),
        );
        set.candidates.push(
            CompletionCandidate::replace(CompletionSource::Spec, "a", Range::new(0, 0))
                .with_score(1),
        );
        set.candidates.push(
            CompletionCandidate::replace(CompletionSource::Spec, "m", Range::new(0, 0))
                .with_score(10),
        );
        set.sort();
        // Spec bucket first (0), then Ai (4). Within Spec, higher
        // score first: m(10), then a(1) < z(1) alphabetical.
        let labels: Vec<_> = set.candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["m", "a", "z", "b-low"]);
    }

    #[test]
    fn empty_completion_set_reports_correctly() {
        let set = CompletionSet::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(set.anchor.is_none());
    }
}
