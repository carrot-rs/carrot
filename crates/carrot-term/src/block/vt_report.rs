//! Typed VT reports flowing from the handler back to the PTY master.
//!
//! Certain escape sequences (DA — Device Attributes, DSR — Device
//! Status Report, mode reports) require the terminal to write a reply
//! back to the PTY. The VT handler cannot perform the write itself:
//! Layer 2 is decoupled from Layer 1 I/O, and the PTY master is owned
//! by the session layer.
//!
//! `VtReport` is the concrete message. The handler's owner (the
//! `Term` wrapper, one layer up) provides a `crossbeam::channel::
//! Sender<VtReport>` to the `VtWriter` constructor. Handler methods
//! push reports without blocking; a receiver task on the session
//! side drains the channel, formats each variant into its ESC
//! sequence, and writes the bytes to the PTY master.
//!
//! Typed channels here instead of a trait object deliberately: the
//! project forbids `dyn` for cross-layer communication, and a
//! channel is concrete, `Send`, and testable in isolation — unit
//! tests construct a `(tx, rx)` pair, feed the writer, and inspect
//! `rx.try_iter()`.

use crossbeam::channel::{Sender, TrySendError};

/// Maximum pending reports. If the session-side receiver falls
/// behind beyond this, newer reports are dropped — the VT handler
/// is already ahead of the PTY in that scenario, and losing reports
/// is preferable to blocking the parser.
pub const REPORT_CHANNEL_CAPACITY: usize = 256;

/// A VT response destined for the PTY master. Each variant maps to
/// a concrete ESC sequence the session-side writer formats before
/// sending to the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtReport {
    /// Reply to `CSI c` — primary device attributes. The writer
    /// formats the usual `ESC [ ? 1 ; 2 c` (VT100 with AVO).
    DeviceAttributes,
    /// Reply to `CSI > c` — secondary device attributes. Writer
    /// formats a terminal-id + version reply.
    DeviceAttributesSecondary,
    /// Reply to `CSI n` with value 6 — current cursor position.
    /// Writer formats `ESC [ row ; col R` using the handler's
    /// current cursor location. Rows and columns are 1-based per
    /// the VT spec.
    CursorPosition { row: u16, col: u16 },
    /// Reply to `CSI n` with value 5 — operating status. Writer
    /// formats `ESC [ 0 n` ("terminal OK").
    TerminalStatus,
    /// Reply to `CSI ? n` / `CSI n` — mode status report. `state`
    /// encodes the three DECRQM answers (set/reset/unknown).
    ModeReport {
        mode: u16,
        state: ModeReportState,
        private: bool,
    },
    /// Identify-terminal request (`ESC Z`). Writer formats the
    /// appropriate response for the terminal type.
    IdentifyTerminal,
}

/// Three-state answer for mode reports per VT spec. DECRQM reports
/// `0` for "mode not recognised", `1` for "set", `2` for "reset",
/// `3` for "permanently set", `4` for "permanently reset" — we
/// collapse those into the common trio since the terminal doesn't
/// expose permanently-fixed modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeReportState {
    NotRecognised,
    Set,
    Reset,
}

impl ModeReportState {
    /// VT-spec numeric code for serialising into the response.
    pub const fn code(self) -> u16 {
        match self {
            Self::NotRecognised => 0,
            Self::Set => 1,
            Self::Reset => 2,
        }
    }
}

/// Dispatcher that takes a sender and offers cheap, non-panicking
/// `push` helpers. Wraps the low-level channel so handlers don't
/// carry the error-handling boilerplate at every call site.
#[derive(Debug, Clone)]
pub struct VtReportSink {
    sender: Sender<VtReport>,
}

impl VtReportSink {
    /// Wrap an existing channel sender. Callers typically build the
    /// `(Sender, Receiver)` pair with [`crossbeam::channel::bounded`]
    /// at `REPORT_CHANNEL_CAPACITY` and hand the sender to the VT
    /// writer.
    pub fn new(sender: Sender<VtReport>) -> Self {
        Self { sender }
    }

    /// Enqueue a report. Drops silently on a full or closed channel
    /// — the VT handler must not block and must not panic on
    /// shutdown-race conditions.
    pub fn push(&self, report: VtReport) {
        match self.sender.try_send(report) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                log::warn!(
                    target: "carrot_term::vt",
                    "VtReport channel full; dropping report"
                );
            }
        }
    }
}

/// Convenience: build a fresh bounded channel sized for VT report
/// traffic. Returns the dispatcher + the receiving end for the
/// session task to drain.
pub fn bounded() -> (VtReportSink, crossbeam::channel::Receiver<VtReport>) {
    let (tx, rx) = crossbeam::channel::bounded(REPORT_CHANNEL_CAPACITY);
    (VtReportSink::new(tx), rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_report_state_codes_match_vt_spec() {
        assert_eq!(ModeReportState::NotRecognised.code(), 0);
        assert_eq!(ModeReportState::Set.code(), 1);
        assert_eq!(ModeReportState::Reset.code(), 2);
    }

    #[test]
    fn sink_push_delivers_via_channel() {
        let (sink, rx) = bounded();
        sink.push(VtReport::DeviceAttributes);
        sink.push(VtReport::CursorPosition { row: 3, col: 7 });
        assert_eq!(rx.try_recv().unwrap(), VtReport::DeviceAttributes);
        assert_eq!(
            rx.try_recv().unwrap(),
            VtReport::CursorPosition { row: 3, col: 7 },
        );
    }

    #[test]
    fn sink_push_drops_when_channel_full() {
        let (tx, rx) = crossbeam::channel::bounded(2);
        let sink = VtReportSink::new(tx);
        sink.push(VtReport::DeviceAttributes);
        sink.push(VtReport::TerminalStatus);
        // Third push lands on a full channel — dropped silently.
        sink.push(VtReport::IdentifyTerminal);
        assert_eq!(rx.len(), 2);
    }

    #[test]
    fn sink_push_tolerates_closed_receiver() {
        let (sink, rx) = bounded();
        drop(rx);
        // Receiver gone — push must not panic.
        sink.push(VtReport::DeviceAttributes);
    }

    #[test]
    fn channel_capacity_matches_declared_constant() {
        let (tx, _rx) = crossbeam::channel::bounded::<VtReport>(REPORT_CHANNEL_CAPACITY);
        assert_eq!(tx.capacity(), Some(REPORT_CHANNEL_CAPACITY));
    }
}
