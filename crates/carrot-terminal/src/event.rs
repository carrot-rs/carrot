use std::io::Write;
use std::sync::{Arc, Mutex};

use carrot_term::event::Event as CarrotTermEvent;
use carrot_term::event::EventListener;

use crate::osc_parser::ShellMarker;

/// Events emitted by the terminal to the UI layer.
pub enum TerminalEvent {
    /// Terminal content changed, UI should repaint.
    Wakeup,
    /// Terminal title changed (via OSC escape sequence).
    Title(String),
    /// BEL character received.
    Bell,
    /// Shell process exited.
    Exit,
    /// Shell integration marker detected (OSC 133).
    ShellMarker(ShellMarker),
    /// Breadcrumb text changed (e.g., agent updated the terminal label).
    BreadcrumbsChanged,
}

/// Bridges carrot-term events to our TerminalEvent channel.
///
/// Also handles PtyWrite events by writing directly to the shared PTY writer.
pub struct CarrotEventListener {
    sender: flume::Sender<TerminalEvent>,
    pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl CarrotEventListener {
    pub fn new(
        sender: flume::Sender<TerminalEvent>,
        pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    ) -> Self {
        Self { sender, pty_writer }
    }
}

impl EventListener for CarrotEventListener {
    fn send_event(&self, event: CarrotTermEvent) {
        match event {
            CarrotTermEvent::Wakeup => {
                let _ = self.sender.send(TerminalEvent::Wakeup);
            }
            CarrotTermEvent::Title(title) => {
                let _ = self.sender.send(TerminalEvent::Title(title));
            }
            CarrotTermEvent::Bell => {
                let _ = self.sender.send(TerminalEvent::Bell);
            }
            CarrotTermEvent::Exit => {
                let _ = self.sender.send(TerminalEvent::Exit);
            }
            CarrotTermEvent::PtyWrite(text) => {
                if let Ok(mut writer) = self.pty_writer.lock() {
                    let _ = writer.write_all(text.as_bytes());
                }
            }
            _ => {}
        }
    }
}
