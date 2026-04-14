#!/bin/sh
# SessionStart hook forwarder.
#
# In addition to the generic emit-event.sh forwarding, we optionally
# source $CLAUDE_ENV_FILE if Claude Code exposed one — that file
# contains env-exports the agent wants applied to the session's
# environment (per Claude Code convention). Sourcing it here means any
# follow-up commands the user runs in the same terminal see the same
# environment the agent is working with.

set -eu

if [ -n "${CLAUDE_ENV_FILE:-}" ] && [ -f "${CLAUDE_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  . "${CLAUDE_ENV_FILE}"
fi

cat | "$(dirname "$0")/emit-event.sh" SessionStart
