//! AI engine trait + zero-dep mock.
//!
//! The ghost-text engine races multiple backends (History, Local,
//! Cloud) and surfaces the best suggestion. Actual local-model /
//! cloud clients live elsewhere; this module owns:
//!
//! - The [`SuggestionEngine`] trait every backend implements.
//! - [`HistoryEngine`] — longest-common-prefix match over
//!   [`History`] (zero-latency, zero-cost, always on).
//! - [`MockEngine`] — deterministic stand-in for tests.
//! - [`RacingEngine`] — composite that runs each inner engine and
//!   merges results into a single ranked [`SuggestionSet`].
//!
//! ## No async here
//!
//! The trait is deliberately synchronous. Real cloud / local
//! backends will wrap this in a dispatcher that fires requests on
//! a pool, but the contract at the engine level is "given a
//! request, return a set". This keeps every backend unit-testable
//! and avoids leaking an async runtime into the data layer.

use carrot_session::command_history::CommandHistory;

use super::ai::{Suggestion, SuggestionRequest, SuggestionSet, SuggestionSource};

/// Core trait. Implementors return candidate suggestions for a
/// given request; ranking / deduplication happens in [`SuggestionSet`].
pub trait SuggestionEngine {
    /// Human-readable identifier (for telemetry / debug).
    fn name(&self) -> &str;

    /// The source bucket every candidate from this engine belongs
    /// to. Used when composing via [`RacingEngine`].
    fn source(&self) -> SuggestionSource;

    /// Compute suggestions for `request`. May return an empty set.
    fn suggest(&self, request: &SuggestionRequest) -> Vec<Suggestion>;
}

/// Suggests the tail of the most-recent history entry that begins
/// with the request prefix. No allocation on the hot path when the
/// prefix doesn't match; single allocation for the tail string
/// otherwise.
pub struct HistoryEngine<'a> {
    history: &'a CommandHistory,
}

impl<'a> HistoryEngine<'a> {
    pub fn new(history: &'a CommandHistory) -> Self {
        Self { history }
    }
}

impl<'a> SuggestionEngine for HistoryEngine<'a> {
    fn name(&self) -> &str {
        "history"
    }

    fn source(&self) -> SuggestionSource {
        SuggestionSource::History
    }

    fn suggest(&self, request: &SuggestionRequest) -> Vec<Suggestion> {
        if !request.is_worth_dispatching() {
            return Vec::new();
        }
        let prefix = request.prefix.as_str();
        let max = request.max_results.max(1) as usize;
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in self.history.entries().iter().rev() {
            if !entry.command.starts_with(prefix) {
                continue;
            }
            let tail = &entry.command[prefix.len()..];
            if tail.is_empty() {
                continue;
            }
            if !seen.insert(tail.to_string()) {
                continue;
            }
            // Confidence weighted by exit success: zero-exit cmds
            // score higher so past-working tail wins over a broken
            // alternative.
            let confidence = if matches!(entry.exit_status, Some(0)) {
                80
            } else {
                40
            };
            out.push(
                Suggestion::new(tail.to_string(), SuggestionSource::History)
                    .with_confidence(confidence),
            );
            if out.len() >= max {
                break;
            }
        }
        out
    }
}

/// Deterministic mock — returns the canned suggestions. Useful for
/// integration tests that need a non-history source.
pub struct MockEngine {
    name: String,
    source: SuggestionSource,
    canned: Vec<Suggestion>,
}

impl MockEngine {
    pub fn new(name: impl Into<String>, source: SuggestionSource, canned: Vec<Suggestion>) -> Self {
        Self {
            name: name.into(),
            source,
            canned,
        }
    }
}

impl SuggestionEngine for MockEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn source(&self) -> SuggestionSource {
        self.source
    }

    fn suggest(&self, request: &SuggestionRequest) -> Vec<Suggestion> {
        if !request.is_worth_dispatching() {
            return Vec::new();
        }
        self.canned.clone()
    }
}

/// Composes multiple engines; runs them in registered order and
/// returns a merged [`SuggestionSet`] with duplicates pruned.
///
/// Today's implementation is serial — each engine's latency adds
/// to the next. A future async variant will race the engines in
/// parallel; the data contract (take a request, return a set)
/// stays identical.
///
/// Lifetime-parameterised so engines can hold borrows (e.g.
/// `HistoryEngine<'a>` references a `&'a History`). Pass `'static`
/// when every engine owns its state.
pub struct RacingEngine<'a> {
    engines: Vec<Box<dyn SuggestionEngine + Send + Sync + 'a>>,
}

impl<'a> Default for RacingEngine<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> RacingEngine<'a> {
    pub fn new() -> Self {
        Self {
            engines: Vec::new(),
        }
    }

    pub fn register<E>(&mut self, engine: E)
    where
        E: SuggestionEngine + Send + Sync + 'a,
    {
        self.engines.push(Box::new(engine));
    }

    pub fn len(&self) -> usize {
        self.engines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.engines.is_empty()
    }

    /// Run every registered engine and return a pruned set.
    pub fn run(&self, request: &SuggestionRequest) -> SuggestionSet {
        let mut set = SuggestionSet::new();
        for engine in &self.engines {
            set.candidates.extend(engine.suggest(request));
        }
        set.prune();
        set
    }

    /// Run with a per-engine wall-clock budget.
    ///
    /// Each backend that exceeds `request.budget` has its results
    /// discarded — the late answer never lands in the UI. Backends
    /// that come in under budget get their suggestions stamped with
    /// the measured latency so the ranker can learn which source is
    /// actually fast for this user on this machine.
    ///
    /// Ghost-text drops silently on budget miss — a late response
    /// must never flash into the UI after the user has moved on.
    /// The async racer keeps the same `Duration` contract.
    pub fn run_with_budget(&self, request: &SuggestionRequest) -> SuggestionSet {
        let mut set = SuggestionSet::new();
        for engine in &self.engines {
            let started = std::time::Instant::now();
            let candidates = engine.suggest(request);
            let elapsed = started.elapsed();
            if elapsed > request.budget {
                // Over budget — drop silently.
                continue;
            }
            for mut s in candidates {
                if s.latency.is_none() {
                    s.latency = Some(elapsed);
                }
                set.candidates.push(s);
            }
        }
        set.prune();
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Mock backend that sleeps `delay` inside `suggest`. Used to
    /// exercise the budget enforcement path without depending on
    /// real network / model latency.
    struct SlowMockEngine {
        delay: Duration,
        canned: Vec<Suggestion>,
    }

    impl SuggestionEngine for SlowMockEngine {
        fn name(&self) -> &str {
            "slow-mock"
        }
        fn source(&self) -> SuggestionSource {
            SuggestionSource::Cloud
        }
        fn suggest(&self, request: &SuggestionRequest) -> Vec<Suggestion> {
            if !request.is_worth_dispatching() {
                return Vec::new();
            }
            std::thread::sleep(self.delay);
            self.canned.clone()
        }
    }

    fn history_with(cmds: &[(&str, Option<i32>)]) -> CommandHistory {
        let mut h = CommandHistory::new();
        for (cmd, exit) in cmds {
            h.push_with_metadata((*cmd).to_string(), *exit, None);
        }
        h
    }

    #[test]
    fn history_engine_returns_empty_for_trivial_prefix() {
        let h = history_with(&[("ls", Some(0))]);
        let engine = HistoryEngine::new(&h);
        let request = SuggestionRequest::from_prefix("l");
        assert!(engine.suggest(&request).is_empty());
    }

    #[test]
    fn history_engine_returns_tail_of_matching_entry() {
        let h = history_with(&[("git checkout main", Some(0))]);
        let engine = HistoryEngine::new(&h);
        let request = SuggestionRequest::from_prefix("git chec");
        let out = engine.suggest(&request);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].completion, "kout main");
    }

    #[test]
    fn history_engine_dedupes_identical_tails() {
        // Two history entries with the same tail — last push wins,
        // dedup keeps a single candidate.
        let h = history_with(&[("git pull", Some(0)), ("git pull", Some(0))]);
        let engine = HistoryEngine::new(&h);
        let request = SuggestionRequest::from_prefix("git");
        let out = engine.suggest(&request);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn history_engine_lowers_confidence_on_failure() {
        let h = history_with(&[("rm -rf /", Some(1))]);
        let engine = HistoryEngine::new(&h);
        let request = SuggestionRequest::from_prefix("rm");
        let out = engine.suggest(&request);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, 40);
    }

    #[test]
    fn history_engine_respects_max_results() {
        let h = history_with(&[("git a", Some(0)), ("git b", Some(0)), ("git c", Some(0))]);
        let engine = HistoryEngine::new(&h);
        let mut request = SuggestionRequest::from_prefix("git");
        request.max_results = 2;
        assert_eq!(engine.suggest(&request).len(), 2);
    }

    #[test]
    fn mock_engine_returns_canned_suggestions() {
        let canned = vec![Suggestion::new(" explain", SuggestionSource::Local).with_confidence(70)];
        let engine = MockEngine::new("local", SuggestionSource::Local, canned);
        let request = SuggestionRequest::from_prefix("git");
        let out = engine.suggest(&request);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].completion, " explain");
        assert_eq!(engine.name(), "local");
        assert_eq!(engine.source(), SuggestionSource::Local);
    }

    #[test]
    fn mock_engine_refuses_trivial_prefix() {
        let canned = vec![Suggestion::new("x", SuggestionSource::Local)];
        let engine = MockEngine::new("local", SuggestionSource::Local, canned);
        assert!(
            engine
                .suggest(&SuggestionRequest::from_prefix("g"))
                .is_empty()
        );
    }

    #[test]
    fn racing_engine_merges_and_prunes() {
        let h = history_with(&[("git checkout main", Some(0))]);
        let mut racer = RacingEngine::new();
        racer.register(HistoryEngine::new(&h));
        racer.register(MockEngine::new(
            "local",
            SuggestionSource::Local,
            vec![Suggestion::new("kout main", SuggestionSource::Local)],
        ));
        let request = SuggestionRequest::from_prefix("git chec");
        let set = racer.run(&request);
        // Both engines produce "kout main"; prune dedupes.
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn racing_engine_best_prefers_history_over_local() {
        let h = history_with(&[("git checkout main", Some(0))]);
        let mut racer = RacingEngine::new();
        racer.register(HistoryEngine::new(&h));
        racer.register(MockEngine::new(
            "cloud",
            SuggestionSource::Cloud,
            vec![Suggestion::new("kout develop", SuggestionSource::Cloud).with_confidence(90)],
        ));
        let set = racer.run(&SuggestionRequest::from_prefix("git chec"));
        let best = set.best().unwrap();
        assert_eq!(best.source, SuggestionSource::History);
    }

    #[test]
    fn racing_engine_empty_until_registered() {
        let racer = RacingEngine::new();
        assert!(racer.is_empty());
        assert_eq!(racer.len(), 0);
        assert!(
            racer
                .run(&SuggestionRequest::from_prefix("anything"))
                .is_empty()
        );
    }

    #[test]
    fn trait_name_and_source_accessors() {
        let h = CommandHistory::new();
        let eng = HistoryEngine::new(&h);
        assert_eq!(eng.name(), "history");
        assert_eq!(eng.source(), SuggestionSource::History);
    }

    #[test]
    fn budget_drops_slow_engine_silently() {
        let h = history_with(&[("git status", Some(0))]);
        let mut racer = RacingEngine::new();
        racer.register(HistoryEngine::new(&h));
        racer.register(SlowMockEngine {
            delay: Duration::from_millis(80),
            canned: vec![Suggestion::new("-sb", SuggestionSource::Cloud)],
        });
        let request =
            SuggestionRequest::from_prefix("git st").with_budget(Duration::from_millis(20));
        let set = racer.run_with_budget(&request);
        // Slow engine dropped; history engine fits within budget.
        assert!(
            set.candidates
                .iter()
                .all(|s| s.source == SuggestionSource::History)
        );
    }

    #[test]
    fn budget_attaches_latency_to_surviving_suggestions() {
        let h = history_with(&[("git status", Some(0))]);
        let mut racer = RacingEngine::new();
        racer.register(HistoryEngine::new(&h));
        let request = SuggestionRequest::from_prefix("git st").with_budget(Duration::from_secs(60));
        let set = racer.run_with_budget(&request);
        assert!(!set.is_empty());
        assert!(set.candidates.iter().all(|s| s.latency.is_some()));
    }

    #[test]
    fn default_budget_is_200ms() {
        let req = SuggestionRequest::from_prefix("ls");
        assert_eq!(req.budget, Duration::from_millis(200));
    }

    #[test]
    fn with_budget_overrides_default() {
        let req = SuggestionRequest::from_prefix("ls").with_budget(Duration::from_millis(50));
        assert_eq!(req.budget, Duration::from_millis(50));
    }
}
