#!/usr/bin/env fish
# Carrot (キャロット) Shell Integration for Fish
# Sends OSC 133 markers for block boundary detection and
# OSC 7777 metadata (JSON, hex-encoded) for context chips.

if set -q _CARROT_HOOKED
    exit 0
end
set -g _CARROT_HOOKED 1

set -g _carrot_state 0

function _carrot_precmd --on-event fish_prompt
    set -l ret $status

    if test $_carrot_state -eq 1
        printf '\e]133;D;%d\a' $ret
    end

    # --- Gather metadata ---
    set -l _cwd (pwd)
    set -l _user $USER
    set -l _host (hostname -s 2>/dev/null; or echo unknown)
    set -l _shell "fish"
    set -l _git_branch ""
    set -l _git_dirty "false"

    set -l _git_root ""

    if git rev-parse --git-dir >/dev/null 2>&1
        set _git_root (git rev-parse --show-toplevel 2>/dev/null)
        set _git_branch (git rev-parse --abbrev-ref HEAD 2>/dev/null)
        if test -n "$_git_branch"; and not git diff --quiet HEAD 2>/dev/null
            set _git_dirty "true"
        end
    end

    # Build JSON
    set -l _json '{'
    set _json "$_json\"cwd\":\"$_cwd\","
    set _json "$_json\"username\":\"$_user\","
    set _json "$_json\"hostname\":\"$_host\","
    set _json "$_json\"shell\":\"$_shell\""
    if test -n "$_git_branch"
        set _json "$_json,\"git_branch\":\"$_git_branch\""
        set _json "$_json,\"git_dirty\":$_git_dirty"
        set _json "$_json,\"git_root\":\"$_git_root\""
    end
    if test $_carrot_state -eq 1
        set _json "$_json,\"last_exit_code\":$ret"
        if set -q _carrot_cmd_start
            set -l _now_ms (date +%s%3N 2>/dev/null; or echo 0)
            set -l _dur_ms (math "$_now_ms - $_carrot_cmd_start")
            set _json "$_json,\"last_duration_ms\":$_dur_ms"
            set -e _carrot_cmd_start
        end
    end
    set _json "$_json}"

    # Hex-encode JSON to prevent escape sequence breakage
    set -l _hex (printf '%s' "$_json" | xxd -p | string join '')

    # Send metadata via custom OSC 7777
    printf '\e]7777;carrot-precmd;%s\a' "$_hex"

    printf '\e]133;A\a'
    set -g _carrot_state 0
end

function _carrot_preexec --on-event fish_preexec
    set -g _carrot_cmd_start (date +%s%3N 2>/dev/null; or echo 0)

    # Emit the about-to-run command line via the OSC 7777 channel that
    # the precmd-time emit uses. ShellContext merges the two emits
    # field-by-field. Sent BEFORE OSC 133;C so it's buffered when
    # CommandStart fires.
    set -l _cmd "$argv[1]"
    set -l _esc (string replace -a '\\' '\\\\' -- $_cmd)
    set _esc (string replace -a '"' '\\"' -- $_esc)
    set _esc (string replace -a \n '\\n' -- $_esc)
    set _esc (string replace -a \r '\\r' -- $_esc)
    set _esc (string replace -a \t '\\t' -- $_esc)
    set -l _cmd_json '{"command":"'$_esc'"}'
    set -l _cmd_hex (printf '%s' "$_cmd_json" | xxd -p | string join '')
    printf '\e]7777;carrot-precmd;%s\a' "$_cmd_hex"

    printf '\e]133;B\a'
    printf '\e]133;C\a'

    # Emit TUI hint for known TUI commands. Skip when a non-interactive
    # flag is present.
    if set -q CARROT_KNOWN_TUIS
        set -l _cmd_line "$argv[1]"
        if not string match -qr -- ' (--version|--help|-V|-h|-\\?)( |$)' " $_cmd_line"
            set -l _cmd_first (string split ' ' -- $_cmd_line)[1]
            set _cmd_first (string split '/' -- $_cmd_first)[-1]
            for _t in (string split ':' -- $CARROT_KNOWN_TUIS)
                if test "$_cmd_first" = "$_t"
                    set -l _hex (printf '%s' '{"tui_mode":true}' | xxd -p | string join '')
                    printf '\e]7777;carrot-tui-hint;%s\a' $_hex
                    break
                end
            end
        end
    end

    set -g _carrot_state 1
end

# Carrot Mode: no shell-side prompt suppression needed.
# The Carrot renderer hides prompt rows on the Rust side.
