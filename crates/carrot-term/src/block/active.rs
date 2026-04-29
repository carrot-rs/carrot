//! Mutable block under active collection.
//!
//! The VT state machine writes into an `ActiveBlock` on every PTY byte.
//! Its `grid` is a `carrot_grid::PageList` — O(1) amortized append, no
//! reflow of scrollback on resize (display-only soft-wrap handles that
//! at render time).
//!
//! On `CommandEnd` the caller invokes [`ActiveBlock::finish`], which
//! consumes the block and returns a [`super::FrozenBlock`] — immutable,
//! Arc-wrappable, cheap to share across the render thread without locks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use carrot_grid::{
    Cell, CellStyle, CellStyleAtlas, CellStyleId, GraphemeStore, HyperlinkStore, ImageStore,
    PageCapacity, PageList,
};

use super::frozen::FrozenBlock;
use super::replay::ReplayBuffer;

/// Metadata captured while a command is running. Most fields match the
/// existing [`crate::block::BlockMetadata`] — we re-declare here to keep
/// the block module independent of the legacy module during migration.
#[derive(Debug, Clone, Default)]
pub struct ActiveMetadata {
    /// Original command text the user typed, if known.
    pub command: Option<String>,
    /// Working directory at command-start.
    pub cwd: Option<String>,
    /// Git branch at command-start.
    pub git_branch: Option<String>,
    /// Username captured at command-start.
    pub username: Option<String>,
    /// Hostname captured at command-start.
    pub hostname: Option<String>,
    /// Wallclock timestamp when the command started.
    pub started_at: Option<Instant>,
}

/// A block under active collection. Mutable, owned by the terminal core.
pub struct ActiveBlock {
    grid: PageList,
    atlas: CellStyleAtlas,
    images: ImageStore,
    hyperlinks: HyperlinkStore,
    graphemes: GraphemeStore,
    metadata: ActiveMetadata,
    /// Captured PTY byte stream for later font / theme-change replay
    /// and debug-replay support. See [`ReplayBuffer`] for the cap
    /// policy — silently truncates once the per-block limit is hit.
    replay: ReplayBuffer,
    /// In-flight text selection, if any. See
    /// [`super::selection::BlockSelection`].
    selection: Option<super::selection::BlockSelection>,
    /// Active live-frame region while a TUI is doing in-place
    /// redraws. `None` during normal linear shell output.
    live_frame: Option<super::live_frame::LiveFrameRegion>,
    /// Lifecycle marker: `Shell` (default) or `Tui` (set by the first
    /// TUI promotion signal). See [`super::kind::BlockKind`] —
    /// **sticky**, never reverts. Renderers route on this to pick
    /// inline (Shell) vs PinnedFooter (Tui).
    kind: super::kind::BlockKind,
    /// Lamport-style monotonic counter. Incremented on every observable
    /// mutation (grid write, atlas intern, hyperlink/grapheme/image
    /// insert, metadata change, kind promotion, …). Render-side caches
    /// trust this single field as the dirty signal — they re-extract a
    /// snapshot iff the counter advanced since their last frame.
    ///
    /// Mirrors `text::Buffer::version` in Zed: one source of truth at
    /// the producer, atomic to keep the VT thread lock-free against the
    /// render thread, `Relaxed` ordering because the surrounding
    /// `Mutex<Term>` already provides synchronization for every actual
    /// mutation site.
    generation: AtomicU64,
}

impl ActiveBlock {
    /// Create a fresh active block sized for `cols`, using 4 KB pages.
    pub fn new(cols: u16) -> Self {
        Self::with_page_bytes(cols, 4096)
    }

    /// Create a fresh active block with a custom page byte size.
    /// Larger pages fit more rows per allocation at the cost of bigger
    /// fixed chunks — tune for wide screens.
    pub fn with_page_bytes(cols: u16, page_bytes: u32) -> Self {
        let cap = PageCapacity::new(cols, page_bytes);
        Self {
            grid: PageList::new(cap),
            atlas: CellStyleAtlas::new(),
            images: ImageStore::new(),
            hyperlinks: HyperlinkStore::new(),
            graphemes: GraphemeStore::new(),
            metadata: ActiveMetadata::default(),
            replay: ReplayBuffer::default(),
            selection: None,
            live_frame: None,
            kind: super::kind::BlockKind::default(),
            generation: AtomicU64::new(1),
        }
    }

    /// Current generation. Render-side caches compare against their
    /// last-seen value to decide whether to re-extract a snapshot.
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    #[inline]
    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Lifecycle marker — `Shell` until the TUI detector promotes it.
    #[inline]
    pub fn kind(&self) -> super::kind::BlockKind {
        self.kind
    }

    /// Internal write access for the TUI detector path. Promotes the
    /// kind in place — sticky, idempotent.
    pub(super) fn promote_kind_to_tui(&mut self) {
        let was_tui = self.kind.is_tui();
        self.kind.promote_to_tui();
        if !was_tui {
            self.bump_generation();
        }
    }

    /// Access the selection slot (internal — the public surface lives
    /// in [`super::selection`] via inherent impl blocks on
    /// `ActiveBlock`).
    pub(super) fn selection_slot(&self) -> &Option<super::selection::BlockSelection> {
        &self.selection
    }

    /// Mutable access to the selection slot.
    pub(super) fn selection_slot_mut(&mut self) -> &mut Option<super::selection::BlockSelection> {
        self.bump_generation();
        &mut self.selection
    }

    /// Internal read access to the live-frame slot. Public API lives
    /// in [`super::live_frame`].
    pub(super) fn live_frame_slot(&self) -> &Option<super::live_frame::LiveFrameRegion> {
        &self.live_frame
    }

    /// Internal write access to the live-frame slot.
    pub(super) fn live_frame_slot_mut(
        &mut self,
    ) -> &mut Option<super::live_frame::LiveFrameRegion> {
        self.bump_generation();
        &mut self.live_frame
    }

    /// Access the captured PTY byte stream (for debug / replay).
    pub fn replay(&self) -> &ReplayBuffer {
        &self.replay
    }

    /// Record PTY bytes going into this block. The VT parser (via
    /// Terminal::advance) calls this with every incoming chunk
    /// before dispatching to the actual state machine. Truncation is
    /// silent; check `replay().is_truncated()` to detect it.
    pub fn record_bytes(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.replay.extend(bytes);
            self.bump_generation();
        }
    }

    /// Access the underlying page list (read-only).
    pub fn grid(&self) -> &PageList {
        &self.grid
    }

    /// Mutable access to the underlying page list. The VT writer uses
    /// this for row-level edits (insert/delete lines, scroll region).
    pub fn grid_mut(&mut self) -> &mut PageList {
        self.bump_generation();
        &mut self.grid
    }

    /// CellStyle atlas for this block.
    pub fn atlas(&self) -> &CellStyleAtlas {
        &self.atlas
    }

    /// Mutable style atlas for interning styles as SGR state changes.
    pub fn atlas_mut(&mut self) -> &mut CellStyleAtlas {
        self.bump_generation();
        &mut self.atlas
    }

    /// Image store for this block.
    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    /// OSC 8 hyperlink store.
    pub fn hyperlinks(&self) -> &HyperlinkStore {
        &self.hyperlinks
    }

    /// Mutable hyperlink store — the VT writer interns here when the
    /// remote emits `ESC ] 8 ; ... ; uri ST`.
    pub fn hyperlinks_mut(&mut self) -> &mut HyperlinkStore {
        self.bump_generation();
        &mut self.hyperlinks
    }

    /// Grapheme cluster store. Read by the renderer when a cell's
    /// `tag == CellTag::Grapheme` so it can resolve the full cluster
    /// UTF-8 from the indexed entry.
    pub fn graphemes(&self) -> &GraphemeStore {
        &self.graphemes
    }

    /// Mutable grapheme store — VtWriter interns here when a
    /// zero-width combiner attaches to the previous cell.
    pub fn graphemes_mut(&mut self) -> &mut GraphemeStore {
        self.bump_generation();
        &mut self.graphemes
    }

    /// Current metadata (mutable for the terminal core to fill in).
    pub fn metadata(&self) -> &ActiveMetadata {
        &self.metadata
    }

    /// Mutably access metadata to set command/cwd/etc. at command-start.
    pub fn metadata_mut(&mut self) -> &mut ActiveMetadata {
        self.bump_generation();
        &mut self.metadata
    }

    /// Intern a style and return its id. See [`CellStyleAtlas::intern`].
    pub fn intern_style(&mut self, style: CellStyle) -> CellStyleId {
        self.bump_generation();
        self.atlas.intern(style)
    }

    /// Append a row of cells to the grid. O(1) amortized.
    pub fn append_row(&mut self, row: &[Cell]) {
        self.grid.append_row(row);
        self.bump_generation();
    }

    /// Mark a specific row dirty for the next render pass.
    pub fn mark_dirty(&mut self, row: usize) {
        self.grid.mark_dirty(row);
        self.bump_generation();
    }

    /// Access the image store mutably — the VT state machine appends
    /// entries here when it sees image protocol sequences.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        self.bump_generation();
        &mut self.images
    }

    /// Number of rows in the block right now.
    pub fn total_rows(&self) -> usize {
        self.grid.total_rows()
    }

    /// Finalize the block at command-end.
    ///
    /// Consumes `self` and returns an [`Arc<FrozenBlock>`] ready to hand
    /// to the render thread. The PageList is handed over as-is; the
    /// style atlas and image store are frozen via `Arc`. The replay
    /// buffer moves with the block — finished blocks retain it for
    /// theme-change replays.
    pub fn finish(self, exit_code: Option<i32>, finished_at: Option<Instant>) -> Arc<FrozenBlock> {
        let atlas_snapshot: Arc<[CellStyle]> = Arc::from(self.atlas.as_slice().to_vec());
        Arc::new(FrozenBlock::new(
            self.grid,
            atlas_snapshot,
            self.images,
            self.hyperlinks,
            self.graphemes,
            self.metadata,
            exit_code,
            finished_at,
            self.replay,
            self.kind,
        ))
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    use carrot_grid::{Cell, CellStyle};

    #[test]
    fn generation_starts_above_zero() {
        let block = ActiveBlock::new(80);
        // Zero is the never-observed sentinel for cache consumers; the
        // initial state is already a real value.
        assert!(block.generation() > 0);
    }

    #[test]
    fn generation_advances_on_append_row() {
        let mut block = ActiveBlock::new(4);
        let g0 = block.generation();
        block.append_row(&[Cell::ascii(b'x', CellStyleId(0)); 4]);
        assert!(block.generation() > g0);
    }

    #[test]
    fn generation_advances_on_grid_mut() {
        let mut block = ActiveBlock::new(4);
        let g0 = block.generation();
        let _ = block.grid_mut();
        assert!(block.generation() > g0);
    }

    #[test]
    fn generation_advances_on_intern_style() {
        let mut block = ActiveBlock::new(4);
        let g0 = block.generation();
        let _ = block.intern_style(CellStyle::DEFAULT);
        assert!(block.generation() > g0);
    }

    #[test]
    fn generation_advances_on_record_bytes() {
        let mut block = ActiveBlock::new(4);
        let g0 = block.generation();
        block.record_bytes(b"hello");
        assert!(block.generation() > g0);
    }

    #[test]
    fn generation_holds_on_empty_record() {
        let mut block = ActiveBlock::new(4);
        let g0 = block.generation();
        block.record_bytes(&[]);
        // Empty input is not a mutation — the cache must not invalidate.
        assert_eq!(block.generation(), g0);
    }

    #[test]
    fn generation_advances_on_in_place_grid_writes() {
        // The bug this whole counter exists to fix: write to a single
        // row repeatedly, total_rows stays flat, generation must still
        // advance so the render-side cache invalidates.
        let mut block = ActiveBlock::new(4);
        let pre_rows = block.total_rows();
        let g0 = block.generation();
        let _ = block.grid_mut();
        let _ = block.grid_mut();
        let _ = block.grid_mut();
        assert_eq!(block.total_rows(), pre_rows);
        assert!(block.generation() >= g0 + 3);
    }

    #[test]
    fn generation_holds_on_idempotent_tui_promotion() {
        let mut block = ActiveBlock::new(4);
        block.promote_kind_to_tui();
        let g_after_first = block.generation();
        block.promote_kind_to_tui();
        // Already-TUI second call must not bump — the kind is sticky and
        // re-promotion is a true no-op, not a state change.
        assert_eq!(block.generation(), g_after_first);
    }
}
