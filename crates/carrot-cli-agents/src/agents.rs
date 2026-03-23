//! Built-in agent implementations. Each module here provides one
//! `CliAgent` and is registered by `carrot_cli_agents::init`.
//!
//! Adding a new agent (Codex, Gemini, Aider, …) later is purely
//! additive: drop a new module in this directory, wire it into
//! `init()`, and the process-tree detection in `detection` picks it
//! up automatically once the binary names match.

pub mod claude_code;

pub use claude_code::ClaudeCodeAgent;
