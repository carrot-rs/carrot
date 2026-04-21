# Carrot plugin for Claude Code

This plugin forwards every Claude Code hook event as an OSC 7777
envelope so [Carrot](https://github.com/nyxb/carrot) can surface live
agent metadata (status, block activity, tasks, worktrees, PR context,
rules, context-window usage, subagent lifecycle) *around* the Claude
Code TUI — never replacing it.

## How it works

For every supported hook, the plugin calls a tiny forwarder under
`scripts/on-<event>.sh` (POSIX) or `scripts/on-<event>.ps1` (Windows).
Each forwarder pipes the raw hook payload into `emit-event.sh` /
`emit-event.ps1`, which wraps it in the Carrot envelope and prints an
OSC 7777 escape sequence with a hex-encoded JSON body:

```
ESC ] 7777 ; <hex({
  "type": "cli_agent_event",
  "agent": "claude_code",
  "protocol_version": 1,
  "event": "<hook-name>",
  "payload": <raw Claude Code hook payload>
})> BEL
```

Carrot's terminal OSC parser decodes the envelope, and the
`carrot-cli-agents` crate dispatches it to the session manager.

## Covered events

All 21 hooks Claude Code exposes:

- `SessionStart`, `SessionEnd`
- `UserPromptSubmit`, `Stop`
- `Notification`, `PermissionRequest`
- `PreToolUse`, `PostToolUse` (async)
- `TaskCreated`, `TaskCompleted`
- `FileChanged` (async), `CwdChanged`
- `PreCompact`, `PostCompact`
- `InstructionsLoaded`
- `SubagentStart`, `SubagentStop`
- `WorktreeCreate`, `WorktreeRemove`
- `Elicitation`, `ElicitationResult`

The `SessionStart` forwarder additionally sources `$CLAUDE_ENV_FILE`
(POSIX: `.` / PowerShell: dot-source or KEY=VALUE parse) when Claude
Code exposes one, so the session inherits the env the agent expects.

## Installation

Carrot installs this plugin automatically on first Claude Code
detection (see the onboarding prompt). If you prefer to install it by
hand:

### End-user install (what Carrot does for you)

Copy the bundle to `~/.claude/plugins/carrot/` and make the POSIX
scripts executable:

```sh
dest="$HOME/.claude/plugins/carrot"
mkdir -p "$dest"
cp -R <carrot-source>/assets/plugins/claude-code-carrot/. "$dest/"
chmod 0755 "$dest"/scripts/*.sh
```

On Windows, `chmod` is unnecessary — PowerShell scripts run via
interpreter:

```powershell
$dest = "$env:USERPROFILE\.claude\plugins\carrot"
New-Item -ItemType Directory -Path $dest -Force | Out-Null
Copy-Item -Recurse -Force '<carrot-source>\assets\plugins\claude-code-carrot\*' $dest
```

### Plugin-developer install (via the Claude Code marketplace)

Useful when iterating on the plugin itself:

```sh
claude plugin marketplace add <path-to-this-directory>
claude plugin install carrot@<marketplace-name>
```

Both paths target the same on-disk layout and produce identical
behaviour; Carrot's first-run installer detects either variant and
will not overwrite a marketplace-managed plugin without consent.

## Verifying

After installation, run `claude` inside a Carrot terminal pane. You
should see Carrot's vertical-tabs panel pick up the session with the
Claude Code icon and a live status dot. For deeper debugging, set
`RUST_LOG=carrot_terminal=debug,carrot_cli_agents=debug` before
launching Carrot and watch for parsed `cli_agent_event` envelopes.

## Uninstalling

Either remove the directory manually:

```sh
rm -rf "$HOME/.claude/plugins/carrot"
```

Or use Carrot's command palette: `CLI Agents: Uninstall Carrot Plugin
for Claude Code`.

## Authoring notes

- All `.sh` scripts use `set -eu` so a broken payload fails fast
  rather than silently emitting half an envelope.
- All `.ps1` scripts use `$ErrorActionPreference = 'Stop'` for the
  same reason.
- Hex encoding (two chars per byte, lowercase) is mandatory — it
  prevents 0x07 (BEL) and 0x9C (ST) bytes inside payloads from
  terminating the escape sequence early.
- `emit-event.sh` uses `od` + `tr` because it is available in every
  POSIX base system; `emit-event.ps1` uses `System.BitConverter` to
  match byte-for-byte.
