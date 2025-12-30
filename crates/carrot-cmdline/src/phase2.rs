//! Parked Phase-2 modules — AI / agent / MCP surface.
//!
//! These modules are not re-exported at the crate root. They compile
//! with the rest of the crate so API drift against
//! `carrot-language-model` — the target integration point — is
//! caught early, but consumers route explicitly through
//! `carrot_cmdline::phase2::…` until the provider-agnostic rewrite
//! is active.
//!
//! What lives here today:
//!
//! - [`ai`] — cmdline's own `Suggestion` / `SuggestionRequest` /
//!   `SuggestionSet` data model. Will be replaced by the
//!   `carrot_language_model::LanguageModel` trait surface.
//! - [`ai_engine`] — local `SuggestionEngine` trait + `HistoryEngine`
//!   + `RacingEngine`. Provider-specific engines (Ollama, Anthropic,
//!   Bedrock, Cloud) come in through the LanguageModel trait.
//! - [`agent`] — `AgentHandoff`, `AgentChip`, `AgentEdit`,
//!   `ChipIntent`.
//! - [`agent_host`] — `CmdlineAgentHost` trait + `InMemoryAgentHost`.
//!   Will become a thin adapter over `carrot-agent`.
//! - [`mcp_completion`] — `McpProvider` / `McpRegistry` stubs.
//!   `carrot-context-server` will plug real MCP clients here.

pub mod agent;
pub mod agent_host;
pub mod ai;
pub mod ai_engine;
pub mod mcp_completion;
