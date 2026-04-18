#!/bin/sh
# Thin forwarder: hand the Claude Code hook payload (stdin) to
# emit-event.sh with the event name "InstructionsLoaded".
set -eu
cat | "$(dirname "$0")/emit-event.sh" InstructionsLoaded
