mod app_bootstrap;
mod carrot;
// DEFERRED: reliability needs chrono + carrot-system-specs deps + reqwest multipart feature + ThemeColor drift
// mod reliability;

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

pub(crate) static STARTUP_TIME: OnceLock<Instant> = OnceLock::new();

use carrot_settings::AppearanceSettings;
use carrot_shell::AppShell;
use carrot_ui::TitleBar;
use inazuma::{
    App, AppContext, Application, Bounds, WindowBounds, WindowOptions, actions, px, size,
};
use inazuma_settings_framework::watch_config_file;

// Global actions — available everywhere (OpenSettings and Quit come from carrot-actions)
actions!(
    carrot,
    [
        ToggleThemeSelector,
        NewWindow,
        CloseWindow,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
    ]
);

// Terminal actions
actions!(
    terminal,
    [
        Copy,
        Paste,
        Clear,
        NewTab,
        NextTab,
        PreviousTab,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        Find,
    ]
);

// Input actions (Carrot Mode)
actions!(
    input,
    [
        Submit,
        AcceptCompletion,
        HistoryPrev,
        HistoryNext,
        Cancel,
        Interrupt,
    ]
);

fn main() {
    if std::env::args().any(|arg| arg == "--printenv") {
        inazuma_util::shell_env::print_env();
        return;
    }

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,carrot=debug,carrot_term=debug"),
    )
    .init();

    // Single-instance check: if another Carrot is already running, hand the
    // request off to it (via TCP handshake) and exit.
    #[cfg(target_os = "macos")]
    if matches!(
        carrot::mac_only_instance::ensure_only_instance(),
        carrot::mac_only_instance::IsOnlyInstance::No
    ) {
        return;
    }
    // DEFERRED: Windows single-instance via carrot::windows_only_instance::handle_single_instance
    // requires OpenListener + Args from open_listener port.

    Application::new()
        .with_assets(carrot_assets::Assets)
        .run(|cx: &mut App| {
            // Register bundled terminal fonts via asset pipeline
            let font_paths = [
                "fonts/dankmono-nerd-font-mono/DankMonoNerdFontMono-Regular.otf",
                "fonts/dankmono-nerd-font-mono/DankMonoNerdFontMono-Bold.otf",
            ];
            let bundled_fonts: Vec<Cow<'static, [u8]>> = font_paths
                .iter()
                .filter_map(|path| cx.asset_source().load(path).ok().flatten())
                .collect();
            cx.text_system()
                .add_fonts(bundled_fonts)
                .expect("failed to register bundled fonts");

            // 1. Initialize SettingsStore with defaults from assets/settings/default.toml
            //    All #[derive(RegisterSetting)] types are automatically registered via inventory
            inazuma_settings_framework::init(cx);

            // 2. Initialize ReleaseChannel (required before Client::production)
            let app_version = semver::Version::new(0, 1, 0);
            carrot_release_channel::init(app_version, cx);

            // 3. Initialize database (required before build_app_state which uses KeyValueStore)
            let app_db = carrot_db::AppDatabase::new();
            cx.set_global(app_db);

            // 4. Build AppState (Client, Session, UserStore, FS, Languages)
            let app_state = app_bootstrap::build_app_state(cx);

            // 4. Watch settings.toml + keymap.toml via Fs::watch
            let fs = app_state.fs.clone();
            let (user_settings_rx, user_settings_watcher) = watch_config_file(
                &cx.background_executor(),
                fs.clone(),
                carrot_paths::settings_file().clone(),
            );
            let (global_settings_rx, global_settings_watcher) = watch_config_file(
                &cx.background_executor(),
                fs.clone(),
                carrot_paths::global_settings_file().clone(),
            );
            carrot::handle_settings_file_changes(
                user_settings_rx,
                user_settings_watcher,
                global_settings_rx,
                global_settings_watcher,
                cx,
            );

            let (user_keymap_rx, user_keymap_watcher) = watch_config_file(
                &cx.background_executor(),
                fs.clone(),
                carrot_paths::keymap_file().clone(),
            );
            carrot::handle_keymap_file_changes(user_keymap_rx, user_keymap_watcher, cx);

            // 5. Initialize theme system (ThemeSettings via SettingsStore + Provider registration)
            //    MUST happen after SettingsStore init — registers ThemeSettingsProvider for carrot-ui
            carrot_theme_settings::init(
                carrot_theme::LoadThemes::All(Box::new(carrot_assets::Assets)),
                cx,
            );

            // Register global action handlers (Quit is registered in carrot::init)
            cx.on_action::<CloseWindow>(|_, cx| {
                cx.defer(|cx| {
                    cx.windows().iter().find(|window| {
                        window
                            .update(cx, |_, window, _| {
                                if window.is_window_active() {
                                    window.remove_window();
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false)
                    });
                });
            });

            // Set up globals for shell switching (used by carrot-terminal-view)
            cx.set_global(carrot_terminal_view::terminal_pane::PendingShellSwitch(
                None,
            ));
            cx.set_global(carrot_terminal_view::terminal_pane::PendingBranchSwitch(
                None,
            ));
            cx.set_global(carrot_terminal_view::terminal_pane::PendingShellInstallName(None));

            // Initialize UI components (keybindings, global state)
            carrot_ui::init(cx);
            carrot_shell::init(cx);

            // Core registries needed by panels and status-bar items.
            // Order matters: downstream panels read these globals during load,
            // so every crate that sets a global must be initialized before the
            // crates that consume it (e.g. collab_panel reads ActiveCall).
            carrot_client::init(&app_state.client, cx);
            carrot_project::Project::init(&app_state.client, cx);
            carrot_workspace::init(app_state.clone(), cx);
            carrot_editor::init(cx);
            carrot_markdown_preview::init(cx);
            carrot_call::init(app_state.client.clone(), app_state.user_store.clone(), cx);
            carrot_channel::init(&app_state.client.clone(), app_state.user_store.clone(), cx);
            carrot_notifications::init(app_state.client.clone(), app_state.user_store.clone(), cx);
            carrot_language_model::init_settings(cx);
            carrot_diagnostics::init(cx);
            carrot_image_viewer::init(cx);
            carrot_debugger_ui::init(cx);
            carrot_debugger_tools::init(cx);
            carrot_git_ui::init(cx);
            carrot_collab_ui::init(&app_state, cx);
            carrot_auto_update::init(app_state.client.clone(), cx);
            carrot_auto_update_ui::init(cx);
            carrot_vim::init(cx);
            carrot_feedback::init(cx);
            carrot_language_tools::init(cx);
            carrot_edit_prediction_ui::init(cx);
            carrot_recent_projects::init(cx);
            carrot_language_selector::init(cx);
            carrot_toolchain_selector::init(cx);
            carrot_go_to_line::init(cx);
            carrot_encoding_selector::init(cx);
            carrot_line_ending_selector::init(cx);
            carrot_keymap_editor::init(cx);
            carrot_onboarding::init(cx);

            // Initialize carrot bootstrap (action handlers, menu bar, etc.)
            let _ = STARTUP_TIME.set(Instant::now());
            carrot::init(cx);

            // Set up PaneSearchBarCallbacks (used by terminal-panel and other
            // features that create their own Panes with toolbars).
            cx.set_global(carrot_workspace::PaneSearchBarCallbacks {
                setup_search_bar: |languages, toolbar, window, cx| {
                    let search_bar =
                        cx.new(|cx| carrot_search::BufferSearchBar::new(languages, window, cx));
                    toolbar.update(cx, |toolbar, cx| {
                        toolbar.add_item(search_bar, window, cx);
                    });
                },
                wrap_div_with_search_actions:
                    carrot_search::buffer_search::register_pane_search_actions,
            });

            carrot_terminal_view::init(cx);
            carrot_breadcrumbs::init(cx);
            carrot_project_panel::init(cx);
            carrot_cli_agents::init(cx);
            carrot_vertical_tabs::init(cx);
            carrot_title_bar::init(cx);
            carrot_command_palette::init(cx);
            carrot_tab_switcher::init(cx);
            carrot_search::init(cx);
            carrot_settings_ui::init(cx);

            // Optional telemetry-log viewer + remote-debug simulation actions.
            carrot::telemetry_log::init(cx);
            carrot::remote_debug::init(cx);

            // Eagerly preload the active theme and icon theme so the user sees
            // their chosen theme immediately, before extensions finish loading.
            carrot::eager_load_active_theme_and_icon_theme(app_state.fs.clone(), cx);

            // Workspace initialization: registers IDE panels (ProjectPanel, GitPanel,
            // OutlinePanel, DebugPanel, CollabPanel, NotificationPanel, TerminalPanel)
            // + status-bar items + workspace actions. All panels start hidden
            // (Panel::starts_open default); user toggles via keybind/Command Palette.
            let prompt_builder =
                carrot_prompt_store::PromptBuilder::load(app_state.fs.clone(), false, cx);
            carrot::initialize_workspace(app_state.clone(), prompt_builder, cx);

            // Initialize pane toolbar items on every new Workspace —
            // Initial active_pane + subscribe to PaneAdded.
            cx.observe_new(|workspace: &mut carrot_workspace::Workspace, window, cx| {
                let Some(window) = window else { return };
                let workspace_handle = cx.entity();
                let center_pane = workspace.active_pane().clone();
                initialize_pane(workspace, &center_pane, window, cx);

                cx.subscribe_in(&workspace_handle, window, {
                    move |workspace, _, event, window, cx| {
                        if let carrot_workspace::Event::PaneAdded(pane) = event {
                            initialize_pane(workspace, pane, window, cx);
                        }
                    }
                })
                .detach();
            })
            .detach();

            // Read colorspace from SettingsStore (already loaded by handle_settings_file)
            let colorspace = {
                use inazuma_settings_framework::Settings;
                AppearanceSettings::get_global(cx).window_colorspace
            };

            let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitleBar::title_bar_options()),
                    colorspace,
                    ..Default::default()
                },
                |window, cx| {
                    // Empty project — worktrees are added reactively when the
                    // shell CWD enters a git repo (via OSC 7/7777 + git root detection).
                    let project = carrot_project::Project::local(
                        app_state.client.clone(),
                        carrot_node_runtime::NodeRuntime::unavailable(),
                        app_state.user_store.clone(),
                        app_state.languages.clone(),
                        app_state.fs.clone(),
                        None,
                        carrot_project::LocalProjectFlags::default(),
                        cx,
                    );

                    // Create the Workspace
                    let workspace = cx.new(|cx| {
                        carrot_workspace::Workspace::new(
                            None,
                            project,
                            app_state.clone(),
                            window,
                            cx,
                        )
                    });

                    // Register the default-item factory used whenever a new
                    // session is created (the `+` in vertical tabs, the
                    // `NewSession` branches of the file-open routing, etc.),
                    // and populate the initial session's pane through the
                    // same factory so "what does a fresh session hold" lives
                    // at a single spot. Docks stay closed by default
                    // (Terminal-First ADE philosophy) — individual panels
                    // opt in via `starts_open()` (e.g. vertical tabs, which
                    // owns session tab management, is always initially open).
                    workspace.update(cx, |ws, cx| {
                        let factory: carrot_workspace::SessionItemFactory =
                            Arc::new(|window, cx| {
                                let pane = cx.new(|cx| {
                                    carrot_terminal_view::terminal_pane::TerminalPane::new(
                                        window, cx,
                                    )
                                });
                                Box::new(pane)
                            });
                        ws.set_default_session_item_factory(factory.clone());
                        let initial = factory(window, cx);
                        ws.add_item_to_active_pane(initial, None, true, window, cx);
                    });

                    // Root wraps Workspace (provides Sheets, Dialogs, Notifications)
                    cx.new(|cx| AppShell::new(workspace, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}

/// Set up toolbar items for a Pane — called for the initial center pane
/// and for every subsequently added pane (splits).
/// See carrot::initialize_pane for the full panel wiring.
fn initialize_pane(
    workspace: &carrot_workspace::Workspace,
    pane: &inazuma::Entity<carrot_workspace::Pane>,
    window: &mut inazuma::Window,
    cx: &mut inazuma::Context<carrot_workspace::Workspace>,
) {
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            // TODO: Breadcrumbs disabled — will be redesigned for terminal context later
            // let breadcrumbs = cx.new(|_| carrot_breadcrumbs::Breadcrumbs::new());
            // toolbar.add_item(breadcrumbs, window, cx);

            let buffer_search_bar = cx.new(|cx| {
                carrot_search::BufferSearchBar::new(
                    Some(workspace.project().read(cx).languages().clone()),
                    window,
                    cx,
                )
            });
            toolbar.add_item(buffer_search_bar, window, cx);
        });
    });
}
