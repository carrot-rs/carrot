//! Command-line input handling + rendering for [`TerminalPane`].
//!
//! Hosts the `carrot-ui::Input` integration: key events (Enter, up/down
//! history), value mutations, and the chip + input-row composition
//! that sits beneath the block list.
//!
//! The TUI input path (alt-screen key feeding) lives in
//! `terminal_pane.rs` directly — it shares state with the shell
//! signal handling and is better kept adjacent to the render method
//! for now. This module owns the **shell-input-bar** surface
//! exclusively.

use carrot_completions::command_correction;
use carrot_ui::{
    h_flex,
    input::{Input, InputEvent, InputState},
};
use inazuma::{
    App, Context, Entity, IntoElement, Oklch, ParentElement, Styled, Window, div, oklcha, px, rgb,
};

use crate::terminal_pane::TerminalPane;

impl TerminalPane {
    pub(crate) fn on_input_event(
        &mut self,
        _state: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { secondary: false } => {
                if self.history_panel.is_visible() {
                    self.history_panel.close();
                }

                let value = self.input_state.read(cx).value();
                if !value.is_empty() {
                    if let Ok(mut hist) = self.command_history.write() {
                        hist.push(value.to_string());
                    }
                    {
                        let handle = self.terminal.handle();
                        let mut term = handle.lock();
                        term.set_pending_block_command(value.to_string());
                    }
                    let mut bytes = value.as_bytes().to_vec();
                    bytes.push(b'\r');
                    self.terminal.write(&bytes);
                    self.show_terminal = true;
                    self.input_state.update(cx, |state, cx| {
                        state.set_value("", window, cx);
                    });
                    self.focus_handle.focus(window, cx);
                    cx.notify();
                }
            }
            InputEvent::HistoryUp => {
                if !self.history_panel.is_visible() {
                    let current = self.input_state.read(cx).value().to_string();
                    if let Ok(hist) = self.command_history.read() {
                        self.history_panel.open(&hist, &current);
                    }
                } else {
                    self.history_panel.select_previous();
                }
                if let Some(cmd) = self.history_panel.selected_command() {
                    let cmd = cmd.to_string();
                    self.input_state.update(cx, |state, cx| {
                        state.set_value(&cmd, window, cx);
                    });
                }
                cx.notify();
            }
            InputEvent::HistoryDown => {
                if self.history_panel.is_visible() {
                    if self.history_panel.is_at_bottom() {
                        let saved = self.history_panel.close();
                        self.input_state.update(cx, |state, cx| {
                            state.set_value(&saved, window, cx);
                        });
                    } else {
                        self.history_panel.select_next();
                        if let Some(cmd) = self.history_panel.selected_command() {
                            let cmd = cmd.to_string();
                            self.input_state.update(cx, |state, cx| {
                                state.set_value(&cmd, window, cx);
                            });
                        }
                    }
                    cx.notify();
                }
            }
            InputEvent::Change => {
                if self.history_panel.is_visible() {
                    let query = self.input_state.read(cx).value().to_string();
                    if let Ok(hist) = self.command_history.read() {
                        self.history_panel.filter(&query, &hist);
                    }
                    cx.notify();
                }
                self.update_input_highlights(cx);
            }
            _ => {}
        }
    }

    pub(crate) fn update_input_highlights(&self, cx: &mut Context<Self>) {
        let text = self.input_state.read(cx).value().to_string();
        let completion_range = self.input_state.read(cx).completion_inserted_range.clone();

        let trimmed = text.trim_start();
        if trimmed.is_empty() {
            self.input_state.update(cx, |state, _| {
                state.overlay_highlights.clear();
            });
            return;
        }

        let cmd_end = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let cmd = &trimmed[..cmd_end];
        let cmd_start = text.len() - trimmed.len();

        let is_valid = if let Ok(executables) = self.shell_completion.path_executables.read() {
            executables.iter().any(|e| e == cmd)
        } else {
            false
        } || matches!(
            cmd,
            "cd" | "echo"
                | "export"
                | "source"
                | "alias"
                | "unalias"
                | "type"
                | "which"
                | "eval"
                | "exec"
                | "set"
                | "unset"
                | "pwd"
                | "pushd"
                | "popd"
                | "dirs"
                | "bg"
                | "fg"
                | "jobs"
                | "kill"
                | "wait"
                | "trap"
                | "umask"
                | "test"
                | "true"
                | "false"
                | "readonly"
                | "shift"
        );

        self.input_state.update(cx, |state, _| {
            state.overlay_highlights.clear();

            if is_valid {
                state.overlay_highlights.push((
                    cmd_start..cmd_start + cmd_end,
                    inazuma::HighlightStyle {
                        color: Some(oklcha(0.75, 0.12, 220.0, 1.0)),
                        ..Default::default()
                    },
                ));
            }

            if let Some(range) = completion_range {
                let clamped_end = range.end.min(text.len());
                if range.start < clamped_end {
                    state.overlay_highlights.push((
                        range.start..clamped_end,
                        inazuma::HighlightStyle {
                            color: Some(oklcha(0.75, 0.12, 220.0, 0.6)),
                            ..Default::default()
                        },
                    ));
                }
            }
        });
    }

    pub(crate) fn render_input_area(&self, window: &mut Window, cx: &App) -> impl IntoElement {
        let ctx = self.build_chip_context(cx);
        let render_ctx = carrot_chips::ChipRenderContext {
            workspace: self
                .workspace
                .as_ref()
                .map(|ws| Box::new(ws.clone()) as Box<dyn std::any::Any>),
            cwd: self.shell_context.cwd.clone(),
        };
        let elements = self.chip_registry.render_all(&ctx, &render_ctx, window, cx);

        let chips = h_flex()
            .gap(px(6.0))
            .px_4()
            .pt_2()
            .flex_wrap()
            .children(elements);

        div()
            .flex_shrink_0()
            .w_full()
            .border_color(Oklch::white().opacity(0.08))
            .border_t_1()
            .child(chips)
            .child(
                div().px_1().pt_1().pb_3().child(
                    Input::new(&self.input_state)
                        .appearance(false)
                        .cleanable(false),
                ),
            )
    }

    pub(crate) fn render_correction_banner(
        &self,
        correction: &command_correction::CorrectionResult,
        theme: &carrot_theme::Theme,
    ) -> impl IntoElement {
        let original = correction.original.clone();
        let suggestion = correction.suggestion.clone();
        let confidence_pct = (correction.confidence * 100.0) as u32;
        div()
            .flex()
            .items_center()
            .w_full()
            .h(px(32.0))
            .px_4()
            .gap_2()
            .bg(Oklch::white().opacity(0.04))
            .border_t_1()
            .border_b_1()
            .border_color(Oklch::white().opacity(0.08))
            .text_xs()
            .child(div().text_color(rgb(0x666666)).child(original))
            .child(div().text_color(rgb(0xffaa00)).child("→"))
            .child(
                div()
                    .px_2()
                    .py(px(2.0))
                    .bg(Oklch::white().opacity(0.08))
                    .rounded(px(4.0))
                    .text_color(crate::constants::accent_color(theme))
                    .child(format!("{} ({}%)", suggestion, confidence_pct)),
            )
            .child(div().text_color(rgb(0x666666)).child("? Press ↵ to run"))
    }
}
