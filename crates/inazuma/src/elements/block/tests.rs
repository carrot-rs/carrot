use crate::FoldState;
use crate::{
    self as inazuma, AppContext, BlockConfig, BlockKind, BlockLifecycle, BlockMeasuringBehavior,
    BlockMetadata, BlockState, Context, IntoElement, Render, ScrollBehavior, ScrollDelta,
    ScrollWheelEvent, Styled, TestAppContext, VisualAnchor, Window, blocks, div, point, px, size,
};
use std::{cell::Cell, rc::Rc};

struct TestView {
    state: BlockState,
    height: Rc<Cell<f32>>,
}

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let h = self.height.get();
        blocks(self.state.clone(), move |_, _, _| {
            div().h(px(h)).w_full().into_any_element()
        })
        .w_full()
        .h_full()
    }
}

fn draw_view(
    cx: &mut crate::VisualTestContext,
    state: &BlockState,
    height: &Rc<Cell<f32>>,
    viewport: crate::Size<crate::Pixels>,
) {
    let state_clone = state.clone();
    let height_clone = height.clone();
    cx.draw(point(px(0.), px(0.)), viewport, move |_, cx| {
        cx.new(|_| TestView {
            state: state_clone,
            height: height_clone,
        })
        .into_any_element()
    });
}

#[inazuma::test]
fn visual_anchor_bottom_with_small_content_pins_to_bottom(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Bottom)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    state.push(BlockMetadata::default(), None);
    let height = Rc::new(Cell::new(300.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(1000.)));

    assert_eq!(state.leading_space(), px(700.));
    assert_eq!(state.entry_count(), 1);
}

#[inazuma::test]
fn visual_anchor_bottom_with_large_content_has_no_leading_space(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Bottom)
            .scroll_behavior(ScrollBehavior::FollowTail),
    );
    for _ in 0..10 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(200.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(1000.)));

    assert_eq!(state.leading_space(), px(0.));
    assert_eq!(state.entry_count(), 10);
}

#[inazuma::test]
fn visual_anchor_top_has_no_leading_space_when_short(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    state.push(BlockMetadata::default(), None);
    let height = Rc::new(Cell::new(300.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(1000.)));

    assert_eq!(state.leading_space(), px(0.));
}

#[inazuma::test]
fn streaming_update_of_entry_size_adjusts_total_height(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let id = state.push(BlockMetadata::default(), None);
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(500.)));

    state.update_size(id, size(px(100.), px(400.)));
    let max = state.max_offset_for_scrollbar();
    assert_eq!(max.y, px(0.)); // still fits in 500px viewport

    state.update_size(id, size(px(100.), px(800.)));
    let max = state.max_offset_for_scrollbar();
    assert_eq!(max.y, px(300.)); // 800 - 500 viewport
}

#[inazuma::test]
fn follow_tail_sticks_to_tail_on_new_entries(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::FollowTail),
    );
    for _ in 0..5 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(200.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(500.)));

    // 5 entries × 200 = 1000px, viewport 500px → tail is at ix 5.
    let scroll = state.logical_scroll_top();
    assert_eq!(scroll.entry_ix, 5);

    state.push(BlockMetadata::default(), None);
    draw_view(&mut cx, &state, &height, size(px(100.), px(500.)));
    let scroll = state.logical_scroll_top();
    assert_eq!(scroll.entry_ix, 6);
}

#[inazuma::test]
fn follow_tail_clips_topmost_entry_when_content_overflows(cx: &mut TestAppContext) {
    // Regression: with `VisualAnchor::Bottom` + `FollowTail`, when the
    // last entry alone is taller than the viewport, the layout pass must
    // clip the topmost visible entry from above by the overshoot — not
    // pin it to the top edge. Otherwise newly-streaming content inside a
    // tall active block scrolls off the bottom of the viewport while the
    // user watches. Verified through `bounds_for_block`: the entry's
    // bottom edge must coincide with the viewport's bottom edge, and
    // its top edge must sit `overshoot` pixels above the viewport top.
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Bottom)
            .scroll_behavior(ScrollBehavior::FollowTail)
            .measuring_behavior(BlockMeasuringBehavior::All),
    );
    let id = state.push(BlockMetadata::default(), None);
    let height = Rc::new(Cell::new(1000.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));

    let bounds = state.bounds_for_block(id).expect("entry is in viewport");
    // Bottom of the only entry sits exactly on the bottom edge.
    assert_eq!(bounds.bottom(), px(400.));
    // Top of the only entry is 600 px above the viewport top.
    assert_eq!(bounds.top(), px(-600.));
}

#[inazuma::test]
fn clear_drops_every_block_and_resets_caches(cx: &mut TestAppContext) {
    // `BlockState::clear` is the consumer-side mirror of
    // `BlockRouter::clear`. Both have to drop every entry, otherwise
    // the next render reads stale ids out of the sumtree and hits
    // the unwrap-or-default branch in layout.
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Bottom)
            .scroll_behavior(ScrollBehavior::FollowTail),
    );
    for _ in 0..5 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(80.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));
    assert_eq!(state.entry_count(), 5);

    state.clear();
    assert_eq!(state.entry_count(), 0);

    // After clear, push a fresh entry and confirm it lands at index 0
    // — id-to-index map was reset.
    let id = state.push(BlockMetadata::default(), None);
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));
    assert_eq!(state.entry_count(), 1);
    let bounds = state
        .bounds_for_block(id)
        .expect("freshly-pushed entry is visible after clear");
    assert!(bounds.bottom() <= px(400.));
}

#[inazuma::test]
fn manual_scroll_breaks_follow_tail(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::FollowTail)
            .measuring_behavior(BlockMeasuringBehavior::All),
    );
    for _ in 0..10 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(50.), px(50.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(200.))),
        ..Default::default()
    });
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));

    let scroll = state.logical_scroll_top();
    assert!(scroll.entry_ix < 10, "entry_ix was {}", scroll.entry_ix);
}

#[inazuma::test]
fn viewport_width_change_invalidates_measurements(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .measuring_behavior(BlockMeasuringBehavior::All),
    );
    for _ in 0..4 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(200.), px(600.)));
    assert_eq!(state.max_offset_for_scrollbar().y, px(0.));

    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));
    assert_eq!(state.max_offset_for_scrollbar().y, px(200.));
}

#[inazuma::test]
fn folded_ids_reports_entries_scrolled_above_viewport(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..6)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(300.)));
    assert_eq!(state.folded_ids(), Vec::<_>::new());

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(10.), px(10.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-250.))),
        ..Default::default()
    });
    draw_view(&mut cx, &state, &height, size(px(100.), px(300.)));

    let folded = state.folded_ids();
    assert!(!folded.is_empty());
    assert_eq!(folded[0], ids[0]);
}

#[inazuma::test]
fn scroll_to_reveal_returns_hidden_entry_to_viewport(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..10)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(300.)));

    state.scroll_to_reveal(ids[7]);
    draw_view(&mut cx, &state, &height, size(px(100.), px(300.)));

    assert!(!state.folded_ids().contains(&ids[7]));
    assert_eq!(state.logical_scroll_top().entry_ix, 7);
}

#[inazuma::test]
fn remove_entry_adjusts_total_height_and_preserves_ids(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..5)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));

    state.remove(ids[1]);
    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));

    assert_eq!(state.entry_count(), 4);
    // Surviving ids remain valid
    assert_eq!(state.id_at_index(0), Some(ids[0]));
    assert_eq!(state.id_at_index(1), Some(ids[2]));
    assert_eq!(state.id_at_index(3), Some(ids[4]));
}

#[inazuma::test]
fn block_ids_stay_stable_across_removals(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let ids: Vec<_> = (0..4)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();

    state.remove(ids[0]);
    state.remove(ids[2]);

    // ids[1] and ids[3] must still identify the originally-pushed entries.
    assert_eq!(state.entry_count(), 2);
    assert_eq!(state.id_at_index(0), Some(ids[1]));
    assert_eq!(state.id_at_index(1), Some(ids[3]));
}

#[inazuma::test]
fn thousand_entries_smoke_render_and_scroll(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    for _ in 0..1000 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(20.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));

    // Scroll into the middle of the list.
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(10.), px(10.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10_000.))),
        ..Default::default()
    });
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));
    assert!(state.logical_scroll_top().entry_ix > 0);

    // Back to the top.
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(10.), px(10.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(1_000_000.))),
        ..Default::default()
    });
    draw_view(&mut cx, &state, &height, size(px(100.), px(400.)));
    assert_eq!(state.logical_scroll_top().entry_ix, 0);
    assert_eq!(state.logical_scroll_top().offset_in_entry, px(0.));
}

#[inazuma::test]
fn measuring_behavior_all_populates_total_height_on_first_paint(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .measuring_behavior(BlockMeasuringBehavior::All),
    );
    for _ in 0..6 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(50.0));
    // 6 × 50 = 300 total; viewport 200 → max scroll 100.
    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));
    assert_eq!(state.max_offset_for_scrollbar().y, px(100.));
}

#[inazuma::test]
fn measuring_behavior_visible_skips_offscreen_entries(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    // Default measuring_behavior is Visible — off-screen entries remain
    // unmeasured until the viewport grows to cover them or the user scrolls.
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual)
            .measuring_behavior(BlockMeasuringBehavior::Visible),
    );
    for _ in 0..10 {
        state.push(BlockMetadata::default(), None);
    }
    let height = Rc::new(Cell::new(50.0));
    // Viewport 100 covers only 2 entries; the remaining 8 are off-screen.
    // Under Visible, the scrollbar reflects only the measured subset so the
    // reported max_offset is < full content (< 10 × 50 − 100 = 400).
    draw_view(&mut cx, &state, &height, size(px(100.), px(100.)));
    let visible_max = state.max_offset_for_scrollbar().y;

    // Contrast: under All, the primitive must measure every entry.
    let state_all = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual)
            .measuring_behavior(BlockMeasuringBehavior::All),
    );
    for _ in 0..10 {
        state_all.push(BlockMetadata::default(), None);
    }
    draw_view(&mut cx, &state_all, &height, size(px(100.), px(100.)));
    let all_max = state_all.max_offset_for_scrollbar().y;

    assert_eq!(all_max, px(400.));
    assert!(
        visible_max < all_max,
        "Visible mode must not pre-measure off-screen entries (got visible={visible_max:?} vs all={all_max:?})"
    );
}

#[inazuma::test]
fn update_metadata_applies_closure_in_place(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let id = state.push(
        BlockMetadata {
            kind: BlockKind::Shell,
            cwd: Some("/tmp".into()),
            ..Default::default()
        },
        None,
    );
    state.update_metadata(id, |m| {
        m.kind = BlockKind::Tui;
        m.cwd = Some("/home".into());
        m.git_branch = Some("main".into());
        m.duration_ms = Some(1234);
    });
    // Round-trip via update_metadata again — no getter exposes metadata, but
    // no panic + subsequent mutation work confirms the entry is still there.
    state.update_metadata(id, |m| {
        assert_eq!(m.kind, BlockKind::Tui);
        assert_eq!(m.cwd.as_deref(), Some("/home"));
        assert_eq!(m.git_branch.as_deref(), Some("main"));
        assert_eq!(m.duration_ms, Some(1234));
    });
    // No-op on unknown id: closure must not be called.
    let called = Rc::new(Cell::new(false));
    let called_clone = called.clone();
    state.update_metadata(inazuma::BlockId(999_999), move |_| {
        called_clone.set(true);
    });
    assert!(!called.get());
}

#[inazuma::test]
fn lifecycle_command_end_sets_exit_code(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let id = state.push(BlockMetadata::default(), None);
    state.on_lifecycle(id, BlockLifecycle::CommandStart);
    state.on_lifecycle(id, BlockLifecycle::CommandEnd { exit_code: 42 });
    state.update_metadata(id, |m| {
        assert_eq!(m.exit_code, Some(42));
        assert!(m.started_at.is_some());
        assert!(m.finished_at.is_some());
    });
}

#[inazuma::test]
fn lifecycle_prompt_and_input_start_are_no_op_but_accepted(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let id = state.push(BlockMetadata::default(), None);
    state.on_lifecycle(id, BlockLifecycle::PromptStart);
    state.on_lifecycle(id, BlockLifecycle::InputStart);
    // Unknown id → no-op, no panic.
    state.on_lifecycle(
        inazuma::BlockId(999_999),
        BlockLifecycle::CommandEnd { exit_code: 0 },
    );
    state.update_metadata(id, |m| {
        assert_eq!(m.exit_code, None);
        assert!(m.started_at.is_none());
        assert!(m.finished_at.is_none());
    });
}

#[inazuma::test]
fn scroll_to_end_lands_past_last_entry_and_reenables_follow_tail(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..4)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));

    state.scroll_to_reveal(ids[0]);
    draw_view(&mut cx, &state, &height, size(px(100.), px(200.)));
    assert_eq!(state.logical_scroll_top().entry_ix, 0);

    state.scroll_to_end();
    assert_eq!(state.logical_scroll_top().entry_ix, state.entry_count());
}

#[inazuma::test]
fn visible_range_reports_ids_currently_in_viewport(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..6)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    // Viewport 250 → first 3 entries fit comfortably (+ part of a 4th).
    draw_view(&mut cx, &state, &height, size(px(100.), px(250.)));

    let range = state.visible_range();
    assert_eq!(range.start, ids[0]);
    assert!(range.end >= ids[2] && range.end <= ids[3]);
}

#[inazuma::test]
fn config_roundtrip_preserves_builder_settings(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let config = BlockConfig::default()
        .visual_anchor(VisualAnchor::Bottom)
        .scroll_behavior(ScrollBehavior::Manual)
        .measuring_behavior(BlockMeasuringBehavior::All)
        .overdraw(px(333.0));
    let state = BlockState::new(config);
    let got = state.config();
    // The config roundtrips through BlockState — consumers can observe
    // the settings the primitive was built with (used by scrollbar host
    // crates that key their behaviour on anchor/scroll mode).
    assert_eq!(
        format!("{:?}", got),
        format!("{:?}", config),
        "BlockConfig must roundtrip unchanged through BlockState::new"
    );
}

#[inazuma::test]
fn viewport_bounds_exposes_last_layout(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    state.push(BlockMetadata::default(), None);
    assert_eq!(state.viewport_bounds().size.width, px(0.));

    let height = Rc::new(Cell::new(50.0));
    draw_view(&mut cx, &state, &height, size(px(120.), px(480.)));
    let bounds = state.viewport_bounds();
    assert_eq!(bounds.size.width, px(120.));
    assert_eq!(bounds.size.height, px(480.));
}

#[inazuma::test]
fn block_kind_default_is_shell_and_tui_is_constructible(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    assert_eq!(BlockKind::default(), BlockKind::Shell);

    // BlockKind::Tui must be constructible and carried through BlockMetadata.
    let state = BlockState::new(BlockConfig::default());
    let id = state.push(
        BlockMetadata {
            kind: BlockKind::Tui,
            ..Default::default()
        },
        None,
    );
    state.update_metadata(id, |m| assert_eq!(m.kind, BlockKind::Tui));
}

// ─── Fold + summary additions ──────────────────────────────────────────

#[inazuma::test]
fn fold_marks_entry_and_stores_summary(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let a = state.push(BlockMetadata::default(), None);
    let b = state.push(BlockMetadata::default(), None);

    assert!(!state.is_folded(a));
    assert!(state.folded_entries().is_empty());

    state.fold(a, "echo hello".into());
    assert!(state.is_folded(a));
    assert!(!state.is_folded(b));

    let folded = state.folded_entries();
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].0, a);
    assert_eq!(folded[0].1.as_ref(), "echo hello");
}

#[inazuma::test]
fn unfold_clears_state(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let a = state.push(BlockMetadata::default(), None);

    state.fold(a, "sum".into());
    assert!(state.is_folded(a));
    state.unfold(a);
    assert!(!state.is_folded(a));
    assert!(state.folded_entries().is_empty());
}

#[inazuma::test]
fn fold_is_idempotent_and_updates_summary(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let a = state.push(BlockMetadata::default(), None);

    state.fold(a, "first".into());
    state.fold(a, "second".into());
    let entries = state.folded_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.as_ref(), "second");
}

#[inazuma::test]
fn fold_unknown_id_is_noop(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    state.fold(inazuma::BlockId(999), "noop".into());
    state.unfold(inazuma::BlockId(999));
    assert!(!state.is_folded(inazuma::BlockId(999)));
    assert!(state.folded_entries().is_empty());
}

#[inazuma::test]
fn pin_sets_and_unpin_clears(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let a = state.push(BlockMetadata::default(), None);
    let b = state.push(BlockMetadata::default(), None);

    assert_eq!(state.pinned_id(), None);
    state.pin(a);
    assert_eq!(state.pinned_id(), Some(a));

    // Pin replacing — no stickiness.
    state.pin(b);
    assert_eq!(state.pinned_id(), Some(b));

    state.unpin();
    assert_eq!(state.pinned_id(), None);
}

#[inazuma::test]
fn pin_unknown_id_is_noop(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    state.pin(inazuma::BlockId(42));
    assert_eq!(state.pinned_id(), None);
}

#[inazuma::test]
fn remove_clears_pin_when_pinned_block_goes_away(cx: &mut TestAppContext) {
    let _ = cx.add_empty_window();
    let state = BlockState::new(BlockConfig::default());
    let a = state.push(BlockMetadata::default(), None);
    let b = state.push(BlockMetadata::default(), None);

    state.pin(a);
    assert_eq!(state.pinned_id(), Some(a));

    // Removing the pinned block clears the pin — otherwise layout
    // resolves the stale id to garbage.
    state.remove(a);
    assert_eq!(state.pinned_id(), None);

    // Pinning the surviving block still works.
    state.pin(b);
    assert_eq!(state.pinned_id(), Some(b));

    // Removing a non-pinned block leaves the pin alone.
    let c = state.push(BlockMetadata::default(), None);
    state.remove(c);
    assert_eq!(state.pinned_id(), Some(b));
}

#[inazuma::test]
fn visible_entries_reports_ids_in_viewport(cx: &mut TestAppContext) {
    let mut cx = cx.add_empty_window();
    let state = BlockState::new(
        BlockConfig::default()
            .visual_anchor(VisualAnchor::Top)
            .scroll_behavior(ScrollBehavior::Manual),
    );
    let ids: Vec<_> = (0..6)
        .map(|_| state.push(BlockMetadata::default(), None))
        .collect();
    let height = Rc::new(Cell::new(100.0));
    // Viewport 250 → first 3 entries fit comfortably (+ part of a 4th).
    draw_view(&mut cx, &state, &height, size(px(100.), px(250.)));

    let visible = state.visible_entries();
    assert!(visible.contains(&ids[0]));
    assert!(visible.contains(&ids[2]));
    // 6th entry is definitely off-screen
    assert!(!visible.contains(&ids[5]));
}

#[inazuma::test]
fn fold_state_default_is_unfolded() {
    assert_eq!(FoldState::default(), FoldState::Unfolded);
    assert!(!FoldState::Unfolded.is_folded());
    assert!(
        FoldState::Folded {
            summary: "x".into()
        }
        .is_folded()
    );
}
