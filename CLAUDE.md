# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Autonomous Execution Contract

These rules override any default caution about session length, context budget, or plan size. They are non-negotiable.

- **Work plans end-to-end, single-session.** Approved plans run from step 1 to the final Plan-Completion-Gate in one continuous execution. Context compaction is automatic — you do not need to plan around it, warn about it, or stop because of it.
- **No mid-plan approval requests.** "Never commit without explicit user approval" means `git commit` only — it does NOT mean "pause and ask whether to continue the plan". Plan execution is already approved when the user says "go".
- **Never hedge about plan size.** Do not say "this is multi-session", "this is ~40 commits", "too large for one run", or any variant. If the plan fits the plan file, it fits execution. Just work.
- **Never summarize plan progress as if done.** Only declare completion at the Plan-Completion-Gate after diffing every plan point against real code.
- **Blockers that are allowed:** compile errors you cannot resolve, a Hard Rule in CLAUDE.md would be violated, a user-only decision (credentials, external approval). Nothing else stops the plan.
- **Blockers that are NOT allowed:** token budget, session length, "out of an abundance of caution", self-imposed quality gates not listed in the plan, re-asking questions the plan already answered.
- **On compaction:** continue seamlessly from the plan file. The plan file is the source of truth, not conversation history. Re-read it after compaction if needed.

## Communication

- **Always respond to the user in German.** All written artifacts (this file, code, comments, commit messages, memory files, PR descriptions) are in English. Spoken/chat responses to the user are in German.

## Build & Run Commands

```bash
cargo build                           # Build carrot-app (default member)
cargo run -p carrot-app               # Run the terminal app
cargo carrot dev                      # Hot-reload dev mode (watches src, rebuilds + relaunches)
cargo carrot dev --release            # Hot-reload in release mode
cargo carrot build                    # Release build + .app bundle
cargo carrot build --debug            # Debug build + .app bundle
cargo carrot icon                     # Compile .icon → Assets.car via actool
cargo check --workspace               # Preferred over cargo build during iteration
cargo test -p carrot-terminal         # Run tests for one crate
cargo test -p carrot-terminal -- osc_parser::tests::name  # Run a single test
cargo test --workspace                # All tests
cargo clippy --workspace              # Lint (dbg! and todo! are denied)
./kill-carrot.sh                      # Kill stale processes left over from hot-reload
```

Requires **Rust stable 1.94+** (edition 2024, resolver 3). Pinned via `rust-toolchain.toml`. Cross-platform: macOS (Metal), Linux (Vulkan), Windows (DX12/Vulkan) — all via Inazuma's dual rendering backends (native Metal + WGPU).

`.cargo/config.toml` sets: `symbol-mangling-version=v0` rustflag, `MACOSX_DEPLOYMENT_TARGET=10.15.7`, and the `cargo carrot` alias.

Common env vars when debugging: `RUST_LOG=carrot=debug`, `RUST_BACKTRACE=1`.

## Hard Rules

Authoritative cross-cutting rules. Detailed context for each lives in the section noted in parentheses.

### Code & Conventions
- **No `mod.rs`** — use `module_name.rs` (modern Rust convention).
- **No stubs at plan completion.** During plan execution, intermediate commits MAY contain `todo!()`, `unimplemented!()`, TODO comments, or partial implementations — resolve them before the Plan-Completion-Gate closes (see Workflow & Process). Silent error swallowing is never acceptable, even mid-plan.
- **Comments in English only** — code, docs, and comments are English. User-facing chat is German.
- **No competitor names in code** — never name other terminal emulators, editors, or IDEs in code, comments, strings, or identifiers.
- **No `#[allow(unused)]` or warning suppressions** — only import what is used; resolve warnings properly.
- **macOS platform code uses `objc2` + `objc2-app-kit` + `objc2-foundation`** — never the old `cocoa` / `objc` crates.
- **Naming: framework is `inazuma`, app is `carrot`** — never call it GPUI; never call rendering "Metal rendering" (it's GPU rendering, with Metal/WGPU backends).
- **Use `log::` macros, not `eprintln!`** — `carrot-log` is the logging crate.
- **`cargo check` over `cargo build`** during iteration.

### Layer Architecture (see "Layer Architecture")
- **`carrot-ui` MUST NOT import `AppShell`** — communicate upward via Actions or Global queues.
- **`carrot-workspace` MUST NOT open windows** — only `prepare_local()`; the shell layer opens.
- **No trait objects for cross-layer communication** — use Actions + Globals instead of `dyn ShellHost`.
- **No code in `carrot-app` outside `main.rs` bootstrap and Global registration.**
- **`OpenOptions` / `OpenResult` belong in `carrot-shell`** — they hold `WindowHandle<AppShell>`.

### Block System (see "Block System Architecture")
- **New block data/logic** → new module in `carrot-term/src/block/` (zero UI dependencies).
- **New block UI** → new module `carrot-terminal-view/src/block_*.rs`.
- **New features always in their own module** — never bolt onto an existing module.
- **All grid access goes through `GridBounds`** — no hand-rolled coordinate arithmetic (`row_offset - history_size` etc.).
- **`GridBounds::content_lines()` is the only grid iteration** — snapshot, search, and text extraction all share that path.
- **`GridBounds::from_block()` recomputes `content_rows` fresh** — never use the cached `block.content_rows` for rendering or search.
- **Cursor is NEVER a block attribute.** Block structs (`ActiveBlock`, `BlockGrid`, `ActiveBlockView`, etc.) hold no `cursor` / `caret` field.
  - **Shell blocks:** user caret lives exclusively in `carrot-cmdline` (the input-editor widget). When the command starts the input widget unmounts and the block receives append-only PTY streams. No cursor rendering in the block.
  - **TUI blocks (`BlockKind::Tui`):** VT cursor state lives in the `carrot-term` VT emulator (Layer 2). Layer 4 (`carrot-block-render`) paints the cursor by reading the term state at render time — never from the block struct. Wide-char mapping is a renderer concern, not a cursor-struct concern.
  - This mirrors the reference-terminal model: same grid data structure for shell-output and alt-screen, but cursor ownership belongs to the lifecycle that produces writes (cmdline for prompt input, VT emulator for alt-screen apps), never to the block container.
- **`BlockKind` is `Shell | Tui`, describing lifecycle semantics — not content type.** Both kinds share the same `inazuma::block` grid primitive with identical PageList layout; only the lifecycle and rendering surface differ. Images, markdown previews, and custom renderers are **Cell tags 4 / 6 inside any block**, never their own `BlockKind` variant. Do not add `BlockKind::Image`, `BlockKind::CustomRender`, `BlockKind::AltScreen`, etc.
- **TUI blocks render via `PinnedFooter`, not inline in the scroll flow.** Active TUI frame stays anchored at the bottom while shell blocks scroll above. On TUI session end the block freezes and rejoins the normal scroll flow. Alt-screen is not "a bigger block in the blocklist" — it is a separate rendering surface backed by the same grid data structure.

### Session Model (see "Session Architecture")
- **`Workspace` owns sessions, not panes** — pane access always via session.
- **No `workspace.active_pane()` shortcut** — explicit hierarchy: `workspace.active_session().read(cx).active_pane()`.
- **Pane = exactly one Item** — no `Vec`, no index, no tab rendering inside `Pane`.
- **New session = new tab.** Not `Pane.add_item()`.
- **Split = new pane in the same session** (file-drop, Cmd+D, etc.).
- **Last pane closed → session closed.** Last session closed → window closed.
- **Terminal pane is never replaced by file-open** — file-open lands in an editor pane (via `last_active_editor_pane`) or a new split.
- **Editor-in-Terminal-Session-Pattern (Warp-parity, verified 2026-04-24):** Die Pane-Wahl-Policy für "wo landet ein neues Item bei welchem aktiven Pane" lebt in **einem einzigen Helper** — nicht in jedem Caller dupliziert. Zwei Achsen sauber getrennt:
  1. **Pane-Wahl (Policy)** — Helper `Workspace::target_pane_for_role(new_role, window, cx) -> Entity<Pane>`. Entscheidet via `PaneRole`-Match, splittet/erzeugt ggf. neue Panes oder Sessions, gibt den Ziel-Pane zurück. **Einzige Stelle der Policy.**
  2. **Item-Insertion (Caller-Semantik)** — jeder Entry-Point fügt das Item nach seiner eigenen Semantik ein, nachdem er den Ziel-Pane vom Helper bekommen hat. `Workspace::add_item_to_active_pane` nutzt `target_pane.add_item(...)` (plain insert). `Pane::open_path_preview` nutzt `target_pane.open_item(...)` und behält damit Dedup (gleiche Datei 2× öffnen fokussiert existing item) + Preview-Slot-Handling (`allow_preview` ersetzt den Preview-Tab). Diese Insertion-Features sind nicht verzichtbar und leben deshalb in ihrem jeweiligen Entry-Point, nicht im Helper.
  
  PaneRole-Match-Table im Helper:
  - `(Terminal, Editor)` → wenn `last_active_editor_pane` existiert, reuse/split danach (Setting `file_finder.open_target.when_editor_open` = `reuse_last | new_split | new_session`). Sonst Split im Terminal-Pane-Group (Setting `file_finder.open_target.when_terminal_active`, Default `split_right`).
  - `(Editor, Editor)` → Default `new_split` (Warp-parity, weil Carrot per Hard-Rule keine Tabbed-File-Viewer pro Pane hat und `reuse_last` hier destruktives Replace wäre). Konfigurierbar.
  - Alle anderen Kombinationen → aktiver Pane (Terminal→Terminal, Editor→Terminal etc.), kein Routing.
  
  Ergebnis: jeder Editor-Item-Open-Flow (File-Finder, Drag-Drop, Command-Palette, Diagnostics-Click, Search-Result-Click, Debugger-Source-Open, Agent-Panel-File-Open) ruft den Helper und behält seine Insertion-Semantik. Neue Callsites kriegen die Policy gratis — sie müssen nur zwischen `add_item` (kein Dedup/Preview) und `open_item` (mit Dedup/Preview) wählen. **Escape-Hatch:** `Workspace::add_to_active_pane_raw` umgeht den Helper für Sonderfälle (Tests, interne Workspace-Operationen).
  
  **Sidebar-Rendering läuft gratis:** `carrot-vertical-tabs` aktualisiert sich automatisch — im Tabs-Mode zeigt die Session-Row den aktiven Pane via `tab_content_text()`, im Panes-Mode werden alle Panes als Rows unter einem Group-Header gerendert. Kein neuer UI-Code nötig. `same_pane`-Setting-Option triggert `log::warn!` + fallback zu `split_right` weil es die "Terminal never replaced by file-open"-Regel brechen würde.
- **`PaneRole` has no default** — every Item must explicitly declare `PaneRole::Terminal` or `PaneRole::Editor`.
- **Session name = override or fallback** — `session.name` if set, else `item.tab_content_text()`.

### Glass UI / Rendering (see "Design System: Glass UI Pattern")
- **The background image lives at the workspace render root** — not in a pane or panel. Edits to image rendering happen at the workspace root.
- **Image layer is the LAST child of the workspace root** — it must be painted on top of everything else.
- **UI containers are opaque** — no alpha on `panel.background`, `title_bar.background`, `status_bar.background`, etc. Image effect comes from the overlay, not from semi-transparent container backgrounds.
- **Window opacity < 100 enables OS-level transparency** via `WindowBackgroundAppearance::Transparent` plus alpha on the root background. Blur radius only applies when opacity < 100.
- **Terminal pane sets its own `colors.background`** — without it, the pane is transparent down to the workspace background.
- **New panels use `FloatingPanel`, not plain `div`. New lists use `Card`, not ad-hoc `h_flex().bg(...)`** — the two primitives in `carrot-ui`.

### Crate Architecture (see "Crate Architecture Rules")
- **Three crate types only:** Framework Primitive (`inazuma-*`), Shared Infrastructure, Feature Crate.
- **Never put code in `carrot-app`** outside bootstrap.
- **Never create a backend crate that only one feature uses** — that logic belongs in the feature crate.
- **Shared Infrastructure never depends on Feature Crates.**
- **Feature Crates never depend on `carrot-app`.**
- **`inazuma-*` crates never depend on `carrot-*` crates.**
- **No circular dependencies.** Extract shared parts into a third crate.

### Project Scope (see "Project Scope Architecture" in ARCHITECTURE.md)
- **One `Entity<Project>` per Workspace.** Never introduce parallel project systems (e.g. the deleted `carrot-project-registry` pattern).
- **Create worktrees via `Project::ensure_browseable_worktree` / `ensure_tracked_worktree` / `ensure_ephemeral_worktree`.** Do not call `find_or_create_worktree(path, visible, cx)` directly in new code — that signature is retained only for backward compatibility and always maps to Tracked.
- **Reactive worktree creation lives in `terminal_pane/shell.rs`.** Scope classification happens in `carrot_shell::scope_policy::classify`; auto-tracking applies only to `ProjectKind::Git`.
- **No synchronous reads of `Workspace` from inside a `Context<TerminalPane>` that runs mid-Workspace-update** — use the cached `WeakEntity<Project>` on `TerminalPane` (`self.project`) instead of `self.workspace.read(cx).project()`, otherwise the re-entrance guard panics.

### Workflow & Process
- **Never commit without explicit user approval** — and never before the user has tested and confirmed.
- **Verify before claiming** — never assert anything about code without actually reading it first.
- **Analyze before acting** — read the file, check dependencies, ask the compiler. No assumptions.
- **Approved plans run end-to-end** — do not stop midway to ask whether to continue. Push through to the end of the plan. Stop only when blocked by something only the user can decide (not by missing time, scope worry, or self-imposed quality gates).
- **Plan-Completion-Gate is the only production-readiness gate.** When the last plan step is implemented:
  1. Re-read the plan top to bottom.
  2. Diff every plan point against the actual code (`grep`/`Read`, not memory).
  3. Close every gap: replace `todo!()` / `unimplemented!()` / TODO comments with real code, remove placeholders, add the missing tests, add the missing docs, run `cargo clippy --workspace` (todo-deny is enforced here, not earlier), run `cargo test --workspace`, run benches if the plan specified them.
  4. Only after all gaps are closed, the plan is "done". Production-ready means: clippy clean, tests green, no `unwrap()`/`expect()`/`panic!()` in production paths, every public API has a `///` doc, every plan-mandated bench passes its budget.
- **During plan execution, do not block on partial-readiness.** `cargo check` is enough to keep moving. Full clippy / test / bench runs belong to the Plan-Completion-Gate, not to every commit.
- **When a design question has no explicit answer:** pick the conservative default that preserves the hard invariants (cell layout, layer architecture, crate boundaries), and keep working. Stop and ask only if the choice would violate a Hard Rule listed in this file.
- **Always do the right and clean approach, never the easy one.**
- **For platform-layer changes, update all platforms simultaneously** (macOS, Linux, Windows).

## Product Identity: Terminal-First ADE

**Carrot (キャロット)** is a **Terminal-First Agentic Development Environment (ADE)** built on the Inazuma (稲妻) GPU UI framework.

### What "Terminal-First ADE" means

Carrot is **neither a pure terminal emulator nor an IDE**. The positioning is precise:

- **Terminal-First** — the terminal (with blocks, shell integration, completions) is the **primary, default-visible surface**. Every session starts as a terminal pane. The block system is Carrot's core, not a side feature.
- **ADE (Agentic Development Environment)** — Carrot has the full panel infrastructure of a modern dev environment: file tree, agent panel, git panel, debug panel, outline panel, collab panel, notification panel. These panels are **registered but hidden by default**. The user opens them on demand (keybind / command palette / sidebar toggle).
- **Not an IDE, not a plain terminal** — the editor is not an IDE replacement; the terminal is not a bare emulator. Carrot combines both: block-based terminal UX on top of full dev-environment panel infrastructure, with the terminal as the dominant surface.

### Mental model: terminal-first, panels optional

| Aspect | Carrot | Editor-first IDE | Plain Terminal |
|--------|--------|------------------|----------------|
| Default surface | Terminal (blocks) | Editor | Terminal (raw) |
| Panels visible at start | No (all hidden) | Yes (file tree, outline) | — (no panels) |
| Panel infrastructure | Yes (8 panels registered) | Yes | No |
| Shell integration (OSC 133) | Yes (blocks) | No (terminal is secondary) | No |
| Agent panel | Yes (AI/agent-first) | Optional (assistant) | No |
| Tab model | Sessions (1 session = 1 tab) | Multi-item panes | — |

**Concretely:** when Carrot starts, the user sees a terminal block. No file tree, no sidebar, no outline. Only when the user presses `Cmd+\` or runs a panel command does the corresponding dock surface appear. This fundamentally distinguishes Carrot from editor-first IDEs and places it next to terminal-first dev tools.

### Panel registration: "Registered but Hidden"

`initialize_workspace()` registers all panels at app start:

- `ProjectPanel` (file tree) — hidden
- `OutlinePanel` — hidden
- `GitPanel` — hidden
- `DebugPanel` — hidden
- `CollabPanel` — hidden
- `NotificationPanel` — hidden
- `TerminalPanel` — hidden (separate terminal dock, **not** the main surface)
- `AgentPanel` — hidden

The workspace default layout shows **only** the central pane group with one terminal item. Docks stay closed until the user opens them.

### Architectural consequences

- **Every new feature must ask: "Is this terminal surface or panel?"** Terminal surface means: always visible, block-integrated, part of the terminal-UX flow (e.g. command palette, completions, block selection). Panel means: optional, hidden by default, dev tooling (e.g. file tree, agent chat).
- **No editor-first patterns** — if a feature only makes sense when the editor is permanently open, it does not belong in Carrot's default flow.
- **The agent panel is first-class** — Carrot is *agentic*; the agent panel is core UX, not a plugin. But hidden by default like every other panel.
- **Terminal rendering is like a real terminal emulator**, not like an editor text surface. See "Terminal vs Editor Rendering".

## Architecture

### Crate dependency graph

```
carrot-app (binary — entry point, workspace layout, terminal rendering)
├── inazuma (GPU UI framework, forked from gpui-ce)
│   └── inazuma-macros (proc-macros: derive Actions, elements, etc.)
├── inazuma-component (70+ UI components: input, chips, title_bar, tabs, etc.)
│   ├── inazuma-component-macros (proc-macros: icon_named!, IntoPlot derive)
│   └── inazuma-component-assets (bundled fonts/icons/SVGs)
├── carrot-terminal (PTY + OSC 133 parser + block system, built on carrot-term)
├── carrot-term (low-level terminal emulation core — VT state machine + BlockGrid + PTY)
├── carrot-shell (window lifecycle — AppShell as window root, open/close/reload)
├── carrot-shell-integration (shell context: CWD, git branch, user info)
├── carrot-settings (user config at ~/.config/carrot/config.toml — theme, font, cursor, scrollback, symbol_map)
├── carrot-completions (spec-based CLI completion engine — JSON specs for 715+ CLIs)
├── carrot-assets (compile-time asset bundling via rust-embed — themes, fonts, keymaps; falls back to inazuma-component-assets)
├── carrot-theme (theme system — ThemeRegistry, ThemeColors, ThemeStyles, OKLCH color pipeline)
│   ├── carrot-theme-settings (connects themes to carrot-settings — ThemeSelection, reload_theme)
│   ├── carrot-theme-extension (dynamic theme loading — ExtensionThemeProxy)
│   └── carrot-theme-selector (UI picker for browsing/switching themes)
├── carrot-log (logging — use `log::` macros, never eprintln)
├── inazuma-fuzzy (fuzzy matching engine — CharBag, match_strings)
├── inazuma-util (general utilities — fs, paths, shell, markdown, etc.)
├── inazuma-collections (FxHashMap/FxHashSet aliases, VecMap)
├── inazuma-gpui-util (GPUI helpers — post_inc, measure, ArcCow)
├── inazuma-util-macros (proc-macros — path! for cross-platform paths)
├── inazuma-perf (perf profiler data types)
└── cargo-carrot (dev tooling binary: cargo carrot dev/build/icon — not a library)
```

### Key subsystems

**Inazuma (稲妻)** — the GPU UI framework. ~90 modules covering app lifecycle, element system, GPU rendering (Metal on macOS, WGPU for cross-platform — Vulkan/DX12/Metal), text shaping, layout (taffy), and platform abstraction. Modify Inazuma directly when it is cleaner than working around it in feature crates.

**Terminal Backend** (`carrot-terminal`) — higher-level terminal service built on `carrot-term`. PTY spawning in `pty.rs` injects shell hooks via `ZDOTDIR` manipulation. The `osc_parser.rs` scans PTY byte streams for OSC 133 (FTCS) shell integration markers. `block.rs` provides `BlockManager` which tracks command blocks (prompt → input → output → exit code).

**Terminal Core** (`carrot-term`) — the low-level terminal emulator: VT state machine, grid storage, `BlockGrid` (per-command grids with independent cursors and scroll regions), PTY abstraction via `rustix-openpty`.

**Shell Hooks** (`shell/carrot.{zsh,bash,fish}`, `shell/nushell/`) — injected into the spawned shell to emit OSC 133 markers (PromptStart, InputStart, CommandStart, CommandEnd) and OSC 7777 JSON metadata (hex-encoded). Zsh uses `ZDOTDIR` injection, Bash uses `--rcfile`. Nushell has dedicated integration in `shell/nushell/` and is Carrot's default shell — first terminal with full Nushell structured-output rendering (tables, lists, records with type badges in block headers), no hook injection needed because Nu has native OSC 133 support.

**Workspace** (`carrot-workspace`) — terminal-style session model. Workspace owns `Vec<Entity<WorkspaceSession>>` — each session is a tab in the title bar. Each `WorkspaceSession` owns its own `PaneGroup` (split tree), and each pane holds exactly 1 item (single-item model). Hierarchy: `Workspace → Sessions (title-bar tabs) → Session → PaneGroup (splits) → Pane (1 Item) → Blocks`. Docks (left/bottom/right) live at the Workspace level.

**Terminal Element** (`carrot-app/src/terminal_element.rs`) — custom Inazuma element that renders the terminal grid cell-by-cell with ANSI color mapping, block headers (command + duration + exit badge), cursor, and content masking.

**Theme System** (`carrot-theme`) — full theme pipeline: TOML theme definitions in `assets/themes/`, loaded into `ThemeRegistry` at startup. `GlobalTheme` provides app-wide access. All colors are token-based via `ThemeColors` / `ThemeStyles` — no hardcoded colors. OKLCH color space throughout. Display P3 wide-gamut on Retina displays.

**Settings** (`carrot-settings`) — `CarrotConfig` implements `inazuma::Global` for app-wide access. Sections: `GeneralConfig` (working_directory, input_mode), `AppearanceConfig` (theme, font_family, font_size, symbol_map for Nerd Font ranges), `TerminalConfig` (scrollback_history, cursor_style).

**Completions** (`carrot-completions`) — parses the user's current input line into `CommandContext` + `TokenPosition`, matches against embedded JSON specs (715+ CLIs including git, cargo, npm, docker, kubectl), returns `CompletionCandidate`s. Supports file paths, git branches/tags/remotes, env vars, process IDs.

**Context Chips** — 69 built-in context providers replace external prompt tools entirely: git branch & status, language versions (Node, Python, Rust, Go, +21 more), DevOps contexts (Kubernetes, Docker, AWS, Terraform), environment info. Zero config.

**Directory Jumping** — frecency-based native directory jumping (zoxide-style), driven by shell-integration history. Fuzzy directory switching without external tools.

**AI Agent Toolbar** — auto-detects running agents (Claude, Codex, Gemini, Aider) and surfaces a native toolbar with file explorer, diff viewer, MCP integration, and natural-language-to-command translation via `#` prefix.

### Theme

Theme definitions live in `assets/themes/` as TOML, loaded via `carrot-theme::ThemeRegistry`. All colors reference theme tokens — no hardcoded hex values outside the theme files themselves.

## Layer Architecture

Carrot follows a strict 4-layer model. Dependencies flow **downward only**.

```
Layer 4: carrot-app          — main.rs, bootstrap, Global registration. Zero logic.
Layer 3: carrot-shell         — AppShell (window root), window lifecycle (open/close/reload)
Layer 2: carrot-workspace     — Workspace, Sessions, Panes, Panels, Items, Toolbar
Layer 1: carrot-ui            — UI primitives, dialogs, notifications, theming widgets
Layer 0: inazuma              — Framework, rendering, platform, element system
```

### What lives where

| Concept | Layer | Crate |
|---------|-------|-------|
| Window open/close/reload | 3 | `carrot-shell` |
| AppShell (window root element) | 3 | `carrot-shell` |
| OpenOptions, OpenResult | 3 | `carrot-shell` |
| Workspace construction (`prepare_local`) | 2 | `carrot-workspace` |
| WorkspaceSession (tab = session) | 2 | `carrot-workspace` |
| Pane (single-item container) | 2 | `carrot-workspace` |
| PaneGroup (split tree) | 2 | `carrot-workspace` |
| Panels, Items, Toolbar | 2 | `carrot-workspace` |
| Dialogs (`PromptDialog`, `ConfirmDialog`) | 1 | `carrot-ui` |
| `PendingDialogs`, `FocusedInputTracker` | 1 | `carrot-ui` (Global definition) |
| Global registration | 4 | `carrot-app` (`main.rs`) |

### Cross-layer communication

**Upward (Layer 1→3): Actions**
When `carrot-ui` needs something from AppShell, it dispatches an Action. AppShell registers the handler.
```rust
// carrot-ui dispatches:
cx.dispatch_action(Box::new(CloseDialog(id)));
// carrot-shell handles in AppShell::render():
.on_action(cx.listener(Self::handle_close_dialog))
```

**Upward with data (Layer 1→3): Global queues**
For complex data (e.g. dialog content) use a `Global` as a queue. The higher layer observes and drains.
```rust
// carrot-ui defines:
#[derive(Default)]
pub struct PendingDialogs { pub queue: Vec<ActiveDialog> }
impl Global for PendingDialogs {}

// carrot-shell (AppShell::new) observes:
cx.observe_global::<PendingDialogs>(|this, window, cx| { ... });
```

**Downward (feature crates → shell): free functions**
Feature crates call `carrot_shell::open_new()`, `carrot_shell::open_dialog()` etc. — normal function calls.

### Initialization

| What | Where | When |
|------|-------|------|
| Globals (`PendingDialogs`, `FocusedInputTracker`, `ThemeRegistry`) | `carrot-app/main.rs` | App start |
| Action handlers (`CloseDialog`, `DeferCloseDialog`) | `AppShell::render()` | Window creation |
| Feature init (terminal, completions, etc.) | Each `crate::init()` | Called from main.rs |
| Panel registration (all 8 panels, default hidden) | `initialize_workspace()` | After `prepare_local()` |
| Default item in central pane (terminal) | `Workspace::new()` | After `prepare_local()` |

## Block System Architecture

The block system is Carrot's core (terminal-style command blocks). Two crates, strict separation:

**`carrot-term/src/block/`** — Shared Infrastructure (data + logic, zero UI dependencies)

| Module | Responsibility |
|--------|----------------|
| `grid.rs` | `BlockGrid` struct, per-block state, grid operations |
| `router.rs` | `BlockGridRouter`: lifecycle, resize, memory eviction |
| `coordinates.rs` | `GridBounds`: central coordinate conversion, `content_lines()` iterator |
| `selection.rs` | Block-level selection (start, update, resolve, to_string) |
| `text.rs` | Text extraction (grid → string, grid → lines) |
| `snapshot.rs` | `RawBlockSnapshot`: grid extraction without color resolution |
| `metadata.rs` | `BlockMetadata`, `BlockHeaderData`, duration formatting |
| `prompt.rs` | `PromptRegionTracker`: prompt-region tracking, prompt suppression |

**`carrot-terminal-view/src/`** — Feature crate (rendering + UI)

| Module | Responsibility |
|--------|----------------|
| `block_list.rs` | `BlockListView`: list container, scroll, orchestration |
| `block_element.rs` | Single-block rendering: header + grid element |
| `block_interaction.rs` | Hit testing, mouse events, block selection |
| `block_fold.rs` | Fold detection, fold lines, expand/collapse |
| `block_search.rs` | Search matching (`SearchQuery`), highlight mapping |
| `grid_element.rs` | Per-cell GPU rendering |
| `grid_snapshot.rs` | Color resolution (raw → OKLCH), snapshot cache |

## Session Architecture

Carrot uses a session model (one session per tab) instead of a multi-item-pane model.

```
Workspace
├── sessions: Vec<Entity<WorkspaceSession>>   ← tabs in the title bar
├── active_session_index: usize
├── docks (left/bottom/right)
└── status_bar

WorkspaceSession                               ← 1 tab = 1 session
├── id, name, color
├── pane_group: PaneGroup                      ← split tree (unchanged)
├── panes: Vec<Entity<Pane>>
├── active_pane: Entity<Pane>
├── last_active_editor_pane: Option<WeakEntity<Pane>>
└── follower_states (collab)

Pane                                           ← single-item container
├── item: Box<dyn ItemHandle>                  ← exactly 1 item
├── focus_handle, toolbar, nav_history
└── zoom, drag_split_direction
```

## Terminal vs Editor Rendering

Strict separation between terminal code and editor/UI code:

- **Terminal output** (grid rendering, PTY, cells): always built like a real terminal emulator. Per-cell rendering, grid positioning at `col * cell_width`, no per-line text shaping, no `force_width`. Box-drawing via `builtin_font.rs` (GPU primitives), emoji via `paint_emoji` with platform-specific font fallback.
- **Editor / UI features** (code editor, text input, completions, panels, settings): Inazuma's text system (`ShapedLine`, `shape_line`, `TextRun`). That is what the framework is built for.

Inazuma is an **editor framework**. Terminal rendering has fundamentally different requirements (fixed grid, per-cell positioning, Unicode width, emoji, box-drawing). Do not apply editor patterns to terminal rendering.

## Design System: Glass UI Pattern

Carrot follows the "Glass" rendering model for its characteristic look. The pattern applies **globally** — title bar, sidebar, terminal, status bar, all panels, all popovers. New UIs conform to the pattern, not the other way around.

### Three separate opacity concepts

This is the most important point. All three are disjoint:

| Concept | Defined in | Effect | Range |
|---------|------------|--------|-------|
| **Background image opacity** | Theme file (`background_image.opacity`) | How opaque the image overlay is over the UI | 0–100 |
| **Window opacity** | User setting (`appearance.window_opacity`) | OS-level window transparency — desktop shows through | 1–100 |
| **Window blur radius** | User setting (optional) | Gaussian blur on OS content behind the window | 0–64 |

Per-color alphas in theme files (e.g. `panel.background` with alpha < 1.0) are **not** part of the pattern. UI containers are fully opaque — the image effect comes purely from the image overlay.

### Rendering hierarchy

```
Window (opaque, bg = colors.background, optionally with window_opacity as OS alpha)
├── Title Bar                        ← opaque, own bg color
├── Docks (left/right/bottom)        ← opaque panel containers
│   └── Panel content                ← Cards, lists, controls
├── Center: Pane tree                ← Terminal pane with colors.background as bg
├── Status Bar                       ← opaque
└── Background image overlay         ← LAST child, absolute, size_full, opacity from theme
                                       sits ABOVE everything (image texture over the whole UI)
```

Concretely: at `background_image.opacity = 5` the overlay is 5 % opaque — the UI is 95 % visible with a subtle image texture on top. At `opacity = 100` the image fully covers the UI (text becomes unreadable).

### Component primitives (carrot-ui)

- **`FloatingPanel`** — container for dock panels. Rounded corners, margin from window edges, opaque bg from `panel.background`.
- **`Card`** — list-item primitive for everything with item rows (vertical tabs, project panel, outline, git panel, agent messages). Default transparent; hover/active with own bg + rounded corners + gap between cards. End slot for hover actions (×, ⋮).

### Transitions

Hover/active transitions: 150 ms ease (cards, buttons). No abrupt bg flips. Use Inazuma's built-in transition APIs, no custom animations.

## Crate Architecture Rules

### The 3 crate types

Every crate in the repo falls into exactly one category:

**1. Shared Infrastructure** — backend logic used by multiple feature crates.
- Contains: state management, events, traits, data models, protocols, algorithms
- Does NOT contain: UI rendering, workspace integration (`impl Item`, `impl Panel`)
- Rule: if >1 feature crate imports this logic, it belongs here
- Examples: `carrot-terminal`, `carrot-project`, `carrot-editor`, `carrot-git`, `carrot-lsp`, `carrot-completions`, `carrot-session`, `carrot-shell-integration`, `carrot-task`

**2. Feature Crates** — self-contained features with logic + UI together.
- Contains: feature-specific logic + settings + UI rendering + workspace traits (`impl Item`, `impl Panel`, `impl Render`)
- Feature-specific logic that ONLY this feature needs lives HERE — not in a separate backend crate
- These are NOT "thin wrappers" — they can be large (`carrot-project-panel` is 18k lines, `carrot-search` is 9.6k, `carrot-terminal-view` is 9.5k)
- Imports from Shared Infrastructure + Framework
- Examples: `carrot-terminal-view`, `carrot-project-panel`, `carrot-search`, `carrot-file-finder`, `carrot-diagnostics`, `carrot-debugger-ui`, `carrot-git-ui`, `carrot-agent-ui`, `carrot-settings-ui`

**3. Framework Primitives** — reusable building blocks with zero app knowledge.
- Contains: generic data structures, UI primitives, rendering engine, settings framework
- Knows NOTHING about Carrot, terminals, editors, or any app features
- Examples: `inazuma`, `inazuma-collections`, `inazuma-text`, `inazuma-rope`, `carrot-ui`, `inazuma-picker`, `inazuma-menu`, `inazuma-settings-framework`

**Entry point:** `carrot-app` is ONLY `main.rs` + bootstrap. It imports and initializes crates. Zero logic, zero rendering, zero UI.

### Decision flowchart: "Where does this code go?"

```
Is it a generic library with no app knowledge?
  YES → Framework Primitive (inazuma-*)
  NO  ↓

Is this logic needed by >1 feature crate?
  YES → Shared Infrastructure (carrot-terminal, carrot-project, etc.)
  NO  ↓

→ Feature Crate (carrot-terminal-view, carrot-search, etc.)
```

### Before writing code

1. We have 220+ crates — check `crates/` directory first.
2. Never create a backend crate that only one feature uses — put that logic in the feature crate instead.
3. Never put code in `carrot-app` — find or create the proper crate.

### Naming convention

- **`inazuma-*`** = framework-level, reusable independent of Carrot (collections, text, rope, fuzzy, settings framework, UI primitives, GPU rendering)
- **`carrot-*`** = application-level, Carrot-specific features (terminal, theme, workspace, editor, agent, collab, project)

### Dependency rules

```
carrot-app (entry point — imports everything, contains nothing)
  ├── Feature Crates (carrot-terminal-view, carrot-search, etc.)
  │     ├── Shared Infrastructure (carrot-terminal, carrot-project, etc.)
  │     ├── carrot-workspace (workspace framework)
  │     └── carrot-ui / inazuma-component (UI components)
  └── Framework Primitives (inazuma, inazuma-collections, etc.)
```

### Architectural conventions

- Framework is `inazuma`, app is `carrot`.
- Colors are in OKLCH throughout.
- Settings format is TOML.
- The terminal is our own block system with block-based UX.
- macOS platform code uses `objc2` (+ `objc2-app-kit` + `objc2-foundation`), never `cocoa`/`objc`.
