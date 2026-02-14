use carrot_ui::{
    Button, ButtonVariants, Context, IntoElement, Modal, ModalFooter, ModalHeader, ParentElement,
    Render, Styled, StyledExt, v_flex,
};
use carrot_workspace::ModalView;
/// Modal offering to run `gh auth login` when `gh` is installed but not
/// authenticated. Shown only after `check_gh_installed()` returned true
/// and `check_gh_authenticated()` returned false — the previous install
/// modal has already closed (or never appeared because gh was already
/// present).
use inazuma::{
    App, DismissEvent, EventEmitter, FocusHandle, Focusable, InteractiveElement, Window,
};

use crate::gh::install_modal::GhPromptDismissed;

const GH_AUTH_COMMAND: &str = "gh auth login";

pub struct GhAuthModal {
    terminal_handle: carrot_terminal::TerminalHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for GhAuthModal {}

impl ModalView for GhAuthModal {
    fn fade_out_background(&self) -> bool {
        true
    }
}

impl Focusable for GhAuthModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GhAuthModal {
    pub fn new(
        terminal_handle: carrot_terminal::TerminalHandle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            terminal_handle,
            focus_handle: cx.focus_handle(),
        }
    }

    fn run(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        {
            let mut term = self.terminal_handle.lock();
            term.set_pending_block_command(GH_AUTH_COMMAND.to_string());
        }
        let mut bytes = GH_AUTH_COMMAND.as_bytes().to_vec();
        bytes.push(b'\r');
        self.terminal_handle.write(&bytes);
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Same dismissal semantics as the install modal — if the user
        // cancels either modal, stop offering until they opt back in.
        cx.global_mut::<GhPromptDismissed>().0 = true;
        cx.emit(DismissEvent);
    }
}

impl Render for GhAuthModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("GhAuthModal")
            .track_focus(&self.focus_handle)
            .elevation_3(cx)
            .w_96()
            .overflow_hidden()
            .child(
                Modal::new("gh-auth", None)
                    .header(
                        ModalHeader::new()
                            .headline("GitHub CLI is not authenticated")
                            .description(
                                "Run `gh auth login` to let Carrot look up pull \
                                 requests for the current branch. The command runs \
                                 in your active terminal so you can complete the \
                                 browser-based login flow.",
                            )
                            .show_dismiss_button(true),
                    )
                    .footer(
                        ModalFooter::new().end_slot(
                            carrot_ui::h_flex()
                                .gap_1()
                                .child(Button::new("gh-auth-cancel", "Cancel").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.cancel(window, cx);
                                    }),
                                ))
                                .child(
                                    Button::new("gh-auth-run", "Run gh auth login")
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.run(window, cx);
                                        })),
                                ),
                        ),
                    ),
            )
    }
}
