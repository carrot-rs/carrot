//! Row data resolution for the vertical tabs panel.
//!
//! `resolve_row_data` walks the workspace's sessions + their panes and
//! produces a flat `Vec<TabRowData>` the render loop iterates over. The
//! logic is pure: given settings + session state, it returns rows —
//! no rendering, no mutation of panel state. Keeping this in its own
//! module makes the render path a straightforward "transform rows to
//! elements" step and gives new metadata types (agent status, session
//! color, etc.) an obvious place to land without bloating render code.

use carrot_cli_agents::{CliAgentSessionState, GlobalCliAgentSessionManager};
use carrot_ui::IconName;
use carrot_workspace::{Pane, item::PaneRole};
use inazuma::{App, Entity, Oklch, Rgba, SharedString, rgb};
use inazuma_settings_framework::{AdditionalMetadata, PaneTitleSource, VerticalTabsViewMode};

use crate::vertical_tabs_settings::VerticalTabsSettings;
use crate::{GhState, VerticalTabsPanel};

/// Agent-session metadata surfaced onto one row. Resolved from the
/// `GlobalCliAgentSessionManager` global during `resolve_row_data` so
/// the render path only reads from the row struct — no additional
/// global lookups while building elements.
///
/// `None` on a `TabRowData` means either the global isn't installed
/// yet (early startup) or no CLI agent is attached to the pane's
/// terminal (plain shell session).
#[derive(Clone)]
pub(crate) struct AgentRowInfo {
    /// Embedded-asset path for the agent's monochrome sparkle SVG
    /// (`fill="currentColor"` so it can be tinted with the brand fg).
    /// Rendered inside the composed brand circle; never shown on its
    /// own. Example: `"icons/agents/claude_code.svg"`.
    pub(crate) icon_path: SharedString,
    /// Brand background colour for the composed circle, pre-converted
    /// from the agent's hex declaration. `None` means the row has no
    /// brand identity and should fall back to the generic pane icon.
    pub(crate) brand_bg: Option<Oklch>,
    /// Brand foreground colour — tints the sparkle SVG on top of the
    /// circle. Only meaningful when `brand_bg` is `Some`.
    pub(crate) brand_fg: Option<Oklch>,
    /// Display label for the agent. Set from the CLI session's
    /// `--name` flag when present, otherwise the agent's static
    /// `display_name()` ("Claude Code", "Codex", …). Used as the row
    /// title when `pane_title = Command/Conversation`.
    pub(crate) display_name: SharedString,
    /// Latest session FSM state. Drives status-badge color.
    pub(crate) state: CliAgentSessionState,
    /// Count of hook events received since the pane last took focus.
    /// Drives the accent notification dot on the card's right edge;
    /// reset to 0 by `carrot_cli_agents::focus_pane` on pane focus.
    pub(crate) unread: u32,
}

/// Parse a `#RRGGBB` hex string into an Oklch colour. Returns `None`
/// on malformed input (empty string, wrong length, bad hex). Used
/// once per row resolution for the brand pair — the cost is tiny and
/// avoids caching complexity for now.
fn parse_brand_hex(hex: &str) -> Option<Oklch> {
    let trimmed = hex.strip_prefix('#').unwrap_or(hex);
    if trimmed.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(trimmed, 16).ok()?;
    let rgba: Rgba = rgb(value);
    Some(rgba.into())
}

/// Look up the CLI-agent session attached to `pane`, if any. Returns
/// `None` when the global manager hasn't been installed yet or the
/// pane's terminal has no recognised agent child process.
fn lookup_agent_for_pane(pane: &Entity<Pane>, cx: &App) -> Option<AgentRowInfo> {
    let wrapper = cx.try_global::<GlobalCliAgentSessionManager>()?;
    let manager = wrapper.0.read(cx);
    let session = manager.session_for_pane(pane.entity_id().as_u64())?;
    let (brand_bg, brand_fg) = match session.agent.brand_colors() {
        Some((bg, fg)) => (parse_brand_hex(bg), parse_brand_hex(fg)),
        None => (None, None),
    };
    let display_name: SharedString = session
        .name
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| session.agent.display_name().into());
    Some(AgentRowInfo {
        icon_path: session.agent.icon_path().into(),
        brand_bg,
        brand_fg,
        display_name,
        state: session.state.clone(),
        unread: session.unread_events_since_focus,
    })
}

/// Resolved row data for one tab.
///
/// Four shapes are possible:
///
/// 1. **Tabs mode, any session** — `pane_id = None`, `header_text = None`.
///    One row representing the whole session; title/subtitle/etc. come
///    from the session's active pane. Clicking activates the session.
/// 2. **Panes mode, session with exactly one pane** — same as #1. The
///    header + single child pane would be redundant, so we collapse
///    it into a single row that behaves like Tabs mode for that
///    session.
/// 3. **Panes mode, group-header row** — `header_text = Some(session
///    name)`, `pane_id = None`. Rendered as a plain muted text label
///    above the session's pane rows. Not clickable, no chip.
/// 4. **Panes mode, pane row inside a multi-pane group** —
///    `pane_id = Some(entity_id)`, `header_text = None`. Rendered as a
///    card scoped to one pane; clicking activates the session + pane.
#[derive(Clone)]
pub(crate) struct TabRowData {
    /// Session position in the workspace's session list. Stable across
    /// renders; used as the primary lookup key for session activation.
    pub(crate) session_index: usize,
    /// EntityId of the specific pane when this row represents one pane
    /// of a multi-pane session group. `None` means either a whole
    /// session (Tabs mode) or a group-header row.
    pub(crate) pane_id: Option<inazuma::EntityId>,
    /// When `Some`, this row is a pure group header (renders as a
    /// muted text label only — no card, no icon, no chip). All other
    /// fields below are ignored for header rows.
    pub(crate) header_text: Option<SharedString>,
    pub(crate) title: SharedString,
    pub(crate) subtitle: Option<SharedString>,
    /// Secondary muted line below subtitle, only rendered in expanded
    /// density. Typically the working directory when subtitle carries
    /// the branch. Derived from the item's `ShellContext::cwd_short`
    /// and suppressed when it would duplicate the title or subtitle.
    pub(crate) description: Option<SharedString>,
    /// Diff stats for the expanded badges row (`+N -M`). Pulled from
    /// `ShellContext::git_stats` — populated by the shell hook via
    /// OSC 7777 on every prompt, so no polling is required.
    pub(crate) diff_stats: Option<carrot_shell_integration::GitStats>,
    /// Worktree git root for the expanded badges row. Set when the
    /// session's `git_root` (from the shell hook) doesn't match any of
    /// the main workspace's visible worktrees — typically a `gh pr
    /// checkout` worktree spawned outside the primary project.
    pub(crate) worktree_root: Option<SharedString>,
    /// Pull-request info for the current branch. Resolved via
    /// `gh pr list` on a background task whose result is cached per
    /// `(branch, cwd)` pair. `None` means either no PR exists for the
    /// branch, the lookup hasn't finished yet, or gh isn't available.
    pub(crate) pr_info: Option<carrot_shell_integration::gh_cli::PrInfo>,
    pub(crate) icon: IconName,
    pub(crate) is_active: bool,
    /// CLI-agent session attached to this pane, if any. Populated by
    /// `resolve_row_data` via `GlobalCliAgentSessionManager`. `None`
    /// either means "no agent attached" or "manager global not yet
    /// installed"; in either case the row renders as a plain terminal.
    pub(crate) agent: Option<AgentRowInfo>,
}

impl VerticalTabsPanel {
    pub(crate) fn resolve_row_data(
        &self,
        settings: &VerticalTabsSettings,
        cx: &App,
    ) -> Vec<TabRowData> {
        // Collect the main workspace project roots once so the per-row
        // loop can cheaply answer "is this session's git_root a worktree
        // or the primary project?". In the terminal-first default layout
        // there are usually zero project worktrees, which means *every*
        // session's git_root counts as a worktree — that's fine: users
        // on a plain terminal won't hit this path because git_root is
        // only populated once the shell reports it.
        let workspace_roots: Vec<String> = self
            .workspace
            .upgrade()
            .map(|ws| {
                ws.read(cx)
                    .visible_worktrees(cx)
                    .map(|wt| wt.read(cx).abs_path().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let is_panes_mode = matches!(settings.view_mode, VerticalTabsViewMode::Panes);
        let mut rows: Vec<TabRowData> = Vec::new();

        log::debug!(
            target: "carrot::routing",
            "sidebar rows: sessions={} panes_mode={}",
            self.cached_sessions.len(),
            is_panes_mode
        );
        for (session_index, session) in self.cached_sessions.iter().enumerate() {
            let session_data = session.read(cx);
            let session_name = session_data.name();
            let session_active_pane = session_data.active_pane().clone();
            let all_panes = session_data.panes();
            log::debug!(
                target: "carrot::routing",
                "sidebar session[{}] panes={} active_pane={:?} pane_ids={:?}",
                session_index,
                all_panes.len(),
                session_active_pane.entity_id(),
                all_panes.iter().map(|p| p.entity_id()).collect::<Vec<_>>()
            );
            // Three shapes:
            //   Tabs mode                → 1 row from active_pane
            //   Panes, 1 pane            → 1 row from active_pane (collapsed group)
            //   Panes, >1 panes          → header row + N pane rows
            // Header rows don't participate in the per-pane loop; they
            // only need a title.
            let emit_group_header = is_panes_mode && all_panes.len() > 1;
            if emit_group_header {
                // Group header: show the session name verbatim, or fall
                // back to a numbered label so the user can still tell
                // groups apart when nothing has been renamed.
                let header = session_name
                    .cloned()
                    .unwrap_or_else(|| format!("Session {}", session_index + 1).into());
                rows.push(TabRowData {
                    session_index,
                    pane_id: None,
                    header_text: Some(header),
                    title: SharedString::new_static(""),
                    subtitle: None,
                    description: None,
                    diff_stats: None,
                    worktree_root: None,
                    pr_info: None,
                    icon: IconName::Terminal,
                    is_active: false,
                    agent: None,
                });
            }
            let panes: Vec<Entity<Pane>> = if emit_group_header {
                all_panes.to_vec()
            } else {
                vec![session_active_pane.clone()]
            };

            for pane in panes {
                let pane_data = pane.read(cx);
                let item = pane_data.active_item();
                let pane_role = item
                    .as_ref()
                    .map(|i| i.pane_role(cx))
                    .unwrap_or(PaneRole::Terminal);
                let tab_text = item
                    .as_ref()
                    .map(|i| i.tab_content_text(0, cx))
                    .unwrap_or_else(|| SharedString::new_static("Session"));
                let shell_context = item.as_ref().and_then(|i| i.shell_context(cx));
                let cwd_short: Option<SharedString> = shell_context
                    .as_ref()
                    .map(|s| SharedString::from(s.cwd_short.clone()));
                let branch: Option<SharedString> = shell_context
                    .as_ref()
                    .and_then(|s| s.git_branch.clone())
                    .map(SharedString::from);

                // Agent metadata uses the actual pane's EntityId as the
                // manager lookup key — matches the id that
                // `TerminalPane::pane_changed` hands to
                // `carrot_cli_agents::register_terminal`. Works for both
                // Panes-mode per-pane rows and the collapsed/Tabs-mode
                // row (where `pane == session_active_pane`). Resolved
                // up-front because the title path below prefers the
                // agent's display name over the raw tab text.
                let agent = lookup_agent_for_pane(&pane, cx);

                // Title priority (highest → lowest):
                //   1. User-applied session rename (`session.set_name`).
                //   2. Agent display name — only under `Command` mode,
                //      because `Directory`/`Branch` are explicit "show me
                //      the cwd/branch" choices and shouldn't be silently
                //      overridden by the agent identity.
                //   3. Per-mode field: tab text (command) / cwd / branch.
                //
                // Multi-pane group rows skip #1 since the group header
                // already carries the session name — per-pane rows
                // should stay distinguishable inside a group.
                let title = if !emit_group_header && let Some(name) = session_name {
                    name.clone()
                } else {
                    match settings.pane_title {
                        PaneTitleSource::Command => agent
                            .as_ref()
                            .map(|a| a.display_name.clone())
                            .unwrap_or_else(|| tab_text.clone()),
                        PaneTitleSource::Directory => {
                            cwd_short.clone().unwrap_or_else(|| tab_text.clone())
                        }
                        PaneTitleSource::Branch => {
                            branch.clone().unwrap_or_else(|| tab_text.clone())
                        }
                    }
                };

                let subtitle_raw: Option<SharedString> = match settings.additional_metadata {
                    AdditionalMetadata::Branch => branch.clone(),
                    AdditionalMetadata::Directory => cwd_short.clone(),
                    AdditionalMetadata::Command => Some(tab_text.clone()),
                };
                let subtitle = subtitle_raw.filter(|s| s != &title);

                let description = cwd_short
                    .clone()
                    .filter(|cwd| cwd != &title && subtitle.as_ref().is_none_or(|sub| sub != cwd));

                let icon = match pane_role {
                    PaneRole::Terminal => IconName::Terminal,
                    PaneRole::Editor => IconName::FileCode,
                };

                let diff_stats = if settings.show_diff_stats {
                    shell_context.as_ref().and_then(|s| s.git_stats.clone())
                } else {
                    None
                };

                let worktree_root = shell_context
                    .as_ref()
                    .and_then(|s| s.git_root.as_ref())
                    .filter(|root| !workspace_roots.iter().any(|w| w == *root))
                    .and_then(|root| {
                        std::path::Path::new(root)
                            .file_name()
                            .map(|n| SharedString::from(n.to_string_lossy().to_string()))
                    });

                let pr_info = if settings.show_pr_link
                    && self.gh_state == Some(GhState::Ready)
                    && let (Some(branch), Some(cwd)) = (
                        branch.clone(),
                        shell_context
                            .as_ref()
                            .map(|s| SharedString::from(s.cwd.clone())),
                    ) {
                    self.pr_cache.get(&(branch, cwd)).cloned().flatten()
                } else {
                    None
                };

                // Multi-pane group rows represent one specific pane —
                // active only when its session is the workspace's active
                // session AND it's that session's active pane. Collapsed
                // single-pane rows behave like Tabs mode (session-active
                // check alone).
                let is_active = if emit_group_header {
                    session_index == self.cached_active_session_index && pane == session_active_pane
                } else {
                    session_index == self.cached_active_session_index
                };

                let pane_id = if emit_group_header {
                    Some(pane.entity_id())
                } else {
                    None
                };

                rows.push(TabRowData {
                    session_index,
                    pane_id,
                    header_text: None,
                    title,
                    subtitle,
                    description,
                    diff_stats,
                    worktree_root,
                    pr_info,
                    icon,
                    is_active,
                    agent,
                });
            }
        }

        rows
    }
}
