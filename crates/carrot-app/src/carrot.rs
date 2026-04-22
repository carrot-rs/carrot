mod app_menus;
// DEFERRED: edit_prediction_registry needs carrot-edit-prediction + carrot-copilot + carrot-codestral wiring
// pub mod edit_prediction_registry;
#[cfg(target_os = "macos")]
pub(crate) mod mac_only_instance;
// DEFERRED: open_listener needs carrot_agent_ui + urlencoding dep + MultiWorkspace→Workspace + carrot_shell::{open_new, open_paths, workspace_windows_for_location} + handle_open_request/restore_or_create_workspace re-exports + OpenOptions drift
// mod open_listener;
mod open_url_modal;
// DEFERRED(plan/29): quick_action_bar requires carrot-repl
// mod quick_action_bar;
pub mod remote_debug;
pub mod telemetry_log;
#[cfg(all(target_os = "macos", any(test, feature = "test-support")))]
pub mod visual_tests;
#[cfg(target_os = "windows")]
pub(crate) mod windows_only_instance;

// DEFERRED: carrot-agent-ui crate not wired in workspace
// use carrot_agent_ui::{AgentDiffToolbar, AgentPanelDelegate};
use anyhow::Context as _;
pub use app_menus::*;
use carrot_assets::Assets;

use carrot_breadcrumbs::Breadcrumbs;
use carrot_client::carrot_urls;
use carrot_debugger_ui::debugger_panel::DebugPanel;
use carrot_editor::{Editor, MultiBuffer};
use carrot_extension_host::ExtensionStore;
use carrot_feature_flags::{FeatureFlagAppExt as _, PanicFeatureFlag};
use carrot_fs::Fs;
use carrot_git_ui::commit_view::CommitViewToolbar;
use carrot_git_ui::git_panel::GitPanel;
use carrot_git_ui::project_diff::{BranchDiffToolbar, ProjectDiffToolbar};
use carrot_image_viewer::ImageInfo;
use carrot_language::Capability;
use carrot_language_onboarding::BasedPyrightBanner;
use carrot_language_tools::lsp_button::{self, LspButton};
use carrot_language_tools::lsp_log_view::LspLogToolbarItemView;
use carrot_markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use carrot_onboarding::DOCS_URL;
use carrot_onboarding::multibuffer_hint::MultibufferHint;
use futures::future::Either;
use futures::{StreamExt, channel::mpsc, select_biased};
use inazuma::{
    Action, App, AppContext as _, Context, DismissEvent, Element, Entity, Focusable, KeyBinding,
    ParentElement, PathPromptOptions, PromptLevel, SharedString, Task, TitlebarOptions,
    UpdateGlobal, WeakEntity, Window, WindowHandle, WindowKind, WindowOptions, actions,
    image_cache, point, px, retain_all,
};
use inazuma_collections::VecDeque;
// DEFERRED: pub use open_listener::*; (see mod declaration above)
use carrot_outline_panel::OutlinePanel;
use carrot_paths::{
    local_debug_file_relative_path, local_settings_file_relative_path,
    local_tasks_file_relative_path,
};
use carrot_project::{DirectoryLister, ProjectItem};
use carrot_project_panel::ProjectPanel;
use carrot_prompt_store::PromptBuilder;
use carrot_vertical_tabs::VerticalTabsPanel;
// DEFERRED(plan/29): QuickActionBar requires carrot-repl
// use quick_action_bar::QuickActionBar;
use carrot_recent_projects::open_remote_project;
use carrot_release_channel::{AppCommitSha, AppVersion, ReleaseChannel};
use carrot_search::project_search::ProjectSearchBar;
use inazuma_rope::Rope;
use inazuma_settings_framework::{
    BaseKeymap, DEFAULT_KEYMAP_PATH, InvalidSettingsError, KeybindSource, KeymapFile,
    KeymapFileLoadResult, Settings, SettingsStore, VIM_KEYMAP_PATH,
    initial_local_debug_tasks_content, initial_project_settings_content, initial_tasks_content,
    update_settings_file,
};

use carrot_terminal_view::terminal_panel::{self, TerminalPanel};
use carrot_theme::{ActiveTheme, SystemAppearance, ThemeRegistry, deserialize_icon_theme};
use carrot_theme_settings::{ThemeSettings, load_user_theme};
use carrot_ui::{PopoverMenuHandle, prelude::*};
use carrot_vim_mode_setting::VimModeSetting;
use carrot_workspace::notifications::{
    NotificationId, dismiss_app_notification, show_app_notification,
};
use inazuma_util::markdown::MarkdownString;
use inazuma_util::rel_path::RelPath;
use inazuma_util::{ResultExt, asset_str, maybe};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{self, AtomicBool},
};
use uuid::Uuid;

use carrot_actions::{
    OpenAccountSettings, OpenBrowser, OpenCarrotUrl, OpenDocs, OpenServerSettings,
    OpenSettingsFile, Quit,
};
use carrot_shell::{open_new, with_active_workspace};
use carrot_workspace::{
    AppState, NewFile, NewWindow, OpenLog, Toast, Workspace, WorkspaceSettings,
    create_and_open_local_file, notifications::simple_message_notification::MessageNotification,
};
use carrot_workspace::{CloseIntent, CloseProject, RestoreBanner};
use carrot_workspace::{Pane, notifications::DetachAndPromptErr};

actions!(
    carrot,
    [
        /// Opens the element inspector for debugging UI.
        DebugElements,
        /// Hides the application window.
        Hide,
        /// Hides all other application windows.
        HideOthers,
        /// Minimizes the current window.
        Minimize,
        /// Opens the default settings file.
        OpenDefaultSettings,
        /// Opens project-specific settings file.
        OpenProjectSettingsFile,
        /// Opens the project tasks configuration.
        OpenProjectTasks,
        /// Opens the tasks panel.
        OpenTasks,
        /// Opens debug tasks configuration.
        OpenDebugTasks,
        /// Shows the default semantic token rules (read-only).
        ShowDefaultSemanticTokenRules,
        /// Resets the application database.
        ResetDatabase,
        /// Shows all hidden windows.
        ShowAll,
        /// Toggles fullscreen mode.
        ToggleFullScreen,
        /// Zooms the window.
        Zoom,
        /// Triggers a test panic for debugging.
        TestPanic,
        /// Triggers a hard crash for debugging.
        TestCrash,
    ]
);

actions!(
    dev,
    [
        /// Opens a prompt to enter a URL to open.
        OpenUrlPrompt,
    ]
);

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &Hide, cx| cx.hide());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(quit);

    cx.on_action(|_: &RestoreBanner, cx| carrot_title_bar::restore_banner(cx));

    cx.observe_flag::<PanicFeatureFlag, _>({
        let mut added = false;
        move |enabled, cx| {
            if added || !enabled {
                return;
            }
            added = true;
            cx.on_action(|_: &TestPanic, _| panic!("Ran the TestPanic action"))
                .on_action(|_: &TestCrash, _| {
                    unsafe extern "C" {
                        fn puts(s: *const i8);
                    }
                    unsafe {
                        puts(0xabad1d3a as *const i8);
                    }
                });
        }
    })
    .detach();
    cx.on_action(|_: &OpenLog, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            open_log_file(workspace, window, cx);
        });
    })
    .on_action(|_: &carrot_workspace::RevealLogInFileManager, cx| {
        cx.reveal_path(carrot_paths::log_file().as_path());
    })
    .on_action(|_: &carrot_actions::OpenLicenses, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            open_bundled_file(
                workspace,
                asset_str::<Assets>("licenses.md"),
                "Open Source License Attribution",
                "Markdown",
                window,
                cx,
            );
        });
    })
    .on_action(|&carrot_actions::OpenKeymapFile, cx| {
        with_active_workspace(cx, |_, window, cx| {
            open_settings_file(
                carrot_paths::keymap_file(),
                || {
                    inazuma_settings_framework::initial_keymap_content()
                        .as_ref()
                        .into()
                },
                window,
                cx,
            );
        });
    })
    .on_action(|_: &OpenSettingsFile, cx| {
        with_active_workspace(cx, |_, window, cx| {
            open_settings_file(
                carrot_paths::settings_file(),
                || {
                    inazuma_settings_framework::initial_user_settings_content()
                        .as_ref()
                        .into()
                },
                window,
                cx,
            );
        });
    })
    .on_action(|_: &OpenAccountSettings, cx| {
        with_active_workspace(cx, |_, _, cx| {
            cx.open_url(&carrot_urls::account_url(cx));
        });
    })
    .on_action(|_: &OpenTasks, cx| {
        with_active_workspace(cx, |_, window, cx| {
            open_settings_file(
                carrot_paths::tasks_file(),
                || {
                    inazuma_settings_framework::initial_tasks_content()
                        .as_ref()
                        .into()
                },
                window,
                cx,
            );
        });
    })
    .on_action(|_: &OpenDebugTasks, cx| {
        with_active_workspace(cx, |_, window, cx| {
            open_settings_file(
                carrot_paths::debug_scenarios_file(),
                || {
                    inazuma_settings_framework::initial_debug_tasks_content()
                        .as_ref()
                        .into()
                },
                window,
                cx,
            );
        });
    })
    .on_action(|_: &ShowDefaultSemanticTokenRules, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            open_bundled_file(
                workspace,
                inazuma_settings_framework::default_semantic_token_rules(),
                "Default Semantic Token Rules",
                "JSONC",
                window,
                cx,
            );
        });
    })
    .on_action(|_: &OpenDefaultSettings, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            open_bundled_file(
                workspace,
                inazuma_settings_framework::default_settings(),
                "Default Settings",
                "JSON",
                window,
                cx,
            );
        });
    })
    .on_action(|_: &carrot_actions::OpenDefaultKeymap, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            open_bundled_file(
                workspace,
                inazuma_settings_framework::default_keymap(),
                "Default Key Bindings",
                "JSON",
                window,
                cx,
            );
        });
    })
    .on_action(|_: &carrot_actions::About, cx| {
        with_active_workspace(cx, |workspace, window, cx| {
            about(workspace, window, cx);
        });
    });
}

fn bind_on_window_closed(cx: &mut App) -> Option<inazuma::Subscription> {
    #[cfg(target_os = "macos")]
    {
        WorkspaceSettings::get_global(cx)
            .on_last_window_closed
            .is_quit_app()
            .then(|| {
                cx.on_window_closed(|cx| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        }))
    }
}

pub fn build_window_options(display_uuid: Option<Uuid>, cx: &mut App) -> WindowOptions {
    let display = display_uuid.and_then(|uuid| {
        cx.displays()
            .into_iter()
            .find(|display| display.uuid().ok() == Some(uuid))
    });
    let app_id = ReleaseChannel::global(cx).app_id();
    let window_decorations = match std::env::var("CARROT_WINDOW_DECORATIONS") {
        Ok(val) if val == "server" => inazuma::WindowDecorations::Server,
        Ok(val) if val == "client" => inazuma::WindowDecorations::Client,
        _ => match WorkspaceSettings::get_global(cx).window_decorations {
            inazuma_settings_framework::WindowDecorations::Server => {
                inazuma::WindowDecorations::Server
            }
            inazuma_settings_framework::WindowDecorations::Client => {
                inazuma::WindowDecorations::Client
            }
        },
    };

    let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;

    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.0), px(9.0))),
        }),
        window_bounds: None,
        focus: false,
        show: false,
        kind: WindowKind::Normal,
        is_movable: true,
        display_id: display.map(|display| display.id()),
        window_background: cx.theme().window_background_appearance(),
        app_id: Some(app_id.to_owned()),
        window_decorations: Some(window_decorations),
        window_min_size: Some(inazuma::Size {
            width: px(360.0),
            height: px(240.0),
        }),
        tabbing_identifier: if use_system_window_tabs {
            Some(String::from("carrot"))
        } else {
            None
        },
        ..Default::default()
    }
}

pub fn initialize_workspace(
    app_state: Arc<AppState>,
    prompt_builder: Arc<PromptBuilder>,
    cx: &mut App,
) {
    let mut _on_close_subscription = bind_on_window_closed(cx);
    cx.observe_global::<SettingsStore>(move |cx| {
        // A 1.92 regression causes unused-assignment to trigger on this variable.
        _ = _on_close_subscription.is_some();
        _on_close_subscription = bind_on_window_closed(cx);
    })
    .detach();

    cx.observe_new(move |workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };

        let workspace_handle = cx.entity();
        let center_pane = workspace.active_pane().clone();
        initialize_pane(workspace, &center_pane, window, cx);

        cx.subscribe_in(&workspace_handle, window, {
            move |workspace, _, event, window, cx| match event {
                carrot_workspace::Event::PaneAdded(pane) => {
                    initialize_pane(workspace, pane, window, cx);
                }
                carrot_workspace::Event::OpenBundledFile {
                    text,
                    title,
                    language,
                } => open_bundled_file(workspace, text.clone(), title, language, window, cx),
                _ => {}
            }
        })
        .detach();

        #[cfg(not(any(test, target_os = "macos")))]
        initialize_file_watcher(window, cx);

        if let Some(specs) = window.gpu_specs() {
            log::info!("Using GPU: {:?}", specs);
            show_software_emulation_warning_if_needed(specs.clone(), window, cx);
            carrot_crashes::set_gpu_info(specs);
        }

        let edit_prediction_menu_handle = PopoverMenuHandle::default();
        let edit_prediction_ui = cx.new(|cx| {
            carrot_edit_prediction_ui::EditPredictionButton::new(
                app_state.fs.clone(),
                app_state.user_store.clone(),
                edit_prediction_menu_handle.clone(),
                workspace.project().clone(),
                cx,
            )
        });
        workspace.register_action({
            move |_, _: &carrot_edit_prediction_ui::ToggleMenu, window, cx| {
                edit_prediction_menu_handle.toggle(window, cx);
            }
        });

        let search_button = cx.new(|_| carrot_search::search_status_button::SearchButton::new());
        let diagnostic_summary =
            cx.new(|cx| carrot_diagnostics::items::DiagnosticIndicator::new(workspace, cx));
        let active_file_name =
            cx.new(|_| carrot_workspace::active_file_name::ActiveFileName::new());
        let activity_indicator = carrot_activity_indicator::ActivityIndicator::new(
            workspace,
            workspace.project().read(cx).languages().clone(),
            window,
            cx,
        );
        let active_buffer_encoding =
            cx.new(|_| carrot_encoding_selector::ActiveBufferEncoding::new(workspace));
        let active_buffer_language =
            cx.new(|_| carrot_language_selector::ActiveBufferLanguage::new(workspace));
        let active_toolchain_language =
            cx.new(|cx| carrot_toolchain_selector::ActiveToolchain::new(workspace, window, cx));
        let vim_mode_indicator = cx.new(|cx| carrot_vim::ModeIndicator::new(window, cx));
        let image_info = cx.new(|_cx| ImageInfo::new(workspace));

        let lsp_button_menu_handle = PopoverMenuHandle::default();
        let lsp_button =
            cx.new(|cx| LspButton::new(workspace, lsp_button_menu_handle.clone(), window, cx));
        workspace.register_action({
            move |_, _: &lsp_button::ToggleMenu, window, cx| {
                lsp_button_menu_handle.toggle(window, cx);
            }
        });

        let cursor_position =
            cx.new(|_| carrot_go_to_line::cursor_position::CursorPosition::new(workspace));
        let line_ending_indicator =
            cx.new(|_| carrot_line_ending_selector::LineEndingIndicator::default());
        workspace.status_bar().update(cx, |status_bar, cx| {
            status_bar.add_left_item(search_button, window, cx);
            status_bar.add_left_item(lsp_button, window, cx);
            status_bar.add_left_item(diagnostic_summary, window, cx);
            status_bar.add_left_item(active_file_name, window, cx);
            status_bar.add_left_item(activity_indicator, window, cx);
            status_bar.add_right_item(edit_prediction_ui, window, cx);
            status_bar.add_right_item(active_buffer_encoding, window, cx);
            status_bar.add_right_item(active_buffer_language, window, cx);
            status_bar.add_right_item(active_toolchain_language, window, cx);
            status_bar.add_right_item(line_ending_indicator, window, cx);
            status_bar.add_right_item(vim_mode_indicator, window, cx);
            status_bar.add_right_item(cursor_position, window, cx);
            status_bar.add_right_item(image_info, window, cx);
        });

        let panels_task = initialize_panels(prompt_builder.clone(), window, cx);
        workspace.set_panels_task(panels_task);
        register_actions(app_state.clone(), workspace, window, cx);

        if !workspace.has_active_modal(window, cx) {
            workspace.focus_handle(cx).focus(window, cx);
        }
    })
    .detach();
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[allow(unused)]
fn initialize_file_watcher(window: &mut Window, cx: &mut Context<Workspace>) {
    if let Err(e) = carrot_fs::fs_watcher::global(|_| {}) {
        let message = format!(
            carrot_db::indoc! {r#"
            inotify_init returned {}

            This may be due to system-wide limits on inotify instances. For troubleshooting see: https://carrot.dev/docs/linux
            "#},
            e
        );
        let prompt = window.prompt(
            PromptLevel::Critical,
            "Could not start inotify",
            Some(&message),
            &["Troubleshoot and Quit"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if prompt.await == Ok(0) {
                cx.update(|cx| {
                    cx.open_url("https://carrot.dev/docs/linux#could-not-start-inotify");
                    cx.quit();
                });
            }
        })
        .detach()
    }
}

#[cfg(target_os = "windows")]
#[allow(unused)]
fn initialize_file_watcher(window: &mut Window, cx: &mut Context<Workspace>) {
    if let Err(e) = carrot_fs::fs_watcher::global(|_| {}) {
        let message = format!(
            carrot_db::indoc! {r#"
            ReadDirectoryChangesW initialization failed: {}

            This may occur on network filesystems and WSL paths. For troubleshooting see: https://carrot.dev/docs/windows
            "#},
            e
        );
        let prompt = window.prompt(
            PromptLevel::Critical,
            "Could not start ReadDirectoryChangesW",
            Some(&message),
            &["Troubleshoot and Quit"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if prompt.await == Ok(0) {
                cx.update(|cx| {
                    cx.open_url("https://carrot.dev/docs/windows");
                    cx.quit()
                });
            }
        })
        .detach()
    }
}

fn show_software_emulation_warning_if_needed(
    specs: inazuma::GpuSpecs,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if specs.is_software_emulated && std::env::var("CARROT_ALLOW_EMULATED_GPU").is_err() {
        let (graphics_api, docs_url, open_url) = if cfg!(target_os = "windows") {
            (
                "DirectX",
                "https://carrot.dev/docs/windows",
                "https://carrot.dev/docs/windows",
            )
        } else {
            (
                "Vulkan",
                "https://carrot.dev/docs/linux",
                "https://carrot.dev/docs/linux#carrot-fails-to-open-windows",
            )
        };
        let message = format!(
            carrot_db::indoc! {r#"
            Carrot uses {} for rendering and requires a compatible GPU.

            Currently you are using a software emulated GPU ({}) which
            will result in awful performance.

            For troubleshooting see: {}
            Set CARROT_ALLOW_EMULATED_GPU=1 env var to permanently override.
            "#},
            graphics_api, specs.device_name, docs_url
        );
        let prompt = window.prompt(
            PromptLevel::Critical,
            "Unsupported GPU",
            Some(&message),
            &["Skip", "Troubleshoot and Quit"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if prompt.await == Ok(1) {
                cx.update(|cx| {
                    cx.open_url(open_url);
                    cx.quit();
                });
            }
        })
        .detach()
    }
}

fn initialize_panels(
    prompt_builder: Arc<PromptBuilder>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Task<anyhow::Result<()>> {
    cx.spawn_in(window, async move |workspace_handle, cx| {
        let project_panel = ProjectPanel::load(workspace_handle.clone(), cx.clone());
        let outline_panel = OutlinePanel::load(workspace_handle.clone(), cx.clone());
        let terminal_panel = TerminalPanel::load(workspace_handle.clone(), cx.clone());
        let git_panel = GitPanel::load(workspace_handle.clone(), cx.clone());
        let channels_panel =
            carrot_collab_ui::collab_panel::CollabPanel::load(workspace_handle.clone(), cx.clone());
        let notification_panel = carrot_collab_ui::notification_panel::NotificationPanel::load(
            workspace_handle.clone(),
            cx.clone(),
        );
        let vertical_tabs_panel = VerticalTabsPanel::load(workspace_handle.clone(), cx.clone());
        let debug_panel = DebugPanel::load(workspace_handle.clone(), cx);

        async fn add_panel_when_ready(
            panel_task: impl Future<Output = anyhow::Result<Entity<impl carrot_workspace::Panel>>>
            + 'static,
            workspace_handle: WeakEntity<Workspace>,
            mut cx: inazuma::AsyncWindowContext,
        ) {
            if let Some(panel) = panel_task.await.context("failed to load panel").log_err() {
                workspace_handle
                    .update_in(&mut cx, |workspace, window, cx| {
                        workspace.add_panel(panel, window, cx);
                    })
                    .log_err();
            }
        }

        futures::join!(
            add_panel_when_ready(project_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(outline_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(terminal_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(git_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(channels_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(notification_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(debug_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(vertical_tabs_panel, workspace_handle.clone(), cx.clone()),
            // DEFERRED: carrot-agent-ui AgentPanel not wired
            // initialize_agent_panel(workspace_handle, prompt_builder, cx.clone()).map(|r| r.log_err()),
        );
        let _ = (workspace_handle, prompt_builder);

        anyhow::Ok(())
    })
}

// DEFERRED: setup_or_teardown_ai_panel is only called by initialize_agent_panel
/*
fn setup_or_teardown_ai_panel<P: Panel>(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    load_panel: impl FnOnce(
        WeakEntity<Workspace>,
        AsyncWindowContext,
    ) -> Task<anyhow::Result<Entity<P>>>
    + 'static,
) -> Task<anyhow::Result<()>> {
    let disable_ai = SettingsStore::global(cx)
        .get::<DisableAiSettings>(None)
        .disable_ai
        || cfg!(test);
    let existing_panel = workspace.panel::<P>(cx);
    match (disable_ai, existing_panel) {
        (false, None) => cx.spawn_in(window, async move |workspace, cx| {
            let panel = load_panel(workspace.clone(), cx.clone()).await?;
            workspace.update_in(cx, |workspace, window, cx| {
                let disable_ai = SettingsStore::global(cx)
                    .get::<DisableAiSettings>(None)
                    .disable_ai;
                let have_panel = workspace.panel::<P>(cx).is_some();
                if !disable_ai && !have_panel {
                    workspace.add_panel(panel, window, cx);
                }
            })
        }),
        (true, Some(existing_panel)) => {
            workspace.remove_panel::<P>(&existing_panel, window, cx);
            Task::ready(Ok(()))
        }
        _ => Task::ready(Ok(())),
    }
}
*/

// DEFERRED: initialize_agent_panel requires carrot-agent-ui
/*
async fn initialize_agent_panel(
    workspace_handle: WeakEntity<Workspace>,
    prompt_builder: Arc<PromptBuilder>,
    mut cx: AsyncWindowContext,
) -> anyhow::Result<()> {
    workspace_handle
        .update_in(&mut cx, |workspace, window, cx| {
            let prompt_builder = prompt_builder.clone();
            setup_or_teardown_ai_panel(workspace, window, cx, move |workspace, cx| {
                carrot_agent_ui::AgentPanel::load(workspace, prompt_builder, cx)
            })
        })?
        .await?;

    workspace_handle.update_in(&mut cx, |workspace, window, cx| {
        let prompt_builder = prompt_builder.clone();
        cx.observe_global_in::<SettingsStore>(window, move |workspace, window, cx| {
            let prompt_builder = prompt_builder.clone();
            setup_or_teardown_ai_panel(workspace, window, cx, move |workspace, cx| {
                carrot_agent_ui::AgentPanel::load(workspace, prompt_builder, cx)
            })
            .detach_and_log_err(cx);
        })
        .detach();

        if !cfg!(test) {
            <dyn AgentPanelDelegate>::set_global(
                Arc::new(carrot_agent_ui::ConcreteAssistantPanelDelegate),
                cx,
            );

            workspace
                .register_action(carrot_agent_ui::AgentPanel::toggle_focus)
                .register_action(carrot_agent_ui::AgentPanel::toggle)
                .register_action(carrot_agent_ui::InlineAssistant::inline_assist);
        }
    })?;

    anyhow::Ok(())
}
*/

fn register_actions(
    app_state: Arc<AppState>,
    workspace: &mut Workspace,
    _: &mut Window,
    cx: &mut Context<Workspace>,
) {
    workspace
        .register_action(|_, _: &OpenDocs, _, cx| cx.open_url(DOCS_URL))
        .register_action(|_, _: &Minimize, window, _| {
            window.minimize_window();
        })
        .register_action(|_, _: &Zoom, window, _| {
            window.zoom_window();
        })
        .register_action(|_, _: &ToggleFullScreen, window, _| {
            window.toggle_fullscreen();
        })
        .register_action(|_, action: &OpenCarrotUrl, _, cx| {
            // DEFERRED: OpenListener requires open_listener module port (carrot_agent_ui, urlencoding, MultiWorkspace→Workspace drift)
            let _ = (action, cx);
        })
        .register_action(|workspace, _: &OpenUrlPrompt, window, cx| {
            workspace.toggle_modal(window, cx, |window, cx| {
                open_url_modal::OpenUrlModal::new(window, cx)
            });
        })
        .register_action(|workspace, action: &OpenBrowser, _window, cx| {
            // Parse and validate the URL to ensure it's properly formatted
            match url::Url::parse(&action.url) {
                Ok(parsed_url) => {
                    // Use the parsed URL's string representation which is properly escaped
                    cx.open_url(parsed_url.as_str());
                }
                Err(e) => {
                    workspace.show_error(
                        &anyhow::anyhow!(
                            "Opening this URL in a browser failed because the URL is invalid: {}\n\nError was: {e}",
                            action.url
                        ),
                        cx,
                    );
                }
            }
        })
        .register_action(|workspace, action: &carrot_workspace::Open, window, cx| {
            carrot_telemetry::event!("Project Opened");
            carrot_workspace::prompt_for_open_path_and_open(
                workspace,
                workspace.app_state().clone(),
                PathPromptOptions {
                    files: true,
                    directories: true,
                    multiple: true,
                    prompt: None,
                },
                action.create_new_window,
                window,
                cx,
            );
        })
        .register_action(|workspace, _: &carrot_workspace::OpenFiles, window, cx| {
            let directories = cx.can_select_mixed_files_and_dirs();
            carrot_workspace::prompt_for_open_path_and_open(
                workspace,
                workspace.app_state().clone(),
                PathPromptOptions {
                    files: true,
                    directories,
                    multiple: true,
                    prompt: None,
                },
                true,
                window,
                cx,
            );
        })
        .register_action(|workspace, action: &carrot_actions::OpenRemote, window, cx| {
            if !action.from_existing_connection {
                cx.propagate();
                return;
            }
            // You need existing remote connection to open it this way
            if workspace.project().read(cx).is_local() {
                return;
            }
            carrot_telemetry::event!("Project Opened");
            let paths = workspace.prompt_for_open_path(
                PathPromptOptions {
                    files: true,
                    directories: true,
                    multiple: true,
                    prompt: None,
                },
                DirectoryLister::Project(workspace.project().clone()),
                window,
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let Some(paths) = paths.await.log_err().flatten() else {
                    return;
                };
                if let Some(task) = this
                    .update_in(cx, |this, window, cx| {
                        open_new_ssh_project_from_project(this, paths, window, cx)
                    })
                    .log_err()
                {
                    task.await.log_err();
                }
            })
            .detach()
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::IncreaseUiFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, cx| {
                        let ui_font_size = ThemeSettings::get_global(cx).ui_font_size(cx) + px(1.0);
                        let _ = settings
                            .theme
                            .ui_font_size
                            .insert(f32::from(carrot_theme_settings::clamp_font_size(ui_font_size)).into());
                    });
                } else {
                    carrot_theme_settings::adjust_ui_font_size(cx, |size| size + px(1.0));
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::DecreaseUiFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, cx| {
                        let ui_font_size = ThemeSettings::get_global(cx).ui_font_size(cx) - px(1.0);
                        let _ = settings
                            .theme
                            .ui_font_size
                            .insert(f32::from(carrot_theme_settings::clamp_font_size(ui_font_size)).into());
                    });
                } else {
                    carrot_theme_settings::adjust_ui_font_size(cx, |size| size - px(1.0));
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::ResetUiFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, _| {
                        settings.theme.ui_font_size = None;
                    });
                } else {
                    carrot_theme_settings::reset_ui_font_size(cx);
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::IncreaseBufferFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, cx| {
                        let buffer_font_size =
                            ThemeSettings::get_global(cx).buffer_font_size(cx) + px(1.0);
                        let _ = settings
                            .theme
                            .buffer_font_size
                            .insert(f32::from(carrot_theme_settings::clamp_font_size(buffer_font_size)).into());
                    });
                } else {
                    carrot_theme_settings::adjust_buffer_font_size(cx, |size| size + px(1.0));
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::DecreaseBufferFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, cx| {
                        let buffer_font_size =
                            ThemeSettings::get_global(cx).buffer_font_size(cx) - px(1.0);
                        let _ = settings
                            .theme
                            .buffer_font_size
                            .insert(f32::from(carrot_theme_settings::clamp_font_size(buffer_font_size)).into());
                    });
                } else {
                    carrot_theme_settings::adjust_buffer_font_size(cx, |size| size - px(1.0));
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::ResetBufferFontSize, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, _| {
                        settings.theme.buffer_font_size = None;
                    });
                } else {
                    carrot_theme_settings::reset_buffer_font_size(cx);
                }
            }
        })
        .register_action({
            let fs = app_state.fs.clone();
            move |_, action: &carrot_actions::ResetAllZoom, _window, cx| {
                if action.persist {
                    update_settings_file(fs.clone(), cx, move |settings, _| {
                        settings.theme.ui_font_size = None;
                        settings.theme.buffer_font_size = None;
                        settings.theme.agent_ui_font_size = None;
                        settings.theme.agent_buffer_font_size = None;
                    });
                } else {
                    carrot_theme_settings::reset_ui_font_size(cx);
                    carrot_theme_settings::reset_buffer_font_size(cx);
                    carrot_theme_settings::reset_agent_ui_font_size(cx);
                    carrot_theme_settings::reset_agent_buffer_font_size(cx);
                }
            }
        })
        .register_action(|_, _: &carrot_install_cli::RegisterCarrotScheme, window, cx| {
            cx.spawn_in(window, async move |workspace, cx| {
                carrot_install_cli::register_carrot_scheme(cx).await?;
                workspace.update_in(cx, |workspace, _, cx| {
                    struct RegisterCarrotScheme;

                    workspace.show_toast(
                        Toast::new(
                            NotificationId::unique::<RegisterCarrotScheme>(),
                            format!(
                                "carrot:// links will now open in {}.",
                                ReleaseChannel::global(cx).display_name()
                            ),
                        ),
                        cx,
                    )
                })?;
                Ok(())
            })
            .detach_and_prompt_err(
                "Error registering carrot:// scheme",
                window,
                cx,
                |_, _, _| None,
            );
        })
        .register_action(open_project_settings_file)
        .register_action(open_project_tasks_file)
        .register_action(open_project_debug_tasks_file)
        .register_action(
            |workspace: &mut Workspace,
             _: &carrot_actions::project_panel::ToggleFocus,
             window: &mut Window,
             cx: &mut Context<Workspace>| {
                workspace.toggle_panel_focus::<ProjectPanel>(window, cx);
            },
        )
        .register_action(
            |workspace: &mut Workspace,
             _: &carrot_outline_panel::ToggleFocus,
             window: &mut Window,
             cx: &mut Context<Workspace>| {
                workspace.toggle_panel_focus::<OutlinePanel>(window, cx);
            },
        )
        .register_action(
            |workspace: &mut Workspace,
             _: &carrot_collab_ui::collab_panel::ToggleFocus,
             window: &mut Window,
             cx: &mut Context<Workspace>| {
                workspace.toggle_panel_focus::<carrot_collab_ui::collab_panel::CollabPanel>(window, cx);
            },
        )
        .register_action(
            |workspace: &mut Workspace,
             _: &carrot_collab_ui::notification_panel::ToggleFocus,
             window: &mut Window,
             cx: &mut Context<Workspace>| {
                workspace.toggle_panel_focus::<carrot_collab_ui::notification_panel::NotificationPanel>(
                    window, cx,
                );
            },
        )
        .register_action(
            |workspace: &mut Workspace,
             _: &terminal_panel::ToggleFocus,
             window: &mut Window,
             cx: &mut Context<Workspace>| {
                workspace.toggle_panel_focus::<TerminalPanel>(window, cx);
            },
        )
        .register_action({
            let app_state = Arc::downgrade(&app_state);
            move |_, _: &NewWindow, _, cx| {
                if let Some(app_state) = app_state.upgrade() {
                    open_new(
                        Default::default(),
                        app_state,
                        cx,
                        |workspace, window, cx| {
                            cx.activate(true);
                            // Create buffer synchronously to avoid flicker
                            let project = workspace.project().clone();
                            let buffer = project.update(cx, |project, cx| {
                                project.create_local_buffer("", None, true, cx)
                            });
                            let editor = cx.new(|cx| {
                                Editor::for_buffer(buffer, Some(project), window, cx)
                            });
                            workspace.add_item_to_active_pane(
                                Box::new(editor),
                                None,
                                true,
                                window,
                                cx,
                            );
                        },
                    )
                    .detach();
                }
            }
        })
        .register_action({
            let app_state = Arc::downgrade(&app_state);
            move |_workspace, _: &CloseProject, window, cx| {
                let Some(window_handle) = window
                    .window_handle()
                    .downcast::<carrot_shell::AppShell>()
                else {
                    return;
                };
                if let Some(app_state) = app_state.upgrade() {
                    cx.spawn_in(window, async move |this, cx| {
                        let should_continue = this
                            .update_in(cx, |workspace, window, cx| {
                                workspace.prepare_to_close(
                                    CloseIntent::ReplaceWindow,
                                    window,
                                    cx,
                                )
                            })?
                            .await?;
                        if should_continue {
                            let task = cx.update(|_window, cx| {
                                open_new(
                                    carrot_shell::OpenOptions {
                                        replace_window: Some(window_handle),
                                        ..Default::default()
                                    },
                                    app_state,
                                    cx,
                                    |workspace, window, cx| {
                                        cx.activate(true);
                                        let project = workspace.project().clone();
                                        let buffer = project.update(cx, |project, cx| {
                                            project.create_local_buffer("", None, true, cx)
                                        });
                                        let editor = cx.new(|cx| {
                                            Editor::for_buffer(buffer, Some(project), window, cx)
                                        });
                                        workspace.add_item_to_active_pane(
                                            Box::new(editor),
                                            None,
                                            true,
                                            window,
                                            cx,
                                        );
                                    },
                                )
                            })?;
                            task.await
                        } else {
                            Ok(())
                        }
                    })
                    .detach_and_log_err(cx);
                }
            }
        })
        .register_action({
            let app_state = Arc::downgrade(&app_state);
            move |_, _: &NewFile, _, cx| {
                if let Some(app_state) = app_state.upgrade() {
                    open_new(
                        Default::default(),
                        app_state,
                        cx,
                        |workspace, window, cx| {
                            Editor::new_file(workspace, &Default::default(), window, cx)
                        },
                    )
                    .detach_and_log_err(cx);
                }
            }
        });

    #[cfg(not(target_os = "windows"))]
    workspace.register_action(install_cli);

    if workspace.project().read(cx).is_via_remote_server() {
        workspace.register_action({
            move |workspace, _: &OpenServerSettings, window, cx| {
                let open_server_settings = workspace
                    .project()
                    .update(cx, |project, cx| project.open_server_settings(cx));

                cx.spawn_in(window, async move |workspace, cx| {
                    let buffer = open_server_settings.await?;

                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            workspace.open_path(
                                buffer
                                    .read(cx)
                                    .project_path(cx)
                                    .expect("Settings file must have a location"),
                                None,
                                true,
                                window,
                                cx,
                            )
                        })?
                        .await?;

                    anyhow::Ok(())
                })
                .detach_and_log_err(cx);
            }
        });
    }
}

fn initialize_pane(
    workspace: &Workspace,
    pane: &Entity<Pane>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let _workspace_handle = cx.weak_entity();
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            let multibuffer_hint = cx.new(|_| MultibufferHint::new());
            toolbar.add_item(multibuffer_hint, window, cx);
            let breadcrumbs = cx.new(|_| Breadcrumbs::new());
            toolbar.add_item(breadcrumbs, window, cx);
            let buffer_search_bar = cx.new(|cx| {
                carrot_search::BufferSearchBar::new(
                    Some(workspace.project().read(cx).languages().clone()),
                    window,
                    cx,
                )
            });
            toolbar.add_item(buffer_search_bar.clone(), window, cx);
            let _ = buffer_search_bar;
            // DEFERRED(plan/29): QuickActionBar requires carrot-repl
            // let quick_action_bar =
            //     cx.new(|cx| QuickActionBar::new(buffer_search_bar, workspace, cx));
            // toolbar.add_item(quick_action_bar, window, cx);
            let diagnostic_editor_controls = cx.new(|_| carrot_diagnostics::ToolbarControls::new());
            toolbar.add_item(diagnostic_editor_controls, window, cx);
            let project_search_bar = cx.new(|_| ProjectSearchBar::new());
            toolbar.add_item(project_search_bar, window, cx);
            let lsp_log_item = cx.new(|_| LspLogToolbarItemView::new());
            toolbar.add_item(lsp_log_item, window, cx);
            let dap_log_item = cx.new(|_| carrot_debugger_tools::DapLogToolbarItemView::new());
            toolbar.add_item(dap_log_item, window, cx);
            let acp_tools_item = cx.new(|_| carrot_acp_tools::AcpToolsToolbarItemView::new());
            toolbar.add_item(acp_tools_item, window, cx);
            let telemetry_log_item =
                cx.new(|cx| telemetry_log::TelemetryLogToolbarItemView::new(window, cx));
            toolbar.add_item(telemetry_log_item, window, cx);
            let syntax_tree_item =
                cx.new(|_| carrot_language_tools::SyntaxTreeToolbarItemView::new());
            toolbar.add_item(syntax_tree_item, window, cx);
            let highlights_tree_item =
                cx.new(|_| carrot_language_tools::HighlightsTreeToolbarItemView::new());
            toolbar.add_item(highlights_tree_item, window, cx);
            let project_diff_toolbar = cx.new(|cx| ProjectDiffToolbar::new(workspace, cx));
            toolbar.add_item(project_diff_toolbar, window, cx);
            let branch_diff_toolbar = cx.new(BranchDiffToolbar::new);
            toolbar.add_item(branch_diff_toolbar, window, cx);
            let commit_view_toolbar = cx.new(|_| CommitViewToolbar::new());
            toolbar.add_item(commit_view_toolbar, window, cx);
            // DEFERRED: AgentDiffToolbar requires carrot-agent-ui
            // let agent_diff_toolbar = cx.new(AgentDiffToolbar::new);
            // toolbar.add_item(agent_diff_toolbar, window, cx);
            let basedpyright_banner = cx.new(|cx| BasedPyrightBanner::new(workspace, cx));
            toolbar.add_item(basedpyright_banner, window, cx);
            let image_view_toolbar =
                cx.new(|_| carrot_image_viewer::ImageViewToolbarControls::new());
            toolbar.add_item(image_view_toolbar, window, cx);
        })
    });
}

fn about(_: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use std::fmt::Write;
    let release_channel = ReleaseChannel::global(cx).display_name();
    let full_version = AppVersion::global(cx);
    let version = env!("CARGO_PKG_VERSION");
    let debug = if cfg!(debug_assertions) {
        "(debug)"
    } else {
        ""
    };
    let message = format!("{release_channel} {version} {debug}");

    let mut detail = AppCommitSha::try_global(cx)
        .map(|sha| sha.full())
        .unwrap_or_default();
    if !detail.is_empty() {
        detail.push('\n');
    }
    _ = write!(&mut detail, "\n{full_version}");

    let detail = Some(detail);

    let prompt = window.prompt(
        PromptLevel::Info,
        &message,
        detail.as_deref(),
        &["Copy", "OK"],
        cx,
    );
    cx.spawn(async move |_, cx| {
        if let Ok(0) = prompt.await {
            let content = format!("{}\n{}", message, detail.as_deref().unwrap_or(""));
            cx.update(|cx| {
                cx.write_to_clipboard(inazuma::ClipboardItem::new_string(content));
            });
        }
    })
    .detach();
}

#[cfg(not(target_os = "windows"))]
fn install_cli(
    _: &mut Workspace,
    _: &carrot_install_cli::InstallCliBinary,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    carrot_install_cli::install_cli_binary(window, cx)
}

static WAITING_QUIT_CONFIRMATION: AtomicBool = AtomicBool::new(false);
fn quit(_: &Quit, cx: &mut App) {
    if WAITING_QUIT_CONFIRMATION.load(atomic::Ordering::Acquire) {
        return;
    }

    let should_confirm = WorkspaceSettings::get_global(cx).confirm_quit;
    cx.spawn(async move |cx| {
        let mut workspace_windows: Vec<WindowHandle<Workspace>> = cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter_map(|window| window.downcast::<Workspace>())
                .collect::<Vec<_>>()
        });

        // If multiple windows have unsaved changes, and need a save prompt,
        // prompt in the active window before switching to a different window.
        cx.update(|cx| {
            workspace_windows.sort_by_key(|window| window.is_active(cx) == Some(false));
        });

        if should_confirm && let Some(multi_workspace) = workspace_windows.first() {
            let answer = multi_workspace
                .update(cx, |_, window, cx| {
                    window.prompt(
                        PromptLevel::Info,
                        "Are you sure you want to quit?",
                        None,
                        &["Quit", "Cancel"],
                        cx,
                    )
                })
                .log_err();

            if let Some(answer) = answer {
                WAITING_QUIT_CONFIRMATION.store(true, atomic::Ordering::Release);
                let answer = answer.await.ok();
                WAITING_QUIT_CONFIRMATION.store(false, atomic::Ordering::Release);
                if answer != Some(0) {
                    return Ok(());
                }
            }
        }

        // If the user cancels any save prompt, then keep the app open.
        for window in &workspace_windows {
            let window = *window;
            if let Some(should_close) = window
                .update(cx, |workspace, window, cx| {
                    window.activate_window();
                    workspace.prepare_to_close(CloseIntent::Quit, window, cx)
                })
                .log_err()
            {
                if !should_close.await? {
                    return Ok(());
                }
            }
        }
        // Flush pending workspace serialization before quitting.
        let mut flush_tasks = Vec::new();
        for window in &workspace_windows {
            window
                .update(cx, |workspace, window, cx| {
                    flush_tasks.push(workspace.flush_serialization(window, cx));
                })
                .log_err();
        }
        futures::future::join_all(flush_tasks).await;

        cx.update(|cx| cx.quit());
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn open_log_file(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    const MAX_LINES: usize = 1000;
    let app_state = workspace.app_state();
    let languages = app_state.languages.clone();
    let fs = app_state.fs.clone();
    cx.spawn_in(window, async move |workspace, cx| {
        let log = {
            let result = futures::join!(
                fs.load(&carrot_paths::old_log_file()),
                fs.load(&carrot_paths::log_file()),
                languages.language_for_name("log")
            );
            match result {
                (Err(_), Err(e), _) => Err(e),
                (old_log, new_log, lang) => {
                    let mut lines = VecDeque::with_capacity(MAX_LINES);
                    for line in old_log
                        .iter()
                        .flat_map(|log| log.lines())
                        .chain(new_log.iter().flat_map(|log| log.lines()))
                    {
                        if lines.len() == MAX_LINES {
                            lines.pop_front();
                        }
                        lines.push_back(line);
                    }
                    Ok((
                        lines
                            .into_iter()
                            .flat_map(|line| [line, "\n"])
                            .collect::<String>(),
                        lang.ok(),
                    ))
                }
            }
        };

        let (log, log_language) = match log {
            Ok((log, log_language)) => (log, log_language),
            Err(e) => {
                struct OpenLogError;

                workspace
                    .update(cx, |workspace, cx| {
                        workspace.show_notification(
                            NotificationId::unique::<OpenLogError>(),
                            cx,
                            |cx| {
                                cx.new(|cx| {
                                    MessageNotification::new(
                                        format!(
                                            "Unable to access/open log file at path \
                                                    {}: {e:#}",
                                            carrot_paths::log_file().display()
                                        ),
                                        cx,
                                    )
                                })
                            },
                        );
                    })
                    .ok();
                return;
            }
        };
        maybe!(async move {
            let project = workspace
                .read_with(cx, |workspace, _| workspace.project().clone())
                .ok()?;
            let buffer = project
                .update(cx, |project, cx| {
                    project.create_buffer(log_language, false, cx)
                })
                .await
                .ok()?;
            buffer.update(cx, |buffer, cx| {
                buffer.set_capability(Capability::ReadOnly, cx);
                buffer.set_text(log, cx);
            });

            let buffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title("Log".into()));

            let editor = cx
                .new_window_entity(|window, cx| {
                    let mut editor = Editor::for_multibuffer(buffer, Some(project), window, cx);
                    editor.set_read_only(true);
                    editor.set_breadcrumb_header(format!(
                        "Last {} lines in {}",
                        MAX_LINES,
                        carrot_paths::log_file().display()
                    ));
                    let last_multi_buffer_offset = editor.buffer().read(cx).len(cx);
                    editor.change_selections(Default::default(), window, cx, |s| {
                        s.select_ranges(Some(last_multi_buffer_offset..last_multi_buffer_offset));
                    });
                    editor
                })
                .ok()?;

            workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);
                })
                .ok()
        })
        .await;
    })
    .detach();
}

fn notify_settings_errors(
    result: inazuma_settings_framework::SettingsParseResult,
    is_user: bool,
    cx: &mut App,
) {
    if let inazuma_settings_framework::ParseStatus::Failed { error: err } = &result.parse_status {
        let settings_type = if is_user { "user" } else { "global" };
        log::error!("Failed to load {} settings: {err}", settings_type);
    }

    let error = match result.parse_status {
        inazuma_settings_framework::ParseStatus::Failed { error } => {
            Some(anyhow::format_err!(error))
        }
        inazuma_settings_framework::ParseStatus::Success => None,
    };
    let id = NotificationId::Named(format!("failed-to-parse-settings-{is_user}").into());

    let showed_parse_error = match error {
        Some(error) => {
            if let Some(InvalidSettingsError::LocalSettings { .. }) =
                error.downcast_ref::<InvalidSettingsError>()
            {
                false
                // Local settings errors are displayed by the projects
            } else {
                show_app_notification(id, cx, move |cx| {
                    cx.new(|cx| {
                        MessageNotification::new(format!("Invalid user settings file\n{error}"), cx)
                            .primary_message("Open Settings File")
                            .primary_icon(IconName::Settings)
                            .primary_on_click(|window, cx| {
                                window.dispatch_action(
                                    carrot_actions::OpenSettingsFile.boxed_clone(),
                                    cx,
                                );
                                cx.emit(DismissEvent);
                            })
                    })
                });
                true
            }
        }
        None => {
            dismiss_app_notification(&id, cx);
            false
        }
    };
    let _ = showed_parse_error;
}

pub fn handle_settings_file_changes(
    mut user_settings_file_rx: mpsc::UnboundedReceiver<String>,
    user_settings_watcher: inazuma::Task<()>,
    mut global_settings_file_rx: mpsc::UnboundedReceiver<String>,
    global_settings_watcher: inazuma::Task<()>,
    cx: &mut App,
) {
    // Initial load of both settings files
    let global_content = cx
        .foreground_executor()
        .block_on(global_settings_file_rx.next())
        .unwrap();
    let user_content = cx
        .foreground_executor()
        .block_on(user_settings_file_rx.next())
        .unwrap();

    SettingsStore::update_global(cx, |store, cx| {
        notify_settings_errors(store.set_user_settings(&user_content, cx), true, cx);
        notify_settings_errors(store.set_global_settings(&global_content, cx), false, cx);
    });

    // Watch for changes in both files
    cx.spawn(async move |cx| {
        let _user_settings_watcher = user_settings_watcher;
        let _global_settings_watcher = global_settings_watcher;
        let mut settings_streams = futures::stream::select(
            global_settings_file_rx.map(Either::Left),
            user_settings_file_rx.map(Either::Right),
        );

        while let Some(content) = settings_streams.next().await {
            let (content, is_user) = match content {
                Either::Left(content) => (content, false),
                Either::Right(content) => (content, true),
            };

            cx.update_global(|store: &mut SettingsStore, cx| {
                let result = if is_user {
                    store.set_user_settings(&content, cx)
                } else {
                    store.set_global_settings(&content, cx)
                };
                notify_settings_errors(result, is_user, cx);
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

pub fn handle_keymap_file_changes(
    mut user_keymap_file_rx: mpsc::UnboundedReceiver<String>,
    user_keymap_watcher: inazuma::Task<()>,
    cx: &mut App,
) {
    let (base_keymap_tx, mut base_keymap_rx) = mpsc::unbounded();
    let (keyboard_layout_tx, mut keyboard_layout_rx) = mpsc::unbounded();
    let mut old_base_keymap = *BaseKeymap::get_global(cx);
    let mut old_vim_enabled = VimModeSetting::get_global(cx).0;
    let mut old_helix_enabled = carrot_vim_mode_setting::HelixModeSetting::get_global(cx).0;

    cx.observe_global::<SettingsStore>(move |cx| {
        let new_base_keymap = *BaseKeymap::get_global(cx);
        let new_vim_enabled = VimModeSetting::get_global(cx).0;
        let new_helix_enabled = carrot_vim_mode_setting::HelixModeSetting::get_global(cx).0;

        if new_base_keymap != old_base_keymap
            || new_vim_enabled != old_vim_enabled
            || new_helix_enabled != old_helix_enabled
        {
            old_base_keymap = new_base_keymap;
            old_vim_enabled = new_vim_enabled;
            old_helix_enabled = new_helix_enabled;

            base_keymap_tx.unbounded_send(()).unwrap();
        }
    })
    .detach();

    #[cfg(target_os = "windows")]
    {
        let mut current_layout_id = cx.keyboard_layout().id().to_string();
        cx.on_keyboard_layout_change(move |cx| {
            let next_layout_id = cx.keyboard_layout().id();
            if next_layout_id != current_layout_id {
                current_layout_id = next_layout_id.to_string();
                keyboard_layout_tx.unbounded_send(()).ok();
            }
        })
        .detach();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut current_mapping = cx.keyboard_mapper().get_key_equivalents().cloned();
        cx.on_keyboard_layout_change(move |cx| {
            let next_mapping = cx.keyboard_mapper().get_key_equivalents();
            if current_mapping.as_ref() != next_mapping {
                current_mapping = next_mapping.cloned();
                keyboard_layout_tx.unbounded_send(()).ok();
            }
        })
        .detach();
    }

    load_default_keymap(cx);

    struct KeymapParseErrorNotification;
    let notification_id = NotificationId::unique::<KeymapParseErrorNotification>();

    cx.spawn(async move |cx| {
        let _user_keymap_watcher = user_keymap_watcher;
        let mut user_keymap_content = String::new();
        loop {
            select_biased! {
                _ = base_keymap_rx.next() => {},
                _ = keyboard_layout_rx.next() => {},
                content = user_keymap_file_rx.next() => {
                    if let Some(content) = content {
                        user_keymap_content = content;
                    }
                }
            };
            cx.update(|cx| {
                let load_result = KeymapFile::load(&user_keymap_content, cx);
                match load_result {
                    KeymapFileLoadResult::Success { key_bindings } => {
                        reload_keymaps(cx, key_bindings);
                        dismiss_app_notification(&notification_id.clone(), cx);
                    }
                    KeymapFileLoadResult::SomeFailedToLoad {
                        key_bindings,
                        error_message,
                    } => {
                        if !key_bindings.is_empty() {
                            reload_keymaps(cx, key_bindings);
                        }
                        show_keymap_file_load_error(notification_id.clone(), error_message, cx);
                    }
                    KeymapFileLoadResult::ParseFailure { error } => {
                        show_keymap_file_json_error(notification_id.clone(), &error, cx)
                    }
                }
            });
        }
    })
    .detach();
}

fn show_keymap_file_json_error(
    notification_id: NotificationId,
    error: &anyhow::Error,
    cx: &mut App,
) {
    let message: SharedString =
        format!("JSON parse error in keymap file. Bindings not reloaded.\n\n{error}").into();
    show_app_notification(notification_id, cx, move |cx| {
        cx.new(|cx| {
            MessageNotification::new(message.clone(), cx)
                .primary_message("Open Keymap File")
                .primary_icon(IconName::Settings)
                .primary_on_click(|window, cx| {
                    window.dispatch_action(carrot_actions::OpenKeymapFile.boxed_clone(), cx);
                    cx.emit(DismissEvent);
                })
        })
    });
}

fn show_keymap_file_load_error(
    notification_id: NotificationId,
    error_message: MarkdownString,
    cx: &mut App,
) {
    show_markdown_app_notification(
        notification_id,
        error_message,
        "Open Keymap File".into(),
        |window, cx| {
            window.dispatch_action(carrot_actions::OpenKeymapFile.boxed_clone(), cx);
            cx.emit(DismissEvent);
        },
        cx,
    )
}

fn show_markdown_app_notification<F>(
    notification_id: NotificationId,
    message: MarkdownString,
    primary_button_message: SharedString,
    primary_button_on_click: F,
    cx: &mut App,
) where
    F: 'static + Send + Sync + Fn(&mut Window, &mut Context<MessageNotification>),
{
    let markdown = cx.new(|cx| Markdown::new(message.0.into(), None, None, cx));
    let primary_button_on_click = Arc::new(primary_button_on_click);

    show_app_notification(notification_id, cx, move |cx| {
        let markdown = markdown.clone();
        let primary_button_message = primary_button_message.clone();
        let primary_button_on_click = primary_button_on_click.clone();

        cx.new(move |cx| {
            MessageNotification::new_from_builder(cx, move |window, cx| {
                image_cache(retain_all("notification-cache"))
                    .child(div().text_ui(cx).child(MarkdownElement::new(
                        markdown.clone(),
                        MarkdownStyle::themed(MarkdownFont::Editor, window, cx),
                    )))
                    .into_any()
            })
            .primary_message(primary_button_message)
            .primary_icon(IconName::Settings)
            .primary_on_click_arc(primary_button_on_click)
        })
    })
}

fn reload_keymaps(cx: &mut App, mut user_key_bindings: Vec<KeyBinding>) {
    cx.clear_key_bindings();
    load_default_keymap(cx);

    for key_binding in &mut user_key_bindings {
        key_binding.set_meta(KeybindSource::User.meta());
    }
    cx.bind_keys(user_key_bindings);

    let menus = app_menus(cx);
    cx.set_menus(menus);
    // On Windows, this is set in the `update_jump_list` method of the `HistoryManager`.
    #[cfg(not(target_os = "windows"))]
    cx.set_dock_menu(vec![inazuma::MenuItem::action(
        "New Window",
        carrot_workspace::NewWindow,
    )]);
    // todo: nicer api here?
    carrot_keymap_editor::KeymapEventChannel::trigger_keymap_changed(cx);
}

pub fn load_default_keymap(cx: &mut App) {
    let base_keymap = *BaseKeymap::get_global(cx);
    if base_keymap == BaseKeymap::None {
        return;
    }

    cx.bind_keys(
        KeymapFile::load_asset(DEFAULT_KEYMAP_PATH, Some(KeybindSource::Default), cx).unwrap(),
    );

    if let Some(asset_path) = base_keymap.asset_path() {
        cx.bind_keys(KeymapFile::load_asset(asset_path, Some(KeybindSource::Base), cx).unwrap());
    }

    if VimModeSetting::get_global(cx).0
        || carrot_vim_mode_setting::HelixModeSetting::get_global(cx).0
    {
        cx.bind_keys(
            KeymapFile::load_asset(VIM_KEYMAP_PATH, Some(KeybindSource::Vim), cx).unwrap(),
        );
    }
}

pub fn open_new_ssh_project_from_project(
    workspace: &mut Workspace,
    paths: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Task<anyhow::Result<()>> {
    let app_state = workspace.app_state().clone();
    let Some(ssh_client) = workspace.project().read(cx).remote_client() else {
        return Task::ready(Err(anyhow::anyhow!("Not an ssh project")));
    };
    let connection_options = ssh_client.read(cx).connection_options();
    cx.spawn_in(window, async move |_, cx| {
        open_remote_project(
            connection_options,
            paths,
            app_state,
            carrot_shell::OpenOptions {
                open_new_workspace: Some(true),
                ..Default::default()
            },
            cx,
        )
        .await
    })
}

fn open_project_settings_file(
    workspace: &mut Workspace,
    _: &OpenProjectSettingsFile,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    open_local_file(
        workspace,
        local_settings_file_relative_path(),
        initial_project_settings_content(),
        window,
        cx,
    )
}

fn open_project_tasks_file(
    workspace: &mut Workspace,
    _: &OpenProjectTasks,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    open_local_file(
        workspace,
        local_tasks_file_relative_path(),
        initial_tasks_content(),
        window,
        cx,
    )
}

fn open_project_debug_tasks_file(
    workspace: &mut Workspace,
    _: &carrot_actions::OpenProjectDebugTasks,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    open_local_file(
        workspace,
        local_debug_file_relative_path(),
        initial_local_debug_tasks_content(),
        window,
        cx,
    )
}

fn open_local_file(
    workspace: &mut Workspace,
    settings_relative_path: &'static RelPath,
    initial_contents: Cow<'static, str>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project().clone();
    let worktree = project
        .read(cx)
        .visible_worktrees(cx)
        .find_map(|tree| tree.read(cx).root_entry()?.is_dir().then_some(tree));
    if let Some(worktree) = worktree {
        let tree_id = worktree.read(cx).id();
        cx.spawn_in(window, async move |workspace, cx| {
            // Check if the file actually exists on disk (even if it's excluded from worktree)
            let file_exists = {
                let full_path = worktree.read_with(cx, |tree, _| {
                    tree.abs_path().join(settings_relative_path.as_std_path())
                });

                let fs = project.read_with(cx, |project, _| project.fs().clone());

                fs.metadata(&full_path)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|metadata| !metadata.is_dir && !metadata.is_fifo)
            };

            if !file_exists {
                if let Some(dir_path) = settings_relative_path.parent()
                    && worktree.read_with(cx, |tree, _| tree.entry_for_path(dir_path).is_none())
                {
                    project
                        .update(cx, |project, cx| {
                            project.create_entry((tree_id, dir_path), true, cx)
                        })
                        .await
                        .context("worktree was removed")?;
                }

                if worktree.read_with(cx, |tree, _| {
                    tree.entry_for_path(settings_relative_path).is_none()
                }) {
                    project
                        .update(cx, |project, cx| {
                            project.create_entry((tree_id, settings_relative_path), false, cx)
                        })
                        .await
                        .context("worktree was removed")?;
                }
            }

            let editor = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_path((tree_id, settings_relative_path), None, true, window, cx)
                })?
                .await?
                .downcast::<Editor>()
                .context("unexpected item type: expected editor item")?;

            editor
                .downgrade()
                .update(cx, |editor, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton()
                        && buffer.read(cx).is_empty()
                    {
                        buffer.update(cx, |buffer, cx| {
                            buffer.edit([(0..0, initial_contents)], None, cx)
                        });
                    }
                })
                .ok();

            anyhow::Ok(())
        })
        .detach();
    } else {
        struct NoOpenFolders;

        workspace.show_notification(NotificationId::unique::<NoOpenFolders>(), cx, |cx| {
            cx.new(|cx| MessageNotification::new("This project has no folders open.", cx))
        })
    }
}

fn open_bundled_file(
    workspace: &mut Workspace,
    text: Cow<'static, str>,
    title: &'static str,
    language: &'static str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace.items_of_type::<Editor>(cx).find(|editor| {
        editor.read_with(cx, |editor, cx| {
            editor.read_only(cx)
                && editor.title(cx).as_ref() == title
                && editor
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .is_some_and(|buffer| buffer.read(cx).file().is_none())
        })
    });
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
        return;
    }

    let language = workspace.app_state().languages.language_for_name(language);
    cx.spawn_in(window, async move |workspace, cx| {
        let language = language.await.log_err();
        workspace
            .update_in(cx, move |workspace, window, cx| {
                let project = workspace.project().clone();
                let buffer = project.update(cx, move |project, cx| {
                    project.create_buffer(language, false, cx)
                });
                cx.spawn_in(window, async move |workspace, cx| {
                    let buffer = buffer.await?;
                    buffer.update(cx, |buffer, cx| {
                        buffer.set_text(text.into_owned(), cx);
                        buffer.set_capability(Capability::ReadOnly, cx);
                    });
                    let buffer =
                        cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title(title.into()));
                    workspace.update_in(cx, |workspace, window, cx| {
                        workspace.add_item_to_active_pane(
                            Box::new(cx.new(|cx| {
                                let mut editor = Editor::for_multibuffer(
                                    buffer,
                                    Some(project.clone()),
                                    window,
                                    cx,
                                );
                                editor.set_read_only(true);
                                editor.set_should_serialize(false, cx);
                                editor.set_breadcrumb_header(title.into());
                                editor
                            })),
                            None,
                            true,
                            window,
                            cx,
                        )
                    })
                })
            })?
            .await
    })
    .detach_and_log_err(cx);
}

fn open_settings_file(
    abs_path: &'static Path,
    default_content: impl FnOnce() -> Rope + Send + 'static,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.spawn_in(window, async move |workspace, cx| {
        let (worktree_creation_task, settings_open_task) = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.with_local_or_wsl_workspace(window, cx, move |workspace, window, cx| {
                    let project = workspace.project().clone();

                    let worktree_creation_task = cx.spawn_in(window, async move |_, cx| {
                        let config_dir = project
                            .update(cx, |project, cx| {
                                project.try_windows_path_to_wsl(
                                    carrot_paths::config_dir().as_path(),
                                    cx,
                                )
                            })
                            .await?;
                        // Set up a dedicated worktree for settings, since
                        // otherwise we're dropping and re-starting LSP servers
                        // for each file inside on every settings file
                        // close/open

                        // TODO: Do note that all other external files (e.g.
                        // drag and drop from OS) still have their worktrees
                        // released on file close, causing LSP servers'
                        // restarts.
                        project
                            .update(cx, |project, cx| {
                                project.find_or_create_worktree(&config_dir, false, cx)
                            })
                            .await
                    });
                    let settings_open_task =
                        create_and_open_local_file(abs_path, window, cx, default_content);
                    (worktree_creation_task, settings_open_task)
                })
            })?
            .await?;
        let _ = worktree_creation_task.await?;
        let _ = settings_open_task.await?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

/// Eagerly loads the active theme and icon theme based on the selections in the
/// theme settings.
///
/// This fast path exists to load these themes as soon as possible so the user
/// doesn't see the default themes while waiting on extensions to load.
pub(crate) fn eager_load_active_theme_and_icon_theme(fs: Arc<dyn Fs>, cx: &mut App) {
    // Extensions are optional in Carrot. When the extension store isn't
    // initialized (the common case today — extension system rewrite pending,
    // see plan/27-EXTENSION-SYSTEM-REWRITE.md), fall back to loading whatever
    // themes are already in the registry instead of panicking.
    let Some(extension_store) = ExtensionStore::try_global(cx) else {
        return;
    };
    let theme_registry = ThemeRegistry::global(cx);
    let theme_settings = ThemeSettings::get_global(cx);
    let appearance = SystemAppearance::global(cx).0;

    enum LoadTarget {
        Theme(PathBuf),
        IconTheme((PathBuf, PathBuf)),
    }

    let theme_name = theme_settings.theme.name(appearance);
    let icon_theme_name = theme_settings.icon_theme.name(appearance);
    let themes_to_load = [
        theme_registry
            .get(&theme_name.0)
            .is_err()
            .then(|| {
                extension_store
                    .read(cx)
                    .path_to_extension_theme(&theme_name.0)
            })
            .flatten()
            .map(LoadTarget::Theme),
        theme_registry
            .get_icon_theme(&icon_theme_name.0)
            .is_err()
            .then(|| {
                extension_store
                    .read(cx)
                    .path_to_extension_icon_theme(&icon_theme_name.0)
            })
            .flatten()
            .map(LoadTarget::IconTheme),
    ];

    enum ReloadTarget {
        Theme,
        IconTheme,
    }

    let executor = cx.background_executor();
    let reload_tasks = parking_lot::Mutex::new(Vec::with_capacity(themes_to_load.len()));

    let mut themes_to_load = themes_to_load.into_iter().flatten().peekable();

    if themes_to_load.peek().is_none() {
        return;
    }

    cx.foreground_executor().block_on(executor.scoped(|scope| {
        for load_target in themes_to_load {
            let theme_registry = &theme_registry;
            let reload_tasks = &reload_tasks;
            let fs = fs.clone();

            scope.spawn(async move {
                match load_target {
                    LoadTarget::Theme(theme_path) => {
                        if let Some(bytes) = fs.load_bytes(&theme_path).await.log_err()
                            && load_user_theme(theme_registry, &theme_path, &bytes)
                                .log_err()
                                .is_some()
                        {
                            reload_tasks.lock().push(ReloadTarget::Theme);
                        }
                    }
                    LoadTarget::IconTheme((icon_theme_path, icons_root_path)) => {
                        if let Some(bytes) = fs.load_bytes(&icon_theme_path).await.log_err()
                            && let Some(icon_theme_family) =
                                deserialize_icon_theme(&bytes).log_err()
                            && theme_registry
                                .load_icon_theme(icon_theme_family, &icons_root_path)
                                .log_err()
                                .is_some()
                        {
                            reload_tasks.lock().push(ReloadTarget::IconTheme);
                        }
                    }
                }
            });
        }
    }));

    for reload_target in reload_tasks.into_inner() {
        match reload_target {
            ReloadTarget::Theme => carrot_theme_settings::reload_theme(cx),
            ReloadTarget::IconTheme => carrot_theme_settings::reload_icon_theme(cx),
        };
    }
}
