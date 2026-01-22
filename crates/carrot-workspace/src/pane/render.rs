use super::*;

impl EventEmitter<Event> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Pane {
    pub fn render_menu_overlay(menu: &Entity<ContextMenu>) -> Div {
        div().absolute().bottom_0().right_0().size_0().child(
            deferred(anchored().anchor(Corner::TopRight).child(menu.clone())).with_priority(1),
        )
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("Pane");
        if self.active_item().is_none() {
            key_context.add("EmptyPane");
        }

        self.toolbar
            .read(cx)
            .contribute_context(&mut key_context, cx);

        // Tab bar display logic removed — session tabs in title bar.
        let Some(project) = self.project.upgrade() else {
            return div().track_focus(&self.focus_handle(cx));
        };
        let is_local = project.read(cx).is_local();

        v_flex()
            .key_context(key_context)
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex_none()
            .overflow_hidden()
            .on_action(cx.listener(|pane, split: &SplitLeft, window, cx| {
                pane.split(SplitDirection::Left, split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, split: &SplitUp, window, cx| {
                pane.split(SplitDirection::Up, split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, split: &SplitHorizontal, window, cx| {
                pane.split(SplitDirection::horizontal(cx), split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, split: &SplitVertical, window, cx| {
                pane.split(SplitDirection::vertical(cx), split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, split: &SplitRight, window, cx| {
                pane.split(SplitDirection::Right, split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, split: &SplitDown, window, cx| {
                pane.split(SplitDirection::Down, split.mode, window, cx)
            }))
            .on_action(cx.listener(|pane, _: &SplitAndMoveUp, window, cx| {
                pane.split(SplitDirection::Up, SplitMode::MovePane, window, cx)
            }))
            .on_action(cx.listener(|pane, _: &SplitAndMoveDown, window, cx| {
                pane.split(SplitDirection::Down, SplitMode::MovePane, window, cx)
            }))
            .on_action(cx.listener(|pane, _: &SplitAndMoveLeft, window, cx| {
                pane.split(SplitDirection::Left, SplitMode::MovePane, window, cx)
            }))
            .on_action(cx.listener(|pane, _: &SplitAndMoveRight, window, cx| {
                pane.split(SplitDirection::Right, SplitMode::MovePane, window, cx)
            }))
            .on_action(cx.listener(|_, _: &JoinIntoNext, _, cx| {
                cx.emit(Event::JoinIntoNext);
            }))
            .on_action(cx.listener(|_, _: &JoinAll, _, cx| {
                cx.emit(Event::JoinAll);
            }))
            .on_action(cx.listener(Pane::toggle_zoom))
            .on_action(cx.listener(Pane::zoom_in))
            .on_action(cx.listener(Pane::zoom_out))
            .on_action(cx.listener(Self::navigate_backward))
            .on_action(cx.listener(Self::navigate_forward))
            .on_action(cx.listener(Self::go_to_older_tag))
            .on_action(cx.listener(Self::go_to_newer_tag))
            .on_action(
                cx.listener(|pane: &mut Pane, _action: &ActivateItem, window, cx| {
                    pane.activate_item(0, true, true, window, cx);
                }),
            )
            .on_action(cx.listener(Self::alternate_file))
            .on_action(cx.listener(Self::activate_last_item))
            .on_action(cx.listener(Self::activate_previous_item))
            .on_action(cx.listener(Self::activate_next_item))
            .on_action(cx.listener(Self::swap_item_left))
            .on_action(cx.listener(Self::swap_item_right))
            .when(PreviewTabsSettings::get_global(cx).enabled, |this| {
                this.on_action(
                    cx.listener(|pane: &mut Pane, _: &TogglePreviewTab, window, cx| {
                        if let Some(active_item_id) = pane.active_item().map(|i| i.item_id()) {
                            if pane.is_active_preview_item(active_item_id) {
                                pane.unpreview_item_if_preview(active_item_id);
                            } else {
                                pane.replace_preview_item_id(active_item_id, window, cx);
                            }
                        }
                    }),
                )
            })
            .on_action(
                cx.listener(|pane: &mut Self, action: &CloseActiveItem, window, cx| {
                    pane.close_active_item(action, window, cx)
                        .detach_and_log_err(cx)
                }),
            )
            .on_action(
                cx.listener(|pane: &mut Self, action: &CloseOtherItems, window, cx| {
                    pane.close_other_items(action, None, window, cx)
                        .detach_and_log_err(cx);
                }),
            )
            .on_action(
                cx.listener(|pane: &mut Self, _: &CloseCleanItems, window, cx| {
                    pane.close_clean_items(window, cx).detach_and_log_err(cx)
                }),
            )
            .on_action(
                cx.listener(|pane: &mut Self, _: &CloseItemsToTheLeft, window, cx| {
                    pane.close_items_to_the_left_by_id(None, window, cx)
                        .detach_and_log_err(cx)
                }),
            )
            .on_action(
                cx.listener(|pane: &mut Self, _: &CloseItemsToTheRight, window, cx| {
                    pane.close_items_to_the_right_by_id(None, window, cx)
                        .detach_and_log_err(cx)
                }),
            )
            .on_action(
                cx.listener(|pane: &mut Self, action: &CloseAllItems, window, cx| {
                    pane.close_all_items(action, window, cx)
                        .detach_and_log_err(cx)
                }),
            )
            .on_action(cx.listener(
                |pane: &mut Self, action: &CloseMultibufferItems, window, cx| {
                    pane.close_multibuffer_items(action, window, cx)
                        .detach_and_log_err(cx)
                },
            ))
            .on_action(
                cx.listener(|pane: &mut Self, action: &RevealInProjectPanel, _, cx| {
                    let entry_id = action
                        .entry_id
                        .map(ProjectEntryId::from_proto)
                        .or_else(|| pane.active_item()?.project_entry_ids(cx).first().copied());
                    if let Some(entry_id) = entry_id {
                        pane.project
                            .update(cx, |_, cx| {
                                cx.emit(carrot_project::Event::RevealInProjectPanel(entry_id))
                            })
                            .ok();
                    }
                }),
            )
            .on_action(cx.listener(|_, _: &inazuma_menu::Cancel, window, cx| {
                if cx.stop_active_drag(window) {
                } else {
                    cx.propagate();
                }
            }))
            // Tab bar disabled — session tabs in title bar handle navigation.
            // The old tab bar render was:
            // .when(self.active_item().is_some() && display_tab_bar, |pane| {
            //     pane.child((self.render_tab_bar.clone())(self, window, cx))
            // })
            .child({
                let has_worktrees = project.read(cx).visible_worktrees(cx).next().is_some();
                // main content
                div()
                    .flex_1()
                    .relative()
                    .group("")
                    .overflow_hidden()
                    .on_drag_move::<DraggedTab>(cx.listener(Self::handle_drag_move))
                    .on_drag_move::<DraggedSelection>(cx.listener(Self::handle_drag_move))
                    .when(is_local, |div| {
                        div.on_drag_move::<ExternalPaths>(cx.listener(Self::handle_drag_move))
                    })
                    .map(|content_div| {
                        if let Some(item) = self.active_item() {
                            let toolbar_overlay = (!self.toolbar.read(cx).hidden()).then(|| {
                                inazuma::div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .child(self.toolbar.clone())
                            });
                            content_div
                                .id("pane_placeholder")
                                .size_full()
                                .relative()
                                .overflow_hidden()
                                .child(item.to_any_view())
                                .children(toolbar_overlay)
                        } else {
                            let placeholder = content_div
                                .id("pane_placeholder")
                                .h_flex()
                                .size_full()
                                .justify_center()
                                .on_click(cx.listener(
                                    move |this, event: &ClickEvent, window, cx| {
                                        if event.click_count() == 2 {
                                            window.dispatch_action(
                                                this.double_click_dispatch_action.boxed_clone(),
                                                cx,
                                            );
                                        }
                                    },
                                ));
                            if has_worktrees || !self.should_display_welcome_page {
                                placeholder
                            } else {
                                if self.welcome_page.is_none() {
                                    let workspace = self.workspace.clone();
                                    self.welcome_page = Some(cx.new(|cx| {
                                        crate::welcome::WelcomePage::new(
                                            workspace, true, window, cx,
                                        )
                                    }));
                                }
                                placeholder.child(self.welcome_page.clone().unwrap())
                            }
                        }
                    })
                    .child(
                        // drag target
                        div()
                            .invisible()
                            .absolute()
                            .bg(cx.theme().colors().drop_target_background)
                            .group_drag_over::<DraggedTab>("", |style| style.visible())
                            .group_drag_over::<DraggedSelection>("", |style| style.visible())
                            .when(is_local, |div| {
                                div.group_drag_over::<ExternalPaths>("", |style| style.visible())
                            })
                            .when_some(self.can_drop_predicate.clone(), |this, p| {
                                this.can_drop(move |a, window, cx| p(a, window, cx))
                            })
                            .on_drop(cx.listener(move |this, dragged_tab, window, cx| {
                                this.handle_tab_drop(
                                    dragged_tab,
                                    this.active_item_index(),
                                    true,
                                    window,
                                    cx,
                                )
                            }))
                            .on_drop(cx.listener(
                                move |this, selection: &DraggedSelection, window, cx| {
                                    this.handle_dragged_selection_drop(selection, None, window, cx)
                                },
                            ))
                            .on_drop(cx.listener(move |this, paths, window, cx| {
                                this.handle_external_paths_drop(paths, window, cx)
                            }))
                            .map(|div| {
                                let size = DefiniteLength::Fraction(0.5);
                                match self.drag_split_direction {
                                    None => div.top_0().right_0().bottom_0().left_0(),
                                    Some(SplitDirection::Up) => {
                                        div.top_0().left_0().right_0().h(size)
                                    }
                                    Some(SplitDirection::Down) => {
                                        div.left_0().bottom_0().right_0().h(size)
                                    }
                                    Some(SplitDirection::Left) => {
                                        div.top_0().left_0().bottom_0().w(size)
                                    }
                                    Some(SplitDirection::Right) => {
                                        div.top_0().bottom_0().right_0().w(size)
                                    }
                                }
                            }),
                    )
            })
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|pane, _, window, cx| {
                    if let Some(workspace) = pane.workspace.upgrade() {
                        let pane = cx.entity().downgrade();
                        window.defer(cx, move |window, cx| {
                            workspace.update(cx, |workspace, cx| {
                                workspace.go_back(pane, window, cx).detach_and_log_err(cx)
                            })
                        })
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|pane, _, window, cx| {
                    if let Some(workspace) = pane.workspace.upgrade() {
                        let pane = cx.entity().downgrade();
                        window.defer(cx, move |window, cx| {
                            workspace.update(cx, |workspace, cx| {
                                workspace
                                    .go_forward(pane, window, cx)
                                    .detach_and_log_err(cx)
                            })
                        })
                    }
                }),
            )
    }
}

pub fn tab_details(items: &[Box<dyn ItemHandle>], _window: &Window, cx: &App) -> Vec<usize> {
    let mut tab_details = items.iter().map(|_| 0).collect::<Vec<_>>();
    let mut tab_descriptions = HashMap::default();
    let mut done = false;
    while !done {
        done = true;

        // Store item indices by their tab description.
        for (ix, (item, detail)) in items.iter().zip(&tab_details).enumerate() {
            let description = item.tab_content_text(*detail, cx);
            if *detail == 0 || description != item.tab_content_text(detail - 1, cx) {
                tab_descriptions
                    .entry(description)
                    .or_insert(Vec::new())
                    .push(ix);
            }
        }

        // If two or more items have the same tab description, increase their level
        // of detail and try again.
        for (_, item_ixs) in tab_descriptions.drain() {
            if item_ixs.len() > 1 {
                done = false;
                for ix in item_ixs {
                    tab_details[ix] += 1;
                }
            }
        }
    }

    tab_details
}

pub fn render_item_indicator(item: Box<dyn ItemHandle>, cx: &App) -> Option<Indicator> {
    maybe!({
        let indicator_color = match (item.has_conflict(cx), item.is_dirty(cx)) {
            (true, _) => Color::Warning,
            (_, true) => Color::Accent,
            (false, false) => return None,
        };

        Some(Indicator::dot().color(indicator_color))
    })
}

impl Render for DraggedTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = ThemeSettings::get_global(cx).ui_font.clone();
        let label = self.item.tab_content(
            TabContentParams {
                detail: Some(self.detail),
                selected: false,
                preview: false,
                deemphasized: false,
            },
            window,
            cx,
        );
        Tab::new("")
            .toggle_state(self.is_active)
            .child(label)
            .render(window, cx)
            .font(ui_font)
    }
}
