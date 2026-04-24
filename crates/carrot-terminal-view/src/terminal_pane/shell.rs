//! Shell-integration event handlers on [`TerminalPane`].
//!
//! Covers the full OSC 133 / OSC 7777 dispatch path plus shell-switch
//! state management. Extracted from the monolith so the terminal-pane
//! facade stays narrow and the chip / metadata logic sits next to the
//! shell context it serves.

use std::rc::Rc;
use std::sync::{Arc, RwLock};

use carrot_completions::command_correction;
use carrot_completions::shell_completion::ShellCompletionProvider;
use carrot_session::command_history::CommandHistory;
use carrot_shell_integration::shell_install;
use carrot_terminal::{Terminal, TerminalEvent};
use carrot_ui::input::InputState;
use inazuma::{Context, Window};

use crate::terminal_pane::{
    PendingShellInstallName, PendingShellSwitch, ShellOption, TerminalPane, TerminalPaneEvent,
    detect_available_shells, general_to_tui_awareness,
};

/// Marker type for deduplicating `ProjectDetected` toasts per worktree root.
pub(crate) struct ProjectDetectedMarker;

impl TerminalPane {
    pub(crate) fn handle_terminal_event(
        &mut self,
        event: TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::Wakeup => {
                self.update_interactive_mode();
                cx.notify();
            }
            TerminalEvent::Title(title) => {
                self.terminal_title = title;
                cx.emit(TerminalPaneEvent::TitleChanged);
                cx.notify();
            }
            TerminalEvent::Bell => {
                cx.emit(TerminalPaneEvent::BellRang);
            }
            TerminalEvent::Exit => {
                cx.emit(TerminalPaneEvent::CloseRequested);
                cx.notify();
            }
            TerminalEvent::ShellMarker(marker) => {
                self.handle_shell_marker(marker, window, cx);
            }
            TerminalEvent::BreadcrumbsChanged => {
                cx.notify();
            }
        }
    }

    pub(crate) fn handle_shell_marker(
        &mut self,
        marker: carrot_terminal::ShellMarker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let carrot_terminal::ShellMarker::Metadata(ref json) = marker {
            match serde_json::from_str::<carrot_shell_integration::ShellMetadataPayload>(json) {
                Ok(payload) => {
                    let cwd_changed = self.shell_context.cwd != payload.cwd;
                    let new_cwd = std::path::PathBuf::from(&payload.cwd);
                    self.shell_context.update_from_metadata(&payload);
                    self.shell_completion.update_cwd(new_cwd.clone());
                    self.detection_cache.invalidate();
                    self.last_exit_code = payload.last_exit_code;
                    self.last_duration_ms = payload.last_duration_ms;

                    let new_git_root = payload.git_root.as_ref().map(std::path::PathBuf::from);
                    if new_git_root != self.current_git_root {
                        self.current_git_root = new_git_root;
                    }

                    if cwd_changed
                        && let Some(project) = self.project.as_ref().and_then(|p| p.upgrade())
                    {
                        use carrot_shell::scope_policy::{ProjectKind, WorktreeRoot, classify};
                        use inazuma_settings_framework::Settings as _;
                        let classification = classify(&new_cwd);
                        let (worktree_path, detected_kind) = match &classification {
                            WorktreeRoot::ProjectLike { root, kind, .. } => {
                                (root.clone(), Some(*kind))
                            }
                            WorktreeRoot::AdHoc { cwd } => (cwd.clone(), None),
                        };
                        let should_track = matches!(detected_kind, Some(ProjectKind::Git)) && {
                            let scope = carrot_settings::WorktreeScopeSettings::get_global(cx);
                            scope
                                .git_track_decision(&worktree_path)
                                .should_track_immediately()
                        };
                        project.update(cx, |project, cx| {
                            if should_track {
                                project
                                    .ensure_tracked_worktree(&worktree_path, cx)
                                    .detach_and_log_err(cx);
                            } else {
                                project
                                    .ensure_browseable_worktree(&worktree_path, cx)
                                    .detach_and_log_err(cx);
                            }
                        });
                        if let Some(kind) = detected_kind
                            && !should_track
                        {
                            let notify = match kind {
                                ProjectKind::Git => {
                                    carrot_settings::WorktreeScopeSettings::get_global(cx)
                                        .git_track_decision(&worktree_path)
                                        .is_ask()
                                }
                                ProjectKind::AgentRules => true,
                                ProjectKind::Manifest(_) => false,
                            };
                            if notify {
                                cx.emit(TerminalPaneEvent::ProjectDetected {
                                    root: worktree_path.clone(),
                                    kind,
                                });
                                self.show_project_detected_toast(worktree_path, kind, cx);
                            }
                        }
                    }

                    cx.emit(TerminalPaneEvent::TitleChanged);
                    cx.notify();
                }
                Err(e) => {
                    log::warn!("Failed to parse shell metadata JSON: {}", e);
                }
            }
        }

        match marker {
            carrot_terminal::ShellMarker::PromptStart => {
                cx.notify();
            }
            carrot_terminal::ShellMarker::InputStart => {}
            carrot_terminal::ShellMarker::CommandStart => {
                {
                    let handle = self.terminal.handle();
                    let mut term = handle.lock();
                    term.block_router_mut().set_last_metadata(
                        carrot_term::block::RouterBlockMetadata {
                            command: None,
                            cwd: Some(self.shell_context.cwd.clone()),
                            username: Some(self.shell_context.username.clone()),
                            hostname: Some(self.shell_context.hostname.clone()),
                            git_branch: self.shell_context.git_branch.clone(),
                            shell: Some(self.shell_name.clone()),
                            started_at: None,
                            finished_at: None,
                            exit_code: None,
                        },
                    );
                }
                self.show_terminal = true;
                cx.notify();
            }
            carrot_terminal::ShellMarker::CommandEnd { exit_code } => {
                log::debug!("Command finished with exit code: {}", exit_code);

                self.input_state.update(cx, |state, cx| {
                    state.focus(window, cx);
                });

                if let Some(shell_name) = cx.global_mut::<PendingShellInstallName>().0.take() {
                    log::debug!("Install completed for {} (exit={})", shell_name, exit_code);
                    if exit_code == 0 && shell_install::check_shell_available(&shell_name) {
                        log::debug!(
                            "Shell {} is now available — queuing auto-switch",
                            shell_name
                        );
                        let path = shell_install::resolve_shell_path(&shell_name);
                        cx.global_mut::<PendingShellSwitch>().0 = Some(ShellOption {
                            name: shell_name,
                            path,
                            installed: true,
                        });
                        cx.notify();
                    } else {
                        log::debug!(
                            "Shell {} not found after install (exit={})",
                            shell_name,
                            exit_code,
                        );
                    }
                }

                if exit_code == 127 {
                    let last_cmd = {
                        let handle = self.terminal.handle();
                        let term = handle.lock();
                        term.block_router()
                            .entries()
                            .last()
                            .and_then(|e| e.metadata.command.clone())
                    };
                    if let Some(last_cmd) = last_cmd {
                        let known: Vec<String> = std::env::var("PATH")
                            .unwrap_or_default()
                            .split(':')
                            .filter_map(|dir| std::fs::read_dir(dir).ok())
                            .flat_map(|entries| entries.flatten())
                            .filter_map(|e| e.file_name().into_string().ok())
                            .collect();
                        self.correction_suggestion =
                            command_correction::suggest_correction(&last_cmd, exit_code, &known);
                    }
                } else {
                    self.correction_suggestion = None;
                }

                cx.notify();
            }
            carrot_terminal::ShellMarker::PromptKind { kind } => {
                log::debug!("Prompt kind: {:?}", kind);
            }
            carrot_terminal::ShellMarker::Metadata(_) => {
                // Already handled above.
            }
            carrot_terminal::ShellMarker::TuiHint(json) => {
                match serde_json::from_str::<carrot_shell_integration::TuiHintPayload>(&json) {
                    Ok(payload) => {
                        let enabled = payload.tui_mode.unwrap_or(false);
                        if enabled {
                            use carrot_term::block::{TuiAwareness, TuiDetector};
                            let handle = self.terminal.handle();
                            let mut term = handle.lock();
                            if let carrot_term::block::ActiveTarget::Block { block, .. } =
                                term.block_router_mut().active()
                            {
                                let origin = block.grid().total_rows() as u64;
                                TuiDetector::new(TuiAwareness::Full)
                                    .on_shell_hint(origin, 1)
                                    .apply(block);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to parse TUI hint JSON: {}", e);
                    }
                }
            }
            carrot_terminal::ShellMarker::AgentEvent(json) => {
                carrot_cli_agents::forward_hook_envelope(&json, cx);
            }
            carrot_terminal::ShellMarker::AgentEditActive => {
                // OSC 133 ;L — consumed by `carrot-cmdline`'s AI
                // ghost-text suppression path, no-op here.
            }
        }
    }

    pub(crate) fn request_shell_change(
        &mut self,
        shell: ShellOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if shell.installed {
            self.switch_shell(&shell, window, cx);
        } else if let Some(info) = shell_install::shell_install_info(&shell.name)
            && let Some(ws) = self.workspace.as_ref().and_then(|w| w.upgrade())
        {
            let terminal_handle = self.terminal.handle();
            ws.update(cx, |workspace, cx| {
                workspace.toggle_modal(window, cx, |window, cx| {
                    crate::shell_install_modal::ShellInstallModal::new(
                        info,
                        terminal_handle,
                        window,
                        cx,
                    )
                });
            });
        }
    }

    pub(crate) fn switch_shell(
        &mut self,
        shell: &ShellOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.global_mut::<PendingShellInstallName>().0 = None;

        let shell_path = match &shell.path {
            Some(p) => p.clone(),
            None => return,
        };
        let shell_name = &shell.name;
        use inazuma_settings_framework::Settings;
        let general = carrot_settings::GeneralSettings::get_global(cx);
        let cwd = std::path::PathBuf::from(&self.shell_context.cwd);
        let input_mode = match general.input_mode {
            carrot_settings::InputMode::Carrot => carrot_terminal::InputMode::Carrot,
            carrot_settings::InputMode::ShellPs1 => carrot_terminal::InputMode::ShellPs1,
        };
        let scrollback = general.scrollback_history;
        let tui_awareness = general_to_tui_awareness(general.tui_awareness);

        let new_terminal = Terminal::with_shell(
            self.last_terminal_rows.max(24),
            self.last_terminal_cols.max(80),
            &cwd,
            input_mode,
            scrollback,
            Some(&shell_path),
        );
        let new_terminal = match new_terminal {
            Ok(t) => t,
            Err(e) => {
                log::error!("Failed to spawn shell {}: {}", shell_path, e);
                carrot_shell::AppShell::update(window, &mut *cx, |shell, window, cx| {
                    shell.push_notification(
                        carrot_ui::Notification::error(format!(
                            "Failed to start {}: {}",
                            shell_name, e
                        )),
                        window,
                        cx,
                    );
                });
                return;
            }
        };
        new_terminal.set_tui_awareness(tui_awareness);

        let events_rx = new_terminal.event_receiver().clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events_rx.recv_async().await {
                this.update_in(cx, |view, window, cx| {
                    view.handle_terminal_event(event, window, cx);
                })
                .ok();
            }
        })
        .detach();

        if let Some(old_pty) = self.registered_pty_pid.take() {
            carrot_cli_agents::unregister_terminal(old_pty, cx);
        }
        self.terminal = new_terminal;
        if let (Some(pane_id), Some(new_pty)) = (self.registered_pane_id, self.terminal.pty_pid()) {
            carrot_cli_agents::register_terminal(
                new_pty,
                pane_id.as_u64(),
                std::path::PathBuf::from(&self.shell_context.cwd),
                cx,
            );
            self.registered_pty_pid = Some(new_pty);
        }

        self.shell_name = shell_name.to_string();
        self.terminal_title = format!("~ {}", shell_name);
        self.show_terminal = false;

        if self.history_panel.is_visible() {
            self.history_panel.close();
        }

        self.available_shells = detect_available_shells();

        let shell_lang = match shell_name.as_str() {
            "nu" => "nu",
            "fish" => "bash",
            _ => "bash",
        };
        self.input_state.update(cx, |state, cx| {
            state.set_shell_language(shell_lang, window, cx);
        });

        self.command_history = Arc::new(RwLock::new(CommandHistory::detect_and_load(shell_name)));

        self.shell_completion = Rc::new(ShellCompletionProvider::new(
            shell_name,
            cwd,
            self.command_history.clone(),
        ));
        self.input_state.update(cx, |state, _cx| {
            state.lsp.completion_provider = Some(self.shell_completion.clone());
        });

        self.block_list.update(cx, |view, _cx| view.clear());

        cx.emit(TerminalPaneEvent::TitleChanged);
        cx.notify();
    }

    /// Show a workspace-level notification offering the user to promote
    /// a freshly-detected scope to Tracked. Deduplicated per root via
    /// `NotificationId::composite` so re-entering the same directory
    /// doesn't stack toasts.
    fn show_project_detected_toast(
        &self,
        root: std::path::PathBuf,
        kind: carrot_shell::scope_policy::ProjectKind,
        cx: &mut Context<Self>,
    ) {
        use carrot_shell::scope_policy::ProjectKind;
        use carrot_workspace::notifications::NotificationId;
        use carrot_workspace::notifications::simple_message_notification::MessageNotification;
        use inazuma::{Action as _, AppContext as _};

        let Some(workspace) = self.workspace.as_ref().and_then(|w| w.upgrade()) else {
            return;
        };
        let root_display = root.display().to_string();
        let kind_label = match kind {
            ProjectKind::Git => "Git project",
            ProjectKind::AgentRules => "Project rules",
            ProjectKind::Manifest(_) => "Project manifest",
        };
        let message = format!("{kind_label} detected at {root_display}. Track it?");
        let offer_never = matches!(kind, ProjectKind::Git);
        workspace.update(cx, move |ws, cx| {
            ws.show_notification(
                NotificationId::composite::<ProjectDetectedMarker>(inazuma::ElementId::from(
                    inazuma::SharedString::from(root_display),
                )),
                cx,
                move |cx| {
                    cx.new(|cx| {
                        let mut notif = MessageNotification::new(message.clone(), cx)
                            .primary_message("Track")
                            .primary_on_click(|window, _cx| {
                                window.dispatch_action(
                                    carrot_actions::TrackActiveScope.boxed_clone(),
                                    _cx,
                                );
                            });
                        if offer_never {
                            notif = notif.secondary_message("Never").secondary_on_click(
                                |window, _cx| {
                                    window.dispatch_action(
                                        carrot_actions::NeverTrackScope.boxed_clone(),
                                        _cx,
                                    );
                                },
                            );
                        }
                        notif
                    })
                },
            );
        });
    }
}

/// Silence unused-import warnings when `InputState` isn't referenced
/// by every feature flag combination — the type is needed for the
/// `switch_shell` refresh path.
const _: fn() = || {
    let _ = std::mem::size_of::<InputState>;
};
