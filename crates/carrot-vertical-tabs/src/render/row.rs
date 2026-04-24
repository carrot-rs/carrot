//! Per-row rendering for the vertical tabs panel.
//!
//! Three row shapes are built here:
//!
//! 1. **Group header** — plain muted text label that tags a session
//!    group in Panes mode. Rendered when `row.header_text` is `Some`.
//! 2. **Inline rename** — an `InputState` in a selected-card frame,
//!    activated when the clicked row matches `rename_in_progress`.
//! 3. **Normal card** — the full card with chip (⋮/×), hover actions,
//!    drag-and-drop wiring, and (in Panes mode) an outer pane wrapper
//!    that carries its own hover state + divider to the next pane.
//!
//! Keeping this isolated lets the `Render` impl stay a short loop
//! that just calls `build_row` on every `TabRowData`.

use std::rc::Rc;

use carrot_cli_agents::CliAgentSessionState;
use carrot_theme::ActiveTheme;
use carrot_ui::{
    ButtonCommon, ButtonCustomVariant, ButtonSize, ButtonStyle, Card, Color, IconButton, IconName,
    IconSize, PopoverMenu, PopoverMenuHandle, Sizable, input::Input, prelude::*,
};
use carrot_workspace::session_menu::{SessionMenuVariant, build_session_context_menu};
use inazuma::{
    AnyElement, App, ClickEvent, Context, Corner, InteractiveElement, IntoElement, Oklch,
    ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, div, point,
    prelude::FluentBuilder, px,
};

use crate::VerticalTabsPanel;
use crate::render::drag::{DraggedVerticalTab, DraggedVerticalTabView};
use crate::render::row_data::{AgentRowInfo, TabRowData};

/// Resolve the status-badge bg colour plus its embedded glyph for one
/// agent session state. All branches return a theme token — never a
/// literal — so themes keep control of badge hue across every
/// installed theme file.
///
/// `None` means "no badge": used for the `Starting` transient phase
/// where we don't know the outcome yet.
fn status_badge(state: &CliAgentSessionState, cx: &App) -> Option<(Oklch, IconName)> {
    let block = &cx.theme().colors().block;
    let accent = cx.theme().colors().accent;
    Some(match state {
        CliAgentSessionState::Starting => return None,
        // Agent finished a turn and is waiting — reads as "done".
        CliAgentSessionState::Idle => (block.success_badge, IconName::Check),
        // Agent is mid-turn; clock glyph conveys "time is ticking".
        CliAgentSessionState::Working { .. } => (block.running_badge, IconName::CountdownTimer),
        // Agent is blocked on user input (tool permission, elicitation,
        // etc.). Stop glyph signals "the agent can't progress until
        // you intervene". No "warning/yellow" token exists today —
        // accent is the best-available "attention needed" cue and is
        // already tuned per theme.
        CliAgentSessionState::WaitingForInput { .. } => (accent, IconName::Stop),
        CliAgentSessionState::Compacting => (accent, IconName::CountdownTimer),
        CliAgentSessionState::Completed { exit_code } => match exit_code {
            Some(0) | None => (block.success_badge, IconName::Check),
            Some(_) => (block.error_badge, IconName::Triangle),
        },
        CliAgentSessionState::Errored { .. } => (block.error_badge, IconName::Triangle),
    })
}

/// Build the card's leading icon slot. Two shapes:
///
/// - **No agent attached** — plain `Icon::new(icon)` tinted with the
///   usual active/muted pair. Identical to pre-6.7 behaviour.
/// - **Agent attached** — a composed *brand circle*: a filled circle
///   in the agent's brand bg colour with the agent's monochrome
///   sparkle SVG painted on top in the brand fg colour. This shape
///   sidesteps Inazuma's SVG renderer, which tints embedded SVGs with
///   a single colour and would therefore flatten a polychrome
///   orange/white asset to a monotone blob. Composing the two layers
///   ourselves keeps the brand identity intact on every theme.
///
/// Either shape gets an optional small circular badge overlaid at
/// the bottom-right whose colour + glyph reflect the session's
/// current [`CliAgentSessionState`] (see [`status_badge`]).
fn build_icon_slot(
    icon: IconName,
    agent: Option<&AgentRowInfo>,
    is_active: bool,
    cx: &App,
) -> AnyElement {
    let Some(agent) = agent else {
        return carrot_ui::Icon::new(icon)
            .size(carrot_ui::IconSize::Small)
            .color(if is_active {
                carrot_ui::Color::Default
            } else {
                carrot_ui::Color::Muted
            })
            .into_any_element();
    };

    let colors = cx.theme().colors();
    // Circle bg / sparkle fg default to neutral theme tokens when the
    // agent did not declare brand colours. Keeps new agents readable
    // even before they ship a brand palette.
    let circle_bg = agent.brand_bg.unwrap_or(colors.element_background);
    let sparkle_fg = agent.brand_fg.unwrap_or(colors.text);

    // Brand circle sized to match the reference vertical-tabs row: big
    // enough to carry the brand identity prominently (roughly the
    // height of title + subtitle stacked) without forcing the card
    // itself taller. `flex_shrink_0` stops narrow panels from
    // squashing it into an ellipse.
    let brand_circle = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_full()
        .bg(circle_bg)
        .child(
            carrot_ui::Icon::from_path(agent.icon_path.clone())
                .size_3p5()
                .text_color(sparkle_fg),
        );

    let badge = status_badge(&agent.state, cx).map(|(badge_color, glyph)| {
        // The badge sits at the bottom-right of the icon square. A 1px
        // ring in the card's resting bg colour keeps the badge crisp
        // against the brand fill regardless of theme. The embedded
        // glyph uses `text_color` to force the SVG's `currentColor`
        // fill to the foreground tint, which lets one white/black
        // icon read cleanly on any `badge_color`.
        let glyph_fg = cx.theme().colors().background;
        div()
            .absolute()
            .bottom_neg_0p5()
            .right_neg_0p5()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(14.))
            .rounded_full()
            .bg(badge_color)
            .border_1()
            .border_color(cx.theme().colors().panel.background)
            .child(carrot_ui::Icon::new(glyph).size_2p5().text_color(glyph_fg))
    });
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(brand_circle)
        .when_some(badge, |el, b| el.child(b))
        .into_any_element()
}

impl VerticalTabsPanel {
    /// Build the floating `⋮ ×` hover chip for a session row.
    ///
    /// Shared between Tabs-mode card overlays, single-pane Panes-mode
    /// rows, and Panes-mode session-container headers — every place
    /// the chip can appear goes through here, so the styling + menu
    /// behaviour stays in lockstep.
    ///
    /// `pane_id` controls the close action:
    /// - `None` → close the whole session (used for Tabs-mode rows
    ///   and the Panes-mode session-container chip).
    /// - `Some(pane)` → close just that pane (currently unused by
    ///   callers because Panes-mode suppresses per-row chips; kept
    ///   here so a future per-pane chip doesn't need another
    ///   code path).
    ///
    /// Returns `(chip, menu_is_open)` so callers can mirror the
    /// open-menu state onto their own hover suppression / pinning.
    pub(crate) fn build_hover_chip(
        &mut self,
        session_index: usize,
        label: SharedString,
        pane_id: Option<inazuma::EntityId>,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let index = session_index;
        let ws_menu = self.workspace.clone();
        let on_rename_cb: Rc<dyn Fn(usize, &mut Window, &mut App)> = {
            let handle = cx.entity().downgrade();
            let rename_label = label.clone();
            Rc::new(move |ix, window, cx| {
                let Some(panel) = handle.upgrade() else {
                    return;
                };
                let label = rename_label.clone();
                panel.update(cx, |this, cx| {
                    this.start_rename(ix, label.clone(), window, cx);
                });
            })
        };

        let colors = cx.theme().colors();
        // Chip-internal hover rectangle. The chip bg is
        // `elevated_surface`; the per-icon hover rectangle uses
        // `element_active` — the next lightness rung up so it stays
        // perceptible on any theme.
        let chip_icon_hover = colors.element_active;
        let chip_transparent = colors.ghost_element_background;
        let chip_button_style = ButtonStyle::Custom(
            ButtonCustomVariant::new(cx)
                .color(chip_transparent)
                .hover(chip_icon_hover)
                .active(chip_icon_hover),
        );
        // Keep the chip_icon_hover bg pinned while the popover menu
        // is open so the ⋮ reads as "still active" even after the
        // cursor moves away.
        let chip_selected_style = ButtonStyle::Custom(
            ButtonCustomVariant::new(cx)
                .color(chip_icon_hover)
                .hover(chip_icon_hover)
                .active(chip_icon_hover),
        );
        let more_button = IconButton::new(("vertical-tab-more", index), IconName::EllipsisVertical)
            .icon_size(IconSize::XSmall)
            .size(ButtonSize::Compact)
            .style(chip_button_style)
            .selected_icon_color(Color::Default)
            .selected_style(chip_selected_style);
        let menu_handle = self
            .menu_handles
            .entry(index)
            .or_insert_with(PopoverMenuHandle::default)
            .clone();
        let menu_is_open = menu_handle.is_deployed();
        let more_menu = PopoverMenu::new(("vertical-tab-more-menu", index))
            .trigger(more_button)
            .with_handle(menu_handle)
            .anchor(Corner::TopLeft)
            .offset(point(px(0.), px(3.)))
            .menu({
                let ws_menu = ws_menu.clone();
                let on_rename = on_rename_cb.clone();
                move |window, cx| {
                    let workspace = ws_menu.upgrade()?;
                    Some(build_session_context_menu(
                        index,
                        workspace,
                        SessionMenuVariant::Vertical,
                        Some(on_rename.clone()),
                        window,
                        cx,
                    ))
                }
            });

        let close_button = div().id(("vertical-tab-close", index)).child(
            IconButton::new(("vertical-tab-close-btn", index), IconName::Close)
                .icon_size(IconSize::XSmall)
                .size(ButtonSize::Compact)
                .style(chip_button_style)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    match pane_id {
                        Some(pid) => this.close_pane(index, pid, cx),
                        None => this.close_session(index, window, cx),
                    }
                })),
        );

        let chip = h_flex()
            .id(("vertical-tab-chip", index))
            .gap_0p5()
            .items_center()
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                if *hovered {
                    this.hovering_chip_index = Some(index);
                } else if this.hovering_chip_index == Some(index) {
                    this.hovering_chip_index = None;
                }
                cx.notify();
            }))
            .child(more_menu)
            .child(close_button)
            .into_any_element();

        (chip, menu_is_open)
    }

    /// Render one row from resolved `TabRowData`. Returns the final
    /// `AnyElement` to slot into the `tab_list` v_flex.
    ///
    /// `previous_pane_row_seen` is a scratch flag threaded through the
    /// render loop so the first Panes-mode row skips its top divider;
    /// the method flips it when it emits a pane-wrapped row.
    pub(crate) fn build_row(
        &mut self,
        row: TabRowData,
        card_height: Pixels,
        is_expanded: bool,
        is_panes_mode_render: bool,
        // When true, the render loop has already wrapped the whole
        // session into a shared pane-strip container. The row must not
        // emit its own pane-wrapper bg/hover — it renders as a
        // transparent card inside the container. Top divider + chip are
        // still emitted so multi-pane rows remain visually separated
        // and the hover chip stays reachable.
        in_session_container: bool,
        previous_pane_row_seen: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Branch 1: group-header row. No card, no chip — just a muted
        // text label that labels the session above its child pane rows.
        if let Some(text) = row.header_text.clone() {
            let colors = cx.theme().colors();
            return div()
                .px_2()
                .pt_2()
                .pb_0p5()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(text)
                .into_any_element();
        }

        let TabRowData {
            session_index: index,
            pane_id,
            header_text: _,
            title,
            subtitle,
            description,
            diff_stats,
            worktree_root,
            pr_info,
            icon,
            is_active,
            agent,
        } = row;

        // Branch 2: user is renaming this row — render an inline-edit
        // cell (same visual frame as a Card, but the title slot is an
        // Input). The rename state lives in `self.rename_in_progress`.
        if let Some((rename_ix, input_state)) = self.rename_in_progress.as_ref()
            && *rename_ix == index
        {
            let colors = cx.theme().colors();
            // Outer wrapper provides the same horizontal inset as the
            // normal card path so the rounded rename frame doesn't touch
            // the sidebar edges. The inner h_flex keeps its own px_2 for
            // the icon+input breathing room.
            return div()
                .w_full()
                .px_2()
                .child(
                    h_flex()
                        .id(("vertical-tab-rename", index))
                        .w_full()
                        .h(card_height)
                        .px_2()
                        .gap_2()
                        .items_center()
                        .rounded(inazuma::px(4.))
                        .bg(colors.element_selected)
                        .child(
                            carrot_ui::Icon::new(icon)
                                .size(carrot_ui::IconSize::Small)
                                .color(carrot_ui::Color::Default),
                        )
                        .child(
                            Input::new(input_state)
                                .appearance(false)
                                .bordered(false)
                                .with_size(carrot_ui::Size::Small),
                        ),
                )
                .into_any_element();
        }

        // Branch 3: normal card. Wire up the ⋮/× chip, hover actions,
        // drag-and-drop, and the Panes-mode outer pane wrapper.
        //
        // `in_session_container` rows are rendered inside a Panes-mode
        // session strip and therefore share one chip at the container
        // level (built by the render loop). No per-row chip here.
        let hover_actions_and_menu = if in_session_container {
            None
        } else {
            Some(self.build_hover_chip(index, title.clone(), pane_id, cx))
        };
        let menu_is_open = hover_actions_and_menu
            .as_ref()
            .map(|(_, open)| *open)
            .unwrap_or(false);
        let hover_actions = hover_actions_and_menu.map(|(el, _)| el);

        // Double-click rename handler — independent of the chip,
        // needs its own closure so pane-row double-clicks still work
        // even when the chip is suppressed.
        let on_rename_for_dblclick: Rc<dyn Fn(usize, &mut Window, &mut App)> = {
            let handle = cx.entity().downgrade();
            let rename_label = title.clone();
            Rc::new(move |ix, window, cx| {
                let Some(panel) = handle.upgrade() else {
                    return;
                };
                let label = rename_label.clone();
                panel.update(cx, |this, cx| {
                    this.start_rename(ix, label.clone(), window, cx);
                });
            })
        };
        let rename_label = title.clone();
        let drag_label = title.clone();
        let chip_hovered = self.hovering_chip_index == Some(index);
        let icon_slot = build_icon_slot(icon, agent.as_ref(), is_active, cx);
        let mut card = Card::new(("vertical-tab", index))
            .height(card_height)
            .selected(is_active)
            .suppress_hover(chip_hovered || menu_is_open)
            .pin_overlay(menu_is_open)
            .title(title)
            .start_slot(icon_slot)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    let _ = &rename_label;
                    // Pane rows don't support rename yet.
                    if pane_id.is_none() {
                        on_rename_for_dblclick(index, window, cx);
                    } else if let Some(pid) = pane_id {
                        this.activate_pane(index, pid, window, cx);
                    }
                } else {
                    match pane_id {
                        Some(pid) => this.activate_pane(index, pid, window, cx),
                        None => this.activate_session(index, window, cx),
                    }
                }
            }));

        // Route the hover chip differently per view mode:
        //
        // - **Tabs mode**: the chip sits on the card itself (`Card::overlay`
        //   anchors it to the card's top-right corner).
        // - **Panes mode, single-pane row**: the chip is lifted out of
        //   the card and pinned to the pane wrapper's top-right so it
        //   sits at the top edge of the strip.
        // - **Panes mode, row inside a session container**:
        //   `hover_actions` is `None` — the container renders one
        //   shared chip at the session level.
        let panes_chip = if is_panes_mode_render {
            hover_actions
        } else {
            if let Some(chip) = hover_actions {
                card = card.overlay(chip);
            }
            None
        };

        if let Some(subtitle) = subtitle {
            card = card.subtitle(subtitle);
        }

        // Unread-activity dot. Rendered in the card's end-slot (inline
        // after the title, always visible) when the attached agent
        // session has accumulated hook events since the pane last took
        // focus. `carrot_cli_agents::focus_pane` — called from the
        // terminal pane's focus handler — resets the counter, so this
        // dot naturally disappears when the user clicks into the pane.
        if let Some(agent_info) = agent.as_ref()
            && agent_info.unread > 0
        {
            let accent = cx.theme().colors().accent;
            let dot = div().size(px(6.)).rounded_full().bg(accent);
            card = card.end_slot(dot);
        }

        if is_expanded {
            if let Some(desc) = description {
                card = card.description(desc);
            }
            let mut badges = h_flex().gap_2().items_center();
            let mut has_badge = false;
            if let Some(stats) = diff_stats.as_ref() {
                let colors = cx.theme().colors();
                if stats.insertions > 0 || stats.deletions > 0 {
                    badges = badges.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.chip.git_stats_insert)
                                    .child(format!("+{}", stats.insertions)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.chip.git_stats_delete)
                                    .child(format!("-{}", stats.deletions)),
                            ),
                    );
                    has_badge = true;
                }
            }
            if let Some(root) = worktree_root.as_ref() {
                let colors = cx.theme().colors();
                badges = badges.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            carrot_ui::Icon::new(IconName::GitBranch)
                                .size(IconSize::XSmall)
                                .color(carrot_ui::Color::Muted),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(colors.text_muted)
                                .child(root.clone()),
                        ),
                );
                has_badge = true;
            }
            if let Some(pr) = pr_info.as_ref() {
                let colors = cx.theme().colors();
                use carrot_shell_integration::gh_cli::PrState;
                let (icon_name, icon_color) = match pr.state {
                    PrState::Open => (IconName::PullRequest, carrot_ui::Color::Success),
                    PrState::Merged => (IconName::GitMerge, carrot_ui::Color::Accent),
                    PrState::Closed => (IconName::GitPullRequestClosed, carrot_ui::Color::Error),
                    PrState::Draft => (IconName::GitPullRequestDraft, carrot_ui::Color::Muted),
                };
                badges = badges.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            carrot_ui::Icon::new(icon_name)
                                .size(IconSize::XSmall)
                                .color(icon_color),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(colors.text_muted)
                                .child(format!("#{}", pr.number)),
                        ),
                );
                has_badge = true;
            }
            if has_badge {
                card = card.badges_row(badges);
            }
        }

        // Drag-and-drop reorder. Wrap the Card in a stateful div that
        // carries the drag payload + accepts drops from other cards.
        let drag_payload = DraggedVerticalTab {
            index,
            label: drag_label.clone(),
        };
        let ws_drop = self.workspace.clone();
        // `px_2` here gives the card horizontal breathing room from the
        // sidebar edge in both view modes. In Panes mode the outer pane
        // wrapper below intentionally stays edge-to-edge so its hover
        // band reaches the sidebar edges; the inset lives on the inner
        // drag-wrapper so only the card shifts, not the hover zone.
        let drag_wrapper = div()
            .id(("vertical-tab-drag", index))
            .w_full()
            .px_2()
            .on_drag(drag_payload, |payload, _, _window, cx| {
                cx.new(|_| DraggedVerticalTabView {
                    label: payload.label.clone(),
                })
            })
            .drag_over::<DraggedVerticalTab>({
                let accent = cx.theme().colors().text_accent;
                move |style, _, _, _| style.bg(accent.opacity(0.1))
            })
            .on_drop(move |dragged: &DraggedVerticalTab, window, cx| {
                let from = dragged.index;
                let to = index;
                if from != to
                    && let Some(ws) = ws_drop.upgrade()
                {
                    let _ = window;
                    ws.update(cx, |ws, cx| ws.move_session(from, to, cx));
                }
            })
            .child(card);

        // Panes mode: each row lives inside an outer "pane" div with
        // its own hover state + divider to the next pane. Tabs mode
        // just emits the drag_wrapper directly.
        if is_panes_mode_render {
            let colors = cx.theme().colors();
            let needs_top_divider = *previous_pane_row_seen;
            *previous_pane_row_seen = true;
            // Floating hover chip for Panes mode. Anchored to the pane
            // wrapper's top-right corner (not the card's), so it sits at
            // the very top edge of the pane strip. Horizontal offset is
            // `right_1` (4px) so the chip hangs near the sidebar's right
            // edge, past the card's own inset. Visibility is tied to the
            // pane wrapper's group so hovering anywhere in the pane strip
            // reveals the chip.
            let pane_chip = panes_chip.map(|chip| {
                div()
                    .absolute()
                    .top_0()
                    .right_1()
                    .flex()
                    .items_center()
                    .p_0p5()
                    .rounded(px(4.))
                    .bg(colors.elevated_surface)
                    .map(|el| {
                        if menu_is_open {
                            el
                        } else {
                            el.invisible()
                                .group_hover("carrot-pane-row", |s| s.visible())
                        }
                    })
                    .child(chip)
            });
            // Session-level dividers live in the render loop as their
            // own 1px spacer elements — they mark session boundaries,
            // never pane boundaries inside a session. Ignore
            // `needs_top_divider` here.
            let _ = needs_top_divider;
            let mut wrapper = div()
                .id(("vertical-tab-pane-wrapper", index))
                .group("carrot-pane-row")
                .relative()
                .w_full()
                .py_1p5();
            // When the outer render loop has already wrapped this
            // row's session into one shared pane-strip container, the
            // per-row bg/hover must not fire — the container owns the
            // strip. Single-pane sessions still get their own strip
            // via this branch (container is skipped when pane_count
            // <= 1 in the render loop).
            if !in_session_container {
                // Darker pane-strip bg so the cards inside (which keep
                // the default `element_hover` / `element_selected`) stand
                // out as distinctly lighter rectangles on the strip
                // instead of blending in. `element_background` sits
                // between `panel.background` and `element.hover` on the
                // lightness ladder, so the strip reads as a distinct
                // hover band above the panel base while still leaving
                // headroom for the card to stand out.
                wrapper = wrapper
                    .when(is_active, |el| el.bg(colors.element_background))
                    .hover(|el| el.bg(colors.element_background));
            }
            wrapper
                .child(drag_wrapper)
                .when_some(pane_chip, |el, chip| el.child(chip))
                .into_any_element()
        } else {
            *previous_pane_row_seen = false;
            drag_wrapper.into_any_element()
        }
    }
}
