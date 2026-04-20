#!/bin/sh
# Thin forwarder: hand the Claude Code hook payload (stdin) to
# emit-event.sh with the event name "Stop".
set -eu
cat | "$(dirname "$0")/emit-event.sh" Stop
