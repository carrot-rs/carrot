//! OSC 8 hyperlink storage.
//!
//! Hyperlinks are range attributes: one open-sequence applies to every
//! cell until the close-sequence. That matches how fg/bg/attrs behave
//! today — those live in the CellStyleAtlas. Hyperlink URLs follow the
//! same pattern: cells in the same span share a `style_id`, the
//! `CellStyle` entry carries a compact [`HyperlinkId`], and the URL
//! strings live here in a per-block interning store.
//!
//! The 8-byte `Cell` invariant is preserved — the hyperlink
//! reference never touches the cell.
//!
//! Capacity is 16 bits (65k unique URLs per block). If exceeded the
//! store returns [`HyperlinkId::NONE`] instead of panicking — the
//! cell then behaves as a plain styled cell.

use std::sync::Arc;

use inazuma_collections::FxHashMap;

/// Compact handle for a hyperlinked span. `HyperlinkId::NONE` (0) is
/// the common "no hyperlink" case — kept cheap (no allocation, no
/// lookup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct HyperlinkId(u16);

impl HyperlinkId {
    /// Sentinel meaning "no hyperlink." Stored inline on every CellStyle
    /// entry; a plain styled cell has `HyperlinkId::NONE`.
    pub const NONE: Self = Self(0);

    /// Raw 16-bit value. Useful for comparisons and serialisation.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// `true` when this is the sentinel (no hyperlink).
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// `true` when this references an actual hyperlink.
    pub const fn is_some(self) -> bool {
        self.0 != 0
    }
}

/// Per-block hyperlink URL interner. Append-only during the block's
/// active lifetime; frozen on `ActiveBlock::finish()` via `Arc` clone.
#[derive(Debug, Clone, Default)]
pub struct HyperlinkStore {
    /// URLs indexed by `HyperlinkId`. Index 0 is `None` (unused slot).
    urls: Vec<Arc<str>>,
    /// Reverse lookup: URL → id. Separate from `urls` so duplicate
    /// `intern` calls don't grow storage.
    interner: FxHashMap<Arc<str>, HyperlinkId>,
}

impl HyperlinkStore {
    /// Empty store. `HyperlinkId::NONE` is reserved at construction
    /// so subsequent ids start at 1.
    pub fn new() -> Self {
        Self {
            urls: vec![Arc::from("")], // slot 0 reserved for NONE
            interner: FxHashMap::default(),
        }
    }

    /// Intern a URL string; returns its id. Identical strings share
    /// an id. Returns [`HyperlinkId::NONE`] when the internal capacity
    /// would be exceeded (16-bit = 65,535 unique entries per block;
    /// hit this only with pathological output).
    pub fn intern(&mut self, url: impl AsRef<str>) -> HyperlinkId {
        let url = url.as_ref();
        if url.is_empty() {
            return HyperlinkId::NONE;
        }
        if let Some(&id) = self.interner.get(url) {
            return id;
        }
        let next_index = self.urls.len();
        if next_index >= u16::MAX as usize {
            return HyperlinkId::NONE;
        }
        let arc: Arc<str> = Arc::from(url);
        let id = HyperlinkId(next_index as u16);
        self.urls.push(arc.clone());
        self.interner.insert(arc, id);
        id
    }

    /// Look up the URL for an id. Returns `None` for
    /// [`HyperlinkId::NONE`] or an out-of-range id.
    pub fn get(&self, id: HyperlinkId) -> Option<&str> {
        if id.is_none() {
            return None;
        }
        self.urls.get(id.0 as usize).map(|s| s.as_ref())
    }

    /// Total number of distinct hyperlinks stored (excluding the
    /// reserved `NONE` slot).
    pub fn len(&self) -> usize {
        self.urls.len().saturating_sub(1)
    }

    /// `true` when no hyperlinks have been interned.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_zero_sentinel() {
        assert_eq!(HyperlinkId::NONE.get(), 0);
        assert!(HyperlinkId::NONE.is_none());
        assert!(!HyperlinkId::NONE.is_some());
    }

    #[test]
    fn intern_returns_stable_id_for_same_url() {
        let mut store = HyperlinkStore::new();
        let a = store.intern("https://example.org");
        let b = store.intern("https://example.org");
        assert_eq!(a, b);
        assert!(a.is_some());
    }

    #[test]
    fn intern_distinguishes_different_urls() {
        let mut store = HyperlinkStore::new();
        let a = store.intern("https://example.org");
        let b = store.intern("https://example.com");
        assert_ne!(a, b);
    }

    #[test]
    fn intern_empty_string_returns_none() {
        let mut store = HyperlinkStore::new();
        assert_eq!(store.intern(""), HyperlinkId::NONE);
    }

    #[test]
    fn get_round_trips() {
        let mut store = HyperlinkStore::new();
        let id = store.intern("https://carrot.dev");
        assert_eq!(store.get(id), Some("https://carrot.dev"));
    }

    #[test]
    fn get_none_returns_none() {
        let store = HyperlinkStore::new();
        assert_eq!(store.get(HyperlinkId::NONE), None);
    }

    #[test]
    fn len_skips_reserved_slot() {
        let mut store = HyperlinkStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
        store.intern("a");
        store.intern("b");
        assert_eq!(store.len(), 2);
        // Dedup doesn't grow len.
        store.intern("a");
        assert_eq!(store.len(), 2);
    }
}
