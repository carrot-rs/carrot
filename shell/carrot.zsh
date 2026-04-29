#!/usr/bin/env zsh
# Carrot (キャロット) Shell Integration for Zsh
# Sends OSC 133 markers for block boundary detection and
# OSC 7777 metadata (JSON, hex-encoded) for context chips.
# Loaded automatically via ZDOTDIR injection when Carrot spawns a PTY.

# Guard against double-sourcing
[[ -n "$_CARROT_HOOKED" ]] && return
_CARROT_HOOKED=1

# State machine: 0=idle, 1=command executing (OSC 133;C sent, D not yet)
_carrot_state=0

# Write OSC markers to stdout (fd 1) so they travel through the PTY in the
# same byte stream as command output. This guarantees correct ordering:
# CommandStart → output → CommandEnd.
# preexec/precmd always have the original stdout (not redirected by user
# commands), so fd 1 is safe here. Using /dev/tty opens a separate fd to
# the same PTY device, which can cause race conditions where markers
# arrive before command output.
_carrot_fd=1

_carrot_precmd() {
    local ret=$?

    # Close previous command block with exit code
    if [[ $_carrot_state == 1 ]]; then
        builtin printf '\e]133;D;%d\a' "$ret" >&$_carrot_fd
    fi

    # --- Gather metadata ---
    local _cwd="$PWD"
    local _user="$USER"
    local _host="${HOST:-$(hostname -s 2>/dev/null || echo unknown)}"
    local _shell="zsh"
    local _git_branch=""
    local _git_dirty="false"

    local _git_root=""

    if git rev-parse --git-dir >/dev/null 2>&1; then
        _git_root=$(git rev-parse --show-toplevel 2>/dev/null)
        _git_branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
        if [[ -n "$_git_branch" ]] && ! git diff --quiet HEAD 2>/dev/null; then
            _git_dirty="true"
        fi
    fi

    # Build JSON — values are escaped to handle quotes/backslashes in paths
    local _json='{'
    _json+='"cwd":"'${_cwd//\\/\\\\}'",'
    _json+='"username":"'${_user//\\/\\\\}'",'
    _json+='"hostname":"'${_host//\\/\\\\}'",'
    _json+='"shell":"'$_shell'"'
    if [[ -n "$_git_branch" ]]; then
        _json+=',"git_branch":"'${_git_branch//\\/\\\\}'"'
        _json+=',"git_dirty":'$_git_dirty
        _json+=',"git_root":"'${_git_root//\\/\\\\}'"'
    fi
    if [[ $_carrot_state == 1 ]]; then
        _json+=',"last_exit_code":'$ret
        # Calculate command duration in milliseconds
        if [[ -n "$_carrot_cmd_start" ]]; then
            local _dur_ms=$(( (EPOCHREALTIME - _carrot_cmd_start) * 1000 ))
            _dur_ms=${_dur_ms%.*}
            _json+=',"last_duration_ms":'${_dur_ms:-0}
            unset _carrot_cmd_start
        fi
    fi
    _json+='}'

    # Hex-encode JSON to prevent bytes like 0x9C (ST terminator in emoji)
    # from breaking the OSC escape sequence.
    local _hex
    _hex=$(builtin printf '%s' "$_json" | xxd -p | tr -d '\n')

    # Send metadata via custom OSC 7777
    builtin printf '\e]7777;carrot-precmd;%s\a' "$_hex" >&$_carrot_fd

    # Mark prompt start
    builtin printf '\e]133;A\a' >&$_carrot_fd

    _carrot_state=0
}

_carrot_preexec() {
    # Record command start time (milliseconds via zsh EPOCHREALTIME)
    zmodload -F zsh/datetime p:EPOCHREALTIME 2>/dev/null
    _carrot_cmd_start=$EPOCHREALTIME

    # Emit the about-to-run command line via the same OSC 7777 channel
    # the precmd-time emit uses. Reusing `carrot-precmd` keeps the
    # parser path single — a new emit just merges into ShellContext
    # field-by-field. JSON escapes backslashes / quotes / newlines so
    # multiline commands and embedded quotes survive intact. Sent
    # BEFORE OSC 133;C so the metadata is buffered when CommandStart
    # fires.
    local _cmd="$1"
    local _esc="${_cmd//\\/\\\\}"
    _esc="${_esc//\"/\\\"}"
    _esc="${_esc//$'\n'/\\n}"
    _esc="${_esc//$'\r'/\\r}"
    _esc="${_esc//$'\t'/\\t}"
    local _cmd_json='{"command":"'$_esc'"}'
    local _cmd_hex
    _cmd_hex=$(builtin printf '%s' "$_cmd_json" | xxd -p | tr -d '\n')
    builtin printf '\e]7777;carrot-precmd;%s\a' "$_cmd_hex" >&$_carrot_fd

    # Mark prompt end / input region start
    builtin printf '\e]133;B\a' >&$_carrot_fd

    # Mark command execution start / output region start
    builtin printf '\e]133;C\a' >&$_carrot_fd

    # Emit TUI hint (OSC 7777;carrot-tui-hint) when the command matches
    # the colon-separated CARROT_KNOWN_TUIS list. Skip when the command
    # line carries a non-interactive flag (--version, --help, -V, -h) —
    # those are plain text output, not TUIs.
    if [[ -n "$CARROT_KNOWN_TUIS" && ! "$1" =~ [[:space:]](--version|--help|-V|-h|-\?)([[:space:]]|$) ]]; then
        local _cmd_first="${1%% *}"
        _cmd_first="${_cmd_first##*/}"
        local _oldIFS="$IFS"
        IFS=':'
        local _tuis=(${=CARROT_KNOWN_TUIS})
        IFS="$_oldIFS"
        local _t
        for _t in "${_tuis[@]}"; do
            if [[ "$_cmd_first" == "$_t" ]]; then
                local _hex
                _hex=$(builtin printf '%s' '{"tui_mode":true}' | xxd -p | tr -d '\n')
                builtin printf '\e]7777;carrot-tui-hint;%s\a' "$_hex" >&$_carrot_fd
                break
            fi
        done
    fi

    _carrot_state=1
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _carrot_precmd
add-zsh-hook preexec _carrot_preexec

# Carrot Mode: no shell-side prompt suppression needed.
# The Carrot renderer hides prompt rows (between PromptStart and CommandStart)
# on the Rust side — shell-agnostic, works with any prompt system.
