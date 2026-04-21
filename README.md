<p align="center">
  <img src=".github/assets/header.png" alt="Carrot" width="100%" />
</p>

<h1 align="center">キャロット Carrot</h1>

<p align="center">
  <strong>The terminal-first agentic development environment.</strong><br />
  Built from scratch in Rust on a custom GPU UI framework.
</p>

<p align="center">
  <em>Per-command GPU grids. Native Nushell. TUI-aware. Semantic command AST.<br />
  AI ghost-text. 715 CLI completions built in. Zero prompt plugins, zero shell plugins.</em>
</p>

<p align="center">
  <a href="#why-carrot">Why</a> ·
  <a href="#what-makes-it-different">What makes it different</a> ·
  <a href="#feature-matrix">Feature matrix</a> ·
  <a href="#performance">Performance</a> ·
  <a href="#install">Install</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#architecture">Architecture</a>
</p>

---

## Why Carrot

Modern terminals force a choice. Either you run an emulator that is fast and faithful but looks like 1995, or you run something with a block UI and lose raw speed, scrollback integrity, and the ability to run the tools you actually use. Carrot refuses that trade.

Carrot is a **terminal-first Agentic Development Environment (ADE)**. Every session starts as a real terminal with a real PTY. The block system, structured-output rendering, context chips, completions, the semantic command AST, AI ghost-text, and agent integrations all sit on top of a conformant, SIMD-accelerated VT state machine — not around it. Panels (file tree, agent, git, debug, outline, collab, notifications) are registered but hidden by default; you open what you need when you need it.

The rendering engine is **[Inazuma](crates/inazuma/) (稲妻)** — our own GPU UI framework with native Metal on macOS, Vulkan/DX12 via WGPU on Linux and Windows. Sub-millisecond input latency, 120 fps compositing, MSDF-atlas glyph rendering, OKLCH color throughout, Display P3 wide gamut on capable displays.

## What makes it different

### Per-block terminal grids

Every command gets its own `BlockGrid` with its own cursor, scroll region, and state. No other terminal on the market does this. It is why Carrot can give you:

- command-accurate scrollback per block, not one flat buffer
- finalized blocks that stay intact when later commands repaint — blocks are **frozen** at completion and immutable from that point forward
- search that targets one block or all blocks with the same primitive
- folding, exit-status badges, timing, and structured-output rendering without heuristics layered over a flat stream
- a strict active/frozen split — running blocks are mutable, finished blocks are typed differently and cannot drift

### 8-byte packed cells, SIMD VT parser

The cell is **8 bytes**, packed. A 3-bit tag covers ASCII, codepoint, grapheme, wide-second, image, shaped-run, and custom-render cells — future-proof without waste. Scrollback uses a **4 KB mmap-aligned PageList**, not a ring buffer, so huge sessions page cleanly. The VT parser has an **AVX2/NEON SIMD fast path** for ASCII-heavy output (`yes`, `cat`, log tails) and a correct scalar fallback for everything else. 30 000-line sessions fit in **under 25 MB RAM**.

### TUI-aware block system — world first

Interactive TUIs that run in non-alt-screen mode — Ink-based CLIs, log-update pipelines, modern AI agent UIs — routinely stack their banners on top of each other inside other terminals because scrollback grows underneath the redraw. Carrot fixes this at three levels at once:

1. **DEC Private Mode 2026 (Synchronized Output)** — real BSU/ESU handling, damage batching, scrollback suppression for atomic redraws.
2. **Shell-level TUI hints** — the first terminal emulator to ship a shell preexec hook that tells the terminal "this is a TUI" before the first byte of output arrives. Zero heuristic lag.
3. **Cursor-up heuristic** — safety net for legacy TUIs that neither emit DEC 2026 nor get detected by the known-TUI list.

When a TUI finalizes, the **last frame is preserved** in scrollback as a single snapshot — not five hundred intermediate repaints. Every block stays scrollable, searchable, and clean.

### Semantic command AST — the invariant

The `carrot-cmdline` crate maintains a live, typed interpretation of what you are typing: `git` is a *command*, `checkout` is a *subcommand*, `ma` is a *partial positional of type `GitRef`*. The AST is rebuilt incrementally on every keystroke (p99 < 2 ms) from the 715 shipped specs plus the live shell state.

That single invariant — *the shell, the agent, and the accessibility layer all read the input through the same typed lens* — is what powers everything else:

- **Role-based syntax highlight.** Paths in one color, git-refs in another, URLs in a third. Schema-driven, not regex-driven.
- **Semantic validation.** `git checkout ma` gets a red underline on `ma` with the exact message "no branch or tag `ma` in the current repo" — not a parse error, an *understanding* error.
- **Typed completions.** Not "suggest tokens after `git checkout`" but "suggest the actual `GitRef`s in this repo".
- **Structured AI context.** The AI sees `{command: git, subcommand: checkout, partial: {kind: GitRef, value: "ma"}}`, not a raw string.
- **Structured screen-reader narration.** "git command, checkout subcommand, partial branch argument, three suggestions." See [Accessibility](#accessibility-first) below.

### AI ghost-text: Next Command

As you type, a muted-color prediction appears after the cursor. Tab accepts it, any keystroke cancels cleanly. Pluggable source:

- **Local** — Ollama, p99 under 50 ms. Your commands never leave the machine.
- **Cloud** — Anthropic, Bedrock, or any `carrot-cloud-llm-client` backend. P99 under 200 ms.
- **Off** — default for unconfigured installs.

Context sent to the model includes the semantic AST (not the raw line), the last five blocks with exit codes, CWD, git branch, and shell type. Budget overruns drop silently — a late prediction never flashes into the UI.

### Active-AI chips on failure

When a command exits non-zero, a dismissible chip appears inline: *"Ask Claude to fix this (Cmd+.)"* Pressing the shortcut pre-fills the prompt with `# fix: <last_command> (exit <code>)` and enters agent-handoff mode. Every error becomes a one-key escalation.

### Agent handoff with `#`

Type `#` as the first character and the cmdline switches mode: accent color, Enter routes to the agent instead of the PTY, the agent streams its answer as a collapsible block above the prompt. Escape returns you to the shell. The cmdline is the agent's input surface — not a separate toolbar, not a modal, not a plugin.

Agents can also **propose** a fill (`agent.cmdline.propose(cmd)`), **watch** edits in real time, or read the **current AST** — all gated behind an explicit per-session opt-in.

### Interactive-prompt re-entry — no more broken `sudo`

Most block terminals break when a command prompts mid-execution: `sudo`, `ssh` passphrase, `git push` with a credential helper, `npm init`, `docker login`, `read -p`. Carrot's `PromptState` is a four-state machine that detects these via OSC-133 re-emission and known-prompt patterns, mounts a transient masked input (for passwords) or plain input (for y/N), and routes keystrokes correctly — then cleanly hands control back to the block stream. It just works, including password masking that is **never** logged, exposed to AI context, or readable via the agent-edit API.

### MCP-native completions

Completion sources are layered: shell-native (zsh `_complete_help`, fish `complete -C`, nu built-in, bash `compgen`), spec-driven (715 Fig-style JSON specs), and **Model Context Protocol providers**. Any MCP-connected tool can register a completion provider by glob pattern (`kubectl *`, `git *`, `npm run *`) — internal CLIs, org-specific tooling, and AI agents all contribute suggestions through the same bridge.

### First-class Nushell

Carrot is the **first terminal with full native Nushell integration**. Tables, lists, and records render with type badges directly in the block header. Nushell's native OSC 133 support means **zero hook injection** — no ZDOTDIR trickery, no trap wrappers, no rc-file fragility. Fish 4.0's Rust-native AST is linked directly — no tree-sitter seam for fish lines, exact parser semantics. Bash, zsh, and nu ship with tree-sitter grammars registered and parsing off the main thread.

### Built-in context chips — goodbye, prompt plugins

**97 native context providers** ship in the box. Git branch, git status, git commit, git state, seven version-control systems. Language runtime detection for Node, Python, Rust, Go, Java, Kotlin, Scala, Ruby, PHP, Haskell, Elixir, Erlang, Nim, Zig, OCaml, Dart, Crystal, Deno, Bun, Swift, C, C++, and more. Cloud contexts for AWS, Azure, GCloud, Kubernetes, Docker, Terraform, Pulumi, Helm, Nix, OpenStack. System chips for battery, memory, jobs, shell level, hostname, localip, OS. All parallelized across Rayon, per-provider timeout-protected, none of them need an external binary to be installed.

No Starship. No Oh-My-Zsh. No Powerlevel10k. Ships empty, boots useful.

### 715 CLI completions — embedded, not downloaded

**715 JSON completion specs** compiled into the binary. Git, cargo, npm, pnpm, yarn, docker, kubectl, helm, aws, gcloud, terraform, ansible, brew, apt, systemctl, ssh, rsync, curl, ffmpeg — and 700 more. Fuzzy matching, descriptions, file paths, git branches and tags and remotes, env vars, process IDs. The same specs feed the semantic command AST — one source of truth, not duplicated schemas.

### Kitty, Sixel, iTerm2 images — native GPU pass

All three inline-image protocols land on a single `ImageStore` in `carrot-grid` and are drawn by a dedicated GPU pass in `carrot-block-render`. No terminal-side re-rasterization, no subprocess, no external viewer — the texture goes straight from the PTY stream to the GPU.

### MSDF glyph atlas

Text is rendered from a **Multi-Channel Signed Distance Field** atlas, not per-size bitmaps. Infinite zoom stays crisp, font-size changes are instant (no re-rasterization), memory is flat with respect to font size. Classic `swash` rasterization remains as a fallback for glyphs the MSDF pipeline can't handle.

### Accessibility, first-class

Because we own the semantic command AST, we can do something no other terminal does today: **narrate structure, not bytes**. Screen readers hear *"git command, checkout subcommand, branch argument, partial input m a, three suggestions: main, master, macos-fixes"* — not `git space checkout space m a`. AccessKit integration covers macOS, Windows, and Linux. IME composition strings and candidates surface as distinct nodes (CJK screen readers work), motion-reduction is honored for cursor blink, ghost-text fade-in, and completion dropdowns.

CLIs have historically been hostile to assistive tech. The combination of semantic AST and AccessKit makes Carrot **the first truly accessible terminal**.

### AI agent toolbar

Carrot auto-detects running AI agents — Claude Code, Codex, Gemini, Aider, Cline — and surfaces a native toolbar with file explorer, inline diff viewer, and MCP integration. Natural-language-to-command translation via `#` prefix (`# find all rust files modified in the last week`). The agent panel is first-class, not a plugin.

### Native directory jumping

Frecency-based directory jumping built into the terminal, driven by the shell integration's own history. Fuzzy directory switching without installing zoxide or autojump or z.lua.

### OKLCH color system

The only terminal using perceptually uniform OKLCH color space throughout. 120+ semantic tokens, 12-step auto-generated color scales, Display P3 wide-gamut support on Retina displays. Ships with seven themes — Carrot Dark, Carrot Light, Catppuccin Mocha, Dracula, Gruvbox Dark, Nord, One Dark — all OKLCH-derived.

### Single-item panes, session tabs

Each tab in the title bar is a **session**, not a pane. Each pane holds exactly one item — terminal or editor. Splits create new panes inside the session. File drops land in an editor pane without replacing your terminal. The last pane closed closes the session; the last session closed closes the window. No more twelve-pane accidents from a stray `Cmd+D`.

### Cross-platform by construction

One rendering engine, three backends. Metal on macOS, Vulkan/DX12 on Linux and Windows, WGPU under the hood. Platform code uses the modern `objc2` toolkit on macOS — no legacy Cocoa bindings anywhere. Every platform ships on day one.

## Feature matrix

| Capability | Carrot | Classic emulators | Block-style terminals |
|---|---|---|---|
| Per-command block grids (active/frozen split) | **yes, per-command GPU grid** | no | approximated via parsing |
| 8-byte packed cell, SIMD VT parser | **yes, AVX2/NEON fast path** | partial | no |
| Native Nushell structured output | **yes, GPU-rendered** | no | no |
| TUI-aware rendering (DEC 2026 + shell hint + heuristic) | **yes** | partial or none | no |
| Semantic command AST, typed positionals | **yes, 715 specs live-typed** | no | no |
| AI ghost-text (Next Command), local + cloud | **yes, <50 ms local** | no | partial |
| Agent handoff in the prompt (`#`) | **yes, first-class** | no | partial |
| Interactive mid-command prompts (sudo, ssh) | **yes, 4-state machine** | yes (raw) | **broken** |
| MCP-native completions | **yes** | no | no |
| Kitty + Sixel + iTerm2 images on one GPU pass | **yes** | partial | no |
| MSDF glyph atlas | **yes** | no | no |
| AccessKit + semantic-AST narration | **yes, structure-aware** | no | no |
| Context chips without Starship | **97 built in** | no | no |
| CLI completions without plugins | **715 specs embedded** | external | limited |
| AI agent integration | **first-class panel + toolbar** | no | partial |
| Directory jumping without zoxide | **built in, frecency** | no | no |
| OKLCH color pipeline | **yes, end to end** | no | no |
| Own GPU UI framework | **yes (Inazuma)** | no | varies |

## Performance

All numbers are binding acceptance criteria, measured with `criterion` benches and on-device profiling. CI fails any regression.

| Metric | Budget | Status |
|---|---|---|
| Cell size | exactly 8 bytes | enforced at compile time |
| Memory at 30 000 scrollback lines | < 25 MB | across platforms |
| Idle CPU per frame | < 1 ms | 60 fps idle |
| Resize dropped frames (4K ↔ 1080p) | 0 | native refresh rate |
| Append throughput | > 1 GB/s | SIMD fast path |
| termbench suite | < 30 s | standard suite |
| Keystroke latency (cmdline) | < 1 ms p99 | inherited from `carrot-editor` |
| Paste 10 MB into cmdline | < 50 ms | SumTree-backed |
| Semantic AST reparse per keystroke | < 2 ms p99 | incremental |
| AI ghost-text (local, Ollama) | < 50 ms p99 | round-trip |
| AI ghost-text (cloud) | < 200 ms p99 | round-trip |
| Interactive-prompt re-entry detection | < 5 ms | OSC-133 or pattern |
| History search over 100 k entries | < 5 ms p99 | fuzzy match |
| Tree-sitter reparse (1 000-line buffer) | < 10 ms p99 | off main thread |
| `#` handoff mode switch | < 1 frame (< 16 ms) | no visible lag |

## Install

```bash
git clone https://github.com/carrot-rs/carrot.git
cd carrot
cargo run -p carrot-app           # run it
cargo carrot dev                  # hot-reload dev loop
cargo carrot build                # release .app bundle (macOS)
cargo carrot build --debug        # debug .app bundle
cargo carrot icon                 # compile .icon → Assets.car
```

Requires Rust stable 1.94+ (pinned via `rust-toolchain.toml`, edition 2024, resolver 3). macOS 10.15.7 and later, Linux (Wayland or X11), Windows 10 and later.

## Configure

Config lives at `~/.config/carrot/config.toml`. TOML, not JSON. Hot-reloaded.

```toml
[general]
working_directory = "~"
input_mode = "carrot"          # "carrot" (native chips) or "shell" (raw PS1)

[appearance]
theme = "carrot-dark"          # carrot-dark, carrot-light, catppuccin-mocha,
                               # dracula, gruvbox-dark, nord, one-dark
font_family = "JetBrainsMono Nerd Font"
font_size = 14.0
window_opacity = 100           # 1–100, enables OS-level transparency < 100
reduced_motion = "auto"        # "auto" (follow OS), "on", "off"

[appearance.symbol_map]
"0xe5fa-0xe6b5" = "Symbols Nerd Font Mono"   # Nerd Font glyph ranges

[terminal]
scrollback_history = 10000
cursor_style = "bar"           # "bar", "block", "underline"
tui_awareness = "full"         # "full", "strict_protocol", "off"

[cmdline.ai]
source = "off"                 # "off", "ollama", "anthropic", "bedrock", "custom"
model = "qwen2.5-coder:7b"     # when source = "ollama"
# api_key_env = "ANTHROPIC_API_KEY"   # when source = "anthropic"

[cmdline.agent]
allow_edit = false             # per-session opt-in for agent-edit APIs
hash_prefix_handoff = true     # "#" first-char → agent mode
```

## Architecture

```
carrot-app           → Binary: main.rs + bootstrap only, zero logic
├── inazuma          → GPU UI framework (Metal / WGPU), 90+ modules
├── inazuma-component → 70+ UI primitives: input, tabs, chips, title bar, toolbar
├── inazuma-sum_tree  → B-tree of 128-byte chunks, dimensional cursors
├── inazuma-rope      → SumTree-backed rope, shared across editor + cmdline
├── carrot-grid       → 8-byte packed cell, 4 KB PageList, StyleAtlas, ImageStore
├── carrot-term       → SIMD VT parser (AVX2/NEON), active/frozen block split
├── carrot-block-render → GPU passes: MSDF text, image, cursor, decoration
├── carrot-terminal-view → Composes blocks + cmdline, tab/pane management
├── carrot-cmdline    → Semantic command AST, AI ghost-text, # handoff, MCP
├── carrot-editor     → Zed-grade editor (wrapped by cmdline via ErasedEditor)
├── carrot-workspace  → Sessions (tabs), panes (single-item), docks, toolbars
├── carrot-shell      → Window lifecycle (AppShell), open / close / reload
├── carrot-terminal   → PTY, OSC 133 + OSC 7777 parser, block manager, MCP bridge
├── carrot-chips      → 97 native context providers (parallel, Rayon-backed)
├── carrot-completions → 715 CLI specs, fuzzy matcher, path / git resolvers
├── carrot-shell-integration → Shell context, known-TUI list, OSC 7777 payloads
├── carrot-settings   → TOML config, live reload, global registry
├── carrot-theme      → OKLCH pipeline, 7 themes, wide-gamut
└── cargo-carrot      → Dev tooling (cargo carrot dev/build/icon)
```

240+ crates total. Strict layered architecture, dependencies flow downward only. The terminal pipeline is a clean five-layer stack — `carrot-grid` (cells) → `carrot-term` (VT + active/frozen blocks) → `inazuma::block` (primitive) → `carrot-block-render` (GPU passes) → `carrot-terminal-view` (composition). `carrot-cmdline` is a Layer-5 sibling that wraps `carrot-editor` via an `ErasedEditor` adapter and owns the semantic AST.

Shell integration ships in-tree: `shell/carrot.zsh`, `shell/carrot.bash`, `shell/carrot.fish`, and `shell/nushell/vendor/autoload/carrot.nu`. Zsh injects via `ZDOTDIR`, bash via `--rcfile`, fish via `conf.d`. Nushell plugs into its `pre_prompt` hook natively because Nu already emits OSC 133 — no hooks to inject.

## Shell integration protocol

Carrot speaks a small, documented protocol. Any shell can implement it.

- **OSC 133 (FTCS)** — `A` = PromptStart, `B` = InputStart, `C` = CommandStart, `D;<exit>` = CommandEnd. Used for block boundary detection. Shells can **re-emit `B`** mid-block to hand control to the cmdline for interactive prompts (sudo, ssh, git credential).
- **OSC 7777 (Carrot metadata)** — hex-encoded JSON payloads. Prefixes:
  - `carrot-precmd` — CWD, git, user, host, duration, shell
  - `carrot-tui-hint` — `{"tui_mode": true}` before a TUI command runs
  - `carrot-interaction-hint` — `{"kind": "password" | "yes_no" | "free_text"}` for interactive-prompt classification
  - agent lifecycle events (Claude, Codex, Gemini, Aider, Cline)

Hex encoding prevents `0x9C` bytes (the ST terminator, which appears in emoji sequences) from breaking the escape. Payloads are `#[serde(default)]`, so new fields can be added without breaking shells that do not yet emit them.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every feature must be production-complete — no `todo!()`, no stubs, no silent swallowing. Comments are English, chat with maintainers is German or English.

## Security

See [SECURITY.md](SECURITY.md) for responsible disclosure.

## License

Carrot is licensed under the [Functional Source License, Version 1.1, Apache 2.0 Future License](LICENSE.md) (FSL-1.1-Apache-2.0). The license converts to Apache-2.0 on the second anniversary of each release.

---

<p align="center">
  <em>Carrot · キャロット · the terminal-first ADE. Because your terminal should move fast.</em>
</p>
