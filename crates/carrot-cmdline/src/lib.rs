//! Carrot cmdline — semantic-AST shell command-entry surface.
//!
//! Layer 5 sibling of `carrot-terminal-view`. Holds an
//! `Entity<carrot_editor::Editor>` directly (`auto_height(1, 20)`
//! mode) and renders it inside `Cmdline::render()` — the pattern
//! used by `CommandPalette`, `FileFinder`, `GoToLine`, `InputField`.
//! This crate's own contribution is the **semantic command AST** +
//! **2026-native agentic surface** (AI ghost-text, `#` handoff,
//! MCP completions, Active-AI chips, 4-state PromptState).
//!
//! # Module layout
//!
//! ```text
//! carrot-cmdline/
//! ├── lib.rs                 — re-exports + Cmdline entry type
//! ├── agent.rs               — `#` handoff, Active-AI chips, AgentEdit
//! ├── ai.rs                  — ghost-text Suggestion data model
//! ├── ai_engine.rs           — SuggestionEngine trait + HistoryEngine + RacingEngine
//! ├── ast.rs                 — Semantic command AST
//! ├── completion.rs          — CompletionCandidate / Set / InsertAction
//! ├── completion_driver.rs   — cursor-aware dispatch to source
//! ├── completion_sources.rs  — filesystem / env / history backends
//! ├── view.rs                — Cmdline: Entity<Editor> composition + render()
//! ├── highlight.rs           — AST-driven HighlightSpan projection
//! ├── history_search.rs      — Ctrl-R dropdown wrapper over carrot-session
//! ├── keymap.rs              — CmdlineAction + per-shell default tables
//! ├── mount.rs               — MountController for cmdline ↔ block lifecycle
//! ├── osc133.rs              — ShellEvent + PromptState transitions
//! ├── osc7777.rs             — structured sidecar metadata parse/encode
//! ├── parse.rs               — shell-agnostic fallback parser
//! ├── prompt_state.rs        — 4-state lifecycle machine
//! ├── session.rs             — CmdlineSession composing all modules
//! ├── shell.rs               — ShellKind + basename detection
//! ├── shell_event.rs         — composed ShellBlockEvent + ShellStream buffer
//! └── validation.rs          — AST live-state validation
//! ```

pub mod accessibility;
pub mod ast;
pub mod completion;
pub mod completion_driver;
pub mod completion_sources;
pub mod handoff_mode;
pub mod highlight;
pub mod highlight_apply;
pub mod history_search;
pub mod keymap;
pub mod motion;
pub mod mount;
pub mod osc133;
pub mod osc7777;
pub mod parse;
/// Parked Phase-2 surface (AI / agent / MCP). Compiles with the
/// crate but is not re-exported at the root — callers go through
/// `carrot_cmdline::phase2::…`.
pub mod phase2;
pub mod prompt_state;
pub mod pty_route;
pub mod schema_typing;
pub mod session;
pub mod shell;
pub mod shell_event;
pub mod syntax;
pub mod validation;
pub mod view;

pub use accessibility::{
    ImeComposition, NarrationLine, NarrationRole, narrate_ast, narrate_completion_summary,
};
pub use ast::{
    ArgKind, AstError, CommandAst, CommandNode, FlagNode, PositionalNode, SubcommandNode,
};
pub use completion::{CompletionCandidate, CompletionSet, CompletionSource, InsertAction};
pub use completion_driver::{DriverContext, suggest_for_cursor};
pub use completion_sources::{
    envvar_candidates, filesystem_candidates, git_candidates, history_candidates,
};
pub use handoff_mode::{HandoffMode, HandoffTransition, cancel_to_shell, detect_mode, strip_sigil};
pub use highlight::{HighlightRole, HighlightSpan, highlight_ast};
pub use highlight_apply::{
    CmdlineHighlightPalette, TIER_COUNT, apply_ast_highlights, role_to_tier,
};
// History is owned by carrot-session (command_history.rs). Re-export
// the canonical types so cmdline consumers don't need to import both
// crates for the common data model.
pub use carrot_session::command_history::{
    CommandHistory, FuzzyMatch, HistfileFormat, HistoryEntry,
};
pub use history_search::{HistoryMatchView, HistorySearch};
pub use keymap::{CmdlineAction, default_bindings};
pub use motion::{AnimationKind, MotionPreference};
pub use mount::{BlockHandle, MountController, MountSite, MountTransition};
pub use osc133::ShellEvent;
pub use osc7777::{BlockMetadata, GitInfo, Osc7777ParseError};
pub use parse::parse_simple;
pub use prompt_state::{
    InteractionKind, InteractivePromptState, PromptState, detect_interaction_kind,
};
pub use pty_route::{
    KeystrokeRoute, agent_consumes, pty_consumes, route_keystroke, route_with_buffer,
};
pub use schema_typing::{apply_specs, arg_template_to_kind, type_positionals};
pub use session::CmdlineSession;
pub use shell::ShellKind;
pub use shell_event::{ShellBlockEvent, ShellStream};
pub use validation::{ValidationContext, apply_validation, validate};
pub use view::{Cmdline, CmdlineEvent};
