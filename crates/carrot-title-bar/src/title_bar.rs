mod application_menu;
pub mod collab;
mod onboarding_banner;
mod plan_chip;
mod title_bar_settings;
mod update_version;

#[cfg(feature = "stories")]
mod stories;

use crate::application_menu::{ApplicationMenu, show_menus};
use crate::plan_chip::PlanChip;
pub use carrot_platform_title_bar::{
    self, DraggedWindowTab, MergeAllWindows, MoveTabToNewWindow, PlatformTitleBar,
    ShowNextWindowTab, ShowPreviousWindowTab,
};

#[cfg(not(target_os = "macos"))]
use crate::application_menu::{
    ActivateDirection, ActivateMenuLeft, ActivateMenuRight, OpenApplicationMenu,
};

use carrot_auto_update::AutoUpdateStatus;
use carrot_call::ActiveCall;
use carrot_client::{Client, UserStore, carrot_urls};
use carrot_cloud_api_types::Plan;

use carrot_project::{Project, git_store::GitStoreEvent, trusted_worktrees::TrustedWorktrees};
use carrot_remote::RemoteConnectionOptions;
use carrot_theme::ActiveTheme;
use carrot_ui::{
    Avatar, ButtonLike, ContextMenu, IconWithIndicator, Indicator, PopoverMenu, PopoverMenuHandle,
    TintColor, Tooltip, prelude::*,
};
use inazuma::{
    Action, AnyElement, App, Context, Corner, Element, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, actions, div,
};
use inazuma_settings_framework::Settings;
use inazuma_settings_framework::WorktreeId;
use onboarding_banner::OnboardingBanner;
use std::sync::Arc;
use title_bar_settings::TitleBarSettings;

#[cfg(not(target_os = "windows"))]
fn platform_title_bar_height(window: &Window) -> inazuma::Pixels {
    (1.75 * window.rem_size()).max(inazuma::px(34.))
}

#[cfg(target_os = "windows")]
fn platform_title_bar_height(_window: &Window) -> inazuma::Pixels {
    inazuma::px(32.)
}
use carrot_actions::OpenRemote;
use carrot_workspace::{ToggleWorktreeSecurity, Workspace, notifications::NotifyResultExt};
use inazuma_util::ResultExt;
use update_version::UpdateVersion;

pub use onboarding_banner::restore_banner;

#[cfg(feature = "stories")]
pub use stories::*;

actions!(
    collab,
    [
        /// Toggles the user menu dropdown.
        ToggleUserMenu,
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch,
        /// A debug action to simulate an update being available to test the update banner UI.
        SimulateUpdateAvailable
    ]
);

pub fn init(cx: &mut App) {
    carrot_platform_title_bar::PlatformTitleBar::init(cx);

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let item = cx.new(|cx| TitleBar::new("title-bar", workspace, window, cx));
        workspace.set_titlebar_item(item.into(), window, cx);

        workspace.register_action(|workspace, _: &SimulateUpdateAvailable, _window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    titlebar.toggle_update_simulation(cx);
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, action: &OpenApplicationMenu, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| menu.open_menu(action, window, cx));
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuRight, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Right, window, cx)
                        });
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuLeft, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Left, window, cx)
                        });
                    }
                });
            }
        });
    })
    .detach();
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    user_store: Entity<UserStore>,
    client: Arc<Client>,
    workspace: WeakEntity<Workspace>,
    application_menu: Option<Entity<ApplicationMenu>>,
    _subscriptions: Vec<Subscription>,
    banner: Entity<OnboardingBanner>,
    update_version: Entity<UpdateVersion>,
    screen_share_popover_handle: PopoverMenuHandle<ContextMenu>,
    _diagnostics_subscription: Option<inazuma::Subscription>,
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let button_layout = title_bar_settings.button_layout;

        let show_menus = show_menus(cx);

        let mut children = Vec::new();

        // Left section: application menu + restricted mode only.
        // Project name, host, and branch are all removed from the title
        // bar. The branch lives in the git-branch chip in the input area;
        // "Open Recent Project" is reachable via the command palette.
        children.push(
            h_flex()
                .h_full()
                .gap_0p5()
                .when_some(
                    self.application_menu.clone().filter(|_| !show_menus),
                    |title_bar, menu| title_bar.child(menu),
                )
                .children(self.render_restricted_mode(cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
        );

        // Center section: the vertical tabs panel owns session tab
        // management. The title bar's middle slot is a flex_1 spacer
        // that the absolute-centred workspace search trigger overlays
        // (justify_between on the outer flex would otherwise pin the
        // flex_1 child off-centre).
        children.push(div().flex_1().into_any_element());

        children.push(self.render_collaborator_list(window, cx).into_any_element());

        if title_bar_settings.show_onboarding_banner {
            children.push(self.banner.clone().into_any_element())
        }

        let status = self.client.status();
        let status = &*status.borrow();
        let user = self.user_store.read(cx).current_user();

        let signed_in = user.is_some();

        children.push(
            h_flex()
                .map(|this| {
                    if signed_in {
                        this.pr_1p5()
                    } else {
                        this.pr_1()
                    }
                })
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(self.render_call_controls(window, cx))
                .children(self.render_connection_status(status, cx))
                .child(self.update_version.clone())
                .when(
                    user.is_none() && TitleBarSettings::get_global(cx).show_sign_in,
                    |this| this.child(self.render_sign_in_button(cx)),
                )
                .when(TitleBarSettings::get_global(cx).show_user_menu, |this| {
                    this.child(self.render_user_menu_button(cx))
                })
                .into_any_element(),
        );

        if show_menus {
            self.platform_titlebar.update(cx, |this, _| {
                this.set_button_layout(button_layout);
                this.set_children(
                    self.application_menu
                        .clone()
                        .map(|menu| menu.into_any_element()),
                );
            });

            let height = platform_title_bar_height(window);
            let title_bar_color = self.platform_titlebar.update(cx, |platform_titlebar, cx| {
                platform_titlebar.title_bar_color(window, cx)
            });

            v_flex()
                .w_full()
                .child(self.platform_titlebar.clone().into_any_element())
                .child(
                    h_flex()
                        .relative()
                        .bg(title_bar_color)
                        .h(height)
                        .pl_2()
                        .justify_between()
                        .w_full()
                        .children(children)
                        .child(
                            h_flex()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .child(self.render_workspace_search(cx)),
                        ),
                )
                .into_any_element()
        } else {
            // macOS path: app menu lives in the OS menubar, the title bar is
            // the platform's PlatformTitleBar with our content as children.
            // To centre the search trigger we wrap the platform titlebar in
            // a relative div and overlay the trigger absolutely.
            self.platform_titlebar.update(cx, |this, _| {
                this.set_button_layout(button_layout);
                this.set_children(children);
            });
            div()
                .relative()
                .w_full()
                .child(self.platform_titlebar.clone().into_any_element())
                .child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(self.render_workspace_search(cx)),
                )
                .into_any_element()
        }
    }
}

impl TitleBar {
    /// Render the per-session tab bar that lives in the title bar.
    /// Each session is rendered as a tab; clicking a tab activates it,
    /// clicking the trailing "+" creates a new session.
    ///
    /// Visual rules:
    /// - Inactive tabs use `theme.colors.tab.inactive_background` which
    ///   themes are expected to set to the same value as
    ///   `theme.colors.title_bar.background` so inactive tabs blend in.
    /// - The active tab uses `theme.colors.tab.active_background` which
    ///   themes are expected to set to a clearly lighter value.
    /// - Each tab has thin left/right borders in `theme.colors.border` to
    ///   visually separate them.
    /// - Text uses `theme.colors.tab.active_foreground` / `inactive_foreground`.
    /// Center area shown when the vertical tabs panel owns tab management.
    /// Replaces the horizontal session tabs with a trigger that opens the
    /// command search modal — clicking expands to a full search panel
    /// (workflows, prompts, files, sessions, …). The trigger renders like
    /// a search input but isn't editable inline.
    fn render_workspace_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        // Returns the bare trigger; absolute centring is done by the caller.
        // No border, larger icon and text — matches the workspace-search
        // affordance you'd expect at the top of a Glass UI window.
        h_flex()
            .id("workspace-search-trigger")
            .w(inazuma::px(320.))
            .h(inazuma::px(28.))
            .px_3()
            .gap_2()
            .items_center()
            .rounded(inazuma::px(6.))
            .bg(colors.element_background)
            .cursor_pointer()
            .hover(|el| el.bg(colors.element_hover))
            .child(
                carrot_ui::Icon::new(carrot_ui::IconName::MagnifyingGlass)
                    .size(carrot_ui::IconSize::Medium)
                    .color(carrot_ui::Color::Muted),
            )
            .child(
                div()
                    .text_size(inazuma::px(14.))
                    .text_color(colors.text_muted)
                    .child("Search sessions, agents, files…"),
            )
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(carrot_command_palette::Toggle), cx);
            })
            .into_any_element()
    }

    pub fn new(
        id: impl Into<ElementId>,
        workspace: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let user_store = workspace.app_state().user_store.clone();
        let client = workspace.app_state().client.clone();
        let active_call = ActiveCall::global(cx);

        let platform_style = PlatformStyle::platform();
        let application_menu = match platform_style {
            PlatformStyle::Mac => {
                if option_env!("ZED_USE_CROSS_PLATFORM_MENU").is_some() {
                    Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
                } else {
                    None
                }
            }
            PlatformStyle::Linux | PlatformStyle::Windows => {
                Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
            }
        };

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe(&project, |this, _, event: &carrot_project::Event, cx| {
                if let carrot_project::Event::BufferEdited = event {
                    // Clear override when user types in any editor,
                    // so the title bar reflects the project they're actually working in
                    this.clear_active_worktree_override(cx);
                    cx.notify();
                }
            }),
        );
        subscriptions.push(cx.observe(&active_call, |this, _, cx| this.active_call_changed(cx)));
        subscriptions.push(cx.observe_window_activation(window, Self::window_activation_changed));
        subscriptions.push(
            cx.subscribe(&git_store, move |this, _, event, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_) => {
                    // Clear override when focus-derived active repo changes
                    // (meaning the user focused a file from a different project)
                    this.clear_active_worktree_override(cx);
                    cx.notify();
                }
                GitStoreEvent::RepositoryUpdated(_, _, true) => {
                    cx.notify();
                }
                _ => {}
            }),
        );
        subscriptions.push(cx.observe(&user_store, |_a, _, cx| cx.notify()));
        subscriptions.push(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            subscriptions.push(cx.subscribe(&trusted_worktrees, |_, _, _, cx| {
                cx.notify();
            }));
        }

        let banner = cx.new(|cx| {
            OnboardingBanner::new(
                "ACP Claude Code Onboarding",
                IconName::AiClaude,
                "Claude Agent",
                Some("Introducing:".into()),
                carrot_actions::agent::OpenClaudeAgentOnboardingModal.boxed_clone(),
                cx,
            )
            // When updating this to a non-AI feature release, remove this line.
            .visible_when(|cx| !carrot_project::DisableAiSettings::get_global(cx).disable_ai)
        });

        let update_version = cx.new(|cx| UpdateVersion::new(cx));
        let platform_titlebar = cx.new(|cx| PlatformTitleBar::new(id, cx));

        let mut this = Self {
            platform_titlebar,
            application_menu,
            workspace: workspace.weak_handle(),
            project,
            user_store,
            client,
            _subscriptions: subscriptions,
            banner,
            update_version,
            screen_share_popover_handle: PopoverMenuHandle::default(),
            _diagnostics_subscription: None,
        };

        this.observe_diagnostics(cx);

        this
    }

    fn toggle_update_simulation(&mut self, cx: &mut Context<Self>) {
        self.update_version
            .update(cx, |banner, cx| banner.update_simulation(cx));
        cx.notify();
    }

    /// Returns the worktree to display in the title bar.
    /// - If there's an override set on the workspace, use that (if still valid)
    /// - Otherwise, derive from the active repository
    /// - Fall back to the first visible worktree
    pub fn effective_active_worktree(&self, cx: &App) -> Option<Entity<carrot_project::Worktree>> {
        let project = self.project.read(cx);

        if let Some(workspace) = self.workspace.upgrade() {
            if let Some(override_id) = workspace.read(cx).active_worktree_override() {
                if let Some(worktree) = project.worktree_for_id(override_id, cx) {
                    return Some(worktree);
                }
            }
        }

        if let Some(repo) = project.active_repository(cx) {
            let repo = repo.read(cx);
            let repo_path = &repo.work_directory_abs_path;

            for worktree in project.visible_worktrees(cx) {
                let worktree_path = worktree.read(cx).abs_path();
                if worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref()) {
                    return Some(worktree);
                }
            }
        }

        project.visible_worktrees(cx).next()
    }

    pub fn set_active_worktree_override(
        &mut self,
        worktree_id: WorktreeId,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.set_active_worktree_override(Some(worktree_id), cx);
            });
        }
        cx.notify();
    }

    fn clear_active_worktree_override(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.clear_active_worktree_override(cx);
            });
        }
        cx.notify();
    }

    fn render_remote_project_connection(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.clone();

        let options = self.project.read(cx).remote_connection_options(cx)?;
        let host: SharedString = options.display_name().into();

        let (nickname, tooltip_title, icon) = match options {
            RemoteConnectionOptions::Ssh(options) => (
                options.nickname.map(|nick| nick.into()),
                "Remote Project",
                IconName::Server,
            ),
            RemoteConnectionOptions::Wsl(_) => (None, "Remote Project", IconName::Linux),
            RemoteConnectionOptions::Docker(_dev_container_connection) => {
                (None, "Dev Container", IconName::Box)
            }
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionOptions::Mock(_) => (None, "Mock Remote Project", IconName::Server),
        };

        let nickname = nickname.unwrap_or_else(|| host.clone());

        let (indicator_color, meta) = match self.project.read(cx).remote_connection_state(cx)? {
            carrot_remote::ConnectionState::Connecting => {
                (Color::Info, format!("Connecting to: {host}"))
            }
            carrot_remote::ConnectionState::Connected => {
                (Color::Success, format!("Connected to: {host}"))
            }
            carrot_remote::ConnectionState::HeartbeatMissed => (
                Color::Warning,
                format!("Connection attempt to {host} missed. Retrying..."),
            ),
            carrot_remote::ConnectionState::Reconnecting => (
                Color::Warning,
                format!("Lost connection to {host}. Reconnecting..."),
            ),
            carrot_remote::ConnectionState::Disconnected => {
                (Color::Error, format!("Disconnected from {host}"))
            }
        };

        let icon_color = match self.project.read(cx).remote_connection_state(cx)? {
            carrot_remote::ConnectionState::Connecting => Color::Info,
            carrot_remote::ConnectionState::Connected => Color::Default,
            carrot_remote::ConnectionState::HeartbeatMissed => Color::Warning,
            carrot_remote::ConnectionState::Reconnecting => Color::Warning,
            carrot_remote::ConnectionState::Disconnected => Color::Error,
        };

        let meta = SharedString::from(meta);

        Some(
            PopoverMenu::new("remote-project-menu")
                .menu(move |window, cx| {
                    let workspace_entity = workspace.upgrade()?;
                    let fs = workspace_entity.read(cx).project().read(cx).fs().clone();
                    Some(carrot_recent_projects::RemoteServerProjects::popover(
                        fs,
                        workspace.clone(),
                        false,
                        window,
                        cx,
                    ))
                })
                .trigger_with_tooltip(
                    ButtonLike::new("remote_project")
                        .selected_style(ButtonStyle::tinted(TintColor::Accent))
                        .child(
                            h_flex()
                                .gap_2()
                                .max_w_32()
                                .child(
                                    IconWithIndicator::new(
                                        Icon::new(icon).size(IconSize::Small).color(icon_color),
                                        Some(Indicator::dot().color(indicator_color)),
                                    )
                                    .indicator_border_color(Some(
                                        cx.theme().colors().title_bar.background,
                                    ))
                                    .into_any_element(),
                                )
                                .child(Label::new(nickname).size(LabelSize::Small).truncate()),
                        ),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            tooltip_title,
                            Some(&OpenRemote {
                                from_existing_connection: false,
                                create_new_window: false,
                            }),
                            meta.clone(),
                            cx,
                        )
                    },
                )
                .anchor(inazuma::Corner::TopLeft)
                .into_any_element(),
        )
    }

    pub fn render_restricted_mode(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let has_restricted_worktrees = TrustedWorktrees::try_get_global(cx)
            .map(|trusted_worktrees| {
                trusted_worktrees
                    .read(cx)
                    .has_restricted_worktrees(&self.project.read(cx).worktree_store(), cx)
            })
            .unwrap_or(false);
        if !has_restricted_worktrees {
            return None;
        }

        let button = Button::new("restricted_mode_trigger", "Restricted Mode")
            .style(ButtonStyle::tinted(TintColor::Warning))
            .label_size(LabelSize::Small)
            .color(Color::Warning)
            .start_icon(
                Icon::new(IconName::Warning)
                    .size(IconSize::Small)
                    .color(Color::Warning),
            )
            .tooltip(|_, cx| {
                Tooltip::with_meta(
                    "You're in Restricted Mode",
                    Some(&ToggleWorktreeSecurity),
                    "Mark this project as trusted and unlock all features",
                    cx,
                )
            })
            .on_click({
                cx.listener(move |this, _, window, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| {
                            workspace.show_worktree_trust_security_modal(true, window, cx)
                        })
                        .log_err();
                })
            });

        if cfg!(macos_sdk_26) {
            // Make up for Tahoe's traffic light buttons having less spacing around them
            Some(div().child(button).ml_0p5().into_any_element())
        } else {
            Some(button.into_any_element())
        }
    }

    pub fn render_project_host(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.project.read(cx).is_via_remote_server() {
            return self.render_remote_project_connection(cx);
        }

        if self.project.read(cx).is_disconnected(cx) {
            return Some(
                Button::new("disconnected", "Disconnected")
                    .disabled(true)
                    .color(Color::Disabled)
                    .label_size(LabelSize::Small)
                    .into_any_element(),
            );
        }

        let host = self.project.read(cx).host()?;
        let host_user = self.user_store.read(cx).get_cached_user(host.user_id)?;
        let participant_index = self
            .user_store
            .read(cx)
            .participant_indices()
            .get(&host_user.id)?;

        Some(
            Button::new("project_owner_trigger", host_user.github_login.clone())
                .color(Color::Player(participant_index.0))
                .label_size(LabelSize::Small)
                .tooltip(move |_, cx| {
                    let tooltip_title = format!(
                        "{} is sharing this project. Click to follow.",
                        host_user.github_login
                    );

                    Tooltip::with_meta(tooltip_title, None, "Click to Follow", cx)
                })
                .on_click({
                    let host_peer_id = host.peer_id;
                    cx.listener(move |this, _, window, cx| {
                        this.workspace
                            .update(cx, |workspace, cx| {
                                workspace.follow(host_peer_id, window, cx);
                            })
                            .log_err();
                    })
                })
                .into_any_element(),
        )
    }

    fn window_activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            ActiveCall::global(cx)
                .update(cx, |call, cx| call.set_location(Some(&self.project), cx))
                .detach_and_log_err(cx);
        } else if cx.active_window().is_none() {
            ActiveCall::global(cx)
                .update(cx, |call, cx| call.set_location(None, cx))
                .detach_and_log_err(cx);
        }
    }

    fn active_call_changed(&mut self, cx: &mut Context<Self>) {
        self.observe_diagnostics(cx);
        cx.notify();
    }

    fn observe_diagnostics(&mut self, cx: &mut Context<Self>) {
        let diagnostics = ActiveCall::global(cx)
            .read(cx)
            .room()
            .and_then(|room| room.read(cx).diagnostics().cloned());

        if let Some(diagnostics) = diagnostics {
            self._diagnostics_subscription = Some(cx.observe(&diagnostics, |_, _, cx| cx.notify()));
        } else {
            self._diagnostics_subscription = None;
        }
    }

    fn share_project(&mut self, cx: &mut Context<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.share_project(project, cx))
            .detach_and_log_err(cx);
    }

    fn unshare_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.unshare_project(project, cx))
            .log_err();
    }

    fn render_connection_status(
        &self,
        status: &carrot_client::Status,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match status {
            carrot_client::Status::ConnectionError
            | carrot_client::Status::ConnectionLost
            | carrot_client::Status::Reauthenticating
            | carrot_client::Status::Reconnecting
            | carrot_client::Status::ReconnectionError { .. } => Some(
                div()
                    .id("disconnected")
                    .child(Icon::new(IconName::Disconnected).size(IconSize::Small))
                    .tooltip(Tooltip::text("Disconnected"))
                    .into_any_element(),
            ),
            carrot_client::Status::UpgradeRequired => {
                let auto_updater = carrot_auto_update::AutoUpdater::get(cx);
                let label = match auto_updater.map(|auto_update| auto_update.read(cx).status()) {
                    Some(AutoUpdateStatus::Updated { .. }) => {
                        "Please restart Carrot to Collaborate"
                    }
                    Some(AutoUpdateStatus::Installing { .. })
                    | Some(AutoUpdateStatus::Downloading { .. })
                    | Some(AutoUpdateStatus::Checking) => "Updating...",
                    Some(AutoUpdateStatus::Idle)
                    | Some(AutoUpdateStatus::Errored { .. })
                    | None => "Please update Carrot to Collaborate",
                };

                Some(
                    Button::new("connection-status", label)
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            if let Some(auto_updater) = carrot_auto_update::AutoUpdater::get(cx)
                                && auto_updater.read(cx).status().is_updated()
                            {
                                carrot_shell::reload(cx);
                                return;
                            }
                            carrot_auto_update::check(&Default::default(), window, cx);
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    pub fn render_sign_in_button(&mut self, _: &mut Context<Self>) -> Button {
        let client = self.client.clone();
        let workspace = self.workspace.clone();
        Button::new("sign_in", "Sign In")
            .label_size(LabelSize::Small)
            .on_click(move |_, window, cx| {
                let client = client.clone();
                let workspace = workspace.clone();
                window
                    .spawn(cx, async move |mut cx| {
                        client
                            .sign_in_with_optional_connect(true, cx)
                            .await
                            .notify_workspace_async_err(workspace, &mut cx);
                    })
                    .detach();
            })
    }

    pub fn render_user_menu_button(&mut self, cx: &mut Context<Self>) -> impl Element {
        let show_update_button = self.update_version.read(cx).show_update_in_menu_bar();

        let user_store = self.user_store.clone();
        let user_store_read = user_store.read(cx);
        let user = user_store_read.current_user();

        let user_avatar = user.as_ref().map(|u| u.avatar_uri.clone());
        let user_login = user.as_ref().map(|u| u.github_login.clone());

        let is_signed_in = user.is_some();

        let has_subscription_period = user_store_read.subscription_period().is_some();
        let plan = user_store_read.plan().filter(|_| {
            // Since the user might be on the legacy free plan we filter based on whether we have a subscription period.
            has_subscription_period
        });

        let has_organization = user_store_read.current_organization().is_some();

        let current_organization = user_store_read.current_organization();
        let business_organization = current_organization
            .as_ref()
            .filter(|organization| !organization.is_personal);
        let organizations: Vec<_> = user_store_read
            .organizations()
            .iter()
            .map(|org| {
                let plan = user_store_read.plan_for_organization(&org.id);
                (org.clone(), plan)
            })
            .collect();

        let show_user_picture = TitleBarSettings::get_global(cx).show_user_picture;

        let trigger = if is_signed_in && show_user_picture {
            let avatar = user_avatar.map(|avatar| Avatar::new(avatar)).map(|avatar| {
                if show_update_button {
                    avatar.indicator(
                        div()
                            .absolute()
                            .bottom_0()
                            .right_0()
                            .child(Indicator::dot().color(Color::Accent)),
                    )
                } else {
                    avatar
                }
            });

            ButtonLike::new("user-menu").child(
                h_flex()
                    .when_some(business_organization, |this, organization| {
                        this.gap_2()
                            .child(Label::new(&organization.name).size(LabelSize::Small))
                    })
                    .children(avatar),
            )
        } else {
            ButtonLike::new("user-menu")
                .child(Icon::new(IconName::ChevronDown).size(IconSize::Small))
        };

        PopoverMenu::new("user-menu")
            .trigger(trigger)
            .menu(move |window, cx| {
                let user_login = user_login.clone();
                let current_organization = current_organization.clone();
                let organizations = organizations.clone();
                let user_store = user_store.clone();

                ContextMenu::build(window, cx, |menu, _, _cx| {
                    menu.when(is_signed_in, |this| {
                        let user_login = user_login.clone();
                        this.custom_entry(
                            move |_window, _cx| {
                                let user_login = user_login.clone().unwrap_or_default();

                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(Label::new(user_login))
                                    .child(PlanChip::new(plan.unwrap_or(Plan::CarrotFree)))
                                    .into_any_element()
                            },
                            move |_, cx| {
                                cx.open_url(&carrot_urls::account_url(cx));
                            },
                        )
                        .separator()
                    })
                    .when(show_update_button, |this| {
                        this.custom_entry(
                            move |_window, _cx| {
                                h_flex()
                                    .w_full()
                                    .gap_1()
                                    .justify_between()
                                    .child(
                                        Label::new("Restart to update Carrot").color(Color::Accent),
                                    )
                                    .child(
                                        Icon::new(IconName::Download)
                                            .size(IconSize::Small)
                                            .color(Color::Accent),
                                    )
                                    .into_any_element()
                            },
                            move |_, cx| {
                                carrot_shell::reload(cx);
                            },
                        )
                        .separator()
                    })
                    .when(has_organization, |this| {
                        let mut this = this.header("Organization");

                        for (organization, plan) in &organizations {
                            let organization = organization.clone();
                            let plan = *plan;

                            let is_current =
                                current_organization
                                    .as_ref()
                                    .is_some_and(|current_organization| {
                                        current_organization.id == organization.id
                                    });

                            this = this.custom_entry(
                                {
                                    let organization = organization.clone();
                                    move |_window, _cx| {
                                        h_flex()
                                            .w_full()
                                            .gap_4()
                                            .justify_between()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(Label::new(&organization.name))
                                                    .when(is_current, |this| {
                                                        this.child(
                                                            Icon::new(IconName::Check)
                                                                .color(Color::Accent),
                                                        )
                                                    }),
                                            )
                                            .child(PlanChip::new(plan.unwrap_or(Plan::CarrotFree)))
                                            .into_any_element()
                                    }
                                },
                                {
                                    let user_store = user_store.clone();
                                    let organization = organization.clone();
                                    move |_window, cx| {
                                        user_store.update(cx, |user_store, cx| {
                                            user_store
                                                .set_current_organization(organization.clone(), cx);
                                        });
                                    }
                                },
                            );
                        }

                        this.separator()
                    })
                    .action("Settings", carrot_actions::OpenSettings.boxed_clone())
                    .action("Keymap", Box::new(carrot_actions::OpenKeymap))
                    .action(
                        "Themes…",
                        carrot_actions::theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        "Icon Themes…",
                        carrot_actions::icon_theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        "Extensions",
                        carrot_actions::Extensions::default().boxed_clone(),
                    )
                    .when(is_signed_in, |this| {
                        this.separator()
                            .action("Sign Out", carrot_client::SignOut.boxed_clone())
                    })
                })
                .into()
            })
            .anchor(Corner::TopRight)
    }
}
