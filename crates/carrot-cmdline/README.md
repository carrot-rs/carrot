# carrot-cmdline

Semantic-AST shell command-entry surface. The long-term goal is to
wrap `carrot-editor` (SumTree, multi-cursor, IME, tree-sitter, vim
mode) via an adapter trait; this crate's own contribution is the
**semantic command AST** plus the 2026-native agentic surface (AI
ghost-text, `#` handoff, MCP completions, Active-AI chips, 4-state
PromptState for interactive mid-command prompts).

Layer 5 sibling of `carrot-terminal-view`.

## Status

Zero-runtime-dependency data-type + logic foundation. Every module
is pure Rust + std — no `carrot-editor`, no `inazuma`, no
`carrot-completions`. That means downstream crates can import the
contract types today without pulling the full UI dep graph.

**217 tests** (202 lib + 7 proptest × 256 cases + 8 integration).
Criterion pipeline bench keeps the full parse → highlight →
validate cycle under 400 ns per keystroke.

| Module | Purpose |
|--------|---------|
| `ast` | `CommandAst`, `ArgKind`, `Range`, positional / flag / subcommand nodes, error severity |
| `parse` | Shell-agnostic fallback parser (whitespace + quote aware, flag classification, byte ranges) |
| `shell` | `ShellKind` (Nu default, Bash, Zsh, Fish) with basename + Windows-exe detection |
| `prompt_state` | 4-state lifecycle machine (Idle / Composing / Running / Interactive) |
| `osc133` | Shell-integration lifecycle events (PromptStart / InputStart / CommandStart / CommandEnd / AgentEditActive) |
| `osc7777` | Carrot-native structured metadata sidecar (cwd, git, user, exit, runtime) |
| `shell_event` | Composed `ShellBlockEvent` + `ShellStream` buffer joining OSC 133 + OSC 7777 |
| `keymap` | `CmdlineAction` vocabulary + per-shell default binding tables |
| `completion` | `CompletionCandidate`, `CompletionSet`, `CompletionSource` with stable ranking |
| `completion_sources` | Filesystem / env-var / history backends (std-only) |
| `completion_driver` | Cursor-aware dispatch: `ArgKind` → right source |
| `history` | Bounded + deduplicated history buffer |
| `history_store` | Parsers for bash / zsh-extended / fish YAML / nu plain text history files |
| `ai` | Ghost-text suggestion data model (History / Local / Cloud sources) |
| `ai_engine` | `SuggestionEngine` trait + `HistoryEngine` + `MockEngine` + `RacingEngine` |
| `agent` | `#` handoff model + Active-AI chips + `AgentResponse` / `AgentEdit` |
| `editor_adapter` | `EditorAdapter` trait + `StringEditor` reference implementation |
| `highlight` | AST-driven `HighlightSpan` + `HighlightRole` projection |
| `session` | `CmdlineSession` — composes every module into a testable state machine |
| `mount` | `MountController` — cmdline ↔ block lifecycle site transitions |
| `validation` | Live-state validation of `CommandAst` against known refs / paths / enums |

## Roadmap (follow-ups requiring external deps)

- `editor_adapter_carrot` — real adapter over `carrot-editor`'s `ErasedEditor`
- `syntax/{bash,zsh,fish,nu}` — tree-sitter + native-fish parsers producing
  schema-typed positionals via the 715 `carrot-completions` specs
- Cloud / local AI backends fulfilling the `SuggestionEngine` trait
- `element` + `element_paint` — `inazuma::Element` view with harfrust
  shaping + MSDF glyph atlas
- `pty_route` — keystroke routing to PTY when `PromptState == Running`

## Testing

```bash
cargo test -p carrot-cmdline                      # full suite
cargo test -p carrot-cmdline --lib                # unit tests
cargo test -p carrot-cmdline --test session_flow  # integration
cargo test -p carrot-cmdline --test parse_fuzz    # property tests
cargo bench -p carrot-cmdline --bench pipeline    # criterion
cargo clippy -p carrot-cmdline --all-targets -- -D warnings
```

## Crate rules

- No `mod.rs`.
- No stubs, no `todo!()`, no `unimplemented!()`, no warning
  suppressions.
- Every public type has rustdoc; every module explains scope +
  non-scope in a file-level `//!` block.
- Comments in English; user chat in German (per `CLAUDE.md`).
