#!/usr/bin/env bash
# Carrot (キャロット) Shell Integration for Bash
# Sends OSC 133 markers for block boundary detection and
# OSC 7777 metadata (JSON, hex-encoded) for context chips.
# Loaded automatically via --rcfile when Carrot spawns a PTY.

# Guard against double-sourcing
[[ -n "$_CARROT_HOOKED" ]] && return
_CARROT_HOOKED=1

_carrot_state=0

# Open fd to TTY directly
if ! exec {_carrot_fd}>/dev/tty 2>/dev/null; then
    _carrot_fd=1
fi

_carrot_precmd() {
    local ret=$?

    if [[ $_carrot_state == 1 ]]; then
        builtin printf '\e]133;D;%d\a' "$ret" >&$_carrot_fd
    fi

    # --- Gather metadata ---
    local _cwd="$PWD"
    local _user="$USER"
    local _host="${HOSTNAME:-$(hostname -s 2>/dev/null || echo unknown)}"
    local _shell="bash"
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

    # Build JSON — escape backslashes and quotes in values
    local _json='{'
    _json+='"cwd":"'"${_cwd//\\/\\\\}"'",'
    _json+='"username":"'"${_user//\\/\\\\}"'",'
    _json+='"hostname":"'"${_host//\\/\\\\}"'",'
    _json+='"shell":"'"$_shell"'"'
    if [[ -n "$_git_branch" ]]; then
        _json+=',"git_branch":"'"${_git_branch//\\/\\\\}"'"'
        _json+=',"git_dirty":'"$_git_dirty"
        _json+=',"git_root":"'"${_git_root//\\/\\\\}"'"'
    fi
    if [[ $_carrot_state == 1 ]]; then
        _json+=',"last_exit_code":'"$ret"
        if [[ -n "$_carrot_cmd_start" ]]; then
            local _now_ms=$(date +%s%3N 2>/dev/null || echo 0)
            local _dur_ms=$(( _now_ms - _carrot_cmd_start ))
            _json+=',"last_duration_ms":'"${_dur_ms}"
            unset _carrot_cmd_start
        fi
    fi
    _json+='}'

    # Hex-encode JSON to prevent escape sequence breakage
    local _hex
    _hex=$(builtin printf '%s' "$_json" | xxd -p | tr -d '\n')

    # Send metadata via custom OSC 7777
    builtin printf '\e]7777;carrot-precmd;%s\a' "$_hex" >&$_carrot_fd

    builtin printf '\e]133;A\a' >&$_carrot_fd
    _carrot_state=0
}

_carrot_preexec() {
    # Avoid firing for PROMPT_COMMAND itself
    if [[ "$BASH_COMMAND" == "$PROMPT_COMMAND" ]]; then
        return
    fi

    _carrot_cmd_start=$(date +%s%3N 2>/dev/null || echo 0)

    # Emit the about-to-run command line via the OSC 7777 channel that
    # the precmd-time emit uses. ShellContext merges the two emits
    # field-by-field. Sent BEFORE OSC 133;C so it's buffered when
    # CommandStart fires.
    local _cmd="$BASH_COMMAND"
    local _esc="${_cmd//\\/\\\\}"
    _esc="${_esc//\"/\\\"}"
    _esc="${_esc//$'\n'/\\n}"
    _esc="${_esc//$'\r'/\\r}"
    _esc="${_esc//$'\t'/\\t}"
    local _cmd_json='{"command":"'"$_esc"'"}'
    local _cmd_hex
    _cmd_hex=$(builtin printf '%s' "$_cmd_json" | xxd -p | tr -d '\n')
    builtin printf '\e]7777;carrot-precmd;%s\a' "$_cmd_hex" >&$_carrot_fd

    builtin printf '\e]133;B\a' >&$_carrot_fd
    builtin printf '\e]133;C\a' >&$_carrot_fd

    # Emit TUI hint for known TUI commands. Skip when a non-interactive
    # flag is present — --version / --help and friends produce plain text.
    if [[ -n "$CARROT_KNOWN_TUIS" && ! "$BASH_COMMAND" =~ [[:space:]](--version|--help|-V|-h|-\?)([[:space:]]|$) ]]; then
        local _cmd_first="${BASH_COMMAND%% *}"
        _cmd_first="${_cmd_first##*/}"
        local _tuis
        IFS=':' read -ra _tuis <<< "$CARROT_KNOWN_TUIS"
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

# Install precmd via PROMPT_COMMAND
if [[ -z "$PROMPT_COMMAND" ]]; then
    PROMPT_COMMAND='_carrot_precmd'
else
    PROMPT_COMMAND="_carrot_precmd;${PROMPT_COMMAND}"
fi

# Install preexec via DEBUG trap
trap '_carrot_preexec' DEBUG

# Source user's bashrc if it exists
if [[ -f "$HOME/.bashrc" ]]; then
    source "$HOME/.bashrc"
fi

# Carrot Mode: no shell-side prompt suppression needed.
# The Carrot renderer hides prompt rows on the Rust side.
