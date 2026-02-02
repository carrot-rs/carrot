//! `carrot_workspace::Item` impl for the terminal pane.
//!
//! Extracted from the 1937-LOC monolith so the Item-trait surface
//! stays legible and A.7 / B-phase refactors can touch it
//! independently of the render / lifecycle / search layers.

use std::any::Any;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use carrot_chips::{ChipRegistry, DetectionCache};
use carrot_completions::shell_completion::ShellCompletionProvider;
use carrot_session::command_history::CommandHistory;
use carrot_shell_integration::ShellContext;
use carrot_terminal::Terminal;
use carrot_ui::{
    Icon, IconName,
    input::{AutoPairConfig, InputState},
};
use carrot_workspace::{
    Item, PaneRole, ToolbarItemLocation, Workspace, WorkspaceId,
    item::{HighlightedText, ItemEvent},
    searchable::SearchableItemHandle,
};
use inazuma::{App, Context, Entity, Window, prelude::*};

use inazuma::SharedString;

use crate::input::history_panel::HistoryPanel;
use crate::terminal_pane::{
    BlockNavigationTarget, TerminalPane, TerminalPaneEvent, detect_available_shells,
    general_to_tui_awareness, render_git_branch_chip, render_shell_chip,
};

impl Item for TerminalPane {
    type Event = TerminalPaneEvent;

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        let icon_name = match self.shell_name.as_str() {
            "nu" => IconName::Terminal,
            "fish" => IconName::Terminal,
            "bash" => IconName::Terminal,
            "zsh" => IconName::Terminal,
            _ => IconName::Terminal,
        };
        Some(Icon::new(icon_name))
    }

    fn buffer_kind(&self, _cx: &App) -> carrot_workspace::item::ItemBufferKind {
        carrot_workspace::item::ItemBufferKind::Singleton
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        // Just the cwd — shell name lives in the shell chip at the bottom
        // input bar, not in the title bar tab.
        self.shell_context.cwd_short.clone().into()
    }

    fn pane_role(&self, _cx: &App) -> PaneRole {
        PaneRole::Terminal
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(self.shell_context.cwd.clone().into())
    }

    fn shell_context(&self, _cx: &App) -> Option<ShellContext> {
        Some(self.shell_context.clone())
    }

    fn terminal_handle(&self, _cx: &App) -> Option<carrot_terminal::TerminalHandle> {
        Some(self.terminal.handle())
    }

    fn breadcrumbs(&self, _cx: &App) -> Option<(Vec<HighlightedText>, Option<inazuma::Font>)> {
        let cwd = self.shell_context.cwd_short.clone();
        let mut segments: Vec<HighlightedText> = cwd
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|segment| HighlightedText {
                text: SharedString::from(segment.to_string()),
                highlights: Vec::new(),
            })
            .collect();

        if segments.is_empty() {
            segments.push(HighlightedText {
                text: "/".into(),
                highlights: Vec::new(),
            });
        }

        segments.push(HighlightedText {
            text: SharedString::from(self.shell_name.clone()),
            highlights: Vec::new(),
        });

        if let Some(ref branch) = self.shell_context.git_branch {
            segments.push(HighlightedText {
                text: SharedString::from(branch.clone()),
                highlights: Vec::new(),
            });
        }

        Some((segments, None))
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(target) = data.downcast_ref::<BlockNavigationTarget>() {
            self.block_list.update(cx, |view, _cx| {
                view.scroll_to_block(target.block_index);
            });
            return true;
        }
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        match event {
            TerminalPaneEvent::TitleChanged => f(ItemEvent::UpdateTab),
            TerminalPaneEvent::CloseRequested => f(ItemEvent::CloseItem),
            TerminalPaneEvent::BellRang => {}
        }
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        _workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> inazuma::Task<Option<Entity<Self>>> {
        let shell_name = self.shell_name.clone();
        let cwd = std::path::PathBuf::from(&self.shell_context.cwd);
        let shell_path = self
            .available_shells
            .iter()
            .find(|s| s.name == shell_name)
            .and_then(|s| s.path.clone());
        let general = {
            use inazuma_settings_framework::Settings;
            carrot_settings::GeneralSettings::get_global(cx).clone()
        };
        let input_mode = match general.input_mode {
            carrot_settings::InputMode::Carrot => carrot_terminal::InputMode::Carrot,
            carrot_settings::InputMode::ShellPs1 => carrot_terminal::InputMode::ShellPs1,
        };
        let scrollback = general.scrollback_history;
        let tui_awareness = general_to_tui_awareness(general.tui_awareness);

        let new_terminal = if let Some(ref path) = shell_path {
            Terminal::with_shell(24, 80, &cwd, input_mode, scrollback, Some(path))
        } else {
            Terminal::new(24, 80, &cwd, input_mode, scrollback)
        };

        let terminal = match new_terminal {
            Ok(t) => {
                t.set_tui_awareness(tui_awareness);
                t
            }
            Err(e) => {
                log::error!("Failed to clone terminal on split: {}", e);
                return inazuma::Task::ready(None);
            }
        };

        let ws = self.workspace.clone();

        let block_list_view = {
            let handle = terminal.handle();
            cx.new(|_cx| crate::block_list::BlockListView::new(handle))
        };

        let shell_lang = match shell_name.as_str() {
            "nu" => "nu",
            "fish" => "bash",
            _ => "bash",
        };

        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .shell_editor(shell_lang, 1, 10)
                .auto_pairs(AutoPairConfig::shell_defaults())
        });

        let command_history = Arc::new(RwLock::new(CommandHistory::detect_and_load(&shell_name)));
        let shell_completion = Rc::new(ShellCompletionProvider::new(
            &shell_name,
            cwd.clone(),
            command_history.clone(),
        ));
        input_state.update(cx, |state, _cx| {
            state.lsp.completion_provider = Some(shell_completion.clone());
        });

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

        cx.subscribe_in(&input_state, window, Self::on_input_event)
            .detach();

        input_state.update(cx, |state, cx| {
            state.focus(window, cx);
        });

        let last_rows = self.last_terminal_rows;
        let last_cols = self.last_terminal_cols;

        let new_entity = cx.new(|cx| Self {
            terminal,
            terminal_title: format!("~ {}", shell_name),
            input_state,
            focus_handle: cx.focus_handle(),
            shell_context: ShellContext::gather_for(&cwd),
            command_history,
            history_panel: HistoryPanel::new(),
            correction_suggestion: None,
            shell_completion,
            shell_name,
            available_shells: detect_available_shells(),
            pending_shell_install: None,
            block_list: block_list_view,
            interactive_mode: false,
            show_terminal: false,
            last_terminal_rows: last_rows,
            last_terminal_cols: last_cols,
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
                cache.get_or_scan(std::path::Path::new(&self.shell_context.cwd), scan_timeout);
                cache
            },
            last_exit_code: None,
            last_duration_ms: None,
            search_matches: Vec::new(),
            active_match_index: None,
            search_text_cache: std::collections::HashMap::new(),
            current_git_root: None,
            workspace: ws,
            registered_pty_pid: None,
            registered_pane_id: None,
            _subscriptions: Vec::new(),
        });

        new_entity.update(cx, |pane, cx| {
            let focus_in = cx.on_focus_in(&pane.focus_handle, window, |this, window, cx| {
                this.input_state.update(cx, |state, cx| {
                    state.focus(window, cx);
                });
            });
            pane._subscriptions.push(focus_in);
        });

        inazuma::Task::ready(Some(new_entity))
    }

    fn is_dirty(&self, _cx: &App) -> bool {
        let handle = self.terminal.handle();
        let term = handle.lock();
        term.block_router().has_active_block()
    }

    fn can_save(&self, _cx: &App) -> bool {
        false
    }

    fn as_searchable(
        &self,
        handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(handle.clone()))
    }

    fn added_to_workspace(
        &mut self,
        workspace: &mut Workspace,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.workspace = Some(workspace.weak_handle());
    }

    fn pane_changed(&mut self, new_pane_id: inazuma::EntityId, cx: &mut Context<Self>) {
        if let Some(old_pty) = self.registered_pty_pid.take() {
            carrot_cli_agents::unregister_terminal(old_pty, cx);
        }

        let Some(pty_pid) = self.terminal.pty_pid() else {
            self.registered_pane_id = None;
            return;
        };
        let cwd = std::path::PathBuf::from(&self.shell_context.cwd);
        carrot_cli_agents::register_terminal(pty_pid, new_pane_id.as_u64(), cwd, cx);
        self.registered_pty_pid = Some(pty_pid);
        self.registered_pane_id = Some(new_pane_id);
    }

    fn on_removed(&self, cx: &mut Context<Self>) {
        if let Some(pty) = self.registered_pty_pid {
            carrot_cli_agents::unregister_terminal(pty, cx);
        }
    }
}
