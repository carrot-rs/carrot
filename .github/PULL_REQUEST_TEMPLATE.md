<!--
  Title format: `type(scope): subject`  (Conventional Commits)

  type     = feat | fix | perf | refactor | docs | test | chore | build | ci
  scope    = the affected crate or area (term, grid, workspace, cli-agents, …)
  subject  = imperative mood, lowercase, no trailing period

  Examples:
    feat(term): wide-character ghost cells in VtWriter
    fix(workspace): clamp pane index after session close
    perf(grid): cache shaped lines per frozen block
    refactor(cli-agents): extract hook-event router
    docs(readme): clarify Nushell setup steps
-->

Closes #<!-- issue number — required. Use `Refs #N` for partial coverage. Multiple `Closes`/`Refs` lines are fine. -->

## What & why

<!-- One short paragraph. User-visible change first, then the motivation. -->

## How

<!-- Non-obvious implementation decisions, tradeoffs, alternatives considered and rejected.
     Skip if the change is trivial. -->

## Test plan

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Manually exercised in `carrot` — command(s): ………
- [ ] Cross-platform (macOS / Linux / Windows) verified, or explicitly N/A

## Performance impact

<!-- Did this touch a perf-sensitive path (rendering, VT parser, page list, scrollback,
     shape cache, GPU pipeline)? If yes, paste numbers from `cargo bench` / `criterion`
     before vs. after.
     Delete this section if not applicable. -->

## Visual changes

<!-- Before / after screenshots, side by side, or a short screen capture (drag the
     file or video into this textarea).
     Delete this section if not applicable. -->

## Breaking changes

<!-- Public API breaks, settings or keymap format changes, behavior shifts users will
     notice on upgrade. If yes, also update README / ARCHITECTURE.md / CLAUDE.md and
     mention the migration path here.
     Delete this section if none. -->

## Documentation

<!-- Did user-visible behavior change? If yes, what doc landed in this PR — or is filed
     as a follow-up issue?
     Delete this section if not applicable. -->

## AI assistance

<!-- If a significant portion of this PR was drafted with an AI tool (Claude Code,
     Cursor, Copilot, etc.), name the tool and confirm a human reviewed every line
     before push. Trivial completions don't need to be disclosed.
     Delete this section if not applicable. -->

## Notes for reviewers

<!-- Tricky invariants, intentional API choices, areas you're unsure about, planned
     follow-ups in subsequent PRs.
     Delete this section if not needed. -->
