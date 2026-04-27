use super::{BlockConfig, BlockLifecycle, BlockMeasuringBehavior, BlockMetadata, ScrollBehavior};
use crate::{
    App, AvailableSpace, Bounds, Edges, FocusHandle, Pixels, Point, SharedString, Size, Window,
    point, px, size,
};
use inazuma_sum_tree::{Bias, SumTree};
use std::{cell::RefCell, rc::Rc};

/// Stable identifier for a block entry. Monotonic, primitive-internal.
///
/// IDs never change once assigned and remain valid after other entries are
/// removed — the caller can hold on to them across mutations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u64);

/// Half-open range of `BlockId`s, returned by `BlockState::visible_range`.
///
/// Not a `Range<BlockId>` because `BlockId` is not a step counter — there
/// can be gaps in the ID sequence after `remove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockIdRange {
    /// First block id in the range (inclusive).
    pub start: BlockId,
    /// Last block id in the range (inclusive).
    pub end: BlockId,
}

/// A scroll offset into the block list, in entry + sub-entry pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockOffset {
    /// Index of the entry the scroll cursor is in.
    pub entry_ix: usize,
    /// Pixels below that entry's top edge.
    pub offset_in_entry: Pixels,
}

/// Explicit fold state attached to a block entry.
///
/// - `Unfolded`: the block renders normally in the list.
/// - `Folded { summary }`: the block is replaced in the list by a compact
///   one-line summary row. The primitive surfaces the id + summary via
///   [`BlockState::folded_entries`] so the caller can render fold-lines
///   however they like.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FoldState {
    /// Block renders at full height.
    #[default]
    Unfolded,
    /// Block collapsed; the `summary` is whatever the caller wants to show
    /// in the fold-line (typically the command text or a truncation).
    Folded {
        /// The summary text rendered by the caller in the fold-line row.
        summary: SharedString,
    },
}

impl FoldState {
    /// Whether this entry is currently folded.
    pub fn is_folded(&self) -> bool {
        matches!(self, FoldState::Folded { .. })
    }
}

#[derive(Clone)]
pub(super) struct BlockEntry {
    pub(super) id: BlockId,
    pub(super) metadata: BlockMetadata,
    pub(super) size: Option<Size<Pixels>>,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) fold: FoldState,
}

impl BlockEntry {
    pub(super) fn size(&self) -> Option<Size<Pixels>> {
        self.size
    }
}

impl std::fmt::Debug for BlockEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockEntry")
            .field("id", &self.id)
            .field("measured", &self.size.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct BlockSummary {
    pub(super) count: usize,
    pub(super) measured_count: usize,
    pub(super) height: Pixels,
    pub(super) has_focus_handles: bool,
}

impl inazuma_sum_tree::Item for BlockEntry {
    type Summary = BlockSummary;

    fn summary(&self, _: ()) -> Self::Summary {
        let has_focus = self.focus_handle.is_some();
        match self.size {
            Some(size) => BlockSummary {
                count: 1,
                measured_count: 1,
                height: size.height,
                has_focus_handles: has_focus,
            },
            None => BlockSummary {
                count: 1,
                measured_count: 0,
                height: px(0.0),
                has_focus_handles: has_focus,
            },
        }
    }
}

impl inazuma_sum_tree::ContextLessSummary for BlockSummary {
    fn zero() -> Self {
        Default::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.count += other.count;
        self.measured_count += other.measured_count;
        self.height += other.height;
        self.has_focus_handles |= other.has_focus_handles;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryCount(pub usize);

#[derive(Clone, Debug, Default)]
pub(super) struct EntryHeight(pub Pixels);

impl inazuma_sum_tree::Dimension<'_, BlockSummary> for EntryCount {
    fn zero(_: ()) -> Self {
        Default::default()
    }
    fn add_summary(&mut self, s: &BlockSummary, _: ()) {
        self.0 += s.count;
    }
}

impl inazuma_sum_tree::Dimension<'_, BlockSummary> for EntryHeight {
    fn zero(_: ()) -> Self {
        Default::default()
    }
    fn add_summary(&mut self, s: &BlockSummary, _: ()) {
        self.0 += s.height;
    }
}

impl inazuma_sum_tree::SeekTarget<'_, BlockSummary, BlockSummary> for EntryCount {
    fn cmp(&self, other: &BlockSummary, _: ()) -> std::cmp::Ordering {
        Ord::cmp(&self.0, &other.count)
    }
}

impl inazuma_sum_tree::SeekTarget<'_, BlockSummary, BlockSummary> for EntryHeight {
    fn cmp(&self, other: &BlockSummary, _: ()) -> std::cmp::Ordering {
        // Block heights never contain NaN (they come from layout measurements
        // which clamp to finite values), so partial_cmp always returns Some.
        self.0
            .partial_cmp(&other.height)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Intrusive state for the `blocks()` element. Held by the owning entity,
/// shared with the rendered `Block` via an inner `Rc<RefCell<…>>`.
pub struct BlockState(pub(super) Rc<RefCell<StateInner>>);

impl Clone for BlockState {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl std::fmt::Debug for BlockState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlockState")
    }
}

pub(super) struct StateInner {
    pub(super) entries: SumTree<BlockEntry>,
    pub(super) id_to_ix: inazuma_collections::HashMap<BlockId, usize>,
    pub(super) next_id: u64,
    pub(super) logical_scroll_top: Option<BlockOffset>,
    pub(super) last_resolved_scroll_top: Option<BlockOffset>,
    pub(super) config: BlockConfig,
    pub(super) last_layout_bounds: Option<Bounds<Pixels>>,
    pub(super) last_padding: Option<Edges<Pixels>>,
    pub(super) last_leading_space: Pixels,
    pub(super) follow_tail: bool,
    pub(super) reset: bool,
    pub(super) measured_all: bool,
    /// Optional pinned footer id — when set, this entry is rendered at
    /// the bottom of the viewport outside the normal scroll flow.
    /// Use case: TUI active frame stays visible while shell blocks scroll.
    pub(super) pinned_footer: Option<BlockId>,
}

impl BlockState {
    /// Create a new block state with the given configuration.
    pub fn new(config: BlockConfig) -> Self {
        let follow_tail = matches!(config.scroll_behavior, ScrollBehavior::FollowTail);
        Self(Rc::new(RefCell::new(StateInner {
            entries: SumTree::default(),
            id_to_ix: inazuma_collections::HashMap::default(),
            next_id: 0,
            logical_scroll_top: None,
            last_resolved_scroll_top: None,
            config,
            last_layout_bounds: None,
            last_padding: None,
            last_leading_space: px(0.0),
            follow_tail,
            reset: false,
            measured_all: false,
            pinned_footer: None,
        })))
    }

    /// Append a new block entry. Returns its stable id.
    pub fn push(&self, metadata: BlockMetadata, focus_handle: Option<FocusHandle>) -> BlockId {
        let mut state = self.0.borrow_mut();
        let id = BlockId(state.next_id);
        state.next_id += 1;
        let entry = BlockEntry {
            id,
            metadata,
            size: None,
            focus_handle,
            fold: FoldState::Unfolded,
        };
        let ix = state.entries.summary().count;
        state.entries.push(entry, ());
        state.id_to_ix.insert(id, ix);
        state.measured_all = false;
        id
    }

    /// Remove the block with the given id. No-op if the id is not known.
    /// If the removed block was the pinned footer, the pin is cleared
    /// — leaving a stale `pinned_footer` id pointing at a removed entry
    /// would resolve to garbage in `layout::compute()`.
    pub fn remove(&self, id: BlockId) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.rebuild_without(ix);
        state.id_to_ix.remove(&id);
        for (_, stored_ix) in state.id_to_ix.iter_mut() {
            if *stored_ix > ix {
                *stored_ix -= 1;
            }
        }
        if state.pinned_footer == Some(id) {
            state.pinned_footer = None;
        }
    }

    /// Mutate the metadata of a block in-place.
    pub fn update_metadata(&self, id: BlockId, f: impl FnOnce(&mut BlockMetadata)) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.update_entry(ix, |entry| f(&mut entry.metadata));
    }

    /// Update the measured size of a block (e.g. as output streams in).
    pub fn update_size(&self, id: BlockId, size: Size<Pixels>) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.update_entry(ix, |entry| entry.size = Some(size));
    }

    /// Dispatch a lifecycle event to the block with the given id.
    pub fn on_lifecycle(&self, id: BlockId, event: BlockLifecycle) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.update_entry(ix, |entry| match event {
            BlockLifecycle::CommandEnd { exit_code } => {
                entry.metadata.exit_code = Some(exit_code);
                entry.metadata.finished_at = Some(std::time::Instant::now());
            }
            BlockLifecycle::CommandStart => {
                entry.metadata.started_at = Some(std::time::Instant::now());
            }
            BlockLifecycle::PromptStart | BlockLifecycle::InputStart => {}
        });
    }

    /// Scroll the viewport so the given block is at the top of the visible range.
    pub fn scroll_to_reveal(&self, id: BlockId) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.logical_scroll_top = Some(BlockOffset {
            entry_ix: ix,
            offset_in_entry: px(0.0),
        });
    }

    /// Scroll past the last block and re-engage `FollowTail`.
    pub fn scroll_to_end(&self) {
        let mut state = self.0.borrow_mut();
        let count = state.entries.summary().count;
        state.logical_scroll_top = Some(BlockOffset {
            entry_ix: count,
            offset_in_entry: px(0.0),
        });
        state.follow_tail = true;
    }

    /// Ids of blocks that have scrolled above the viewport's top edge.
    ///
    /// These are the entries the consumer should render as compact fold-lines
    /// at the top of the pane. Sorted in storage order (oldest first).
    ///
    /// Note: this is the implicit "scroll-derived" fold set. For user-driven
    /// fold state, use [`Self::folded_entries`] or [`Self::is_folded`].
    pub fn folded_ids(&self) -> Vec<BlockId> {
        let state = self.0.borrow();
        let scroll_top = state
            .last_resolved_scroll_top
            .unwrap_or_else(|| state.logical_scroll_top());
        state
            .entries
            .iter()
            .take(scroll_top.entry_ix)
            .map(|e| e.id)
            .collect()
    }

    /// Mark the block with `id` as folded. When folded, the block is
    /// replaced by a compact summary row — the renderer queries
    /// [`Self::folded_entries`] to get `(id, summary)` pairs and picks its
    /// own visual treatment.
    ///
    /// Idempotent: re-folding a folded block replaces the summary.
    /// No-op if the id is unknown.
    pub fn fold(&self, id: BlockId, summary: SharedString) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.update_entry(ix, |entry| {
            entry.fold = FoldState::Folded {
                summary: summary.clone(),
            };
            // Fold invalidates the cached size — re-measure on next layout.
            entry.size = None;
        });
    }

    /// Clear the fold state for `id`. No-op if the id is unknown or not folded.
    pub fn unfold(&self, id: BlockId) {
        let mut state = self.0.borrow_mut();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return;
        };
        state.update_entry(ix, |entry| {
            entry.fold = FoldState::Unfolded;
            entry.size = None;
        });
    }

    /// Whether the block with `id` is currently folded. `false` for unknown ids.
    pub fn is_folded(&self, id: BlockId) -> bool {
        let state = self.0.borrow();
        let Some(&ix) = state.id_to_ix.get(&id) else {
            return false;
        };
        state
            .entries
            .iter()
            .nth(ix)
            .map(|e| e.fold.is_folded())
            .unwrap_or(false)
    }

    /// All folded entries with their summaries, in storage order.
    ///
    /// The renderer uses this to emit fold-lines. Separate from
    /// [`Self::folded_ids`] — this is the **explicit user-driven** fold
    /// set, not the scroll-derived one.
    pub fn folded_entries(&self) -> Vec<(BlockId, SharedString)> {
        let state = self.0.borrow();
        state
            .entries
            .iter()
            .filter_map(|e| match &e.fold {
                FoldState::Folded { summary } => Some((e.id, summary.clone())),
                FoldState::Unfolded => None,
            })
            .collect()
    }

    /// Pin the given block as a footer — it will render at the bottom of
    /// the viewport, outside the normal scroll flow. TUI-block use case:
    /// keep the running frame visible while older shell blocks scroll.
    ///
    /// Setting a new pin replaces any previous one. Passing an unknown id
    /// is a no-op.
    pub fn pin(&self, id: BlockId) {
        let mut state = self.0.borrow_mut();
        if state.id_to_ix.contains_key(&id) {
            state.pinned_footer = Some(id);
        }
    }

    /// Clear the pinned footer. No-op if nothing is pinned.
    pub fn unpin(&self) {
        self.0.borrow_mut().pinned_footer = None;
    }

    /// Currently pinned footer id, if any.
    pub fn pinned_id(&self) -> Option<BlockId> {
        self.0.borrow().pinned_footer
    }

    /// Iterator over ids currently inside the viewport. O(N_visible).
    ///
    /// Returned by value — the caller gets a `Vec<BlockId>` snapshot so
    /// there is no borrow back into the primitive's interior `RefCell`.
    /// Renderers use this to drive per-block GPU passes.
    pub fn visible_entries(&self) -> Vec<BlockId> {
        let state = self.0.borrow();
        let bounds = state.last_layout_bounds.unwrap_or_default();
        let scroll_top = state
            .last_resolved_scroll_top
            .unwrap_or_else(|| state.logical_scroll_top());
        let range = StateInner::visible_range(&state.entries, bounds.size.height, &scroll_top);
        state
            .entries
            .iter()
            .skip(range.start)
            .take(range.end.saturating_sub(range.start))
            .map(|e| e.id)
            .collect()
    }

    /// Id range of the blocks currently inside the viewport.
    pub fn visible_range(&self) -> BlockIdRange {
        let state = self.0.borrow();
        let bounds = state.last_layout_bounds.unwrap_or_default();
        let scroll_top = state
            .last_resolved_scroll_top
            .unwrap_or_else(|| state.logical_scroll_top());
        let range = StateInner::visible_range(&state.entries, bounds.size.height, &scroll_top);
        let start = state
            .entries
            .iter()
            .nth(range.start)
            .map(|e| e.id)
            .unwrap_or(BlockId(0));
        let end = state
            .entries
            .iter()
            .nth(range.end.saturating_sub(1))
            .map(|e| e.id)
            .unwrap_or(start);
        BlockIdRange { start, end }
    }

    /// Lookup the id of the block at a positional index.
    pub fn id_at_index(&self, ix: usize) -> Option<BlockId> {
        let state = self.0.borrow();
        state.entries.iter().nth(ix).map(|e| e.id)
    }

    /// Total number of entries currently in the state.
    pub fn entry_count(&self) -> usize {
        self.0.borrow().entries.summary().count
    }

    /// The configuration the state was built with.
    pub fn config(&self) -> BlockConfig {
        self.0.borrow().config
    }

    /// Current scroll offset in logical (entry + sub-pixel) terms.
    pub fn logical_scroll_top(&self) -> BlockOffset {
        self.0.borrow().logical_scroll_top()
    }

    /// Pixel bounds of the viewport at last layout. Zero until first paint.
    pub fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().last_layout_bounds.unwrap_or_default()
    }

    /// Pixels of blank space inserted above the first visible entry after
    /// the most recent layout. Non-zero for `VisualAnchor::Bottom` when the
    /// measured content is shorter than the viewport; always zero otherwise.
    pub fn leading_space(&self) -> Pixels {
        self.0.borrow().last_leading_space
    }

    /// Pixel length of the scrollable area — used by scrollbar components.
    pub fn max_offset_for_scrollbar(&self) -> Point<Pixels> {
        let state = self.0.borrow();
        let bounds = state.last_layout_bounds.unwrap_or_default();
        let height = state.entries.summary().height;
        point(Pixels::ZERO, Pixels::ZERO.max(height - bounds.size.height))
    }

    /// Bounds of a block entry in window coordinates, if it's measured and
    /// currently inside the viewport.
    pub fn bounds_for_block(&self, id: BlockId) -> Option<Bounds<Pixels>> {
        let state = &*self.0.borrow();
        let ix = *state.id_to_ix.get(&id)?;
        let bounds = state.last_layout_bounds.unwrap_or_default();
        let scroll_top = state
            .last_resolved_scroll_top
            .unwrap_or_else(|| state.logical_scroll_top());
        if ix < scroll_top.entry_ix {
            return None;
        }
        let mut cursor = state.entries.cursor::<BlockSummary>(());
        cursor.seek(&EntryCount(scroll_top.entry_ix), Bias::Right);
        let scroll_px = cursor.start().height + scroll_top.offset_in_entry;
        cursor.seek_forward(&EntryCount(ix), Bias::Right);
        if let Some(entry) = cursor.item() {
            if cursor.start().count == ix {
                let sz = entry.size()?;
                let top = bounds.top() + cursor.start().height - scroll_px;
                return Some(Bounds::from_corners(
                    crate::point(bounds.left(), top),
                    crate::point(bounds.right(), top + sz.height),
                ));
            }
        }
        None
    }

    /// Called when the user begins dragging the scrollbar thumb.
    pub fn scrollbar_drag_started(&self) {}

    /// Called when the user releases the scrollbar thumb.
    pub fn scrollbar_drag_ended(&self) {}

    /// Apply a scrollbar-driven offset (pixels from top).
    pub fn set_offset_from_scrollbar(&self, point: Point<Pixels>) {
        let target = -point.y;
        let mut state = self.0.borrow_mut();
        let entries = state.entries.clone();
        let (start, ..) =
            entries.find::<BlockSummary, _>((), &EntryHeight(target.max(px(0.0))), Bias::Right);
        state.logical_scroll_top = Some(BlockOffset {
            entry_ix: start.count,
            offset_in_entry: target - start.height,
        });
    }

    /// Current scroll offset in pixels — used by scrollbar components.
    pub fn scroll_px_offset_for_scrollbar(&self) -> Point<Pixels> {
        let state = self.0.borrow();
        let scroll_top = state
            .last_resolved_scroll_top
            .unwrap_or_else(|| state.logical_scroll_top());
        let mut cursor = state.entries.cursor::<BlockSummary>(());
        let summary: BlockSummary = cursor.summary(&EntryCount(scroll_top.entry_ix), Bias::Right);
        let offset = summary.height + scroll_top.offset_in_entry;
        Point::new(px(0.0), -offset)
    }
}

impl StateInner {
    pub(super) fn logical_scroll_top(&self) -> BlockOffset {
        self.logical_scroll_top
            .unwrap_or_else(|| match self.config.scroll_behavior {
                ScrollBehavior::FollowTail => BlockOffset {
                    entry_ix: self.entries.summary().count,
                    offset_in_entry: px(0.0),
                },
                ScrollBehavior::Manual => BlockOffset {
                    entry_ix: 0,
                    offset_in_entry: px(0.0),
                },
            })
    }

    pub(super) fn scroll_top_pixels(&self, scroll_top: &BlockOffset) -> Pixels {
        let mut cursor = self.entries.cursor::<BlockSummary>(());
        let summary: BlockSummary = cursor.summary(&EntryCount(scroll_top.entry_ix), Bias::Right);
        summary.height + scroll_top.offset_in_entry
    }

    pub(super) fn visible_range(
        entries: &SumTree<BlockEntry>,
        height: Pixels,
        scroll_top: &BlockOffset,
    ) -> std::ops::Range<usize> {
        let mut cursor = entries.cursor::<BlockSummary>(());
        cursor.seek(&EntryCount(scroll_top.entry_ix), Bias::Right);
        let start_y = cursor.start().height + scroll_top.offset_in_entry;
        cursor.seek_forward(&EntryHeight(start_y + height), Bias::Left);
        scroll_top.entry_ix..cursor.start().count + 1
    }

    pub(super) fn scroll(
        &mut self,
        scroll_top: &BlockOffset,
        height: Pixels,
        delta: Point<Pixels>,
    ) {
        if self.reset {
            return;
        }
        let padding = self.last_padding.unwrap_or_default();
        let scroll_max =
            (self.entries.summary().height + padding.top + padding.bottom - height).max(px(0.0));
        let new_scroll_top = (self.scroll_top_pixels(scroll_top) - delta.y)
            .max(px(0.0))
            .min(scroll_max);

        if matches!(self.config.scroll_behavior, ScrollBehavior::FollowTail)
            && new_scroll_top == scroll_max
        {
            self.logical_scroll_top = None;
        } else {
            let (start, ..) =
                self.entries
                    .find::<BlockSummary, _>((), &EntryHeight(new_scroll_top), Bias::Right);
            self.logical_scroll_top = Some(BlockOffset {
                entry_ix: start.count,
                offset_in_entry: new_scroll_top - start.height,
            });
        }

        if self.follow_tail && delta.y > px(0.0) {
            self.follow_tail = false;
        }
    }

    pub(super) fn update_entry(&mut self, ix: usize, f: impl FnOnce(&mut BlockEntry)) {
        let mut cursor = self.entries.cursor::<EntryCount>(());
        let mut new_entries: SumTree<BlockEntry> = cursor.slice(&EntryCount(ix), Bias::Right);
        if let Some(entry) = cursor.item() {
            let mut updated = entry.clone();
            f(&mut updated);
            new_entries.push(updated, ());
            cursor.next();
        }
        new_entries.append(cursor.suffix(), ());
        drop(cursor);
        self.entries = new_entries;
    }

    pub(super) fn rebuild_without(&mut self, ix: usize) {
        let mut cursor = self.entries.cursor::<EntryCount>(());
        let mut new_entries: SumTree<BlockEntry> = cursor.slice(&EntryCount(ix), Bias::Right);
        if cursor.item().is_some() {
            cursor.next();
        }
        new_entries.append(cursor.suffix(), ());
        drop(cursor);
        self.entries = new_entries;
    }
}

pub(super) fn ensure_all_measured(
    state: &mut StateInner,
    available_width: Pixels,
    render_entry: &mut RenderEntryFn,
    window: &mut Window,
    cx: &mut App,
) {
    if !matches!(state.config.measuring_behavior, BlockMeasuringBehavior::All) {
        return;
    }
    if state.measured_all {
        return;
    }
    let count = state.entries.summary().count;
    let available_item_space = size(
        AvailableSpace::Definite(available_width),
        AvailableSpace::MinContent,
    );
    let mut ids: Vec<BlockId> = Vec::with_capacity(count);
    for entry in state.entries.iter() {
        ids.push(entry.id);
    }
    for id in ids {
        let Some(&ix) = state.id_to_ix.get(&id) else {
            continue;
        };
        let needs_measure = state
            .entries
            .iter()
            .nth(ix)
            .map(|e| e.size.is_none())
            .unwrap_or(false);
        if !needs_measure {
            continue;
        }
        let mut element = render_entry(id, window, cx);
        let sz = element.layout_as_root(available_item_space, window, cx);
        state.update_entry(ix, |entry| entry.size = Some(sz));
    }
    state.measured_all = true;
}

pub(super) type RenderEntryFn =
    dyn FnMut(BlockId, &mut Window, &mut App) -> crate::AnyElement + 'static;
