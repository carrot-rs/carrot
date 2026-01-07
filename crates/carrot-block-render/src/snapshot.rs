//! Lock-free terminal snapshot for the render thread.
//!
//! # Why this exists
//!
//! Two threads share the terminal state:
//! - **VT thread** runs the parser on PTY bytes and writes to the
//!   live block data model (`carrot_term::block::ActiveBlock` etc.).
//! - **Render thread** walks the blocks for each frame.
//!
//! If the render thread holds a mutex on terminal state while painting,
//! every incoming byte from the PTY has to wait its turn behind the
//! GPU pipeline. At 120 fps that's a ~8 ms lock every frame, which
//! caps the PTY throughput at exactly the screen refresh rate —
//! catastrophic for `yes`, `cat large.log`, etc.
//!
//! The fix is an `ArcSwap`-based snapshot: the VT thread builds a new
//! immutable `TerminalSnapshot` after each meaningful write batch and
//! `store()`s it atomically. The render thread `load()`s in O(1) and
//! paints from its own `Arc` reference. The VT thread never blocks;
//! the render thread never blocks. Old snapshots are reclaimed by
//! `arc-swap`'s epoch GC once no reader holds them.
//!
//! # Granularity
//!
//! Snapshot construction is cheap by design:
//! - Frozen blocks are already `Arc<FrozenBlock>` — listing them is
//!   an `Arc<[Arc<FrozenBlock>]>` clone (no data copy, just a
//!   Vec-to-Arc collapse).
//! - The active block's state is captured via
//!   [`ActiveBlockSnapshot`] — an `Arc`-cloned view of the pages and
//!   atlas. Pages are owned inside the active block for now; a later
//!   step swaps them to `Arc<Page>` so snapshotting is truly zero-
//!   copy. Until then, this struct clones the `Vec<Cell>` rows.
//!
//! The VT thread decides the snapshot cadence — typically after a
//! synchronization boundary (ESU, newline, end-of-output-burst). The
//! render thread pulls the current snapshot once per frame and
//! consumes what's there.

use std::sync::Arc;

use arc_swap::ArcSwap;
use carrot_grid::{Cell, CellStyle};

/// Snapshot of one active block — the part the VT thread is currently
/// writing to. Immutable from the render thread's perspective.
///
/// This is deliberately **lightweight**: rows are owned `Vec<Cell>`
/// (cheap cloning from the live PageList), atlas is an `Arc<[CellStyle]>`
/// (shared). Rows may later be upgraded to `Arc<[Cell]>` once PageList
/// supports direct Arc exposure.
pub struct ActiveBlockSnapshot {
    pub rows: Vec<Vec<Cell>>,
    pub atlas: Arc<[CellStyle]>,
    pub cols: u16,
    /// Monotonic generation number — incremented per snapshot so
    /// consumers can tell if anything changed without diffing content.
    pub generation: u64,
}

impl ActiveBlockSnapshot {
    /// Empty snapshot for tests + initial state.
    pub fn empty(cols: u16) -> Self {
        Self {
            rows: Vec::new(),
            atlas: Arc::from(vec![CellStyle::DEFAULT]),
            cols,
            generation: 0,
        }
    }
}

/// Lock-free view of the terminal the render thread consumes.
///
/// - `frozen`: chronological list of finished blocks. Cheap `Arc` list
///   of per-block `Arc`s — mutations are add-only (plus prune on
///   scrollback-cap), so the render thread always sees a consistent
///   snapshot regardless of which block the VT thread is currently
///   extending.
/// - `active`: optional snapshot of the block under collection.
///   `None` between commands (at the shell prompt).
pub struct TerminalSnapshot {
    pub frozen: Arc<[Arc<FrozenBlockHandle>]>,
    pub active: Option<Arc<ActiveBlockSnapshot>>,
}

/// Opaque handle over the frozen block type. Defined as a newtype so
/// carrot-block-render doesn't need to depend on carrot-term — the
/// concrete block lives in carrot-term::block::FrozenBlock and
/// callers wrap it with this handle when building snapshots.
pub struct FrozenBlockHandle {
    pub rows: Vec<Vec<Cell>>,
    pub atlas: Arc<[CellStyle]>,
    pub cols: u16,
    pub exit_code: Option<i32>,
}

impl TerminalSnapshot {
    /// Empty snapshot — no frozen blocks, no active block.
    pub fn empty() -> Self {
        Self {
            frozen: Arc::from(Vec::new()),
            active: None,
        }
    }

    /// Total row count across all frozen blocks + active.
    pub fn total_rows(&self) -> usize {
        let frozen: usize = self.frozen.iter().map(|b| b.rows.len()).sum();
        let active = self.active.as_ref().map(|s| s.rows.len()).unwrap_or(0);
        frozen + active
    }

    /// Number of blocks (frozen + 0 or 1 active).
    pub fn block_count(&self) -> usize {
        self.frozen.len() + if self.active.is_some() { 1 } else { 0 }
    }
}

/// Lock-free shared-pointer store for the terminal snapshot.
///
/// Readers get `Arc<TerminalSnapshot>` via [`Self::load`] in O(1).
/// The writer publishes new snapshots via [`Self::store`]. Memory for
/// old snapshots is reclaimed by `arc-swap`'s epoch scheme once no
/// reader references them.
pub struct SharedTerminal {
    inner: ArcSwap<TerminalSnapshot>,
}

impl SharedTerminal {
    /// Construct with an initial snapshot.
    pub fn new(initial: TerminalSnapshot) -> Self {
        Self {
            inner: ArcSwap::new(Arc::new(initial)),
        }
    }

    /// Construct with an empty initial snapshot.
    pub fn empty() -> Self {
        Self::new(TerminalSnapshot::empty())
    }

    /// Atomically load the current snapshot. The returned `Arc` is
    /// the reader's handle for the duration of a frame — as long as
    /// it's alive, the underlying snapshot cannot be dropped by the
    /// writer.
    pub fn load(&self) -> Arc<TerminalSnapshot> {
        self.inner.load_full()
    }

    /// Atomically publish a new snapshot. Previous readers keep their
    /// handle; once the last reader drops, the old snapshot is GCed.
    pub fn store(&self, next: TerminalSnapshot) {
        self.inner.store(Arc::new(next));
    }

    /// Publish a new snapshot built from a function over the current
    /// one. Convenience for small edits like "append one new frozen
    /// block" without recomputing the entire state from scratch.
    pub fn rcu(&self, f: impl Fn(&TerminalSnapshot) -> TerminalSnapshot) {
        self.inner.rcu(|prev| Arc::new(f(prev)));
    }
}

impl Default for SharedTerminal {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn row(cols: u16, content: u8) -> Vec<Cell> {
        (0..cols)
            .map(|_| Cell::ascii(content, carrot_grid::CellStyleId(0)))
            .collect()
    }

    fn make_active(cols: u16, rows: usize) -> ActiveBlockSnapshot {
        ActiveBlockSnapshot {
            rows: (0..rows)
                .map(|i| row(cols, b'a' + (i as u8 % 26)))
                .collect(),
            atlas: Arc::from(vec![CellStyle::DEFAULT]),
            cols,
            generation: 1,
        }
    }

    fn make_frozen(cols: u16, rows: usize, exit: i32) -> FrozenBlockHandle {
        FrozenBlockHandle {
            rows: (0..rows)
                .map(|i| row(cols, b'0' + (i as u8 % 10)))
                .collect(),
            atlas: Arc::from(vec![CellStyle::DEFAULT]),
            cols,
            exit_code: Some(exit),
        }
    }

    #[test]
    fn empty_snapshot_has_zero_blocks() {
        let snap = TerminalSnapshot::empty();
        assert_eq!(snap.block_count(), 0);
        assert_eq!(snap.total_rows(), 0);
    }

    #[test]
    fn snapshot_counts_frozen_and_active() {
        let snap = TerminalSnapshot {
            frozen: Arc::from(vec![
                Arc::new(make_frozen(80, 10, 0)),
                Arc::new(make_frozen(80, 5, 1)),
            ]),
            active: Some(Arc::new(make_active(80, 3))),
        };
        assert_eq!(snap.block_count(), 3);
        assert_eq!(snap.total_rows(), 18);
    }

    #[test]
    fn shared_load_returns_current_snapshot() {
        let shared = SharedTerminal::empty();
        assert_eq!(shared.load().block_count(), 0);

        shared.store(TerminalSnapshot {
            frozen: Arc::from(vec![Arc::new(make_frozen(40, 2, 0))]),
            active: None,
        });
        assert_eq!(shared.load().block_count(), 1);
        assert_eq!(shared.load().total_rows(), 2);
    }

    #[test]
    fn rcu_produces_new_snapshot() {
        let shared = SharedTerminal::empty();
        // Wrap the frozen block in Arc so the Fn closure can clone it
        // each time rcu loops. ArcSwap::rcu requires the fn to be Fn,
        // not FnOnce — `shared.rcu(|prev| ...)` may call the closure
        // more than once under contention.
        let block_proto = Arc::new(make_frozen(40, 7, 0));
        shared.rcu(|prev| {
            let mut new_frozen: Vec<Arc<FrozenBlockHandle>> = prev.frozen.iter().cloned().collect();
            new_frozen.push(block_proto.clone());
            TerminalSnapshot {
                frozen: Arc::from(new_frozen),
                active: prev.active.clone(),
            }
        });
        assert_eq!(shared.load().block_count(), 1);
    }

    #[test]
    fn concurrent_load_under_store_is_consistent() {
        let shared = Arc::new(SharedTerminal::new(TerminalSnapshot {
            frozen: Arc::from(vec![Arc::new(make_frozen(10, 3, 0))]),
            active: None,
        }));

        let reads = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        // Spawn 4 reader threads each doing 1000 loads. Each load must
        // observe a non-zero block_count — the writer never removes
        // blocks, only adds.
        for _ in 0..4 {
            let shared = shared.clone();
            let reads = reads.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    let s = shared.load();
                    assert!(s.block_count() >= 1);
                    reads.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }

        // Single writer appends 100 new frozen blocks while readers run.
        let writer = {
            let shared = shared.clone();
            thread::spawn(move || {
                for i in 0..100 {
                    let proto = Arc::new(make_frozen(10, 1, i));
                    shared.rcu(|prev| {
                        let mut new_frozen: Vec<Arc<FrozenBlockHandle>> =
                            prev.frozen.iter().cloned().collect();
                        new_frozen.push(proto.clone());
                        TerminalSnapshot {
                            frozen: Arc::from(new_frozen),
                            active: prev.active.clone(),
                        }
                    });
                }
            })
        };

        for h in handles {
            h.join().expect("reader joined");
        }
        writer.join().expect("writer joined");

        assert_eq!(reads.load(Ordering::Relaxed), 4_000);
        // Final state has the initial block + 100 appended.
        assert_eq!(shared.load().block_count(), 101);
    }

    #[test]
    fn active_block_snapshot_clones_across_generations() {
        let a = Arc::new(make_active(80, 2));
        let b = Arc::new(ActiveBlockSnapshot {
            rows: a.rows.clone(),
            atlas: a.atlas.clone(),
            cols: a.cols,
            generation: a.generation + 1,
        });
        // `atlas` is Arc-shared between a and b — no copy.
        assert!(Arc::ptr_eq(&a.atlas, &b.atlas));
        assert_ne!(a.generation, b.generation);
    }
}
