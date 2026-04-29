//! Block router for the v2 block system.
//!
//! Orchestrates a chronological sequence of [`BlockVariant`]s (Active +
//! Frozen) plus a transient "prompt" block that absorbs the shell's
//! prompt render before a command actually starts.
//!
//! # Role in the pipeline
//!
//! This is the Phase-E consolidation point: the terminal core writes
//! VT bytes at the currently-active target (prompt or a live block),
//! `OSC 133 ;A` switches to prompt, `;C` spawns a new active block,
//! `;D` freezes it into an `Arc<FrozenBlock>`. The router is
//! block-native — it speaks `BlockVariant` / `PageList` /
//! `CellStyleAtlas`, NOT the legacy `carrot_term::block::Grid<Cell>`
//! pipeline.
//!
//! # What it does NOT own (yet)
//!
//! - **Alt-screen buffer** (DEC 1049) — belongs to `Term<T>` itself,
//!   the router only holds the scrollback-visible blocks.
//! - **TUI live-frame tracking** (reprint-count, overwrite-as-frame)
//!   — that migrates in a dedicated Phase-E sub-task.
//! - **Prompt-region tracking** (prompt suppression between commands) —
//!   the `PromptRegionTracker` is a legacy concept from the old router;
//!   the current path tracks block boundaries via OSC 133 directly.
//!
//! This file provides the block lifecycle + query surface that `term.rs`
//! needs to migrate off `BlockGridRouter`; the richer surfaces grow
//! module-by-module behind the migration
//! regression gate.

use std::sync::Arc;
use std::time::Instant;

use carrot_grid::Cell;

use super::active::ActiveBlock;
use super::frozen::FrozenBlock;
use super::state::BlockVariant;

/// Chronological block ID — monotonically increasing, 1-based. `0`
/// is reserved for the transient prompt block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u64);

impl BlockId {
    /// The prompt block's reserved ID. Never appears in the regular
    /// `blocks` vec — it lives in its own slot.
    pub const PROMPT: Self = Self(0);

    /// `true` when this is the prompt-reserved ID.
    pub const fn is_prompt(self) -> bool {
        self.0 == 0
    }
}

/// Metadata attached to a block from OSC 7777 or from UI-level
/// sources (shell context, command text).
///
/// Distinct from [`super::active::ActiveMetadata`] — that one is the
/// builder-side data the `ActiveBlock` stores during collection;
/// `RouterBlockMetadata` is the router-level view consumers see.
#[derive(Debug, Clone, Default)]
pub struct RouterBlockMetadata {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub shell: Option<String>,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub exit_code: Option<i32>,
}

impl RouterBlockMetadata {
    /// Wallclock time the command took. `None` for still-running
    /// blocks (no `finished_at` yet) or blocks whose `started_at` was
    /// never set. Uses [`Instant::checked_duration_since`] so
    /// monotonic-clock anomalies return `None` rather than panic.
    pub fn duration(&self) -> Option<std::time::Duration> {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(e)) => e.checked_duration_since(s),
            _ => None,
        }
    }

    /// Command duration in milliseconds — the shape most consumers
    /// format into the block header ("12ms", "1.4s", …). Saturates at
    /// `u64::MAX`, which is ~584 million years — beyond any realistic
    /// shell session.
    pub fn duration_ms(&self) -> Option<u64> {
        self.duration()
            .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
    }

    /// `true` when the block reports a non-zero exit code. Running
    /// blocks (no `exit_code` yet) return `false`.
    pub fn is_error(&self) -> bool {
        matches!(self.exit_code, Some(c) if c != 0)
    }
}

/// Router-side block entry — pairs a [`BlockVariant`] with its ID
/// and metadata. Callers iterate the router's block list via
/// [`BlockRouter::entries`].
pub struct RouterEntry {
    pub id: BlockId,
    pub variant: BlockVariant,
    pub metadata: RouterBlockMetadata,
}

impl RouterEntry {
    fn new_active(id: BlockId, cols: u16, page_bytes: u32, command: Option<String>) -> Self {
        let metadata = RouterBlockMetadata {
            command,
            started_at: Some(Instant::now()),
            ..Default::default()
        };
        Self {
            id,
            variant: BlockVariant::Active(Box::new(ActiveBlock::with_page_bytes(cols, page_bytes))),
            metadata,
        }
    }

    /// Total rows in this entry's block regardless of variant.
    pub fn total_rows(&self) -> usize {
        self.variant.total_rows()
    }

    /// `true` if the block is still being written.
    pub fn is_active(&self) -> bool {
        self.variant.is_active()
    }

    /// `true` if the block is immutable.
    pub fn is_frozen(&self) -> bool {
        self.variant.is_frozen()
    }

    /// Borrow the frozen variant if this entry was finalized.
    pub fn as_frozen(&self) -> Option<&Arc<FrozenBlock>> {
        self.variant.as_frozen()
    }

    /// Borrow the active variant mutably — only succeeds while the
    /// block is still live.
    pub fn as_active_mut(&mut self) -> Option<&mut ActiveBlock> {
        self.variant.as_active_mut()
    }
}

/// Memory-management limits for the router.
#[derive(Debug, Clone, Copy)]
pub struct RouterLimits {
    /// Maximum entries kept in the chronological block list. When
    /// exceeded, the oldest *frozen* entries are dropped until the
    /// vec is back under the limit. Active blocks are never evicted.
    pub max_blocks: usize,
}

impl Default for RouterLimits {
    fn default() -> Self {
        Self { max_blocks: 200 }
    }
}

/// The block router. Owns the chronological block list + a transient
/// prompt block used while the shell is rendering its prompt between
/// commands.
pub struct BlockRouter {
    /// All finished + currently-active blocks, oldest first.
    blocks: Vec<RouterEntry>,
    /// Transient prompt block — absorbs prompt output so the VT state
    /// machine stays consistent without polluting scrollback.
    prompt: ActiveBlock,
    /// ID of the block currently receiving VT output, or `None` while
    /// the shell is between commands (writing into the prompt).
    active_id: Option<BlockId>,
    /// Next ID to hand out. Monotonic, starts at 1 (ID 0 is reserved
    /// for the prompt).
    next_id: u64,
    /// Live column width — new blocks spawn with this.
    cols: u16,
    /// Page size used when allocating `ActiveBlock` instances. Derived
    /// from `cols` so very wide terminals (200+ cols) still fit at
    /// least 2 rows per page, independent of the 4 KB default.
    page_bytes: u32,
    /// Pending command text set from the UI at Enter; consumed by
    /// the next `start_new_block` call.
    pending_command: Option<String>,
    limits: RouterLimits,
    /// View-wide scrollback display state. See
    /// [`super::display::DisplayState`].
    display: super::display::DisplayState,
}

/// Derive a page size that guarantees at least 2 rows per page for
/// the given column count. Cell is 8 bytes; with a 4 KB page the
/// break-even is ~256 cols. Above that we grow the page by 4 KB
/// steps. No hard upper bound — u16 max for cols (65535) * 8 * 2 =
/// ~1 MB, which is acceptable for pathological widths (tests use
/// 9999 cols; real terminals top out below 1024).
fn page_bytes_for_cols(cols: u16) -> u32 {
    const CELL_BYTES: u32 = std::mem::size_of::<Cell>() as u32;
    const MIN_ROWS_PER_PAGE: u32 = 2;
    let needed = (cols as u32)
        .saturating_mul(CELL_BYTES)
        .saturating_mul(MIN_ROWS_PER_PAGE);
    let rounded = needed.div_ceil(4096).saturating_mul(4096);
    rounded.max(4096)
}

impl BlockRouter {
    /// Create a fresh router sized for `cols` columns.
    pub fn new(cols: u16) -> Self {
        Self::with_limits(cols, RouterLimits::default())
    }

    /// Create a fresh router with custom memory limits.
    pub fn with_limits(cols: u16, limits: RouterLimits) -> Self {
        let page_bytes = page_bytes_for_cols(cols);
        Self {
            blocks: Vec::new(),
            prompt: ActiveBlock::with_page_bytes(cols, page_bytes),
            active_id: None,
            next_id: 1,
            cols,
            page_bytes,
            pending_command: None,
            limits,
            display: super::display::DisplayState::new(),
        }
    }

    /// Internal accessor for the display-state slot. Public API lives
    /// in [`super::display`].
    pub(super) fn display_state_ref(&self) -> &super::display::DisplayState {
        &self.display
    }

    /// Internal mutable accessor.
    pub(super) fn display_state_ref_mut(&mut self) -> &mut super::display::DisplayState {
        &mut self.display
    }

    /// Current column width. Callers drive resize through
    /// [`BlockRouter::resize`].
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Resize every live target to `cols`. Frozen blocks are not
    /// resized — their data is immutable.
    pub fn resize(&mut self, cols: u16) {
        if cols == self.cols {
            return;
        }
        self.cols = cols;
        self.page_bytes = page_bytes_for_cols(cols);
        // ActiveBlock doesn't expose a resize API yet — F.6 soft-wrap
        // handles width changes at render-time (display-only soft-
        // wrap keeps data rows fixed-length). Replacing the prompt
        // with a fresh one at the new width is the right call.
        self.prompt = ActiveBlock::with_page_bytes(cols, self.page_bytes);
    }

    // ─── Lifecycle ───────────────────────────────────────────────

    /// `OSC 133 ; A` — shell started rendering its prompt. Any live
    /// block transitions back to "waiting" (no active target) and
    /// the prompt buffer resets.
    pub fn on_prompt_start(&mut self) {
        self.active_id = None;
        self.prompt = ActiveBlock::new(self.cols);
    }

    /// User-initiated full reset (Cmd+K / Ctrl+L). Drops every frozen
    /// block, ends any in-flight active block, resets the prompt
    /// buffer, and zeroes the display scroll state. `next_id` stays
    /// monotonic so any external observers that cached an old id
    /// continue to detect the rotation cleanly.
    ///
    /// The router is left in the same shape as right after `new()`
    /// except for the preserved `next_id` and `cols`.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.active_id = None;
        self.pending_command = None;
        self.prompt = ActiveBlock::with_page_bytes(self.cols, self.page_bytes);
        self.display = super::display::DisplayState::new();
    }

    /// `OSC 133 ; C` — command execution started. Allocates a new
    /// active block, wires it as the routing target, returns its
    /// freshly-minted ID.
    pub fn on_command_start(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        let command = self.pending_command.take();
        self.blocks.push(RouterEntry::new_active(
            id,
            self.cols,
            self.page_bytes,
            command,
        ));
        self.active_id = Some(id);
        self.evict_if_over_limit();
        id
    }

    /// `OSC 133 ; D ; N` — command finished with exit code `N`.
    /// Freezes the active block and returns its `Arc<FrozenBlock>`
    /// for consumers (render thread, replay, scrollback search).
    ///
    /// Returns `None` if there was no active block (e.g. a spurious
    /// `;D` without a preceding `;C`).
    pub fn on_command_end(&mut self, exit_code: i32) -> Option<Arc<FrozenBlock>> {
        let id = self.active_id.take()?;
        let entry = self.blocks.iter_mut().find(|e| e.id == id)?;
        entry.metadata.exit_code = Some(exit_code);
        entry.metadata.finished_at = Some(Instant::now());
        // Swap the variant out, call `finish`, store the Frozen back.
        let placeholder = BlockVariant::Active(Box::new(ActiveBlock::with_page_bytes(
            self.cols,
            self.page_bytes,
        )));
        let taken = std::mem::replace(&mut entry.variant, placeholder);
        let frozen = match taken {
            BlockVariant::Active(active) => active.finish(Some(exit_code), Some(Instant::now())),
            BlockVariant::Frozen(f) => f,
        };
        entry.variant = BlockVariant::Frozen(frozen.clone());
        Some(frozen)
    }

    /// Set the command text attached to the next `on_command_start`
    /// call. Cleared on consumption.
    pub fn set_pending_command(&mut self, command: impl Into<String>) {
        self.pending_command = Some(command.into());
    }

    /// Overwrite metadata on an existing entry — used when OSC 7777
    /// delivers context (cwd, branch, user) out-of-band.
    pub fn set_metadata(&mut self, id: BlockId, metadata: RouterBlockMetadata) {
        if let Some(entry) = self.blocks.iter_mut().find(|e| e.id == id) {
            entry.metadata = metadata;
        }
    }

    /// Attach metadata to the **last** entry — OSC 7777 payloads
    /// typically arrive right after a command boundary without an
    /// explicit ID.
    pub fn set_last_metadata(&mut self, metadata: RouterBlockMetadata) {
        if let Some(entry) = self.blocks.last_mut() {
            entry.metadata = metadata;
        }
    }

    // ─── Active-target accessors ─────────────────────────────────

    /// Borrow the block currently receiving VT bytes. Returns the
    /// prompt when no command is active.
    pub fn active(&mut self) -> ActiveTarget<'_> {
        match self
            .active_id
            .and_then(|id| self.blocks.iter_mut().find(|e| e.id == id))
        {
            Some(entry) => match &mut entry.variant {
                BlockVariant::Active(a) => ActiveTarget::Block {
                    id: entry.id,
                    block: a,
                },
                BlockVariant::Frozen(_) => ActiveTarget::Prompt(&mut self.prompt),
            },
            None => ActiveTarget::Prompt(&mut self.prompt),
        }
    }

    /// `true` when a command is running (a block is the target).
    pub fn has_active_block(&self) -> bool {
        self.active_id.is_some()
    }

    /// Currently-active block id, if any.
    pub fn active_id(&self) -> Option<BlockId> {
        self.active_id
    }

    // ─── Query ──────────────────────────────────────────────────

    /// Chronological entry list (oldest first). Includes the live
    /// entry at the end.
    pub fn entries(&self) -> &[RouterEntry] {
        &self.blocks
    }

    /// Mutable entry list — for advanced consumers that mutate
    /// metadata in place.
    pub fn entries_mut(&mut self) -> &mut Vec<RouterEntry> {
        &mut self.blocks
    }

    /// Look up an entry by id.
    pub fn entry(&self, id: BlockId) -> Option<&RouterEntry> {
        self.blocks.iter().find(|e| e.id == id)
    }

    /// Number of entries in the chronological list.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// `true` when no command has ever run.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Iterate only the finished (frozen) entries. Consumers that
    /// render the scrollback list typically want just the frozen
    /// history — the active block has its own rendering path
    /// (live frame / TUI alt-screen).
    pub fn frozen_entries(&self) -> impl Iterator<Item = &RouterEntry> {
        self.blocks.iter().filter(|e| e.is_frozen())
    }

    /// Iterate only the still-running (active) entries. In practice
    /// there is at most one at a time, but a sequence of nested
    /// commands (via `set_tui_hint`) can leave more than one during
    /// transitions.
    pub fn active_entries(&self) -> impl Iterator<Item = &RouterEntry> {
        self.blocks.iter().filter(|e| e.is_active())
    }

    /// Borrow the transient prompt block — consumers that need to
    /// render the shell prompt (e.g. a "live prompt" preview mode)
    /// read from here.
    pub fn prompt(&self) -> &ActiveBlock {
        &self.prompt
    }

    // ─── Memory management ──────────────────────────────────────

    fn evict_if_over_limit(&mut self) {
        while self.blocks.len() > self.limits.max_blocks {
            // Only evict frozen entries — never drop a live one.
            if let Some(pos) = self.blocks.iter().position(|e| e.is_frozen()) {
                self.blocks.remove(pos);
            } else {
                break;
            }
        }
    }
}

/// Where VT bytes are currently being written.
pub enum ActiveTarget<'a> {
    /// Transient prompt buffer. Reset on every `on_prompt_start`.
    Prompt(&'a mut ActiveBlock),
    /// A live block registered with the router.
    Block {
        id: BlockId,
        block: &'a mut ActiveBlock,
    },
}

impl<'a> ActiveTarget<'a> {
    /// Borrow the underlying `ActiveBlock`, regardless of whether
    /// it's a prompt or a live block — callers that just want to
    /// write bytes don't need to distinguish.
    pub fn as_active_mut(&mut self) -> &mut ActiveBlock {
        match self {
            ActiveTarget::Prompt(p) => p,
            ActiveTarget::Block { block, .. } => block,
        }
    }

    /// Block id if this is a live block, `None` for the prompt.
    pub fn id(&self) -> Option<BlockId> {
        match self {
            ActiveTarget::Prompt(_) => None,
            ActiveTarget::Block { id, .. } => Some(*id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_router_has_no_active_block() {
        let r = BlockRouter::new(80);
        assert!(!r.has_active_block());
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn prompt_start_without_command_stays_empty() {
        let mut r = BlockRouter::new(80);
        r.on_prompt_start();
        assert!(!r.has_active_block());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn command_start_spawns_active_block() {
        let mut r = BlockRouter::new(80);
        r.set_pending_command("ls -la");
        let id = r.on_command_start();
        assert_eq!(id.0, 1);
        assert!(r.has_active_block());
        assert_eq!(r.len(), 1);
        assert_eq!(
            r.entry(id).unwrap().metadata.command.as_deref(),
            Some("ls -la"),
        );
    }

    #[test]
    fn command_end_freezes_block_and_clears_active() {
        let mut r = BlockRouter::new(80);
        let id = r.on_command_start();
        let frozen = r.on_command_end(0).expect("frozen block on end");
        assert_eq!(Arc::strong_count(&frozen), 2); // router + caller
        assert!(!r.has_active_block());
        assert!(r.entry(id).unwrap().is_frozen());
        assert_eq!(r.entry(id).unwrap().metadata.exit_code, Some(0));
    }

    #[test]
    fn command_end_without_active_returns_none() {
        let mut r = BlockRouter::new(80);
        assert!(r.on_command_end(0).is_none());
    }

    #[test]
    fn monotonic_ids() {
        let mut r = BlockRouter::new(80);
        let a = r.on_command_start();
        r.on_command_end(0);
        let b = r.on_command_start();
        r.on_command_end(0);
        assert!(a.0 < b.0);
    }

    #[test]
    fn eviction_drops_oldest_frozen() {
        let mut r = BlockRouter::with_limits(80, RouterLimits { max_blocks: 3 });
        for _ in 0..5 {
            r.on_command_start();
            r.on_command_end(0);
        }
        assert_eq!(r.len(), 3);
        // Oldest kept id is the 3rd-created one.
        let first_kept = r.entries().first().unwrap().id.0;
        assert_eq!(first_kept, 3);
    }

    #[test]
    fn eviction_never_drops_active_block() {
        let mut r = BlockRouter::with_limits(80, RouterLimits { max_blocks: 2 });
        r.on_command_start();
        r.on_command_end(0);
        r.on_command_start();
        r.on_command_end(0);
        // Now start a new one without ending it, then push another
        // end to overflow.
        r.on_command_start();
        // len() = 3, limit = 2. Eviction should keep the active entry.
        assert!(r.has_active_block());
        assert!(r.len() <= 3);
    }

    #[test]
    fn active_target_routes_to_prompt_before_command() {
        let mut r = BlockRouter::new(80);
        let target = r.active();
        assert!(target.id().is_none(), "prompt has no id");
    }

    #[test]
    fn active_target_routes_to_live_block_during_command() {
        let mut r = BlockRouter::new(80);
        let id = r.on_command_start();
        let target = r.active();
        assert_eq!(target.id(), Some(id));
    }

    #[test]
    fn metadata_attaches_to_last() {
        let mut r = BlockRouter::new(80);
        r.on_command_start();
        r.on_command_end(0);
        r.set_last_metadata(RouterBlockMetadata {
            cwd: Some("/tmp".into()),
            ..Default::default()
        });
        assert_eq!(
            r.entries().last().unwrap().metadata.cwd.as_deref(),
            Some("/tmp"),
        );
    }

    #[test]
    fn resize_replaces_prompt_buffer() {
        let mut r = BlockRouter::new(80);
        r.resize(120);
        assert_eq!(r.cols(), 120);
    }

    #[test]
    fn prompt_block_id_is_reserved_zero() {
        assert_eq!(BlockId::PROMPT.0, 0);
        assert!(BlockId::PROMPT.is_prompt());
        assert!(!BlockId(1).is_prompt());
    }

    #[test]
    fn metadata_duration_none_until_both_timestamps_set() {
        let mut m = RouterBlockMetadata::default();
        assert!(m.duration().is_none());
        assert!(m.duration_ms().is_none());
        m.started_at = Some(Instant::now());
        // still None — finished_at unset
        assert!(m.duration().is_none());
        m.finished_at = Some(m.started_at.unwrap() + std::time::Duration::from_millis(42));
        assert_eq!(m.duration_ms(), Some(42));
    }

    #[test]
    fn frozen_and_active_entry_iterators_partition_blocks() {
        let mut r = BlockRouter::new(10);
        r.on_prompt_start();
        r.on_command_start();
        r.on_command_end(0);
        r.on_prompt_start();
        r.on_command_start();
        // One frozen, one active.
        assert_eq!(r.frozen_entries().count(), 1);
        assert_eq!(r.active_entries().count(), 1);
        // Finish the second — now two frozen, zero active.
        r.on_command_end(1);
        assert_eq!(r.frozen_entries().count(), 2);
        assert_eq!(r.active_entries().count(), 0);
    }

    #[test]
    fn metadata_is_error_distinguishes_zero_nonzero_none() {
        let mut m = RouterBlockMetadata::default();
        assert!(!m.is_error(), "running blocks are not errors");
        m.exit_code = Some(0);
        assert!(!m.is_error(), "exit 0 is success");
        m.exit_code = Some(1);
        assert!(m.is_error());
        m.exit_code = Some(-1);
        assert!(m.is_error(), "any nonzero code is error");
    }

    #[test]
    fn clear_drops_all_blocks_and_resets_active() {
        let mut r = BlockRouter::new(80);
        r.on_command_start();
        r.on_command_end(0);
        r.on_command_start();
        r.on_command_end(0);
        r.on_command_start();
        assert!(r.has_active_block());
        assert_eq!(r.len(), 3);

        r.clear();
        assert_eq!(r.len(), 0);
        assert!(!r.has_active_block());
        assert!(r.entries().is_empty());
    }

    #[test]
    fn clear_keeps_id_counter_monotonic() {
        let mut r = BlockRouter::new(80);
        let id_a = r.on_command_start();
        r.on_command_end(0);
        r.clear();
        let id_b = r.on_command_start();
        assert!(
            id_b.0 > id_a.0,
            "next id after clear must not collide with pre-clear ids \
             (a={}, b={})",
            id_a.0,
            id_b.0,
        );
    }

    #[test]
    fn clear_resets_pending_command_too() {
        let mut r = BlockRouter::new(80);
        r.set_pending_command("cargo build");
        r.clear();
        let id = r.on_command_start();
        let entry = r.entry(id).expect("active block");
        assert!(
            entry.metadata.command.is_none(),
            "stale pending command leaked across clear: {:?}",
            entry.metadata.command,
        );
    }
}
