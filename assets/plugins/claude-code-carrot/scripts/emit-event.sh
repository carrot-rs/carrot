#!/bin/sh
# Emit a single Carrot CLI-agent-event OSC 7777 sequence.
#
# Usage:
#   emit-event.sh <event-name>
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
# Hex encoding follows the existing OSC 7777 convention — two hex chars
# per byte — so emoji or control bytes inside the payload cannot
# accidentally terminate the escape sequence.

set -eu

event_name="$1"
payload=$(cat)

# If stdin was empty, fall back to an empty JSON object so the
# resulting envelope is still valid.
if [ -z "$payload" ]; then
  payload='{}'
fi

wrapper=$(printf '{"type":"cli_agent_event","agent":"claude_code","protocol_version":1,"event":"%s","payload":%s}' \
                 "$event_name" "$payload")

hex=$(printf '%s' "$wrapper" | od -An -tx1 | tr -d ' \n')

printf '\033]7777;%s\007' "$hex"
