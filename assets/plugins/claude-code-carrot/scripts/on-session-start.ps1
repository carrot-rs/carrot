# SessionStart hook forwarder.
#
# In addition to the generic emit-event.ps1 forwarding, we optionally
# dot-source $env:CLAUDE_ENV_FILE if Claude Code exposed one. The file
# may be a PowerShell script (.ps1) or a plain env-file; in the latter
# case we parse KEY=VALUE lines and set them as environment variables.

$ErrorActionPreference = 'Stop'

if ($env:CLAUDE_ENV_FILE -and (Test-Path $env:CLAUDE_ENV_FILE)) {
    $envPath = $env:CLAUDE_ENV_FILE
    if ($envPath.EndsWith('.ps1')) {
        . $envPath
    } else {
        Get-Content -LiteralPath $envPath | ForEach-Object {
            $line = $_.Trim()
            if ($line -and -not $line.StartsWith('#')) {
                $split = $line -split '=', 2
                if ($split.Length -eq 2) {
                    Set-Item -Path "env:$($split[0])" -Value $split[1]
                }
            }
        }
    }
}

$stdin = [Console]::In.ReadToEnd()
$stdin | & (Join-Path $PSScriptRoot 'emit-event.ps1') 'SessionStart'
