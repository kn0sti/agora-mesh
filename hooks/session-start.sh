#!/usr/bin/env bash
# Starts the agora daemon if this machine is set up. Never fails the session.
root="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
# `agora status` starts a stopped daemon; before `agora setup` it only prints a hint.
"$root/bin/agora" status >/dev/null 2>&1 || true
exit 0
