#!/bin/sh
# Thin forwarder: hand the Claude Code hook payload (stdin) to
# emit-event.sh with the event name "PermissionRequest".
set -eu
cat | "$(dirname "$0")/emit-event.sh" PermissionRequest
