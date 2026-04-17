# Thin forwarder: hand the Claude Code hook payload (stdin) to
# emit-event.ps1 with the event name "PreCompact".
$ErrorActionPreference = 'Stop'
$stdin = [Console]::In.ReadToEnd()
$stdin | & (Join-Path $PSScriptRoot 'emit-event.ps1') 'PreCompact'
