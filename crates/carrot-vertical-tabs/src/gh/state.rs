//! Async gh-cli state machine + modal deployment for the PR badge
//! feature.
//!
//! Three panel methods live here because they share the same narrow
//! concern (detecting whether `gh` is usable and nudging the user
//! when it isn't):
//!
//! - `ensure_gh_state` — one-shot background detection of
//!   `gh --version` + `gh auth status`, guarded so render can call it
//!   every frame without respawning;
//! - `ensure_pr_fetch` — per-`(branch, cwd)` background `gh pr list`
//!   lookup with inline cache + in-flight set, identical pattern to
//!   the state probe;
//! - `deploy_gh_prompt_if_needed` — opens the install or auth modal
//!   when the detection result says gh isn't ready, exactly once per
//!   panel lifetime (and not at all if the user has already dismissed
//!   the prompt this session via `GhPromptDismissed`).

use inazuma::{AppContext, Context, SharedString, Window};

use crate::gh::{auth_modal, install_modal};
use crate::{GhPromptDismissed, GhState, VerticalTabsPanel};

impl VerticalTabsPanel {
    /// Kick off gh-availability detection on first use. Idempotent:
    /// does nothing once `gh_state` has been resolved, and uses
    /// `gh_detection_in_flight` to avoid spawning a second task while
    /// the first is still running. Blocking shell-out stays on the
    /// background executor so the UI thread never stalls on gh.
    pub(crate) fn ensure_gh_state(&mut self, cx: &mut Context<Self>) {
        if self.gh_state.is_some() || self.gh_detection_in_flight {
            return;
        }
        self.gh_detection_in_flight = true;
        let probe = cx.background_spawn(async move {
            use carrot_shell_integration::gh_cli::{check_gh_authenticated, check_gh_installed};
            if !check_gh_installed() {
                GhState::NotInstalled
            } else if !check_gh_authenticated() {
                GhState::NotAuthenticated
            } else {
                GhState::Ready
            }
        });
        cx.spawn(async move |this, cx| {
            let state = probe.await;
            this.update(cx, |this, cx| {
                this.gh_state = Some(state);
                this.gh_detection_in_flight = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Kick off a PR lookup for one (branch, cwd) pair if we don't
    /// have one in flight and it isn't cached. The `gh pr list` call
    /// can take several hundred milliseconds so it runs on the
    /// background executor; once resolved, the cache entry lands and
    /// the panel re-renders with the badge.
    pub(crate) fn ensure_pr_fetch(
        &mut self,
        branch: SharedString,
        cwd: SharedString,
        cx: &mut Context<Self>,
    ) {
        let key = (branch.clone(), cwd.clone());
        if self.pr_cache.contains_key(&key) || self.pr_fetches_in_flight.contains(&key) {
            return;
        }
        self.pr_fetches_in_flight.insert(key.clone());
        let branch_s = branch.to_string();
        let cwd_path = std::path::PathBuf::from(cwd.to_string());
        let fetch = cx.background_spawn(async move {
            use carrot_shell_integration::gh_cli::fetch_pr_for_branch;
            // An Err result (gh missing, not authed, offline, not a
            // repo) is treated the same as "no PR" — the badge simply
            // doesn't render. The install/auth modal covers the
            // user-facing remediation for the first two cases.
            fetch_pr_for_branch(&branch_s, &cwd_path).ok().flatten()
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                this.pr_fetches_in_flight.remove(&key);
                this.pr_cache.insert(key, result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Deploy the install or auth modal when gh isn't usable. Called
    /// once per panel lifetime (guarded by `gh_prompt_shown`) and
    /// skipped entirely if the user has cancelled the prompt before
    /// (tracked in the `GhPromptDismissed` Global).
    pub(crate) fn deploy_gh_prompt_if_needed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.gh_prompt_shown {
            return;
        }
        if cx.global::<GhPromptDismissed>().0 {
            return;
        }
        let state = match self.gh_state {
            Some(GhState::NotInstalled) | Some(GhState::NotAuthenticated) => self.gh_state.unwrap(),
            _ => return,
        };
        // Grab a terminal handle from the currently active session so
        // the install/auth command runs inside the user's own shell
        // (matching the ShellInstallModal UX). Bail silently if no
        // terminal pane is active — we don't want to wedge the user
        // in a modal they can't act on.
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let handle = workspace.read(cx).active_session().read(cx).active_pane();
        let handle = handle.read(cx).active_item();
        let Some(handle) = handle.and_then(|i| i.terminal_handle(cx)) else {
            return;
        };
        self.gh_prompt_shown = true;
        workspace.update(cx, |ws, cx| match state {
            GhState::NotInstalled => {
                ws.toggle_modal(window, cx, |window, cx| {
                    install_modal::GhInstallModal::new(handle, window, cx)
                });
            }
            GhState::NotAuthenticated => {
                ws.toggle_modal(window, cx, |window, cx| {
                    auth_modal::GhAuthModal::new(handle, window, cx)
                });
            }
            GhState::Ready => {}
        });
    }
}
