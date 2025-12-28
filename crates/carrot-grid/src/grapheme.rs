//! Grapheme cluster storage.
//!
//! A grapheme cluster (UAX#29) is a user-perceived character that may
//! be composed of multiple codepoints: a base letter plus combining
//! accents, an emoji with a skin-tone modifier, a ZWJ sequence. When
//! the incoming byte stream contains such a cluster, the VT writer
//! can no longer fit it into a single `Cell` (cells hold 21 bits of
//! content — enough for one codepoint, not a variable-length sequence).
//!
//! The fix matches the block plan (§A1 cell layout): cells flip
//! their tag to [`crate::CellTag::Grapheme`] and carry a
//! [`GraphemeIndex`] that points into a per-block [`GraphemeStore`]
//! of UTF-8 strings.
//!
//! The 8-byte `Cell` invariant is preserved — the grapheme cluster
//! bytes never touch the cell.
//!
//! Capacity is 21 bits (`2^21 ≈ 2_097_152` distinct clusters per
//! block) to match the [`crate::cell::CONTENT_BITS`] budget. Exceeding
//! that is unrealistic; the store returns
//! [`GraphemeIndex::OUT_OF_CAPACITY`] instead of panicking.

use std::sync::Arc;

use inazuma_collections::FxHashMap;

use crate::cell::GraphemeIndex;

impl GraphemeIndex {
    /// Sentinel returned when the 21-bit grapheme table fills up.
    /// Callers should degrade to `Cell::codepoint` using the base
    /// character only (dropping the combiners).
    pub const OUT_OF_CAPACITY: Self = Self(u32::MAX);

    /// `true` when this id lies inside the 21-bit content budget.
    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

/// Per-block grapheme-cluster interner. Append-only during the block's
/// active lifetime; frozen on `ActiveBlock::finish()` so the renderer
/// can read clusters lock-free.
#[derive(Debug, Clone, Default)]
pub struct GraphemeStore {
    /// Cluster strings indexed by `GraphemeIndex`. Arc so freezing is
    /// just a `.clone()` — strings don't move.
    clusters: Vec<Arc<str>>,
    /// Reverse map from string to id. Identical clusters dedup.
    interner: FxHashMap<Arc<str>, GraphemeIndex>,
}

impl GraphemeStore {
    /// Fresh store. No clusters pre-reserved — the first `intern`
    /// call hands out id 0.
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            interner: FxHashMap::default(),
        }
    }

    /// Intern a cluster. Identical strings return the same id.
    /// Returns [`GraphemeIndex::OUT_OF_CAPACITY`] if the table has
    /// more than 2^21 distinct entries (unreachable in realistic
    /// workloads).
    pub fn intern(&mut self, cluster: impl AsRef<str>) -> GraphemeIndex {
        let cluster = cluster.as_ref();
        if cluster.is_empty() {
            return GraphemeIndex::OUT_OF_CAPACITY;
        }
        if let Some(&id) = self.interner.get(cluster) {
            return id;
        }
        let next_index = self.clusters.len();
        if next_index >= (1usize << 21) {
            return GraphemeIndex::OUT_OF_CAPACITY;
        }
        let arc: Arc<str> = Arc::from(cluster);
        let id = GraphemeIndex(next_index as u32);
        self.clusters.push(arc.clone());
        self.interner.insert(arc, id);
        id
    }

    /// Look up the cluster for an id. Returns `None` for
    /// [`GraphemeIndex::OUT_OF_CAPACITY`] or out-of-range ids.
    pub fn get(&self, id: GraphemeIndex) -> Option<&str> {
        if !id.is_valid() {
            return None;
        }
        self.clusters.get(id.0 as usize).map(|s| s.as_ref())
    }

    /// Number of distinct clusters stored.
    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    /// `true` when no clusters have been interned.
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cluster_returns_out_of_capacity() {
        let mut store = GraphemeStore::new();
        assert_eq!(store.intern(""), GraphemeIndex::OUT_OF_CAPACITY);
    }

    #[test]
    fn intern_is_stable_for_same_cluster() {
        let mut store = GraphemeStore::new();
        let a = store.intern("á");
        let b = store.intern("á");
        assert_eq!(a, b);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn distinct_clusters_get_distinct_ids() {
        let mut store = GraphemeStore::new();
        let a = store.intern("á");
        let b = store.intern("é");
        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn get_returns_interned_cluster() {
        let mut store = GraphemeStore::new();
        let id = store.intern("👨‍👩‍👧");
        assert_eq!(store.get(id), Some("👨\u{200d}👩\u{200d}👧"));
    }

    #[test]
    fn get_for_out_of_capacity_returns_none() {
        let store = GraphemeStore::new();
        assert!(store.get(GraphemeIndex::OUT_OF_CAPACITY).is_none());
    }

    #[test]
    fn default_store_is_empty() {
        let store = GraphemeStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn is_valid_matches_sentinel() {
        assert!(!GraphemeIndex::OUT_OF_CAPACITY.is_valid());
        assert!(GraphemeIndex(0).is_valid());
        assert!(GraphemeIndex(42).is_valid());
    }
}
