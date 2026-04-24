//! TTL-bounded cache of live-walk results per scope-root. Lets the
//! files source re-open the same scope within `ttl` and return results
//! instantly without re-walking the filesystem.
//!
//! Registered as an `inazuma::Global` via [`init`] so every modal shares
//! the same cache — opening `Cmd+O` in `~/projects/foo`, dismissing, and
//! reopening within TTL returns the cached result list instantly.

use inazuma::{App, Global};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub results: Vec<PathBuf>,
    pub cached_at: Instant,
    pub scanned: usize,
    pub truncated: bool,
}

#[derive(Debug, Default)]
pub struct LiveWalkCache {
    entries: HashMap<PathBuf, CacheEntry>,
}

impl Global for LiveWalkCache {}

/// Install an empty `LiveWalkCache` as a global on the app if one is not
/// already registered. Idempotent — safe to call multiple times.
pub fn init(cx: &mut App) {
    if cx.try_global::<LiveWalkCache>().is_none() {
        cx.set_global(LiveWalkCache::default());
    }
}

impl LiveWalkCache {
    pub fn get_fresh(&self, scope: &Path, ttl: Duration) -> Option<&CacheEntry> {
        let entry = self.entries.get(scope)?;
        if entry.cached_at.elapsed() < ttl {
            Some(entry)
        } else {
            None
        }
    }

    pub fn put(&mut self, scope: PathBuf, results: Vec<PathBuf>, scanned: usize, truncated: bool) {
        self.entries.insert(
            scope,
            CacheEntry {
                results,
                cached_at: Instant::now(),
                scanned,
                truncated,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn put_then_fresh_hit() {
        let mut cache = LiveWalkCache::default();
        let scope = PathBuf::from("/tmp/scope");
        cache.put(
            scope.clone(),
            vec![PathBuf::from("/tmp/scope/a.txt")],
            1,
            false,
        );
        let entry = cache.get_fresh(&scope, Duration::from_secs(30));
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().results.len(), 1);
    }

    #[test]
    fn stale_entry_returns_none() {
        let mut cache = LiveWalkCache::default();
        let scope = PathBuf::from("/tmp/scope");
        cache.put(scope.clone(), vec![], 0, false);
        sleep(Duration::from_millis(20));
        assert!(cache.get_fresh(&scope, Duration::from_millis(10)).is_none());
    }
}
