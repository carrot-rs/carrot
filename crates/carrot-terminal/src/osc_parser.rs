//! OSC scanner for shell-integration markers (OSC 133 + OSC 7777).
//!
//! The **marker types** (`ShellMarker`, `PromptKindType`,
//! `PositionedMarker`) live in `carrot-shell-integration` so both
//! this crate (scanner/parser) and `carrot-cmdline` (state-machine
//! consumer) share a single source of truth.
//!
//! Layer note: this crate must not depend on `carrot-cli-agents`.
//! Agent events are surfaced as `ShellMarker::AgentEvent(String)` —
//! the raw decoded-JSON envelope — and dispatched by higher layers.

pub use carrot_shell_integration::{PositionedMarker, PromptKindType, ShellMarker};

/// Scans a byte stream for OSC 133 and OSC 7777 shell integration markers.
///
/// The scanner is stateful — it handles OSC sequences that may be split
/// across multiple read() calls (e.g., `\x1b]133;` in one chunk, `A\x07`
/// in the next).
///
/// The scanned bytes are NOT modified. They should still be passed to
/// carrot_term which will ignore unknown OSC sequences.
pub struct OscScanner {
    state: ScanState,
    param_buf: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    /// Normal byte processing, not inside an OSC sequence.
    Normal,
    /// Saw ESC (0x1B), waiting for ] to confirm OSC start.
    SawEsc,
    /// Inside an OSC sequence, accumulating parameter bytes.
    InOsc,
    /// Inside OSC, saw ESC — waiting for \ (ST terminator).
    InOscSawEsc,
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            state: ScanState::Normal,
            param_buf: Vec::with_capacity(512),
        }
    }

    /// Scan a chunk of bytes for OSC 133 and OSC 7777 markers.
    ///
    /// Returns all markers found in this chunk with their byte positions.
    /// Positions are relative to the start of `bytes`.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<PositionedMarker> {
        let mut markers = Vec::new();
        let mut osc_start = 0usize;

        for (i, &byte) in bytes.iter().enumerate() {
            match self.state {
                ScanState::Normal => {
                    if byte == 0x1B {
                        osc_start = i;
                        self.state = ScanState::SawEsc;
                    } else if byte == 0x9D {
                        osc_start = i;
                        self.param_buf.clear();
                        self.state = ScanState::InOsc;
                    }
                }

                ScanState::SawEsc => {
                    if byte == b']' {
                        self.param_buf.clear();
                        self.state = ScanState::InOsc;
                    } else {
                        self.state = ScanState::Normal;
                    }
                }

                ScanState::InOsc => {
                    if byte == 0x07 {
                        if let Some(marker) = self.try_parse_osc() {
                            markers.push(PositionedMarker {
                                marker,
                                start: osc_start,
                                end: i + 1,
                            });
                        }
                        self.state = ScanState::Normal;
                    } else if byte == 0x1B {
                        self.state = ScanState::InOscSawEsc;
                    } else {
                        self.param_buf.push(byte);
                    }
                }

                ScanState::InOscSawEsc => {
                    if byte == b'\\' {
                        if let Some(marker) = self.try_parse_osc() {
                            markers.push(PositionedMarker {
                                marker,
                                start: osc_start,
                                end: i + 1,
                            });
                        }
                        self.state = ScanState::Normal;
                    } else {
                        self.param_buf.push(0x1B);
                        self.param_buf.push(byte);
                        self.state = ScanState::InOsc;
                    }
                }
            }
        }

        markers
    }

    /// Try parsing the accumulated OSC against every known sub-schema
    /// in priority order. Shell-metadata (subprefix `carrot-precmd;`)
    /// is tried before bare-hex, so legacy shell-integration payloads
    /// never get misrouted as agent events.
    fn try_parse_osc(&self) -> Option<ShellMarker> {
        self.parse_osc_133()
            .or_else(|| self.parse_osc_7777_metadata())
            .or_else(|| self.parse_osc_7777_tui_hint())
            .or_else(|| self.parse_osc_7777_agent())
            .or_else(|| self.parse_osc_1337_iterm2_image())
    }

    /// Parse `OSC 1337 ; File=key=value,...:base64-data` into
    /// [`ShellMarker::ImageInlineITerm2`]. The decoded marker carries
    /// the raw payload **after** `1337;` (i.e. starts with `File=`);
    /// `carrot_grid::parse_iterm2_payload` handles the rest.
    ///
    /// Reference: <https://iterm2.com/documentation-images.html>
    fn parse_osc_1337_iterm2_image(&self) -> Option<ShellMarker> {
        let params = &self.param_buf;
        let prefix = b"1337;File=";
        if params.len() <= prefix.len() || params[..prefix.len()] != *prefix {
            return None;
        }
        // Hand the rest of the OSC body — including `File=` — to the
        // grid layer's parser. Cloning here avoids borrowing from the
        // scanner's reusable buffer; the marker outlives a `.scan()`
        // call.
        Some(ShellMarker::ImageInlineITerm2(
            params[b"1337;".len()..].to_vec(),
        ))
    }

    /// Parse `OSC 7777;carrot-precmd;<hex>` into `ShellMarker::Metadata`.
    ///
    /// The payload after the prefix is hex-encoded JSON (two hex chars per byte).
    /// This encoding prevents bytes like 0x9C (ST terminator) in emoji/special
    /// chars from breaking the OSC escape sequence.
    fn parse_osc_7777_metadata(&self) -> Option<ShellMarker> {
        let params = &self.param_buf;
        let prefix = b"7777;carrot-precmd;";
        if params.len() <= prefix.len() || params[..prefix.len()] != *prefix {
            return None;
        }
        let hex_bytes = &params[prefix.len()..];
        let decoded = hex_decode(hex_bytes)?;
        let json = std::str::from_utf8(&decoded).ok()?;
        Some(ShellMarker::Metadata(json.to_string()))
    }

    /// Parse `OSC 7777;carrot-tui-hint;<hex>` into `ShellMarker::TuiHint`.
    ///
    /// Emitted by the shell preexec hook immediately before a known TUI
    /// command begins executing. Carries a small JSON payload (currently
    /// `{"tui_mode":true}`). Same hex-encoding as every other OSC 7777
    /// payload; sub-prefix `carrot-tui-hint;` distinguishes it from
    /// `carrot-precmd;` metadata and from bare agent events.
    fn parse_osc_7777_tui_hint(&self) -> Option<ShellMarker> {
        let params = &self.param_buf;
        let prefix = b"7777;carrot-tui-hint;";
        if params.len() <= prefix.len() || params[..prefix.len()] != *prefix {
            return None;
        }
        let hex_bytes = &params[prefix.len()..];
        let decoded = hex_decode(hex_bytes)?;
        let json = std::str::from_utf8(&decoded).ok()?;
        Some(ShellMarker::TuiHint(json.to_string()))
    }

    /// Parse `OSC 7777;<hex>` into `ShellMarker::AgentEvent` when the
    /// decoded JSON is an agent-event envelope.
    ///
    /// This path intentionally rejects any payload whose decoded JSON
    /// does not carry `"type":"cli_agent_event"` — that keeps the
    /// router forward-compatible with other OSC-7777 sub-schemas that
    /// may land later. We check for the literal substring rather than
    /// parsing JSON here: parsing belongs in `carrot-cli-agents`, and
    /// a substring check is sufficient to discriminate.
    fn parse_osc_7777_agent(&self) -> Option<ShellMarker> {
        let params = &self.param_buf;
        let prefix = b"7777;";
        if params.len() <= prefix.len() || params[..prefix.len()] != *prefix {
            return None;
        }
        let hex_bytes = &params[prefix.len()..];
        let decoded = hex_decode(hex_bytes)?;
        let json = std::str::from_utf8(&decoded).ok()?;
        if !json.contains("\"type\":\"cli_agent_event\"")
            && !json.contains("\"type\": \"cli_agent_event\"")
        {
            return None;
        }
        Some(ShellMarker::AgentEvent(json.to_string()))
    }

    /// Parse accumulated OSC parameters to check for 133;X markers.
    fn parse_osc_133(&self) -> Option<ShellMarker> {
        let params = &self.param_buf;

        // Must start with "133;"
        if params.len() < 4 || &params[..4] != b"133;" {
            return None;
        }

        let rest = &params[4..];
        if rest.is_empty() {
            return None;
        }

        match rest[0] {
            b'A' => Some(ShellMarker::PromptStart),
            b'B' => Some(ShellMarker::InputStart),
            b'C' => Some(ShellMarker::CommandStart),
            b'D' => {
                // Parse exit code from "D;N" or "D" (default 0)
                let exit_code = if rest.len() > 2 && rest[1] == b';' {
                    std::str::from_utf8(&rest[2..])
                        .ok()
                        .and_then(|s| {
                            // Exit code may have additional params like ";aid=123"
                            s.split(';').next().and_then(|code| code.parse().ok())
                        })
                        .unwrap_or(0)
                } else {
                    0
                };
                Some(ShellMarker::CommandEnd { exit_code })
            }
            b'P' => {
                // OSC 133;P;k=<kind> — Nushell prompt kind
                let kind = if rest.len() > 4 && &rest[1..4] == b";k=" {
                    match rest[4] {
                        b'i' => PromptKindType::Initial,
                        b'c' => PromptKindType::Continuation,
                        b's' => PromptKindType::Secondary,
                        b'r' => PromptKindType::Right,
                        _ => PromptKindType::Initial,
                    }
                } else {
                    PromptKindType::Initial
                };
                Some(ShellMarker::PromptKind { kind })
            }
            _ => None,
        }
    }
}

/// Decode a hex-encoded byte slice (e.g., b"48656c6c6f" → b"Hello").
/// Returns None if the input has odd length or contains non-hex characters.
fn hex_decode(input: &[u8]) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for pair in input.chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract just the ShellMarker values from positioned markers for assertion convenience.
    fn marker_types(markers: &[PositionedMarker]) -> Vec<ShellMarker> {
        markers.iter().map(|m| m.marker.clone()).collect()
    }

    #[test]
    fn test_scan_prompt_start() {
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(b"\x1b]133;A\x07");
        assert_eq!(marker_types(&markers), vec![ShellMarker::PromptStart]);
    }

    #[test]
    fn test_scan_tui_hint() {
        // Hex-encoded `{"tui_mode":true}` should decode to ShellMarker::TuiHint.
        let payload = br#"{"tui_mode":true}"#;
        let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
        let input = format!("\x1b]7777;carrot-tui-hint;{hex}\x07");

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(input.as_bytes());
        let types = marker_types(&markers);
        assert_eq!(types.len(), 1);
        match &types[0] {
            ShellMarker::TuiHint(json) => {
                assert_eq!(json, r#"{"tui_mode":true}"#);
            }
            other => panic!("expected TuiHint, got {other:?}"),
        }
    }

    #[test]
    fn test_scan_tui_hint_does_not_match_metadata_path() {
        // Carrot-precmd payload must not be routed to TuiHint.
        let payload = br#"{"cwd":"/tmp"}"#;
        let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
        let input = format!("\x1b]7777;carrot-precmd;{hex}\x07");

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(input.as_bytes());
        let types = marker_types(&markers);
        assert!(
            matches!(types[0], ShellMarker::Metadata(_)),
            "precmd payload must parse as Metadata, not TuiHint"
        );
    }

    #[test]
    fn test_scan_tui_hint_rejects_malformed_hex() {
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(b"\x1b]7777;carrot-tui-hint;nothex!\x07");
        assert!(
            marker_types(&markers).is_empty(),
            "malformed hex must be ignored"
        );
    }

    #[test]
    fn test_scan_command_end_with_exit_code() {
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(b"\x1b]133;D;127\x07");
        assert_eq!(
            marker_types(&markers),
            vec![ShellMarker::CommandEnd { exit_code: 127 }]
        );
    }

    #[test]
    fn test_scan_multiple_markers() {
        let mut scanner = OscScanner::new();
        let input = b"hello\x1b]133;A\x07world\x1b]133;C\x07done";
        let markers = scanner.scan(input);
        assert_eq!(
            marker_types(&markers),
            vec![ShellMarker::PromptStart, ShellMarker::CommandStart]
        );
    }

    #[test]
    fn test_scan_split_across_chunks() {
        let mut scanner = OscScanner::new();
        // Split the sequence across two reads
        let m1 = scanner.scan(b"text\x1b]133;");
        assert!(m1.is_empty());
        let m2 = scanner.scan(b"D;42\x07more");
        assert_eq!(
            marker_types(&m2),
            vec![ShellMarker::CommandEnd { exit_code: 42 }]
        );
    }

    #[test]
    fn test_scan_st_terminator() {
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(b"\x1b]133;B\x1b\\");
        assert_eq!(marker_types(&markers), vec![ShellMarker::InputStart]);
    }

    #[test]
    fn test_scan_ignores_other_osc() {
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(b"\x1b]0;window title\x07");
        assert!(markers.is_empty());
    }

    // --- OSC 7777 (carrot-precmd metadata) tests ---

    fn hex_encode(input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() * 2);
        for &b in input {
            out.push(b"0123456789abcdef"[(b >> 4) as usize]);
            out.push(b"0123456789abcdef"[(b & 0x0f) as usize]);
        }
        out
    }

    #[test]
    fn test_scan_osc_7777_basic() {
        let json = br#"{"cwd":"/tmp","username":"nyxb"}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;carrot-precmd;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(
            marker_types(&markers),
            vec![ShellMarker::Metadata(
                r#"{"cwd":"/tmp","username":"nyxb"}"#.to_string()
            )]
        );
    }

    #[test]
    fn test_scan_osc_7777_st_terminator() {
        let json = br#"{"cwd":"/"}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;carrot-precmd;".to_vec();
        seq.extend_from_slice(&hex);
        seq.extend_from_slice(b"\x1b\\");

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(
            marker_types(&markers),
            vec![ShellMarker::Metadata(r#"{"cwd":"/"}"#.to_string())]
        );
    }

    #[test]
    fn test_scan_osc_7777_split_across_chunks() {
        let json = br#"{"cwd":"/home"}"#;
        let hex = hex_encode(json);
        let mut full = b"\x1b]7777;carrot-precmd;".to_vec();
        full.extend_from_slice(&hex);
        full.push(0x07);

        // Split at an arbitrary midpoint
        let mid = full.len() / 2;
        let mut scanner = OscScanner::new();
        let m1 = scanner.scan(&full[..mid]);
        assert!(m1.is_empty());
        let m2 = scanner.scan(&full[mid..]);
        assert_eq!(
            marker_types(&m2),
            vec![ShellMarker::Metadata(r#"{"cwd":"/home"}"#.to_string())]
        );
    }

    #[test]
    fn test_scan_osc_7777_interleaved_with_133() {
        let json = br#"{"cwd":"/"}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;carrot-precmd;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);
        seq.extend_from_slice(b"\x1b]133;A\x07");

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(
            marker_types(&markers),
            vec![
                ShellMarker::Metadata(r#"{"cwd":"/"}"#.to_string()),
                ShellMarker::PromptStart,
            ]
        );
    }

    #[test]
    fn test_scan_osc_7777_invalid_hex() {
        // Odd-length hex should be ignored
        let mut seq = b"\x1b]7777;carrot-precmd;abc\x07".to_vec();
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert!(markers.is_empty());

        // Non-hex characters
        seq = b"\x1b]7777;carrot-precmd;ZZZZ\x07".to_vec();
        let markers = scanner.scan(&seq);
        assert!(markers.is_empty());
    }

    #[test]
    fn test_hex_decode_roundtrip() {
        let original = b"Hello, World!";
        let encoded = hex_encode(original);
        let decoded = super::hex_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    // --- OSC 7777 (cli_agent_event) tests ---

    #[test]
    fn test_scan_osc_7777_agent_event_basic() {
        let json = br#"{"type":"cli_agent_event","agent":"claude_code","protocol_version":1,"event":"Stop","payload":{"session_id":"s1"}}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(markers.len(), 1);
        match &markers[0].marker {
            ShellMarker::AgentEvent(json_str) => {
                assert!(json_str.contains("\"event\":\"Stop\""));
                assert!(json_str.contains("\"session_id\":\"s1\""));
            }
            other => panic!("expected AgentEvent, got {:?}", other),
        }
    }

    #[test]
    fn test_scan_osc_7777_agent_event_tolerates_whitespace_in_type() {
        // The plugin's emit script may emit compact or pretty-printed
        // JSON depending on shell quirks; tolerate the spaced form.
        let json = br#"{ "type": "cli_agent_event", "agent": "x", "protocol_version": 1, "event": "Stop", "payload": {} }"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(markers.len(), 1);
        assert!(matches!(markers[0].marker, ShellMarker::AgentEvent(_)));
    }

    #[test]
    fn test_scan_osc_7777_bare_hex_without_agent_type_is_ignored() {
        // Any bare-hex OSC-7777 payload whose decoded JSON lacks the
        // cli_agent_event type must NOT be routed as an agent event.
        // This keeps the wire format forward-compatible for future
        // sub-schemas that pick different `type` strings.
        let json = br#"{"type":"shell_context_v2","data":{}}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert!(
            markers.is_empty(),
            "unknown bare-hex sub-schema must produce no marker"
        );
    }

    #[test]
    fn test_scan_osc_7777_precmd_takes_priority_over_agent() {
        // A payload with the carrot-precmd; sub-prefix must route as
        // Metadata, never as AgentEvent — even if the decoded JSON
        // happens to contain the cli_agent_event literal.
        let json = br#"{"cwd":"/","type":"cli_agent_event"}"#;
        let hex = hex_encode(json);
        let mut seq = b"\x1b]7777;carrot-precmd;".to_vec();
        seq.extend_from_slice(&hex);
        seq.push(0x07);

        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert_eq!(markers.len(), 1);
        assert!(
            matches!(markers[0].marker, ShellMarker::Metadata(_)),
            "carrot-precmd; sub-prefix must route as Metadata"
        );
    }

    #[test]
    fn test_scan_osc_7777_agent_event_split_across_chunks() {
        let json = br#"{"type":"cli_agent_event","agent":"x","protocol_version":1,"event":"Stop","payload":{"session_id":"s"}}"#;
        let hex = hex_encode(json);
        let mut full = b"\x1b]7777;".to_vec();
        full.extend_from_slice(&hex);
        full.push(0x07);
        let mid = full.len() / 2;

        let mut scanner = OscScanner::new();
        let m1 = scanner.scan(&full[..mid]);
        assert!(m1.is_empty());
        let m2 = scanner.scan(&full[mid..]);
        assert_eq!(m2.len(), 1);
        assert!(matches!(m2[0].marker, ShellMarker::AgentEvent(_)));
    }

    #[test]
    fn test_scan_osc_7777_agent_event_invalid_hex() {
        // Odd-length hex in bare form is rejected.
        let mut seq = b"\x1b]7777;abc\x07".to_vec();
        let mut scanner = OscScanner::new();
        let markers = scanner.scan(&seq);
        assert!(markers.is_empty());

        // Non-hex chars in bare form are also rejected.
        seq = b"\x1b]7777;ZZZZ\x07".to_vec();
        let markers = scanner.scan(&seq);
        assert!(markers.is_empty());
    }
}
