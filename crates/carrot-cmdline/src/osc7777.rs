//! Carrot-native OSC 7777 sidecar metadata.
//!
//! OSC 133 tells the terminal *when* a block starts / ends. OSC
//! 7777 tells it *what* — rich structured metadata emitted by the
//! shell integration alongside the lifecycle markers:
//!
//! ```text
//! ESC ] 7777 ; <hex-encoded JSON> BEL
//! ```
//!
//! JSON payload (all fields optional):
//!
//! ```json
//! {
//!   "cwd":    "/home/nyxb/projects/carrot",
//!   "git":    { "branch": "main", "dirty": false, "ahead": 0, "behind": 0 },
//!   "user":   "nyxb",
//!   "host":   "moonbox",
//!   "shell":  "nu",
//!   "exit":   0,
//!   "runtime_ms": 247
//! }
//! ```
//!
//! Carrot's shell hooks emit this at PromptStart (pre-fill for
//! the upcoming block's header). This module:
//!
//! - Defines the typed [`BlockMetadata`] payload.
//! - Parses the hex-encoded JSON into the struct.
//! - Encodes a payload back to the wire format for round-tripping
//!   (tests + shell-hook unit-testing).
//!
//! Why 7777: the sequence number is free, unused by any other
//! convention, memorable.
//!
//! # Scope
//!
//! No JSON library dependency — we hand-roll a minimal parser that
//! handles the exact subset Carrot's hooks emit. The `serde_json`
//! upgrade can land later when the crate grows a JSON use beyond
//! this one.

use std::num::ParseIntError;

/// Typed metadata the shell hook attaches to a block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockMetadata {
    pub cwd: Option<String>,
    pub git: Option<GitInfo>,
    pub user: Option<String>,
    pub host: Option<String>,
    pub shell: Option<String>,
    /// Present on CommandEnd; `None` at PromptStart.
    pub exit: Option<i32>,
    /// Command runtime in milliseconds. Present on CommandEnd.
    pub runtime_ms: Option<u64>,
}

/// Git view at the time the hook fired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub dirty: Option<bool>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

/// Error producing structured metadata from the wire format.
#[derive(Debug, PartialEq, Eq)]
pub enum Osc7777ParseError {
    /// Hex decode failed.
    Hex(String),
    /// JSON structure malformed (unterminated string, unexpected
    /// char, unbalanced braces).
    Json(&'static str),
}

impl std::fmt::Display for Osc7777ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Osc7777ParseError::Hex(msg) => write!(f, "hex decode failed: {msg}"),
            Osc7777ParseError::Json(msg) => write!(f, "JSON parse error: {msg}"),
        }
    }
}

impl std::error::Error for Osc7777ParseError {}

impl From<ParseIntError> for Osc7777ParseError {
    fn from(e: ParseIntError) -> Self {
        Osc7777ParseError::Hex(e.to_string())
    }
}

/// Parse a hex-encoded JSON payload into typed metadata.
pub fn parse(hex_payload: &str) -> Result<BlockMetadata, Osc7777ParseError> {
    let bytes = decode_hex(hex_payload)?;
    let json =
        String::from_utf8(bytes).map_err(|_| Osc7777ParseError::Hex("invalid utf8".into()))?;
    parse_json(&json)
}

/// Encode typed metadata into hex-encoded JSON wire format.
pub fn encode(metadata: &BlockMetadata) -> String {
    let json = encode_json(metadata);
    encode_hex(json.as_bytes())
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, Osc7777ParseError> {
    if !hex.len().is_multiple_of(2) {
        return Err(Osc7777ParseError::Hex("odd-length string".into()));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let s =
            std::str::from_utf8(chunk).map_err(|_| Osc7777ParseError::Hex("non-ascii".into()))?;
        out.push(u8::from_str_radix(s, 16)?);
    }
    Ok(out)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn parse_json(input: &str) -> Result<BlockMetadata, Osc7777ParseError> {
    let mut p = Parser::new(input);
    p.expect_char('{')?;
    let mut md = BlockMetadata::default();
    loop {
        p.skip_ws();
        if p.peek() == Some('}') {
            p.advance();
            break;
        }
        let key = p.parse_string()?;
        p.skip_ws();
        p.expect_char(':')?;
        p.skip_ws();
        match key.as_str() {
            "cwd" => md.cwd = Some(p.parse_string()?),
            "user" => md.user = Some(p.parse_string()?),
            "host" => md.host = Some(p.parse_string()?),
            "shell" => md.shell = Some(p.parse_string()?),
            "exit" => md.exit = Some(p.parse_i32()?),
            "runtime_ms" => md.runtime_ms = Some(p.parse_u64()?),
            "git" => md.git = Some(parse_git(&mut p)?),
            _ => p.skip_value()?,
        }
        p.skip_ws();
        match p.peek() {
            Some(',') => {
                p.advance();
            }
            Some('}') => {}
            _ => return Err(Osc7777ParseError::Json("expected , or }")),
        }
    }
    Ok(md)
}

fn parse_git(p: &mut Parser) -> Result<GitInfo, Osc7777ParseError> {
    p.expect_char('{')?;
    let mut git = GitInfo::default();
    loop {
        p.skip_ws();
        if p.peek() == Some('}') {
            p.advance();
            break;
        }
        let key = p.parse_string()?;
        p.skip_ws();
        p.expect_char(':')?;
        p.skip_ws();
        match key.as_str() {
            "branch" => git.branch = Some(p.parse_string()?),
            "dirty" => git.dirty = Some(p.parse_bool()?),
            "ahead" => git.ahead = Some(p.parse_u32()?),
            "behind" => git.behind = Some(p.parse_u32()?),
            _ => p.skip_value()?,
        }
        p.skip_ws();
        match p.peek() {
            Some(',') => {
                p.advance();
            }
            Some('}') => {}
            _ => return Err(Osc7777ParseError::Json("expected , or } in git")),
        }
    }
    Ok(git)
}

fn encode_json(md: &BlockMetadata) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = &md.cwd {
        parts.push(format!("\"cwd\":{}", encode_string(cwd)));
    }
    if let Some(git) = &md.git {
        parts.push(format!("\"git\":{}", encode_git(git)));
    }
    if let Some(user) = &md.user {
        parts.push(format!("\"user\":{}", encode_string(user)));
    }
    if let Some(host) = &md.host {
        parts.push(format!("\"host\":{}", encode_string(host)));
    }
    if let Some(shell) = &md.shell {
        parts.push(format!("\"shell\":{}", encode_string(shell)));
    }
    if let Some(exit) = md.exit {
        parts.push(format!("\"exit\":{exit}"));
    }
    if let Some(rt) = md.runtime_ms {
        parts.push(format!("\"runtime_ms\":{rt}"));
    }
    format!("{{{}}}", parts.join(","))
}

fn encode_git(git: &GitInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(branch) = &git.branch {
        parts.push(format!("\"branch\":{}", encode_string(branch)));
    }
    if let Some(d) = git.dirty {
        parts.push(format!("\"dirty\":{d}"));
    }
    if let Some(a) = git.ahead {
        parts.push(format!("\"ahead\":{a}"));
    }
    if let Some(b) = git.behind {
        parts.push(format!("\"behind\":{b}"));
    }
    format!("{{{}}}", parts.join(","))
}

fn encode_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Manual \uXXXX emission — hex-digit lookup is
                // infallible so we don't need `write!` + unwrap.
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let codepoint = c as u32;
                out.push_str("\\u");
                out.push(HEX[((codepoint >> 12) & 0xf) as usize] as char);
                out.push(HEX[((codepoint >> 8) & 0xf) as usize] as char);
                out.push(HEX[((codepoint >> 4) & 0xf) as usize] as char);
                out.push(HEX[(codepoint & 0xf) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).map(|b| *b as char)
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn expect_char(&mut self, ch: char) -> Result<(), Osc7777ParseError> {
        self.skip_ws();
        match self.advance() {
            Some(c) if c == ch => Ok(()),
            _ => Err(Osc7777ParseError::Json("unexpected char")),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, Osc7777ParseError> {
        self.skip_ws();
        self.expect_char('"')?;
        let mut out = String::new();
        loop {
            let ch = self
                .advance()
                .ok_or(Osc7777ParseError::Json("unterminated string"))?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self
                        .advance()
                        .ok_or(Osc7777ParseError::Json("bad escape"))?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        _ => return Err(Osc7777ParseError::Json("unsupported escape")),
                    }
                }
                c => out.push(c),
            }
        }
    }

    fn parse_bool(&mut self) -> Result<bool, Osc7777ParseError> {
        self.skip_ws();
        if self.remaining().starts_with(b"true") {
            self.pos += 4;
            Ok(true)
        } else if self.remaining().starts_with(b"false") {
            self.pos += 5;
            Ok(false)
        } else {
            Err(Osc7777ParseError::Json("expected true/false"))
        }
    }

    fn parse_i32(&mut self) -> Result<i32, Osc7777ParseError> {
        self.parse_number_token()
            .and_then(|s| s.parse::<i32>().map_err(Into::into))
    }

    fn parse_u32(&mut self) -> Result<u32, Osc7777ParseError> {
        self.parse_number_token()
            .and_then(|s| s.parse::<u32>().map_err(Into::into))
    }

    fn parse_u64(&mut self) -> Result<u64, Osc7777ParseError> {
        self.parse_number_token()
            .and_then(|s| s.parse::<u64>().map_err(Into::into))
    }

    fn parse_number_token(&mut self) -> Result<String, Osc7777ParseError> {
        self.skip_ws();
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if matches!(ch, '0'..='9' | '-' | '+') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(Osc7777ParseError::Json("expected number"));
        }
        Ok(std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Osc7777ParseError::Json("non-utf8 number"))?
            .to_string())
    }

    fn skip_value(&mut self) -> Result<(), Osc7777ParseError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                self.parse_string()?;
            }
            Some('t') | Some('f') => {
                self.parse_bool()?;
            }
            Some('{') => {
                let mut depth = 1;
                self.advance();
                while depth > 0 {
                    match self.advance() {
                        Some('{') => depth += 1,
                        Some('}') => depth -= 1,
                        Some('"') => {
                            self.pos -= 1;
                            self.parse_string()?;
                        }
                        None => return Err(Osc7777ParseError::Json("unterminated object")),
                        _ => {}
                    }
                }
            }
            Some(c) if c == '-' || c.is_ascii_digit() => {
                self.parse_number_token()?;
            }
            _ => return Err(Osc7777ParseError::Json("unexpected value")),
        }
        Ok(())
    }

    fn remaining(&self) -> &[u8] {
        &self.input[self.pos..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_all_fields() {
        let md = BlockMetadata {
            cwd: Some("/home/nyxb/carrot".into()),
            git: Some(GitInfo {
                branch: Some("main".into()),
                dirty: Some(false),
                ahead: Some(2),
                behind: Some(0),
            }),
            user: Some("nyxb".into()),
            host: Some("moonbox".into()),
            shell: Some("nu".into()),
            exit: Some(0),
            runtime_ms: Some(247),
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back, md);
    }

    #[test]
    fn encode_then_decode_handles_empty_metadata() {
        let md = BlockMetadata::default();
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back, md);
    }

    #[test]
    fn hex_decode_odd_length_errors() {
        assert!(matches!(parse("abc"), Err(Osc7777ParseError::Hex(_))));
    }

    #[test]
    fn hex_decode_nonhex_errors() {
        assert!(matches!(parse("zz"), Err(Osc7777ParseError::Hex(_))));
    }

    #[test]
    fn string_escapes_survive_roundtrip() {
        let md = BlockMetadata {
            cwd: Some("/weird \"path\"\nwith\ttabs".into()),
            ..Default::default()
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back.cwd, md.cwd);
    }

    #[test]
    fn partial_metadata_parses_correctly() {
        let md = BlockMetadata {
            exit: Some(1),
            runtime_ms: Some(1234),
            ..Default::default()
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back.exit, Some(1));
        assert_eq!(back.runtime_ms, Some(1234));
        assert!(back.cwd.is_none());
        assert!(back.git.is_none());
    }

    #[test]
    fn git_info_roundtrips_with_dirty_true() {
        let md = BlockMetadata {
            git: Some(GitInfo {
                branch: Some("feat/x".into()),
                dirty: Some(true),
                ahead: Some(0),
                behind: Some(3),
            }),
            ..Default::default()
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back.git, md.git);
    }

    #[test]
    fn unknown_keys_are_skipped() {
        // Manually craft JSON with an extra key, hex-encode, parse.
        let json = r#"{"cwd":"/x","future_field":"ignored","exit":0}"#;
        let hex = encode_hex(json.as_bytes());
        let back = parse(&hex).unwrap();
        assert_eq!(back.cwd.as_deref(), Some("/x"));
        assert_eq!(back.exit, Some(0));
    }

    #[test]
    fn unterminated_string_errors() {
        let json = r#"{"cwd":"/no-close"#;
        let hex = encode_hex(json.as_bytes());
        assert!(matches!(parse(&hex), Err(Osc7777ParseError::Json(_))));
    }

    #[test]
    fn negative_exit_code_parses() {
        let md = BlockMetadata {
            exit: Some(-1),
            ..Default::default()
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back.exit, Some(-1));
    }

    #[test]
    fn error_display_mentions_category() {
        let hex = format!("{}", Osc7777ParseError::Hex("oops".into()));
        let json = format!("{}", Osc7777ParseError::Json("oops"));
        assert!(hex.contains("hex"));
        assert!(json.contains("JSON"));
    }

    #[test]
    fn boolean_field_toggles_roundtrip() {
        let md = BlockMetadata {
            git: Some(GitInfo {
                dirty: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let hex = encode(&md);
        let back = parse(&hex).unwrap();
        assert_eq!(back.git.unwrap().dirty, Some(false));
    }
}
