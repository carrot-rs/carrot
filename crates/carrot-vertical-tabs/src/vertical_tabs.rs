mod actions;
mod gh;
mod render;
mod settings_popup;
mod vertical_tabs_settings;

pub use gh::install_modal::GhPromptDismissed;

pub use vertical_tabs_settings::{
    AdditionalMetadata, PaneTitleSource, VerticalTabsDensity, VerticalTabsSettings,
    VerticalTabsViewMode,
};

use std::collections::HashMap;

use crate::render::row_data::TabRowData;

use carrot_ui::{ContextMenu, IconName, PopoverMenuHandle, input::InputState, prelude::*};
use carrot_workspace::{
    Workspace, WorkspaceSession,
    dock::{DockPosition, PanelEvent},
};
use inazuma::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Render,
    SharedString, Subscription, WeakEntity, Window, actions, div, px,
};
use inazuma_settings_framework::{DockSide, Settings};

const VERTICAL_TABS_PANEL_KEY: &str = "VerticalTabsPanel";

actions!(
    vertical_tabs,
    [
        /// Toggles focus on the vertical tabs panel.
        ToggleFocus,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<VerticalTabsPanel>(window, cx);
        });

        workspace.register_action(
            |workspace, _: &carrot_actions::session::RenameActiveSession, window, cx| {
                let Some(panel) = workspace.panel::<VerticalTabsPanel>(cx) else {
                    return;
                };
                let session_index = workspace.active_session_index();
                let label = workspace
                    .sessions()
                    .get(session_index)
                    .map(|s| s.read(cx).display_label(cx))
                    .unwrap_or_default();
                workspace.focus_panel::<VerticalTabsPanel>(window, cx);
                panel.update(cx, |panel, cx| {
                    panel.start_rename(session_index, label, window, cx);
                });
            },
        );
    })
    .detach();
}

pub struct VerticalTabsPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    cached_sessions: Vec<Entity<WorkspaceSession>>,
    cached_active_session_index: usize,
    /// When the user double-clicks a card or picks "Rename tab" from the
    /// ⋮ menu, we capture the target session index together with a fresh
    /// `InputState` pre-filled with the current label. The card for that
    /// index renders the input instead of the static title until the user
    /// confirms (Enter) or cancels (Escape / blur).
    rename_in_progress: Option<(usize, Entity<InputState>)>,
    /// Index of the card whose hover chip is currently under the cursor.
    /// Used to suppress the card's own hover bg while the chip is hovered,
    /// so only the chip + its icon hover read as highlighted.
    hovering_chip_index: Option<usize>,
    /// Per-card popover menu handle. Lets us ask each card's ⋮ menu
    /// whether it is currently deployed (`is_deployed()`), so a card with
    /// its menu open keeps rendering in the hover state even after the
    /// cursor has moved off — matching the reference behaviour where the
    /// card stays "active" until the menu dismisses.
    menu_handles: HashMap<usize, PopoverMenuHandle<ContextMenu>>,
    /// Cache of PR lookups keyed by `(branch, cwd)`. A value of
    /// `Some(None)` means "gh pr list ran successfully and returned no
    /// PR" — distinct from "not yet fetched" (entry missing entirely).
    /// The cache is cleared when either the workspace or the branch set
    /// changes, not on every render, so gh is invoked at most once per
    /// distinct (branch, repo) pair per panel lifetime.
    pr_cache:
        HashMap<(SharedString, SharedString), Option<carrot_shell_integration::gh_cli::PrInfo>>,
    /// Branches currently being fetched. Prevents the render path from
    /// respawning a task while a previous fetch is still in flight.
    pr_fetches_in_flight: std::collections::HashSet<(SharedString, SharedString)>,
    /// Resolved gh CLI availability: `None` = not yet checked, `Some(state)`
    /// = last known state. Updated by a background task on first render
    /// when `show_pr_link` is on. Invalidated when the user toggles the
    /// setting off/on so install progress is reflected.
    gh_state: Option<GhState>,
    /// Guards against re-spawning the detection task while a previous
    /// check is still running. `gh_state` being `None` isn't enough —
    /// render can fire again before the first task completes.
    gh_detection_in_flight: bool,
    /// True once we've already deployed either the install or auth modal
    /// this session. Combined with `GhPromptDismissed`, this prevents
    /// the panel from reopening modals the user just closed.
    gh_prompt_shown: bool,
    _subscriptions: Vec<Subscription>,
}

/// Tri-state gh availability. Computed once (via background task) and
/// reused until the user toggles `show_pr_link` — reflects whether the
/// gh binary is on PATH and whether `gh auth status` succeeds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GhState {
    NotInstalled,
    NotAuthenticated,
    Ready,
}

impl VerticalTabsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            cx.new(|cx| Self::new(workspace, window, cx))
        })
    }

    pub fn new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace_weak = workspace.weak_handle();
        let focus_handle = cx.focus_handle();

        let sessions = workspace.sessions().to_vec();
        let active_index = workspace.active_session_index();

        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search tabs…"));

        let mut subscriptions = Vec::new();

        if let Some(ws) = workspace_weak.upgrade() {
            subscriptions.push(cx.observe_in(&ws, window, |this, workspace, _window, cx| {
                this.cached_sessions = workspace.read(cx).sessions().to_vec();
                this.cached_active_session_index = workspace.read(cx).active_session_index();
                cx.notify();
            }));
        }

        // Re-render whenever the search query changes so the tab list filters
        // live as the user types.
        subscriptions.push(cx.observe(&search_input, |_, _, cx| cx.notify()));

        // Live agent status + unread tracking. The manager emits a
        // `ManagerEventEnvelope` for every lifecycle event (state
        // transitions, unread resets, session start/end, etc.).
        // Subscribing here means the panel re-resolves rows on every
        // envelope, so status badges and unread dots update without
        // needing any workspace change to trigger a render. Guarded
        // by `try_global` because the manager is installed by
        // `carrot_cli_agents::init` and may not be present in minimal
        // test harnesses.
        if let Some(manager_wrapper) =
            cx.try_global::<carrot_cli_agents::GlobalCliAgentSessionManager>()
        {
            let manager = manager_wrapper.0.clone();
            subscriptions.push(cx.subscribe(
                &manager,
                |_this, _manager, _envelope: &carrot_cli_agents::ManagerEventEnvelope, cx| {
                    cx.notify();
                },
            ));
        }

        Self {
            workspace: workspace_weak,
            focus_handle,
            search_input,
            cached_sessions: sessions,
            cached_active_session_index: active_index,
            rename_in_progress: None,
            hovering_chip_index: None,
            menu_handles: HashMap::new(),
            pr_cache: HashMap::new(),
            pr_fetches_in_flight: std::collections::HashSet::new(),
            gh_state: None,
            gh_detection_in_flight: false,
            gh_prompt_shown: false,
            _subscriptions: subscriptions,
        }
    }
}

impl Focusable for VerticalTabsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for VerticalTabsPanel {}

impl Render for VerticalTabsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = *VerticalTabsSettings::get_global(cx);

        // PR-badge support path. Kicks off gh availability detection on
        // first use, then either deploys a remediation modal
        // (install/auth) or schedules per-branch PR lookups once gh is
        // known to be usable. All work is async — render returns with
        // whatever state we have right now.
        if settings.show_pr_link {
            self.ensure_gh_state(cx);
            self.deploy_gh_prompt_if_needed(window, cx);
            if self.gh_state == Some(GhState::Ready) {
                // Collect (branch, cwd) pairs from active panes before
                // mutating self — can't hold the iterator while we call
                // ensure_pr_fetch (which takes &mut self).
                let mut pending: Vec<(SharedString, SharedString)> = Vec::new();
                for session in self.cached_sessions.iter() {
                    let pane = session.read(cx).active_pane().clone();
                    let item = pane.read(cx).active_item();
                    let ctx = item.as_ref().and_then(|i| i.shell_context(cx));
                    if let (Some(branch), Some(ctx)) = (
                        ctx.as_ref().and_then(|c| c.git_branch.clone()),
                        ctx.as_ref(),
                    ) {
                        pending.push((
                            SharedString::from(branch),
                            SharedString::from(ctx.cwd.clone()),
                        ));
                    }
                }
                for (branch, cwd) in pending {
                    self.ensure_pr_fetch(branch, cwd, cx);
                }
            }
        }

        let query = self.search_query(cx).to_lowercase();
        let rows = self
            .resolve_row_data(&settings, cx)
            .into_iter()
            .filter(|row| {
                if query.is_empty() {
                    return true;
                }
                // Group-header rows match on the header text; pane
                // rows match on title + subtitle.
                if let Some(text) = row.header_text.as_ref() {
                    return text.to_lowercase().contains(&query);
                }
                let title_match = row.title.to_lowercase().contains(&query);
                let subtitle_match = row
                    .subtitle
                    .as_ref()
                    .map(|s| s.to_lowercase().contains(&query))
                    .unwrap_or(false);
                title_match || subtitle_match
            })
            .collect::<Vec<_>>();

        // Expanded rows carry up to 4 stacked slots (title + subtitle +
        // description + badges row). 64px gives each slot visible air
        // without stretching single-line compact rows.
        let card_height = match settings.density {
            VerticalTabsDensity::Compact => px(36.),
            VerticalTabsDensity::Expanded => px(64.),
        };

        let header = self.build_control_bar(cx);

        let is_expanded = matches!(settings.density, VerticalTabsDensity::Expanded);
        let is_panes_mode_render = matches!(settings.view_mode, VerticalTabsViewMode::Panes);

        // Tabs mode keeps a 2px gap between cards so they read as
        // floating chips. Panes mode drops the gap so adjacent pane
        // wrappers touch edge-to-edge — the 1px `border_t` on each
        // wrapper becomes the visible seam instead of a 2px panel-bg
        // strip showing through.
        let mut tab_list = v_flex()
            .id("vertical-tabs-list")
            .size_full()
            .overflow_y_scroll();
        if !is_panes_mode_render {
            tab_list = tab_list.gap_0p5();
        }
        // Chunk rows by session so every session renders as one visual
        // unit. Panes-mode sessions with > 1 pane wrap all their pane
        // rows into a single shared "pane strip" (bg only when hovered
        // or when one of its panes is active). The optional group
        // header sits above the strip. Sessions are separated from
        // each other by an explicit 1px divider element — no divider
        // between pane rows inside a session.
        let divider_color = cx.theme().colors().text.alpha(0.06);
        let mut previous_session_rendered = false;
        let mut i = 0;
        while i < rows.len() {
            let session_index = rows[i].session_index;
            let group_start = i;
            while i < rows.len() && rows[i].session_index == session_index {
                i += 1;
            }
            let group_end = i;

            let mut pane_start = group_start;
            let mut header_row: Option<TabRowData> = None;
            if rows[group_start].header_text.is_some() {
                header_row = Some(rows[group_start].clone());
                pane_start += 1;
            }
            let pane_count = group_end - pane_start;
            let group_has_active = rows[pane_start..group_end]
                .iter()
                .any(|r| r.is_active);
            let wrap_into_container = is_panes_mode_render && pane_count > 1;

            if previous_session_rendered {
                tab_list = tab_list.child(
                    div()
                        .w_full()
                        .h(px(1.))
                        .bg(divider_color),
                );
            }

            if let Some(header) = header_row {
                let mut previous_seen = false;
                let el = self.build_row(
                    header,
                    card_height,
                    is_expanded,
                    is_panes_mode_render,
                    false,
                    &mut previous_seen,
                    cx,
                );
                tab_list = tab_list.child(el);
            }

            if wrap_into_container {
                let active_bg = cx.theme().colors().element_background;
                let hover_bg = cx.theme().colors().element_background;
                let mut container = div()
                    .id(("session-pane-strip", session_index))
                    .group("carrot-pane-row")
                    .w_full()
                    .when(group_has_active, move |el| el.bg(active_bg))
                    .hover(move |el| el.bg(hover_bg));
                let mut previous_seen = false;
                for r in rows[pane_start..group_end].iter().cloned() {
                    container = container.child(self.build_row(
                        r,
                        card_height,
                        is_expanded,
                        is_panes_mode_render,
                        true,
                        &mut previous_seen,
                        cx,
                    ));
                }
                tab_list = tab_list.child(container);
            } else {
                let mut previous_seen = false;
                for r in rows[pane_start..group_end].iter().cloned() {
                    tab_list = tab_list.child(self.build_row(
                        r,
                        card_height,
                        is_expanded,
                        is_panes_mode_render,
                        false,
                        &mut previous_seen,
                        cx,
                    ));
                }
            }
            previous_session_rendered = true;
        }

        div()
            .key_context("VerticalTabsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            // Direct inline layout instead of FloatingPanel: the
            // FloatingPanel's outer `.p(margin)` was pushing content
            // 8px inward on every side, which broke the edge-to-edge
            // pane-wrapper look. We still want the panel background
            // and the header + tab-list vertical stack, just without
            // the outer margin — vertical tabs should meet the dock
            // edges so the per-pane hover band reaches full sidebar
            // width.
            .child(
                v_flex()
                    .size_full()
                    .bg(cx.theme().colors().panel.background)
                    .child(header)
                    .child(tab_list),
            )
    }
}

impl carrot_workspace::Panel for VerticalTabsPanel {
    fn persistent_name() -> &'static str {
        "Vertical Tabs"
    }

    fn panel_key() -> &'static str {
        VERTICAL_TABS_PANEL_KEY
    }

    fn position(&self, _window: &Window, cx: &App) -> DockPosition {
        match VerticalTabsSettings::get_global(cx).dock {
            DockSide::Left => DockPosition::Left,
            DockSide::Right => DockPosition::Right,
        }
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).app_state().fs.clone());
        if let Some(fs) = fs {
            inazuma_settings_framework::update_settings_file(fs, cx, move |settings, _| {
                let dock = match position {
                    DockPosition::Left | DockPosition::Bottom => DockSide::Left,
                    DockPosition::Right => DockSide::Right,
                };
                settings.vertical_tabs.get_or_insert_default().dock = Some(dock);
            });
        }
    }

    fn default_size(&self, _window: &Window, cx: &App) -> inazuma::Pixels {
        VerticalTabsSettings::get_global(cx).default_width
    }

    fn min_size(&self, _window: &Window, _cx: &App) -> inazuma::Pixels {
        // Below this width the search input + sliders + plus controls in the
        // header collide and the tab cards become unreadable.
        inazuma::px(200.)
    }

    fn icon(&self, _window: &Window, cx: &App) -> Option<IconName> {
        VerticalTabsSettings::get_global(cx)
            .button
            .then_some(IconName::ListTree)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Vertical Tabs")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        0
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }
}
