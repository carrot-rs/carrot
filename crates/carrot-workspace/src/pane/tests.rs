use std::iter::zip;

use super::*;
use crate::{
    Member,
    item::test::{TestItem, TestProjectItem},
};
use carrot_project::FakeFs;
use carrot_theme::LoadThemes;
use inazuma::{AppContext, Axis, TestAppContext, VisualTestContext};
use inazuma_settings_framework::SettingsStore;

#[inazuma::test]
async fn test_dont_save_single_file_reloads_from_disk(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());

    let project = Project::test(fs, None, cx).await;
    let (workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
    let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

    let item = add_labeled_item(&pane, "Dirty", true, cx);
    item.update(cx, |item, cx| {
        item.project_items
            .push(TestProjectItem::new_dirty(1, "Dirty.txt", cx))
    });
    assert_item_labels(&pane, ["Dirty*^"], cx);

    let close_task = pane.update_in(cx, |pane, window, cx| {
        pane.close_item_by_id(item.item_id(), SaveIntent::Close, window, cx)
    });

    cx.executor().run_until_parked();
    cx.simulate_prompt_answer("Don't Save");
    close_task.await.unwrap();
    assert_item_labels(&pane, [], cx);

    item.read_with(cx, |item, _| {
        assert_eq!(item.reload_count, 1, "item should have been reloaded");
        assert!(
            !item.is_dirty,
            "item should no longer be dirty after reload"
        );
    });
}
// Disabled: single-item pane model has no tab bar scroll
#[cfg(never)]
#[inazuma::test]
async fn test_new_tab_scrolls_into_view_completely(cx: &mut TestAppContext) {
    // Arrange
    init_test(cx);
    let fs = FakeFs::new(cx.executor());

    let project = Project::test(fs, None, cx).await;
    let (workspace, cx) = cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
    let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

    cx.simulate_resize(size(px(300.), px(300.)));

    add_labeled_item(&pane, "untitled", false, cx);
    add_labeled_item(&pane, "untitled", false, cx);
    add_labeled_item(&pane, "untitled", false, cx);
    add_labeled_item(&pane, "untitled", false, cx);
    // Act: this should trigger a scroll
    add_labeled_item(&pane, "untitled", false, cx);
    // Assert
    let tab_bar_scroll_handle =
        pane.update_in(cx, |pane, _window, _cx| pane.tab_bar_scroll_handle.clone());
    assert_eq!(tab_bar_scroll_handle.children_count(), 6);
    let tab_bounds = cx.debug_bounds("TAB-4").unwrap();
    let new_tab_button_bounds = cx.debug_bounds("ICON-Plus").unwrap();
    let scroll_bounds = tab_bar_scroll_handle.bounds();
    let scroll_offset = tab_bar_scroll_handle.offset();
    assert!(tab_bounds.right() <= scroll_bounds.right());
    // -39.5 is the magic number for this setup
    assert_eq!(scroll_offset.x, px(-39.5));
    assert!(
        !tab_bounds.intersects(&new_tab_button_bounds),
        "Tab should not overlap with the new tab button, if this is failing check if there's been a redesign!"
    );
}

// Disabled: single-item pane model has no tab bar scroll
#[cfg(never)]
#[inazuma::test]
async fn ensure_item_closing_actions_do_not_panic_when_no_items_exist(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let project = Project::test(fs, None, cx).await;
    let (workspace, cx) = cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

    let pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
    assert_item_labels(&pane, [], cx);

    pane.update_in(cx, |pane, window, cx| {
        pane.close_active_item(&CloseActiveItem { save_intent: None }, window, cx)
    })
    .await
    .unwrap();

    pane.update_in(cx, |pane, window, cx| {
        pane.close_other_items(&CloseOtherItems { save_intent: None }, None, window, cx)
    })
    .await
    .unwrap();

    pane.update_in(cx, |pane, window, cx| {
        pane.close_all_items(&CloseAllItems { save_intent: None }, window, cx)
    })
    .await
    .unwrap();

    pane.update_in(cx, |pane, window, cx| pane.close_clean_items(window, cx))
        .await
        .unwrap();

    pane.update_in(cx, |pane, window, cx| {
        pane.close_items_to_the_right_by_id(None, window, cx)
    })
    .await
    .unwrap();

    pane.update_in(cx, |pane, window, cx| {
        pane.close_items_to_the_left_by_id(None, window, cx)
    })
    .await
    .unwrap();
}
#[inazuma::test]
async fn test_split_empty(cx: &mut TestAppContext) {
    for split_direction in SplitDirection::all() {
        test_single_pane_split(["A"], split_direction, SplitMode::EmptyPane, cx).await;
    }
}

#[inazuma::test]
async fn test_split_clone(cx: &mut TestAppContext) {
    for split_direction in SplitDirection::all() {
        test_single_pane_split(["A"], split_direction, SplitMode::ClonePane, cx).await;
    }
}

#[inazuma::test]
async fn test_split_move_right_on_single_pane(cx: &mut TestAppContext) {
    test_single_pane_split(["A"], SplitDirection::Right, SplitMode::MovePane, cx).await;
}
fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        carrot_theme_settings::init(LoadThemes::JustBase, cx);
    });
}

fn add_labeled_item(
    pane: &Entity<Pane>,
    label: &str,
    is_dirty: bool,
    cx: &mut VisualTestContext,
) -> Box<Entity<TestItem>> {
    pane.update_in(cx, |pane, window, cx| {
        let labeled_item =
            Box::new(cx.new(|cx| TestItem::new(cx).with_label(label).with_dirty(is_dirty)));
        pane.add_item(labeled_item.clone(), false, false, None, window, cx);
        labeled_item
    })
}

// Assert the item label, with the active item label suffixed with a '*'
#[track_caller]
fn assert_item_labels<const COUNT: usize>(
    pane: &Entity<Pane>,
    expected_states: [&str; COUNT],
    cx: &mut VisualTestContext,
) {
    let actual_states = pane.update(cx, |pane, cx| {
        let active_index = pane.active_item_index();
        pane.items()
            .enumerate()
            .map(|(ix, item)| {
                let mut state = item
                    .to_any_view()
                    .downcast::<TestItem>()
                    .unwrap()
                    .read(cx)
                    .label
                    .clone();
                if ix == active_index {
                    state.push('*');
                }
                if item.is_dirty(cx) {
                    state.push('^');
                }
                state
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        actual_states, expected_states,
        "pane items do not match expectation"
    );
}

// Assert the item label, with the active item label expected active index
#[track_caller]
fn assert_item_labels_active_index(
    pane: &Entity<Pane>,
    expected_states: &[&str],
    expected_active_idx: usize,
    cx: &mut VisualTestContext,
) {
    let actual_states = pane.update(cx, |pane, cx| {
        let active_index = pane.active_item_index();
        pane.items()
            .enumerate()
            .map(|(ix, item)| {
                let mut state = item
                    .to_any_view()
                    .downcast::<TestItem>()
                    .unwrap()
                    .read(cx)
                    .label
                    .clone();
                if ix == active_index {
                    assert_eq!(ix, expected_active_idx);
                }
                if item.is_dirty(cx) {
                    state.push('^');
                }
                state
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        actual_states, expected_states,
        "pane items do not match expectation"
    );
}

#[track_caller]
fn assert_pane_ids_on_axis<const COUNT: usize>(
    workspace: &Entity<Workspace>,
    expected_ids: [&EntityId; COUNT],
    expected_axis: Axis,
    cx: &mut VisualTestContext,
) {
    workspace.read_with(cx, |workspace, _| match &workspace.center.root {
        Member::Axis(axis) => {
            assert_eq!(axis.axis, expected_axis);
            assert_eq!(axis.members.len(), expected_ids.len());
            assert!(
                zip(expected_ids, &axis.members).all(|(e, a)| {
                    if let Member::Pane(p) = a {
                        p.entity_id() == *e
                    } else {
                        false
                    }
                }),
                "pane ids do not match expectation: {expected_ids:?} != {actual_ids:?}",
                actual_ids = axis.members
            );
        }
        Member::Pane(_) => panic!("expected axis"),
    });
}

async fn test_single_pane_split<const COUNT: usize>(
    pane_labels: [&str; COUNT],
    direction: SplitDirection,
    operation: SplitMode,
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    let project = Project::test(fs, None, cx).await;
    let (workspace, cx) = cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));

    let mut pane_before = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
    for label in pane_labels {
        add_labeled_item(&pane_before, label, false, cx);
    }
    pane_before.update_in(cx, |pane, window, cx| {
        pane.split(direction, operation, window, cx)
    });
    cx.executor().run_until_parked();
    let pane_after = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

    let num_labels = pane_labels.len();
    let last_as_active = format!("{}*", String::from(pane_labels[num_labels - 1]));

    // check labels for all split operations
    match operation {
        SplitMode::EmptyPane => {
            assert_item_labels_active_index(&pane_before, &pane_labels, num_labels - 1, cx);
            assert_item_labels(&pane_after, [], cx);
        }
        SplitMode::ClonePane => {
            assert_item_labels_active_index(&pane_before, &pane_labels, num_labels - 1, cx);
            assert_item_labels(&pane_after, [&last_as_active], cx);
        }
        SplitMode::MovePane => {
            let head = &pane_labels[..(num_labels - 1)];
            if num_labels == 1 {
                // We special-case this behavior and actually execute an empty pane command
                // followed by a refocus of the old pane for this case.
                pane_before = workspace.read_with(cx, |workspace, _cx| {
                    workspace
                        .panes()
                        .into_iter()
                        .find(|pane| *pane != &pane_after)
                        .unwrap()
                        .clone()
                });
            };

            assert_item_labels_active_index(&pane_before, &head, head.len().saturating_sub(1), cx);
            assert_item_labels(&pane_after, [&last_as_active], cx);
            pane_after.update_in(cx, |pane, window, cx| {
                window.focused(cx).is_some_and(|focus_handle| {
                    focus_handle == pane.active_item().unwrap().item_focus_handle(cx)
                })
            });
        }
    }

    // expected axis depends on split direction
    let expected_axis = match direction {
        SplitDirection::Right | SplitDirection::Left => Axis::Horizontal,
        SplitDirection::Up | SplitDirection::Down => Axis::Vertical,
    };

    // expected ids depends on split direction
    let expected_ids = match direction {
        SplitDirection::Right | SplitDirection::Down => {
            [&pane_before.entity_id(), &pane_after.entity_id()]
        }
        SplitDirection::Left | SplitDirection::Up => {
            [&pane_after.entity_id(), &pane_before.entity_id()]
        }
    };

    // check pane axes for all operations
    match operation {
        SplitMode::EmptyPane | SplitMode::ClonePane => {
            assert_pane_ids_on_axis(&workspace, expected_ids, expected_axis, cx);
        }
        SplitMode::MovePane => {
            assert_pane_ids_on_axis(&workspace, expected_ids, expected_axis, cx);
        }
    }
}
