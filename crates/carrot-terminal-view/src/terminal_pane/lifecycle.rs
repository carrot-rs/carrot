//! Focus + key-dispatch lifecycle handlers on [`TerminalPane`].
//!
//! `pane_changed` + `on_removed` live in `item.rs` with the rest of
//! the `Item` trait impl; this module carries the keystroke-dispatch
//! surface (cmd+c / cmd+k / escape + alt-screen key feeding) plus
//! the alt-screen mode observer.

use carrot_term::term::TermMode;
use inazuma::{Context, KeyDownEvent, Window};

use crate::terminal_pane::{TerminalPane, keystroke_to_bytes};

impl TerminalPane {
    /// Mirror the terminal's alt-screen flag onto `interactive_mode`.
    /// Called from the wakeup-event path — keeps key dispatch aware
    /// of whether a TUI app owns the screen.
    pub(crate) fn update_interactive_mode(&mut self) {
        let handle = self.terminal.handle();
        let term = handle.lock();
        self.interactive_mode = term.mode().contains(TermMode::ALT_SCREEN);
    }

    /// Forward Ctrl+C (terminal::SendInterrupt) to the PTY as ETX
    /// (0x03). The kernel's line discipline translates ETX on the
    /// PTY master side into SIGINT for the foreground process, so
    /// `claude`, `vim`, or any other TUI hosted in Carrot is cleanly
    /// interruptible. Emitting 0x03 unconditionally matches real-
    /// terminal semantics: if no process is currently reading input,
    /// the shell echoes `^C` and prints a fresh prompt — the user
    /// never ends up "stuck".
    pub(crate) fn on_send_interrupt(
        &mut self,
        _: &crate::terminal_panel::SendInterrupt,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.write(&[0x03]);
        cx.notify();
    }

    /// KeyDown handler used while the pane is in interactive mode
    /// (alt-screen TUI active or a shell command running). Routes
    /// Cmd-shortcuts first, then feeds remaining keys to the PTY.
    pub(crate) fn on_key_down_interactive(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cmd+K: Clear all blocks
        if event.keystroke.key.as_str() == "k" && event.keystroke.modifiers.platform {
            let handle = self.terminal.handle();
            let mut term = handle.lock();
            term.block_router_mut().on_prompt_start();
            drop(term);
            self.block_list.update(cx, |view, _cx| view.clear());
            cx.notify();
            return;
        }

        // Cmd+C → copy text selection or selected block command.
        if event.keystroke.key.as_str() == "c" && event.keystroke.modifiers.platform {
            let text = self.block_list.read(cx).copy_selection_text();
            if let Some(text) = text {
                cx.write_to_clipboard(inazuma::ClipboardItem::new_string(text));
                cx.notify();
                return;
            }
        }

        if event.keystroke.key.as_str() == "escape" {
            if self.history_panel.is_visible() {
                let saved = self.history_panel.close();
                self.input_state.update(cx, |state, cx| {
                    state.set_value(&saved, window, cx);
                });
                cx.notify();
                return;
            }
            if self.block_list.read(cx).selected_block().is_some() {
                self.block_list
                    .update(cx, |view, _cx| view.set_selected_block(None));
                cx.notify();
                return;
            }
        }

        // Shift+PageUp / Shift+PageDown: scrollback navigation via the
        // router-level `DisplayState`. The Inazuma block-list handles
        // visual scrolling; this updates the VT-side display_offset so
        // TUI apps that query cursor position (e.g. `less` inside the
        // terminal) see the correct viewport.
        if event.keystroke.modifiers.shift
            && (event.keystroke.key.as_str() == "pageup"
                || event.keystroke.key.as_str() == "pagedown")
        {
            let handle = self.terminal.handle();
            let mut term = handle.lock();
            let viewport_rows = term.screen_lines();
            let max_offset = term
                .block_router()
                .entries()
                .iter()
                .map(|e| e.total_rows())
                .sum::<usize>()
                .saturating_sub(viewport_rows);
            let scroll = if event.keystroke.key.as_str() == "pageup" {
                carrot_term::block::Scroll::PageUp
            } else {
                carrot_term::block::Scroll::PageDown
            };
            term.block_router_mut()
                .scroll_display(scroll, viewport_rows, max_offset);
            drop(term);
            cx.notify();
            return;
        }

        let command_running = {
            let handle = self.terminal.handle();
            let term = handle.lock();
            term.block_router().has_active_block()
        };

        if !self.interactive_mode && !command_running {
            return;
        }

        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            return;
        }

        let bytes = keystroke_to_bytes(keystroke, &self.terminal);
        if !bytes.is_empty() {
            self.terminal.write(&bytes);
            cx.notify();
        }
    }
}
