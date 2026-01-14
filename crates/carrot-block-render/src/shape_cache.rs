//! LRU cache for shaped-glyph runs.
//!
//! Shaping the same `(font, size, text)` tuple is deterministic — if
//! we've already shaped it this frame (or any recent frame), we can
//! reuse the result and skip harfrust entirely. That matters
//! because:
//!
//! - A terminal re-renders the same cell lots of times (static
//!   scrollback rows repaint on every scroll event).
//! - `carrot-cmdline` trees-sitter pass shapes the same command
//!   tokens over and over as the user types.
//! - On the slow path (complex scripts with many ligatures), shape
//!   can be 50–200 µs/call — more than our frame budget at 120 Hz.
//!
//! The cache is a plain LRU keyed by `(font_hash, px_size, text)`
//! and bounded by entry count. Evictions are O(1). The cached value
//! is `Arc<Vec<ShapedGlyph>>` so hits are essentially free clones.
//!
//! # When to invalidate
//!
//! - Font change — happens on theme switch or settings change.
//!   Rebuild the cache (or call [`ShapeCache::clear`]).
//! - Feature-list change — the cache key currently includes only
//!   script-detection-relevant fields. Pass the features through
//!   as part of the key if the caller toggles ligatures on/off at
//!   runtime. Rare enough that we treat it as a clear-on-change.
//! - Normal operation — the LRU handles eviction by itself.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;

use crate::shaping::{ShapeOptions, ShapedGlyph, ShapingError, ShapingFont, shape_run};

/// Cache key. `font_hash` should be a stable id for the font (often
/// the xxh3/sha of its bytes, or the file-system path hash). `px_size`
/// is the render pixel size — different sizes shape differently for
/// hinted fonts.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShapeKey {
    pub font_hash: u64,
    pub px_size: u32,
    pub text: String,
}

/// A hit or miss record — the consumer sees either the cached value
/// or the shape-call result.
pub enum CacheOutcome {
    Hit(Arc<Vec<ShapedGlyph>>),
    Miss(Arc<Vec<ShapedGlyph>>),
}

impl CacheOutcome {
    pub fn glyphs(&self) -> &Arc<Vec<ShapedGlyph>> {
        match self {
            CacheOutcome::Hit(v) => v,
            CacheOutcome::Miss(v) => v,
        }
    }

    pub fn is_hit(&self) -> bool {
        matches!(self, CacheOutcome::Hit(_))
    }
}

/// LRU-bounded shape-result cache.
///
/// Typical capacity: 1024–4096 entries. One entry is a `Vec<ShapedGlyph>`
/// + key; ≈ 100–500 bytes in practice. 4096 entries ≈ 2 MB — fine
/// trade for skipping the shape call on hot paths.
pub struct ShapeCache {
    inner: LruCache<ShapeKey, Arc<Vec<ShapedGlyph>>>,
    hits: u64,
    misses: u64,
}

impl ShapeCache {
    /// Construct with a bounded capacity. Capacity of 0 is not
    /// allowed — use a small number (like 1) if you want effectively
    /// no caching but still the API.
    pub fn new(capacity: usize) -> Self {
        // `capacity == 0` is coerced to 1 — documented contract.
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            inner: LruCache::new(cap),
            hits: 0,
            misses: 0,
        }
    }

    /// Default capacity of 1024 — enough for typical shell sessions
    /// where unique `(font, text)` tuples are rare.
    pub fn with_default_capacity() -> Self {
        Self::new(1024)
    }

    /// Shape `text` with `font` at `px_size`, consulting the cache
    /// first. `font_hash` must uniquely identify the font binary
    /// for the duration of the cache's lifetime — a mismatched
    /// hash returns stale glyph IDs.
    pub fn shape(
        &mut self,
        font_hash: u64,
        px_size: u32,
        text: &str,
        font: &ShapingFont,
        options: ShapeOptions<'_>,
    ) -> Result<CacheOutcome, ShapingError> {
        let key = ShapeKey {
            font_hash,
            px_size,
            text: text.to_string(),
        };
        if let Some(cached) = self.inner.get(&key) {
            self.hits += 1;
            return Ok(CacheOutcome::Hit(cached.clone()));
        }

        let glyphs = shape_run(font, text, options)?;
        let arc = Arc::new(glyphs);
        self.inner.put(key, arc.clone());
        self.misses += 1;
        Ok(CacheOutcome::Miss(arc))
    }

    /// Insert a pre-shaped result without consulting the cache
    /// first. Used for priming (e.g. after a theme change we
    /// re-shape a block's entire content off-thread and drop the
    /// results in).
    pub fn insert(&mut self, key: ShapeKey, glyphs: Vec<ShapedGlyph>) {
        self.inner.put(key, Arc::new(glyphs));
    }

    /// Look up a key without inserting. Returns `None` on miss.
    pub fn peek(&self, key: &ShapeKey) -> Option<Arc<Vec<ShapedGlyph>>> {
        self.inner.peek(key).cloned()
    }

    /// Drop all entries. Use on font change, feature-flag flip, or
    /// any other wholesale invalidation.
    pub fn clear(&mut self) {
        self.inner.clear();
        // Stats preserved — callers can watch the miss spike after
        // clear() without needing to reset themselves. Use
        // `reset_stats` if a zero baseline is wanted.
    }

    /// Reset hit/miss counters without touching the cached entries.
    pub fn reset_stats(&mut self) {
        self.hits = 0;
        self.misses = 0;
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Cache capacity — the LRU evicts oldest entries when `len ==
    /// capacity` and a new insert would push beyond.
    pub fn capacity(&self) -> usize {
        self.inner.cap().get()
    }

    /// Total cache hits since construction or last [`Self::reset_stats`].
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Total cache misses.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Hit ratio as `f32` between 0.0 and 1.0. Returns 0.0 when no
    /// lookups have happened yet.
    pub fn hit_ratio(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f32) / (total as f32)
        }
    }
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_glyph(id: u32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: id,
            cluster: 0,
            x_advance: 600,
            y_advance: 0,
            x_offset: 0,
            y_offset: 0,
        }
    }

    fn key(text: &str) -> ShapeKey {
        ShapeKey {
            font_hash: 0xDEADBEEF,
            px_size: 14,
            text: text.to_string(),
        }
    }

    #[test]
    fn empty_cache_reports_zero_len_and_hit_ratio() {
        let cache = ShapeCache::new(16);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.hit_ratio(), 0.0);
    }

    #[test]
    fn insert_then_peek_returns_entry() {
        let mut cache = ShapeCache::new(16);
        cache.insert(key("hello"), vec![mock_glyph(1), mock_glyph(2)]);
        let got = cache.peek(&key("hello")).expect("present");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].glyph_id, 1);
    }

    #[test]
    fn peek_returns_none_for_missing_key() {
        let cache = ShapeCache::new(16);
        assert!(cache.peek(&key("absent")).is_none());
    }

    #[test]
    fn insert_past_capacity_evicts_oldest() {
        let mut cache = ShapeCache::new(2);
        cache.insert(key("a"), vec![mock_glyph(1)]);
        cache.insert(key("b"), vec![mock_glyph(2)]);
        cache.insert(key("c"), vec![mock_glyph(3)]);
        // "a" should be evicted; "b" and "c" survive.
        assert!(cache.peek(&key("a")).is_none());
        assert!(cache.peek(&key("b")).is_some());
        assert!(cache.peek(&key("c")).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn clear_drops_all_entries_but_keeps_capacity() {
        let mut cache = ShapeCache::new(8);
        cache.insert(key("x"), vec![mock_glyph(1)]);
        cache.insert(key("y"), vec![mock_glyph(2)]);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.capacity(), 8);
    }

    #[test]
    fn capacity_zero_clamps_to_one() {
        // A capacity of 0 would break LruCache's NonZeroUsize contract;
        // we clamp to 1 so the API stays infallible.
        let cache = ShapeCache::new(0);
        assert_eq!(cache.capacity(), 1);
    }

    #[test]
    fn stats_hit_ratio_math() {
        let mut cache = ShapeCache::new(4);
        // Simulate by using peek which doesn't touch hit/miss counters,
        // but we can exercise insert (which bumps miss via `shape` path
        // when wired). For the pure-data-structure test, we directly
        // drive the counter fields through reset + manual manipulation
        // by calling shape() with a dummy font later. Here we just
        // verify the math on an empty-then-primed counter.
        assert_eq!(cache.hit_ratio(), 0.0);
        cache.insert(key("a"), vec![mock_glyph(1)]);
        // hit_ratio stays 0 because insert() doesn't count as a hit —
        // correct. Only `shape()` lookups do.
        assert_eq!(cache.hit_ratio(), 0.0);
    }

    #[test]
    fn default_capacity_is_generous() {
        let cache = ShapeCache::default();
        assert!(cache.capacity() >= 1024);
    }

    #[test]
    fn reset_stats_preserves_entries() {
        let mut cache = ShapeCache::new(4);
        cache.insert(key("a"), vec![mock_glyph(1)]);
        cache.reset_stats();
        assert!(cache.peek(&key("a")).is_some());
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 0);
    }

    #[test]
    fn cache_outcome_is_hit_distinguishes_variants() {
        let hit = CacheOutcome::Hit(Arc::new(vec![mock_glyph(1)]));
        let miss = CacheOutcome::Miss(Arc::new(vec![mock_glyph(2)]));
        assert!(hit.is_hit());
        assert!(!miss.is_hit());
        assert_eq!(hit.glyphs().len(), 1);
        assert_eq!(miss.glyphs()[0].glyph_id, 2);
    }

    #[test]
    fn arc_sharing_across_get_calls() {
        let mut cache = ShapeCache::new(4);
        cache.insert(key("shared"), vec![mock_glyph(7)]);
        let a = cache.peek(&key("shared")).expect("first");
        let b = cache.peek(&key("shared")).expect("second");
        // Both Arcs should point at the same allocation.
        assert!(Arc::ptr_eq(&a, &b));
    }
}
