# Contributing to Carrot

Thanks for your interest in contributing to Carrot! This guide will help you get set up and familiar with our conventions.

## Development Setup

### Prerequisites

- **Rust nightly** (edition 2024, resolver 3) — install via `rustup install nightly`
- **macOS 10.15.7+** for Metal rendering (primary development platform)
- **Xcode Command Line Tools** (`xcode-select --install`)

### Getting Started

```bash
git clone https://github.com/carrot-rs/carrot.git
cd carrot

# Build the app
cargo build

# Run with hot-reload
cargo carrot dev

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace
```

### Useful Commands

| Command | Description |
|---------|-------------|
| `cargo build` | Build carrot-app (default workspace member) |
| `cargo carrot dev` | Hot-reload dev mode |
| `cargo carrot dev --release` | Hot-reload in release mode |
| `cargo carrot build` | Release build + .app bundle |
| `cargo test -p carrot-terminal` | Terminal tests (OSC parser, blocks) |
| `cargo test -p inazuma-macros` | Framework macro tests |
| `cargo test --workspace` | All tests |
| `cargo clippy --workspace` | Lint check |

## Code Conventions

### Rust

- **Edition 2024** on nightly with `resolver = "3"`
- **No `mod.rs`** — use `module_name.rs` (modern Rust module convention)
- **No stubs or placeholders** — every feature must be production-complete. No `todo!()`, no `unimplemented!()`, no silent error swallowing.
- **Clippy lints**: `dbg_macro` and `todo` are **denied**. Your code will not compile if these are present.

### Architecture

- The UI framework is called **Inazuma**, not GPUI. All imports use `inazuma::`.
- Modify Inazuma directly when it's cleaner than working around it in carrot-app.
- **Terminal rendering** (grid, PTY, cells) follows real terminal-emulator conventions: per-cell rendering, grid positioning, no per-line text shaping.
- **Editor/UI features** (input, completions, panels) use Inazuma's text system (`ShapedLine`, `TextRun`).

### File Organization

- Keep modules focused and modular — business logic never belongs in UI handlers.
- Prefer editing existing files over creating new ones.

## Pull Requests

- Create a feature branch from `main` (`feat/`, `fix/`, `refactor/`)
- Write clear commit messages that explain the *why*
- Ensure `cargo clippy --workspace` passes with no warnings
- Ensure `cargo test --workspace` passes
- Keep PRs focused — one logical change per PR

## Project Structure

See the [Architecture section](README.md#architecture) in the README for a crate dependency overview. Detailed roadmap and phase plans are in the `plan/` directory.
