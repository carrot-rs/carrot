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
        self.replay.extend(bytes);
    }

    /// Access the underlying page list (read-only).
    pub fn grid(&self) -> &PageList {
        &self.grid
    }

    /// Mutable access to the underlying page list. The VT writer uses
    /// this for row-level edits (insert/delete lines, scroll region).
    pub fn grid_mut(&mut self) -> &mut PageList {
        &mut self.grid
    }

    /// CellStyle atlas for this block.
    pub fn atlas(&self) -> &CellStyleAtlas {
        &self.atlas
    }

    /// Mutable style atlas for interning styles as SGR state changes.
    pub fn atlas_mut(&mut self) -> &mut CellStyleAtlas {
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
        &mut self.graphemes
    }

    /// Current metadata (mutable for the terminal core to fill in).
    pub fn metadata(&self) -> &ActiveMetadata {
        &self.metadata
    }

    /// Mutably access metadata to set command/cwd/etc. at command-start.
    pub fn metadata_mut(&mut self) -> &mut ActiveMetadata {
        &mut self.metadata
    }

    /// Intern a style and return its id. See [`CellStyleAtlas::intern`].
    pub fn intern_style(&mut self, style: CellStyle) -> CellStyleId {
        self.atlas.intern(style)
    }

    /// Append a row of cells to the grid. O(1) amortized.
    pub fn append_row(&mut self, row: &[Cell]) {
        self.grid.append_row(row);
    }

    /// Mark a specific row dirty for the next render pass.
    pub fn mark_dirty(&mut self, row: usize) {
        self.grid.mark_dirty(row);
    }

    /// Access the image store mutably — the VT state machine appends
    /// entries here when it sees image protocol sequences.
    pub fn images_mut(&mut self) -> &mut ImageStore {
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
        ))
    }
}
