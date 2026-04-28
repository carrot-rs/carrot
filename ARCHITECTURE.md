# Architecture

High-level map of Carrot's crates, organised by the six layers of the block system.

## Layers

```
┌──────────────────────────────────────────────────────────────────┐
│ Layer 5 — carrot-terminal-view    (composition / app integration) │
│            carrot-cmdline (NEW)    (command-entry surface)        │
├──────────────────────────────────────────────────────────────────┤
│ Layer 4 — carrot-block-render     (GPU passes: text/image/cursor) │
├──────────────────────────────────────────────────────────────────┤
│ Layer 3 — inazuma::block          (virtualised block primitive)   │
├──────────────────────────────────────────────────────────────────┤
│ Layer 2 — carrot-term             (VT parser, OSC dispatch, PTY)  │
├──────────────────────────────────────────────────────────────────┤
│ Layer 1 — carrot-grid             (Cell, PageList, StyleAtlas)    │
├──────────────────────────────────────────────────────────────────┤
│ Layer 0 — inazuma                 (GPU UI framework)              │
└──────────────────────────────────────────────────────────────────┘
```

Dependencies flow **downward only**. `carrot-grid` has zero `inazuma` / `carrot-*` deps; `carrot-block-render` depends on `inazuma` + `carrot-grid` but not on `carrot-term` (cross-layer tests use it for integration); `carrot-terminal-view` composes everything.

## Layer 1 — `carrot-grid`

Pure data primitives for terminal content. 8-byte packed `Cell`, `StyleAtlas` interning, `PageList` with O(1) append + O(1) scrollback prune + O(N_visible) range iter. Standalone; publishable on crates.io as-is. Details: [crates/carrot-grid/README.md](crates/carrot-grid/README.md).

## Layer 2 — `carrot-term`

VT state machine + PTY lifecycle + block data model. See [the crate's module-level docs](crates/carrot-term/src/lib.rs) for the internal split. `block::{ActiveBlock, FrozenBlock, VtWriter, ReplayBuffer}` + `simd_scan` AVX2/NEON fast-path.

## Layer 3 — `inazuma::block`

Virtualised-list primitive for terminal-shaped content. Bottom-anchor layout, per-entry fold state, pinned-footer mechanic, O(N_visible) cursor-based iteration. Lives inside `inazuma` because it needs direct access to the element system. Terminal-agnostic — `carrot-terminal-view` is its first consumer but not its only one.

## Layer 4 — `carrot-block-render`

GPU-bound rendering for terminal blocks. Composes every primitive into a single `build_frame` call. Details: [crates/carrot-block-render/README.md](crates/carrot-block-render/README.md).

## Layer 5 — App composition

- `carrot-terminal-view` hooks the render pass into the workspace.
- `carrot-cmdline` wraps `carrot-editor` for the command-entry surface.

## Project Scope Architecture

Each Workspace owns exactly one `Entity<Project>`. A `Project` holds any number of `Worktree`s. Each worktree carries three orthogonal attributes — **visibility**, **fs_watch**, **indexing** — captured as a single `WorktreeMode`:

| Mode         | Visible | Scanner | Use                                                   |
|--------------|---------|---------|-------------------------------------------------------|
| `Ephemeral`  | no      | no      | path anchor for buffers outside visible worktrees     |
| `Browseable` | yes     | no      | default for cwd-driven worktrees; lazy expand on click |
| `Tracked`    | yes     | yes     | user opt-in or auto-track policy; GitStore attaches   |

Worktrees are created through the mode-typed helpers on `Project`:

- `project.ensure_browseable_worktree(path, cx)` — lazy visible anchor. Cheap enough to call on every shell cwd change.
- `project.ensure_tracked_worktree(path, cx)` — full tracking, scanner active.
- `project.ensure_ephemeral_worktree(path, cx)` — path-anchor only, invisible.

Legacy `project.find_or_create_worktree(path, visible, cx)` is preserved for backward compatibility; new code paths should use the `ensure_*` helpers.

**Scope classification.** Terminal `cwd` changes run through `carrot_shell::scope_policy::classify(&cwd)`, which walks the ancestor chain for `.git`, `AGENTS.md`/`CLAUDE.md`/`WARP.md`, and package manifests. The first marker wins; absence of markers yields `WorktreeRoot::AdHoc` with the cwd itself as root.

**Auto-tracking** applies only to `ProjectKind::Git`. Other classes (AgentRules, Manifest, AdHoc) stay Browseable and must be upgraded explicitly via the `projects::TrackActiveScope` action. Settings: `worktree_scope.auto_track_git` (`never` | `ask` | `always`), `never_track_paths`, `always_track_paths`.

## Command Palette

Single quick-open modal for every kind of search. One crate (`carrot-command-palette`), four pre-filter shortcuts, a shared chip strip so users always see which scope is active.

### Shortcuts

| Shortcut | Scope | Action |
|----------|-------|--------|
| `Cmd+P` | universal | `command_palette::Toggle` |
| `Cmd+O` | Files | `command_palette::ToggleWithFilter { category_filter = "Files" }` |
| `Cmd+Shift+P` | Sessions | `command_palette::ToggleWithFilter { category_filter = "Sessions" }` |
| `Cmd+R` | History | `command_palette::ToggleWithFilter { category_filter = "History" }` |

Each opens the same modal; only the pre-selected chip differs. Linux/Windows swap `Cmd` for `Ctrl`.

### Categories

`SearchCategory` (in `category.rs`) lists every chip the modal can display. Adding a new one is a single enum variant plus four trait arms (`label`, `icon`, `icon_color`, `prefix`) — the chip strip iterates `SearchCategory::all()` so new categories appear automatically. Every category has both a long prefix (`history:`) and a single-letter short form (`h:`) for fast scoping.

| Category | Default-visible in universal mode | Source status |
|----------|-----------------------------------|---------------|
| Actions | yes | wired (dynamic via `CommandPaletteFilter` + frecency) |
| Sessions | yes | wired |
| Files | yes (recents only until query ≥ 2 chars) | wired (LiveWalker) |
| History | yes (top 50) | wired (falls back to disk when no terminal is focused) |
| EnvironmentVariables | no (chip-only) | wired |
| Workflows / Prompts / Notebooks / Drive / LaunchConfigurations / Conversations | no | reserved (future plan) |

### Source trait

```rust
pub trait SearchSource: Send + Sync {
    fn category(&self) -> SearchCategory;
    fn collect(&self, workspace: &Entity<Workspace>, query: &str, window: &Window, cx: &mut App)
        -> Vec<SearchResult>;
    fn default_visible(&self) -> bool { true }
    fn footer_status(&self, _cx: &App) -> Option<FilesSourceStatus> { None }
}
```

`SearchAction` is the runnable payload: `ActivateSession`, `CopyToClipboard`, `DispatchAction(Box<dyn Action>)`, `OpenPath(PathBuf)`. `OpenPath` routes through `Workspace::open_path_at_target_pane_for_role` with `PaneRole::Editor` so file-opens land in an editor pane instead of replacing a terminal.

### File handling

`FilesSource` spawns an `ignore::WalkBuilder` traversal on its own thread and drains the channel non-blocking each `collect()`. A shared `LiveWalkCache` global TTLs results per scope-root for 30 s so reopening the palette is instant. Include-ignored is togglable via a footer chip. A frecency list (`PoolState.recents`) surfaces recently-opened files for empty queries.

### History handling

`HistorySource` first reads the `ActiveCommandHistory` global — set by the focused `TerminalPane` on focus-in — so Cmd+R recalls commands from the exact terminal the user is in. Without a focused terminal it falls back to `CommandHistory::detect_and_load` from the shell's on-disk histfile. Enter dispatches `carrot_actions::command_palette::InsertIntoInput`, handled by `TerminalPane::on_insert_into_input`, which sets the input editor's value without executing.

### Deleted / absorbed

`carrot-file-finder` and the legacy Zed-style `carrot-command-palette` no longer exist. Their useful parts — the LiveWalker, the `CommandPaletteDB` frecency store, the `humanize_action_name` / `normalize_action_query` helpers — moved into the new crate. Every quick-open flow now dispatches `command_palette::Toggle` or `ToggleWithFilter`; no feature should reintroduce a parallel modal.

## Testing

Every layer's crate ships `cargo test` + `cargo bench` with concrete perf numbers recorded in the bench module docstrings (see `crates/*/benches/*.rs`). Cross-layer integration is covered by `crates/carrot-block-render/tests/full_pipeline.rs` — one test that exercises PageList + VtWriter + ActiveBlock + style atlas + image store + build_frame + damage + replay + grid-diff in a single realistic scenario. When this passes, every seam is wired correctly.

## Status

- Layer 1 `carrot-grid` — bench-validated.
- Layer 2 `carrot-term` — `block` + `simd_scan` landed.
- Layer 3 `inazuma::block` — fold, pin, and `visible_entries` in place.
- Layer 4 `carrot-block-render` — MSDF atlas + wgpu image textures covered.
- Layer 5 `carrot-terminal-view` + `carrot-cmdline` — composition layer.
