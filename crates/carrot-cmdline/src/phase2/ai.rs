//! AI ghost-text suggestion data model.
//!
//! "Ghost-text" = the dim, inline continuation the cmdline shows
//! after the cursor while the user is typing:
//!
//! ```text
//! $ git checkout ma▌in         ← "in" rendered dim, accepted with →
//! ```
//!
//! Suggestions come from multiple backends with different
//! latency / quality trade-offs:
//!
//! - **History**  — zero-latency: longest-common-prefix match over
//!   the user's own recent commands.
//! - **Local**    — sub-100ms: small on-device model (e.g. Ollama
//!   with a coding-completion checkpoint).
//! - **Cloud**    — 100–500ms: Anthropic / Bedrock / OpenAI, higher
//!   quality, requires network + auth.
//!
//! The ghost-text engine races them and renders the first acceptable
//! suggestion. Higher-priority source "wins" once it arrives.
//!
//! # Scope
//!
//! This module defines the shapes. The actual race / render logic
//! lives in [`crate::ai_engine`] (`HistoryEngine`, `MockEngine`,
//! `RacingEngine`). HTTP / local-model transports attach behind
//! their respective crate deps without needing this module to
//! change. Keeping the data types here lets the UI layer import the
//! contract without pulling in model runtime deps.

use std::time::Duration;

/// Where the suggestion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionSource {
    /// Longest-common-prefix match over the user's own history.
    /// Lowest latency, highest precision, zero privacy concern.
    History,
    /// On-device model (e.g. Ollama). Fast, offline, variable quality.
    Local,
    /// Cloud LLM. Higher quality, network cost, privacy trade-off.
    Cloud,
}

impl SuggestionSource {
    /// Display-priority bucket. Lower = preferred when two sources
    /// race to the same prefix with comparable scores.
    pub fn priority_bucket(self) -> u8 {
        match self {
            SuggestionSource::History => 0,
            SuggestionSource::Local => 1,
            SuggestionSource::Cloud => 2,
        }
    }

    /// Whether the source is offline-capable.
    pub fn is_offline(self) -> bool {
        matches!(self, SuggestionSource::History | SuggestionSource::Local)
    }
}

/// A single ghost-text suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Text to append after the cursor. Should NOT repeat what the
    /// user already typed — the engine returns only the tail.
    pub completion: String,
    /// Source that produced the suggestion.
    pub source: SuggestionSource,
    /// Producer-reported confidence (0-100). Ranking blends this
    /// with recency + source priority.
    pub confidence: u8,
    /// Wall-clock latency from request to response. Used for
    /// auto-tuning which source to query first on next keystroke.
    pub latency: Option<Duration>,
}

impl Suggestion {
    /// Minimal constructor used by tests + simple backends.
    pub fn new(completion: impl Into<String>, source: SuggestionSource) -> Self {
        Self {
            completion: completion.into(),
            source,
            confidence: 50,
            latency: None,
        }
    }

    /// Builder: attach a measured latency.
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = Some(latency);
        self
    }

    /// Builder: clamp-set the confidence (caps to 100).
    pub fn with_confidence(mut self, confidence: u8) -> Self {
        self.confidence = confidence.min(100);
        self
    }

    /// Trivial suggestions are filtered out by the engine — empty or
    /// whitespace-only completions carry no value.
    pub fn is_trivial(&self) -> bool {
        self.completion.trim().is_empty()
    }
}

/// A suggestion request the engine dispatches to each source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionRequest {
    /// What the user has typed so far (the "prefix").
    pub prefix: String,
    /// Current working directory — lets model-backed sources ground
    /// the answer in the project.
    pub cwd: Option<String>,
    /// Bounded recent history slice — the ranker consults this for
    /// dedup + recency weighting.
    pub recent_commands: Vec<String>,
    /// Max number of suggestions to return from a single source.
    pub max_results: u8,
    /// Per-source latency budget. Backends that can't respond
    /// within this window are silently dropped so a late answer
    /// never flashes into the UI after the user has moved on.
    pub budget: std::time::Duration,
}

impl SuggestionRequest {
    /// Fresh request from a prefix, no context. Used by tests +
    /// cold-start.
    pub fn from_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            cwd: None,
            recent_commands: Vec::new(),
            max_results: 3,
            // Budget: local <50 ms, cloud <200 ms. We default
            // to the cloud budget so the engine is usable with either
            // source; tighter budgets are opt-in per-request.
            budget: std::time::Duration::from_millis(200),
        }
    }

    /// Builder: set the per-source latency budget.
    pub fn with_budget(mut self, budget: std::time::Duration) -> Self {
        self.budget = budget;
        self
    }

    /// Whether the request is substantial enough to issue. Trivial
    /// prefixes (empty / whitespace / single-char) waste round trips.
    pub fn is_worth_dispatching(&self) -> bool {
        self.prefix.trim().chars().count() >= 2
    }
}

/// Ranked list of candidate suggestions, best-first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuggestionSet {
    pub candidates: Vec<Suggestion>,
}

impl SuggestionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Best candidate = lowest source bucket, tie-break on confidence
    /// (higher wins), then shortest completion (less is more).
    pub fn best(&self) -> Option<&Suggestion> {
        self.candidates.iter().min_by(|a, b| {
            a.source
                .priority_bucket()
                .cmp(&b.source.priority_bucket())
                .then_with(|| b.confidence.cmp(&a.confidence))
                .then_with(|| a.completion.len().cmp(&b.completion.len()))
        })
    }

    /// Remove trivial / duplicate completions in place. Stable order.
    pub fn prune(&mut self) {
        self.candidates.retain(|s| !s.is_trivial());
        let mut seen = std::collections::HashSet::new();
        self.candidates
            .retain(|s| seen.insert(s.completion.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_priority_history_wins() {
        assert!(
            SuggestionSource::History.priority_bucket() < SuggestionSource::Local.priority_bucket()
        );
        assert!(
            SuggestionSource::Local.priority_bucket() < SuggestionSource::Cloud.priority_bucket()
        );
    }

    #[test]
    fn offline_sources_classified() {
        assert!(SuggestionSource::History.is_offline());
        assert!(SuggestionSource::Local.is_offline());
        assert!(!SuggestionSource::Cloud.is_offline());
    }

    #[test]
    fn suggestion_builder_clamps_confidence() {
        let s = Suggestion::new("in", SuggestionSource::History).with_confidence(200);
        assert_eq!(s.confidence, 100);
    }

    #[test]
    fn trivial_suggestion_detected() {
        let blank = Suggestion::new("", SuggestionSource::History);
        let ws = Suggestion::new("   ", SuggestionSource::History);
        let real = Suggestion::new("in", SuggestionSource::History);
        assert!(blank.is_trivial());
        assert!(ws.is_trivial());
        assert!(!real.is_trivial());
    }

    #[test]
    fn worth_dispatching_gates_short_prefixes() {
        assert!(!SuggestionRequest::from_prefix("").is_worth_dispatching());
        assert!(!SuggestionRequest::from_prefix("g").is_worth_dispatching());
        assert!(SuggestionRequest::from_prefix("gi").is_worth_dispatching());
        assert!(SuggestionRequest::from_prefix("git ").is_worth_dispatching());
    }

    #[test]
    fn best_picks_history_over_cloud_at_equal_confidence() {
        let mut set = SuggestionSet::new();
        set.candidates
            .push(Suggestion::new("in", SuggestionSource::Cloud).with_confidence(90));
        set.candidates
            .push(Suggestion::new("in", SuggestionSource::History).with_confidence(90));
        let best = set.best().unwrap();
        assert_eq!(best.source, SuggestionSource::History);
    }

    #[test]
    fn best_picks_higher_confidence_within_same_source() {
        let mut set = SuggestionSet::new();
        set.candidates
            .push(Suggestion::new("in", SuggestionSource::Local).with_confidence(40));
        set.candidates
            .push(Suggestion::new("ain", SuggestionSource::Local).with_confidence(90));
        let best = set.best().unwrap();
        assert_eq!(best.completion, "ain");
    }

    #[test]
    fn prune_removes_trivial_and_dupes() {
        let mut set = SuggestionSet::new();
        set.candidates
            .push(Suggestion::new("in", SuggestionSource::History));
        set.candidates
            .push(Suggestion::new("", SuggestionSource::Local));
        set.candidates
            .push(Suggestion::new("in", SuggestionSource::Cloud));
        set.candidates
            .push(Suggestion::new("   ", SuggestionSource::Local));
        set.prune();
        assert_eq!(set.len(), 1);
        assert_eq!(set.candidates[0].completion, "in");
    }
}
