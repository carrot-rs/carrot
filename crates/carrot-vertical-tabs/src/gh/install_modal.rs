use carrot_shell_integration::gh_cli::{GH_CLI_URL, detect_gh_installer};
use carrot_ui::{
    Button, ButtonVariants, Context, IntoElement, Modal, ModalFooter, ModalHeader, ParentElement,
    Render, Styled, StyledExt, v_flex,
};
use carrot_workspace::ModalView;
/// Modal offering to install `gh` (GitHub CLI) for the vertical-tabs
/// PR-badge feature. Mirrors the `ShellInstallModal` pattern from
/// `carrot-terminal-view`: the install command is written to the active
/// terminal's PTY so the user sees (and can respond to — e.g. sudo
/// password prompts) the install in their own shell.
use inazuma::{
    App, DismissEvent, EventEmitter, FocusHandle, Focusable, Global, InteractiveElement, Window,
};

/// Tracks whether the user has dismissed the gh install/auth offer in
/// this app session. Prevents re-prompting on every render. Lives as a
/// Global so both modals and the panel can consult / update it.
#[derive(Default)]
pub struct GhPromptDismissed(pub bool);

impl Global for GhPromptDismissed {}

pub struct GhInstallModal {
    install_command: Option<String>,
    pm_name: Option<&'static str>,
    terminal_handle: carrot_terminal::TerminalHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for GhInstallModal {}

impl ModalView for GhInstallModal {
    fn fade_out_background(&self) -> bool {
        true
    }
}

impl Focusable for GhInstallModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GhInstallModal {
    pub fn new(
        terminal_handle: carrot_terminal::TerminalHandle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let installer = detect_gh_installer();
        Self {
            install_command: installer.map(|i| i.command.to_string()),
            pm_name: installer.map(|i| i.package_manager),
            terminal_handle,
            focus_handle: cx.focus_handle(),
        }
    }

    fn install(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref cmd) = self.install_command {
            {
                let mut term = self.terminal_handle.lock();
                term.set_pending_block_command(cmd.clone());
            }
            let mut bytes = cmd.as_bytes().to_vec();
            bytes.push(b'\r');
            self.terminal_handle.write(&bytes);
        } else {
            // No package manager detected — open the official install
            // page so the user can follow distro-specific instructions.
            let _ = std::process::Command::new("open").arg(GH_CLI_URL).spawn();
        }
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Mark the prompt as dismissed so the panel stops nagging. The
        // flag resets on app restart; users who change their mind can
        // toggle `show_pr_link` off and back on to re-trigger the flow.
        cx.global_mut::<GhPromptDismissed>().0 = true;
        cx.emit(DismissEvent);
    }
}

impl Render for GhInstallModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let description = if let Some(pm) = self.pm_name {
            format!(
                "The PR badge on vertical tabs uses GitHub CLI to look up \
                 pull requests for the current branch. Install via {pm} to \
                 enable it."
            )
        } else {
            "The PR badge on vertical tabs uses GitHub CLI to look up \
             pull requests for the current branch. Visit the installation \
             page to get started."
                .to_string()
        };

        let install_label = if self.install_command.is_some() {
            format!("Install via {}", self.pm_name.unwrap_or("package manager"))
        } else {
            "Open installation page".to_string()
        };

        v_flex()
            .key_context("GhInstallModal")
            .track_focus(&self.focus_handle)
            .elevation_3(cx)
            .w_96()
            .overflow_hidden()
            .child(
                Modal::new("gh-install", None)
                    .header(
                        ModalHeader::new()
                            .headline("GitHub CLI is not installed")
                            .description(description)
                            .show_dismiss_button(true),
                    )
                    .footer(
                        ModalFooter::new().end_slot(
                            carrot_ui::h_flex()
                                .gap_1()
                                .child(Button::new("gh-install-cancel", "Cancel").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.cancel(window, cx);
                                    }),
                                ))
                                .child(
                                    Button::new("gh-install-confirm", install_label)
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.install(window, cx);
                                        })),
                                ),
                        ),
                    ),
            )
    }
}
