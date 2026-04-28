mod cmdline;
mod item;
mod keymap;
mod lifecycle;
mod searchable;
mod shell;

pub(crate) use keymap::keystroke_to_bytes;

use carrot_chips::{ChipContext, ChipRegistry, DetectionCache, collect_chip_env_vars};
use carrot_shell_integration::ShellContext;
use carrot_terminal::Terminal;
use carrot_ui::{
    Anchor, Chip, Popover, h_flex,
    input::{AutoPairConfig, InputState},
    v_flex,
};
use carrot_workspace::Workspace;
use inazuma::{
    App, Context, Entity, FocusHandle, Focusable, Oklch, ParentElement, Render, Styled, Window,
    div, oklcha, prelude::*, px, rgb,
};

use std::rc::Rc;
use std::sync::{Arc, RwLock};

use crate::input::history_panel::HistoryPanel;
use carrot_completions::command_correction;
use carrot_completions::shell_completion::ShellCompletionProvider;
use carrot_session::command_history::CommandHistory;
use carrot_shell_integration::shell_install;

/// A detected shell on the system.
#[derive(Clone)]
pub struct ShellOption {
    pub name: String,
    pub path: Option<String>,
    pub installed: bool,
}

/// Global state for pending shell switch requests from UI click handlers.
pub struct PendingShellSwitch(pub Option<ShellOption>);

impl inazuma::Global for PendingShellSwitch {}

/// Global state for pending branch checkout requests from UI click handlers.
pub struct PendingBranchSwitch(pub Option<String>);

impl inazuma::Global for PendingBranchSwitch {}

/// Global state for pending shell install — set when install command is sent to PTY.
/// Cleared on CommandEnd (success or failure) or manual shell switch.
pub struct PendingShellInstallName(pub Option<String>);

impl inazuma::Global for PendingShellInstallName {}

/// Navigation target for scrolling to a specific block in the terminal.
/// Used with `Item::navigate()` to support block-level navigation from
/// outline, go-to-block, search results, etc.
#[derive(Debug, Clone)]
pub struct BlockNavigationTarget {
    pub block_index: usize,
}

/// Detect all supported shells and their install status.
/// Convert the user-facing settings `TuiAwareness` enum to the runtime
/// variant consumed by `carrot-term`. Same discriminants, different
/// crates (one is the TOML schema, the other is the terminal config).
pub(crate) fn general_to_tui_awareness(
    setting: carrot_settings::TuiAwareness,
) -> carrot_term::term::TuiAwareness {
    use carrot_settings::TuiAwareness as In;
    use carrot_term::term::TuiAwareness as Out;
    match setting {
        In::Full => Out::Full,
        In::StrictProtocol => Out::StrictProtocol,
        In::Off => Out::Off,
    }
}

pub(crate) fn detect_available_shells() -> Vec<ShellOption> {
    let candidates: &[(&str, &[&str])] = &[
        ("zsh", &["/bin/zsh", "/usr/bin/zsh"]),
        ("bash", &["/bin/bash", "/usr/bin/bash"]),
        ("fish", &["/usr/local/bin/fish", "/opt/homebrew/bin/fish"]),
        ("nu", &["/usr/local/bin/nu", "/opt/homebrew/bin/nu"]),
    ];

    let mut shells = Vec::new();
    for (name, paths) in candidates {
        let mut found_path = None;
        for path in *paths {
            if std::path::Path::new(path).exists() {
                found_path = Some(path.to_string());
                break;
            }
        }
        if found_path.is_none() {
            found_path = shell_install::resolve_shell_path(name);
        }
        shells.push(ShellOption {
            name: name.to_string(),
            installed: found_path.is_some(),
            path: found_path,
        });
    }
    shells
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TerminalPaneEvent {
    TitleChanged,
    CloseRequested,
    BellRang,
    /// A scope marker was detected at `root`. Consumed by a workspace-side
    /// subscriber that decides whether to show a "track this project"
    /// notification.
    ProjectDetected {
        root: std::path::PathBuf,
        kind: carrot_shell::scope_policy::ProjectKind,
    },
}

impl inazuma::EventEmitter<TerminalPaneEvent> for TerminalPane {}

// ---------------------------------------------------------------------------
// TerminalPane — Carrot's block-based terminal as a Workspace Item
// ---------------------------------------------------------------------------

/// Carrot's terminal view — a block-based terminal with context chips,
/// shell integration, input bar, history panel, and command correction.
/// Implements `Item` so it can live inside a `carrot_workspace::Workspace` pane.
pub struct TerminalPane {
    pub(crate) terminal: Terminal,
    pub(crate) terminal_title: String,
    pub(crate) focus_handle: FocusHandle,

    // Input system
    pub(crate) input_state: Entity<InputState>,
    pub(crate) shell_completion: Rc<ShellCompletionProvider>,
    pub command_history: Arc<RwLock<CommandHistory>>,
    pub(crate) history_panel: HistoryPanel,
    pub(crate) correction_suggestion: Option<command_correction::CorrectionResult>,

    // Shell context
    pub(crate) shell_context: ShellContext,
    pub(crate) shell_name: String,
    pub(crate) available_shells: Vec<ShellOption>,
    pub(crate) pending_shell_install: Option<&'static shell_install::ShellInstallInfo>,

    // Rendering
    pub(crate) block_list: Entity<crate::block_list::BlockListView>,
    // Theme background image is now rendered at Workspace root (Glass UI).

    // UI state
    pub(crate) interactive_mode: bool,
    pub(crate) show_terminal: bool,
    pub(crate) last_terminal_rows: u16,
    pub(crate) last_terminal_cols: u16,

    // Chip system
    pub(crate) chip_registry: ChipRegistry,
    pub(crate) detection_cache: DetectionCache,
    pub(crate) last_exit_code: Option<i32>,
    pub(crate) last_duration_ms: Option<u64>,

    // Search state
    pub(crate) search_matches: Vec<crate::block_search::BlockMatch>,
    pub(crate) active_match_index: Option<usize>,
    /// Cached extracted text per block: (block_id, content_rows) → (text, command)
    pub(crate) search_text_cache:
        std::collections::HashMap<(carrot_term::BlockId, usize), (String, String)>,

    // Project detection (reactive git root from shell CWD)
    pub(crate) current_git_root: Option<std::path::PathBuf>,

    // Workspace reference (for modal access via action dispatch)
    pub(crate) workspace: Option<inazuma::WeakEntity<Workspace>>,

    // Cached Project weak-ref populated in `Item::added_to_workspace`.
    // Used by `for_each_project_item` to avoid `workspace.read(cx)` during
    // Workspace-Reconciliation (`active_item_path_changed`), which would
    // panic with "cannot read Workspace while it is already being updated".
    pub(crate) project: Option<inazuma::WeakEntity<carrot_project::Project>>,

    // cli-agents session-manager registration state. `pane_changed`
    // (Item trait) populates these so `on_removed` and
    // `switch_shell` can tear down / rebind cleanly without a
    // manager-side atomic "rebind" operation.
    pub(crate) registered_pty_pid: Option<u32>,
    pub(crate) registered_pane_id: Option<inazuma::EntityId>,

    // Subscriptions (focus, terminal events, etc.)
    pub(crate) _subscriptions: Vec<inazuma::Subscription>,
}

impl TerminalPane {
    /// PID of the shell process attached to this pane's PTY, or
    /// `None` if the platform backend could not report it (rare).
    /// Exposed so `terminal_view::init` can hand the pid to
    /// `carrot_cli_agents::register_terminal` right after the pane
    /// is attached to the workspace.
    pub fn pty_pid(&self) -> Option<u32> {
        self.terminal.pty_pid()
    }

    /// Working directory of the shell at spawn time. Matches what
    /// the session manager's CWD-based hook-routing expects.
    pub fn spawn_cwd(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.shell_context.cwd)
    }

    /// Live shell context (cwd, git branch, diff stats, etc.) as last
    /// reported by the shell hook via OSC 7777. Surfaced so panels like
    /// `carrot-vertical-tabs` can render rich per-pane metadata without
    /// duplicating shell-state tracking.
    pub fn shell_context(&self) -> &ShellContext {
        &self.shell_context
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        use inazuma_settings_framework::Settings;
        let general = carrot_settings::GeneralSettings::get_global(cx);
        let cwd = general.resolve_working_directory();
        let input_mode = match general.input_mode {
            carrot_settings::InputMode::Carrot => carrot_terminal::InputMode::Carrot,
            carrot_settings::InputMode::ShellPs1 => carrot_terminal::InputMode::ShellPs1,
        };
        let scrollback = general.scrollback_history;
        let tui_awareness = general_to_tui_awareness(general.tui_awareness);
        let terminal =
            Terminal::new(24, 80, &cwd, input_mode, scrollback).expect("failed to create terminal");
        terminal.set_tui_awareness(tui_awareness);

        let block_list_view = {
            let handle = terminal.handle();
            cx.new(|_cx| crate::block_list::BlockListView::new(handle))
        };

        let focus_handle = cx.focus_handle();

        // Detect shell language for syntax highlighting
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_name = shell.rsplit('/').next().unwrap_or("zsh");
        let shell_lang = match shell_name {
            "nu" => "nu",
            "fish" => "bash",
            _ => "bash",
        };

        // Check if the shell binary is actually available
        let pending_shell_install = if !shell_install::check_shell_available(shell_name) {
            shell_install::shell_install_info(shell_name)
        } else {
            None
        };

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .shell_editor(shell_lang, 1, 10)
                .auto_pairs(AutoPairConfig::shell_defaults())
        });

        cx.subscribe_in(&input_state, window, Self::on_input_event)
            .detach();

        let events_rx = terminal.event_receiver().clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events_rx.recv_async().await {
                this.update_in(cx, |view, window, cx| {
                    view.handle_terminal_event(event, window, cx);
                })
                .ok();
            }
        })
        .detach();

        // Focus the input so the user can type immediately
        input_state.update(cx, |state, cx| {
            state.focus(window, cx);
        });

        let shell_context = ShellContext::gather_for(&cwd);

        // Load command history from shell's histfile
        let command_history = Arc::new(RwLock::new(CommandHistory::detect_and_load(shell_name)));

        // Set up shell completion provider
        let shell_completion = Rc::new(ShellCompletionProvider::new(
            shell_name,
            cwd.clone(),
            command_history.clone(),
        ));
        input_state.update(cx, |state, _cx| {
            state.lsp.completion_provider = Some(shell_completion.clone());
        });

        let focus_in = cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            this.input_state.update(cx, |state, cx| {
                state.focus(window, cx);
            });
            // Tell the command palette's History source which shell
            // history belongs to the pane the user is actively in.
            // Last-focused wins; this is exactly the semantics the
            // palette wants since Cmd+R recalls *this* terminal's
            // history, not whatever was focused before.
            carrot_session::command_history::ActiveCommandHistory::set_global(
                this.command_history.clone(),
                cx,
            );
            let cwd = std::path::PathBuf::from(&this.shell_context.cwd);
            if !cwd.as_os_str().is_empty() {
                let project_root = this.current_git_root.clone().unwrap_or_else(|| cwd.clone());
                carrot_session::command_history::ActiveTerminalScope::set_global(
                    cwd,
                    project_root,
                    cx,
                );
            }
            // Notify the cli-agents session manager that this
            // pane received focus so it can clear the Vertical-
            // Tabs unread dot. `registered_pane_id` is populated
            // the first time our Item::pane_changed hook fires,
            // so on very first focus — before the pane is fully
            // mounted — this is a no-op, which is correct.
            if let Some(pane_id) = this.registered_pane_id {
                carrot_cli_agents::focus_pane(pane_id.as_u64(), cx);
            }
        });

        Self {
            terminal,
            terminal_title: "~ zsh".to_string(),
            input_state,
            focus_handle,
            shell_context,
            command_history,
            history_panel: HistoryPanel::new(),
            correction_suggestion: None,
            shell_completion,
            shell_name: shell_name.to_string(),
            available_shells: detect_available_shells(),
            pending_shell_install,
            block_list: block_list_view,
            interactive_mode: false,
            show_terminal: false,
            last_terminal_rows: 0,
            last_terminal_cols: 0,
            chip_registry: {
                use inazuma_settings_framework::Settings;
                let chip_settings = carrot_settings::ChipSettings::get_global(cx);
                let mut r = ChipRegistry::with_all_providers();
                r.set_renderer("shell", render_shell_chip);
                r.set_renderer("git_branch", render_git_branch_chip);
                r.apply_layout(&chip_settings.layout);
                r
            },
            detection_cache: {
                use inazuma_settings_framework::Settings;
                let mut cache = DetectionCache::new();
                let scan_timeout = carrot_settings::ChipSettings::get_global(cx).scan_timeout;
                cache.get_or_scan(&cwd, scan_timeout);
                cache
            },
            last_exit_code: None,
            last_duration_ms: None,
            search_matches: Vec::new(),
            active_match_index: None,
            search_text_cache: std::collections::HashMap::new(),
            current_git_root: None,
            workspace: None,
            registered_pty_pid: None,
            registered_pane_id: None,
            project: None,
            _subscriptions: vec![focus_in],
        }
    }

    // -----------------------------------------------------------------------
    // Background image helpers
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Terminal event handling
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Shell management
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------

    fn build_chip_context(&self, cx: &App) -> ChipContext {
        use inazuma_settings_framework::Settings;
        let chip_settings = carrot_settings::ChipSettings::get_global(cx);

        let time_str = time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
            .format(time::macros::format_description!("[hour]:[minute]"))
            .unwrap_or_else(|_| "--:--".to_string());

        ChipContext {
            shell_context: self.shell_context.clone(),
            shell_name: self.shell_name.clone(),
            cwd: std::path::PathBuf::from(&self.shell_context.cwd),
            time_str,
            dir_contents: self.detection_cache.contents_ref().clone(),
            env: collect_chip_env_vars(),
            last_exit_code: self.last_exit_code,
            last_duration_ms: self.last_duration_ms,
            command_timeout: chip_settings.command_timeout,
            battery_info_provider: std::sync::Arc::new(
                carrot_chips::providers::battery::BatteryInfoProviderImpl,
            ),
            kubernetes_config: chip_settings.kubernetes.clone(),
            aws_config: chip_settings.aws.clone(),
            directory_config: chip_settings.directory.clone(),
            git_status_config: chip_settings.git_status.clone(),
            python_config: chip_settings.python.clone(),
            package_config: chip_settings.package.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Render — the block-based terminal layout (NO title bar, NO modal layer)
// ---------------------------------------------------------------------------

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Show shell install modal on first render if shell is missing
        if let Some(shell_info) = self.pending_shell_install.take() {
            if let Some(ws) = self.workspace.as_ref().and_then(|w| w.upgrade()) {
                let terminal_handle = self.terminal.handle();
                ws.update(cx, |workspace, cx| {
                    workspace.toggle_modal(window, cx, |window, cx| {
                        crate::shell_install_modal::ShellInstallModal::new(
                            shell_info,
                            terminal_handle,
                            window,
                            cx,
                        )
                    });
                });
            }
        }

        // Process pending shell switch from the shell selector popover
        let pending = cx.global_mut::<PendingShellSwitch>().0.take();
        if let Some(shell) = pending {
            cx.defer_in(window, move |pane, window, cx| {
                pane.request_shell_change(shell, window, cx);
            });
        }

        // Process pending branch checkout from the branch selector popover
        let pending_branch = cx.global_mut::<PendingBranchSwitch>().0.take();
        if let Some(branch) = pending_branch {
            let cmd = format!("git checkout {branch}\n");
            self.terminal.input(cmd.as_bytes());
        }

        let theme = carrot_theme::GlobalTheme::theme(cx).clone();
        let bg_color = theme.styles.colors.background;

        // Terminal pane sets its own bg color — the Workspace root paints the
        // theme background image ABOVE every pane and panel as an overlay
        // (see CLAUDE.md "Design System: Glass UI Pattern"). The pane itself
        // must stay opaque so the overlay has an opaque canvas to blend with.
        let mut container = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(bg_color)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_send_interrupt))
            .on_action(cx.listener(Self::on_insert_into_input))
            .on_key_down(cx.listener(Self::on_key_down_interactive));

        // NO title bar here — the Workspace renders it above us

        // Terminal output + resize logic
        let handle = self.terminal.handle();
        let font = carrot_theme::terminal_font(cx).clone();
        let font_size: f32 = carrot_theme::terminal_font_size(cx).into();
        let line_height_multiplier =
            carrot_theme::theme_settings(cx).line_height(carrot_theme::FontRole::Terminal, cx);

        {
            let font_id = window.text_system().resolve_font(&font);
            let font_px = px(font_size);
            let cell_width = window
                .text_system()
                .advance(font_id, font_px, 'm')
                .expect("glyph not found for 'm'")
                .width;
            let ascent = window.text_system().ascent(font_id, font_px);
            let descent = window.text_system().descent(font_id, font_px);
            let base_height = ascent + descent.abs();
            let cell_height = base_height * line_height_multiplier;

            let viewport = window.viewport_size();
            let horizontal_padding = px(crate::constants::BLOCK_HEADER_PAD_X) * 2.0;
            // Reserve space for the block-list's vertical scrollbar on the
            // right edge. Without this reservation, grid cells overflow
            // over the scrollbar (TUIs like claude paint their bottom
            // rule right up to the window edge, covering the scrollbar).
            // Scrollbar width is 8 px (carrot-ui ScrollbarWidth::Normal),
            // plus a small gap so the last grid column doesn't touch.
            let scrollbar_reserve = px(12.0);
            let cols = ((viewport.width - horizontal_padding - scrollbar_reserve) / cell_width)
                .max(2.0) as u16;
            let rows = (viewport.height / cell_height).max(1.0) as u16;
            if rows != self.last_terminal_rows || cols != self.last_terminal_cols {
                handle.set_size(rows, cols);
                self.last_terminal_rows = rows;
                self.last_terminal_cols = cols;
            }
        }

        if self.show_terminal || self.interactive_mode {
            container = container.child(self.block_list.clone());
        } else {
            container = container.child(div().flex_1().min_h_0());
        }

        if !self.interactive_mode {
            let command_running = {
                let handle = self.terminal.handle();
                let term = handle.lock();
                term.block_router().has_active_block()
            };

            if let Some(ref correction) = self.correction_suggestion {
                container = container.child(self.render_correction_banner(correction, &theme));
            }

            if !command_running {
                if self.history_panel.is_visible() {
                    container = container.child(self.history_panel.render());
                }
                container = container.child(self.render_input_area(window, cx));
            }
        }

        // NO modal layer here — the Workspace renders it on top

        container
    }
}

// ---------------------------------------------------------------------------
// Focusable
// ---------------------------------------------------------------------------

impl Focusable for TerminalPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
// ---------------------------------------------------------------------------
// Shell chip custom renderer (terminal-specific, needs PendingShellSwitch)
// ---------------------------------------------------------------------------

/// Custom renderer for the shell chip — wraps it in a Popover with shell selector.
///
/// Registered as `ChipRenderFn` on the shell chip. Uses `detect_available_shells()`
/// and `PendingShellSwitch` global which are terminal-specific.
pub(crate) fn render_shell_chip(
    output: &carrot_chips::ChipOutput,
    colors: &carrot_theme::ChipColors,
    _render_ctx: &carrot_chips::ChipRenderContext,
    _window: &mut Window,
    _cx: &App,
) -> inazuma::AnyElement {
    let current = output.label.clone();
    let shells = detect_available_shells();
    let color = colors.shell;

    Popover::new("shell-selector")
        .anchor(Anchor::BottomLeft)
        .bg(oklcha(0.23, 0.0, 0.0, 1.0))
        .border_color(oklcha(0.30, 0.0, 0.0, 1.0))
        .rounded_lg()
        .shadow_lg()
        .trigger(
            Chip::new(current.clone())
                .color(color)
                .interactive()
                .tooltip(carrot_ui::ChipTooltip::text("Switch shell")),
        )
        .content(move |_state, _window, cx| {
            let popover_entity = cx.entity();
            let mut list = v_flex().min_w(px(180.0));

            for shell in &shells {
                let is_current = shell.name == current;
                let name = shell.name.clone();
                let installed = shell.installed;
                let detail = shell
                    .path
                    .clone()
                    .unwrap_or_else(|| "Not installed".to_string());

                let shell_name_for_click = shell.name.clone();
                let shell_path_for_click = shell.path.clone();
                let popover = popover_entity.clone();
                let row = div()
                    .id(inazuma::ElementId::Name(
                        format!("shell-{}", shell.name).into(),
                    ))
                    .px(px(8.0))
                    .py(px(5.0))
                    .text_sm()
                    .rounded(px(4.0))
                    .when(!is_current, |s| {
                        s.cursor_pointer()
                            .hover(|s| s.bg(Oklch::white().opacity(0.06)))
                    })
                    .on_mouse_down(inazuma::MouseButton::Left, move |_, window, cx| {
                        if is_current {
                            return;
                        }
                        popover.update(cx, |state, cx| {
                            state.dismiss(window, cx);
                        });
                        cx.global_mut::<PendingShellSwitch>().0 = Some(ShellOption {
                            name: shell_name_for_click.clone(),
                            path: shell_path_for_click.clone(),
                            installed,
                        });
                        window.refresh();
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(div().w(px(14.0)).text_center().when(is_current, |s| {
                                        s.text_color(rgb(0x14F195)).child("✓")
                                    }))
                                    .child(
                                        div()
                                            .when(is_current, |s| s.text_color(rgb(0x14F195)))
                                            .when(!is_current && installed, |s| {
                                                s.text_color(rgb(0xf1f1f1))
                                            })
                                            .when(!installed, |s| {
                                                s.text_color(Oklch::white().opacity(0.4))
                                            })
                                            .child(name),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .when(installed, |s| {
                                        s.text_color(Oklch::white().opacity(0.3)).child(detail)
                                    })
                                    .when(!installed, |s| {
                                        s.text_color(rgb(0xa78bfa)).child("Install")
                                    }),
                            ),
                    );

                list = list.child(row);
            }
            list
        })
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Git branch chip custom renderer — opens modal BranchPicker via workspace
// ---------------------------------------------------------------------------

/// Custom renderer for the git branch chip.
///
/// Renders as an interactive chip. On click, opens a full modal BranchPicker
/// with fuzzy search, keyboard navigation, and accent-colored current branch.
pub(crate) fn render_git_branch_chip(
    output: &carrot_chips::ChipOutput,
    colors: &carrot_theme::ChipColors,
    render_ctx: &carrot_chips::ChipRenderContext,
    _window: &mut Window,
    _cx: &App,
) -> inazuma::AnyElement {
    let current_branch = output.label.clone();
    let icon_color = colors.git_branch_icon;
    let text_color = colors.git_branch_text;
    let cwd = render_ctx.cwd.clone();

    // Extract workspace handle from render context (downcast from Box<dyn Any>)
    let workspace_handle: Option<inazuma::WeakEntity<Workspace>> = render_ctx
        .workspace
        .as_ref()
        .and_then(|ws| ws.downcast_ref::<inazuma::WeakEntity<Workspace>>().cloned());

    Chip::new(current_branch.clone())
        .icon_colored(carrot_ui::IconName::GitBranch, icon_color)
        .color(text_color)
        .interactive()
        .tooltip(carrot_ui::ChipTooltip::text("Switch branch"))
        .on_click({
            move |_, window, cx| {
                let Some(ws) = workspace_handle.clone() else {
                    return;
                };
                let branches = crate::branch_picker::list_git_branches(&cwd);
                let current = current_branch.clone();
                window
                    .spawn(cx, async move |cx| {
                        cx.update(|window, cx| {
                            if let Some(ws) = ws.upgrade() {
                                ws.update(cx, |workspace, cx| {
                                    workspace.toggle_modal(window, cx, |window, cx| {
                                        crate::branch_picker::BranchPicker::new(
                                            branches, current, window, cx,
                                        )
                                    });
                                });
                            }
                        })
                        .ok();
                    })
                    .detach();
            }
        })
        .into_any_element()
}
