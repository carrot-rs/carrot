#!/bin/sh
# Thin forwarder: hand the Claude Code hook payload (stdin) to
# emit-event.sh with the event name "PreCompact".
set -eu
cat | "$(dirname "$0")/emit-event.sh" PreCompact
