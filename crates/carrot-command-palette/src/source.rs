//! Search sources — providers of searchable items.
//!
//! A `SearchSource` owns one category and produces `SearchResult`s for the
//! command palette modal. Results carry a `SearchAction` so the modal can
//! dispatch them without knowing source-specific types.

mod actions;
mod env_vars;
mod files;
mod history;
mod sessions;

use std::path::PathBuf;
use std::sync::Arc;

use carrot_ui::IconName;
use carrot_workspace::{OpenOptions, Workspace};
use inazuma::{Action, App, ClipboardItem, Entity, SharedString, Window};

use crate::category::SearchCategory;

pub use actions::ActionsSource;
pub use env_vars::EnvVarsSource;
pub(crate) use files::init as files_init;
pub(crate) use files::split_path_positions;
pub use files::{FilesSource, FilesSourceStatus};
pub use history::HistorySource;
pub use sessions::SessionsSource;

/// One row in the result list. The `id` is stable within a single source so
/// the list can preserve selection across re-renders.
pub struct SearchResult {
    pub id: SharedString,
    pub category: SearchCategory,
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub icon: IconName,
    pub action: SearchAction,
}

/// Runnable invocation bound to a result. Kept as a closed enum so the modal
/// can dispatch without needing a trait object that captures the workspace.
pub enum SearchAction {
    /// Switch to the session at this index.
    ActivateSession { index: usize },
    /// Copy the given text to the clipboard (used for env vars).
    CopyToClipboard(String),
    /// Dispatch a global action through the window — the workspace (or
    /// whichever layer registered the handler) runs the action.
    DispatchAction(Box<dyn Action>),
    /// Open an absolute filesystem path — routed through the workspace's
    /// Editor-in-Terminal-Session pane-role helper so it lands in the
    /// right pane instead of replacing a terminal.
    OpenPath(PathBuf),
}

impl SearchResult {
    /// Returns the underlying [`Action`] if this result dispatches one —
    /// used by the palette UI to render a keybinding badge next to each
    /// command.
    pub fn action_ref(&self) -> Option<&dyn Action> {
        match &self.action {
            SearchAction::DispatchAction(a) => Some(a.as_ref()),
            _ => None,
        }
    }
}

impl SearchAction {
    pub fn run(self, workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
        match self {
            SearchAction::ActivateSession { index } => {
                workspace.update(cx, |ws, cx| {
                    ws.activate_session(index, window, cx);
                });
            }
            SearchAction::CopyToClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            SearchAction::DispatchAction(action) => {
                window.dispatch_action(action, cx);
            }
            SearchAction::OpenPath(path) => {
                workspace.update(cx, |ws, cx| {
                    // Try the direct Editor-in-Terminal-Session pathway
                    // first: resolves the absolute path to a `ProjectPath`
                    // and hands it straight to `open_path_at_target_pane_for_role`.
                    // If the file lives outside any known worktree we fall
                    // back to `open_paths`, which creates a worktree for it
                    // and still routes through the same pane-role helper.
                    let project_path = ws
                        .project()
                        .read(cx)
                        .project_path_for_absolute_path(&path, cx);
                    if let Some(project_path) = project_path {
                        ws.open_path_at_target_pane_for_role(
                            project_path,
                            false,
                            true,
                            true,
                            window,
                            cx,
                        )
                        .detach_and_log_err(cx);
                    } else {
                        ws.open_paths(vec![path], OpenOptions::default(), None, window, cx)
                            .detach();
                    }
                });
            }
        }
    }
}

/// Provider of searchable items for one category. `collect()` runs each
/// time the user's query or scope changes; sources should be cheap to call
/// and produce plain data. Mutable access to the app context is provided so
/// stateful sources (e.g. [`FilesSource`]) can spawn walkers or write back
/// to caches without threading a separate channel.
///
/// Two orthogonal gates control when a source participates:
/// - [`default_visible`] decides whether the source contributes to the
///   empty-query "Suggested" view (typically a tiny curated slice).
/// - [`searchable`] decides whether the source is queried once the user
///   types something with no explicit category filter active.
///
/// Being opt-out of Suggested (`default_visible=false`) does not prevent
/// typed-query matches — the two choices are independent.
pub trait SearchSource: Send + Sync {
    fn category(&self) -> SearchCategory;
    fn collect(
        &self,
        workspace: &Entity<Workspace>,
        query: &str,
        window: &Window,
        cx: &mut App,
    ) -> Vec<SearchResult>;

    /// Contributes to the empty-query "Suggested" view. Return `false`
    /// to stay out of the default opening list; the chip and the
    /// category prefix still reach this source on demand.
    fn default_visible(&self) -> bool {
        true
    }

    /// Participates in universal search once the user types a query.
    /// Return `false` for bulk sources like env vars that would otherwise
    /// match nearly every keystroke; the user can still reach them via
    /// the explicit `env:` prefix or the chip.
    fn searchable(&self) -> bool {
        true
    }

    /// Optional footer line — status such as "2,453 files scanned" or a
    /// scope breadcrumb. Returned as a plain string so the modal doesn't
    /// have to know source-specific UI widgets.
    fn footer_status(&self, _cx: &App) -> Option<FilesSourceStatus> {
        None
    }
}

/// Built-in sources registered by the command palette modal. Order matters —
/// `Actions` first so curated commands are the most prominent row when the
/// modal opens, followed by `Sessions`, then bulk sources.
pub fn default_sources() -> Vec<Arc<dyn SearchSource>> {
    vec![
        Arc::new(ActionsSource),
        Arc::new(SessionsSource),
        Arc::new(FilesSource::new()),
        Arc::new(HistorySource::new()),
        Arc::new(EnvVarsSource),
    ]
}
