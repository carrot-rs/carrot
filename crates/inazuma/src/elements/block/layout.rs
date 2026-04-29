use super::state::{BlockEntry, BlockOffset, EntryCount, RenderEntryFn, StateInner};
use super::{BlockId, VisualAnchor};
use crate::{App, AvailableSpace, Edges, Pixels, Window, px, size};
use inazuma_collections::VecDeque;
use inazuma_sum_tree::Bias;

pub(super) struct EntryLayout {
    pub(super) element: crate::AnyElement,
    pub(super) size: crate::Size<Pixels>,
}

pub(super) struct LayoutResponse {
    pub(super) scroll_top: BlockOffset,
    pub(super) leading_space: Pixels,
    pub(super) entry_layouts: VecDeque<EntryLayout>,
    /// Pinned footer block — rendered at the bottom of the viewport
    /// on top of the scrollable content. Reserved space is already
    /// subtracted from the main flow's available height.
    pub(super) pinned_footer: Option<EntryLayout>,
}

pub(super) fn layout_entries(
    state: &mut StateInner,
    available_width: Option<Pixels>,
    available_height: Pixels,
    padding: &Edges<Pixels>,
    render_entry: &mut RenderEntryFn,
    window: &mut Window,
    cx: &mut App,
) -> LayoutResponse {
    let available_item_space = size(
        available_width.map_or(AvailableSpace::MinContent, AvailableSpace::Definite),
        AvailableSpace::MinContent,
    );

    // Pinned footer is measured first so the main flow knows how
    // much vertical space is left. The footer block is excluded from
    // the scrollable entry list — it doesn't matter where in the
    // SumTree the block lives, it always renders at the bottom.
    let pinned_footer_id = state.pinned_footer;
    let pinned_footer = pinned_footer_id.map(|id| {
        let mut element = render_entry(id, window, cx);
        let element_size = element.layout_as_root(available_item_space, window, cx);
        EntryLayout {
            element,
            size: element_size,
        }
    });
    let footer_height = pinned_footer
        .as_ref()
        .map(|f| f.size.height)
        .unwrap_or(px(0.0));
    // Main flow has less room — the footer overlays the bottom.
    let main_available_height = (available_height - footer_height).max(px(0.0));

    let mut measured_items = VecDeque::new();
    let mut entry_layouts: VecDeque<EntryLayout> = VecDeque::new();
    let mut rendered_height = padding.top;
    let mut scroll_top = state.logical_scroll_top();

    if state.follow_tail {
        scroll_top = BlockOffset {
            entry_ix: state.entries.summary().count,
            offset_in_entry: px(0.0),
        };
        state.logical_scroll_top = Some(scroll_top);
    }

    let overdraw = state.config.overdraw;
    let old_entries = state.entries.clone();
    let mut cursor = old_entries.cursor::<EntryCount>(());
    cursor.seek(&EntryCount(scroll_top.entry_ix), Bias::Right);

    for entry in cursor.by_ref() {
        // Pinned footer (if any) is rendered separately — skip it here.
        if Some(entry.id) == pinned_footer_id {
            measured_items.push_back(BlockEntry {
                id: entry.id,
                metadata: entry.metadata.clone(),
                size: entry.size(),
                focus_handle: entry.focus_handle.clone(),
                fold: entry.fold.clone(),
            });
            continue;
        }
        let visible_height = rendered_height - scroll_top.offset_in_entry;
        if visible_height >= main_available_height + overdraw {
            break;
        }

        let mut measured_size = entry.size();

        if visible_height < main_available_height || measured_size.is_none() {
            let id: BlockId = entry.id;
            let mut element = render_entry(id, window, cx);
            let element_size = element.layout_as_root(available_item_space, window, cx);
            measured_size = Some(element_size);

            if visible_height < main_available_height {
                entry_layouts.push_back(EntryLayout {
                    element,
                    size: element_size,
                });
            }
        }

        let sz = measured_size.unwrap_or_default();
        rendered_height += sz.height;
        measured_items.push_back(BlockEntry {
            id: entry.id,
            metadata: entry.metadata.clone(),
            size: Some(sz),
            focus_handle: entry.focus_handle.clone(),
            fold: entry.fold.clone(),
        });
    }

    rendered_height += padding.bottom;

    cursor.seek(&EntryCount(scroll_top.entry_ix), Bias::Right);

    let mut leading_space = px(0.0);
    if rendered_height - scroll_top.offset_in_entry < main_available_height {
        while rendered_height < main_available_height {
            cursor.prev();
            let Some(entry) = cursor.item() else { break };
            if Some(entry.id) == pinned_footer_id {
                // Skip pinned footer when walking back up.
                continue;
            }
            let id = entry.id;
            let mut element = render_entry(id, window, cx);
            let element_size = element.layout_as_root(available_item_space, window, cx);
            rendered_height += element_size.height;
            measured_items.push_front(BlockEntry {
                id: entry.id,
                metadata: entry.metadata.clone(),
                size: Some(element_size),
                focus_handle: entry.focus_handle.clone(),
                fold: entry.fold.clone(),
            });
            entry_layouts.push_front(EntryLayout {
                element,
                size: element_size,
            });
        }

        // When the rendered content overflows the viewport, the topmost
        // entry has to be clipped from above by exactly the overshoot —
        // otherwise the LAST visible rows of the LAST entry slide off
        // the bottom edge as new content streams in. The clip lives on
        // the first entry's `offset_in_entry`; the per-entry paint loop
        // in `element.rs` consumes it via `start_y - offset_in_entry`.
        //
        // Mirrors Zed's `gpui::list::layout`
        // (`.reference/zed/crates/gpui/src/elements/list.rs:821-838`):
        // ports are line-for-line, but the overshoot assignment was
        // dropped on the way in. Recovering it fixes the
        // "active block taller than viewport ⇒ content scrolls out the
        // bottom while the user watches" regression.
        let overshoot = (rendered_height - main_available_height).max(px(0.0));
        scroll_top = BlockOffset {
            entry_ix: cursor.start().0,
            offset_in_entry: overshoot,
        };

        if rendered_height < main_available_height {
            let free = main_available_height - rendered_height;
            leading_space = match state.config.visual_anchor {
                VisualAnchor::Top => px(0.0),
                VisualAnchor::Bottom => free,
            };
        }
    }

    let measured_range = cursor.start().0..(cursor.start().0 + measured_items.len());
    let mut cursor = old_entries.cursor::<EntryCount>(());
    let mut new_entries = cursor.slice(&EntryCount(measured_range.start), Bias::Right);
    new_entries.extend(measured_items, ());
    cursor.seek(&EntryCount(measured_range.end), Bias::Right);
    new_entries.append(cursor.suffix(), ());
    drop(cursor);
    state.entries = new_entries;

    LayoutResponse {
        scroll_top,
        leading_space,
        entry_layouts,
        pinned_footer,
    }
}
