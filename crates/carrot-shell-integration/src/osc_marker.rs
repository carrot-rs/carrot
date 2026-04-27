//! Shell-integration markers from OSC sequences.
//!
//! OSC 133 markers are sent by the shell hooks
//! (`carrot.zsh` / `carrot.bash` / `carrot.fish` / `nushell`) to
//! indicate command block boundaries.
//!
//! OSC 7777 carries JSON metadata:
//!   * Shell precmd metadata (CWD, git branch, user info) uses the
//!     `carrot-precmd;<hex>` sub-prefix.
//!   * CLI-agent hook events (Claude Code plugin and later agents)
//!     use a bare-hex payload whose decoded JSON carries
//!     `type: "cli_agent_event"`. The envelope is then routed to
//!     `carrot-cli-agents::parse_envelope`.
//!
//! # Layer note
//!
//! This enum lives in `carrot-shell-integration` (the shared
//! infrastructure crate) so both `carrot-terminal` (scanner /
//! parser) and `carrot-cmdline` (state-machine consumer) can
//! reference the **same** event type. Neither crate needs to
//! duplicate or translate it.

/// A shell-integration marker decoded from an OSC sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShellMarker {
    /// `OSC 133 ; A` — Prompt region starts. A new potential block
    /// begins.
    PromptStart,
    /// `OSC 133 ; B` — Input region starts (prompt ended, user can
    /// type).
    InputStart,
    /// `OSC 133 ; C` — Command execution starts, output region
    /// begins.
    CommandStart,
    /// `OSC 133 ; D ; N` — Command finished with the given exit
    /// code. `N` defaults to `0` when the shell only emitted
    /// `OSC 133 ; D` with no parameter.
    CommandEnd { exit_code: i32 },
    /// `OSC 133 ; L` — Carrot extension: agent-edit in progress.
    /// Consumers use it to freeze AI ghost-text while the shell
    /// rewrites the buffer under an agent-driven edit.
    AgentEditActive,
    /// `OSC 133 ; P ; k=<kind>` — Prompt kind (Nushell-specific).
    /// `i=initial`, `c=continuation`, `s=secondary`, `r=right`.
    PromptKind { kind: PromptKindType },
    /// `OSC 7777 ; carrot-precmd ; <hex>` — Shell metadata (JSON,
    /// hex-decoded). Decoded shape lives in
    /// [`crate::ShellMetadataPayload`].
    Metadata(String),
    /// `OSC 7777 ; carrot-tui-hint ; <hex>` — TUI-mode hint from the
    /// shell preexec hook, emitted before a known TUI command's
    /// first byte arrives. Lets the terminal pre-activate a
    /// live-frame region.
    TuiHint(String),
    /// `OSC 7777 ; <hex>` — CLI-agent hook event. Payload is the
    /// hex-decoded envelope JSON; routing and typing happens in
    /// `carrot-cli-agents::parse_envelope`.
    AgentEvent(String),
    /// `OSC 1337 ; File=key=value,...:base64-payload` — iTerm2 inline
    /// image. The carried `Vec<u8>` is the **raw OSC payload after
    /// `1337;`** — i.e. starts with `File=`. The marker consumer
    /// passes it to `carrot_grid::parse_iterm2_payload` which handles
    /// the header parsing + base64 + image decode in one call.
    ///
    /// Held as bytes (not String) because the base64 payload may be
    /// arbitrarily large (~MB for screenshots) and we don't want to
    /// pay UTF-8-validation cost on the hot OSC scan path.
    ImageInlineITerm2(Vec<u8>),
}

/// Nushell prompt-kind types from `OSC 133 ; P`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptKindType {
    /// Initial prompt (default).
    Initial,
    /// Continuation prompt (multi-line).
    Continuation,
    /// Secondary prompt.
    Secondary,
    /// Right-side prompt.
    Right,
}

/// A shell marker with its byte position in a scanned chunk.
///
/// Emitted by the OSC scanner in `carrot-terminal` so higher layers
/// can correlate the marker with offsets inside the original PTY
/// byte stream.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedMarker {
    pub marker: ShellMarker,
    /// Byte offset where the OSC sequence STARTS (the ESC or `0x9D`
    /// byte).
    pub start: usize,
    /// Byte offset AFTER the OSC terminator (BEL or ST).
    pub end: usize,
}
