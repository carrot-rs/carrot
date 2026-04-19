# Emit a single Carrot CLI-agent-event OSC 7777 sequence.
#
# Usage:
#   emit-event.ps1 <event-name>
#     stdin = raw Claude Code hook payload (JSON)
#     stdout = OSC 7777 escape sequence with hex-encoded envelope
#
# Envelope shape (consumed by carrot-cli-agents::parse_envelope):
#   {
#     "type": "cli_agent_event",
#     "agent": "claude_code",
#     "protocol_version": 1,
#     "event": "<event-name>",
#     "payload": <hook-payload>
#   }
#
# Hex encoding uses the UTF-8 byte representation — two hex chars per
# byte — matching the POSIX variant and the carrot-terminal decoder.

param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$EventName
)

$ErrorActionPreference = 'Stop'

$payload = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($payload)) {
    $payload = '{}'
}

$wrapper = '{"type":"cli_agent_event","agent":"claude_code","protocol_version":1,"event":"' + $EventName + '","payload":' + $payload + '}'

$bytes = [System.Text.Encoding]::UTF8.GetBytes($wrapper)
$hex = [System.BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant()

[Console]::Out.Write("`e]7777;$hex`a")
