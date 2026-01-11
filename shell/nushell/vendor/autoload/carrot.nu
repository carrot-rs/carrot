# Carrot Terminal — Nushell Integration
#
# OSC 133 (block boundaries) is handled natively by Nushell's reedline.
# This script adds Carrot-specific features:
# - OSC 7777 metadata (CWD, git branch, username, shell info)
# - sudo wrapper preserving TERMINFO

let features = ($env.CARROT_SHELL_FEATURES? | default "metadata,sudo" | split row ",")

if "metadata" in $features {
    $env.config = ($env.config | upsert hooks {|config|
        let existing = ($config.hooks? | default {})
        let existing_pre_prompt = ($existing.pre_prompt? | default [])

        $existing | upsert pre_prompt ($existing_pre_prompt | append {||
            mut meta = {
                cwd: ($env.PWD),
                shell: "nu",
            }

            # Username
            $meta = ($meta | upsert username (whoami | str trim))

            # Git info (only if in a git repo)
            let git_check = (do { git rev-parse --git-dir } | complete)
            if $git_check.exit_code == 0 {
                let git_root = (git rev-parse --show-toplevel | str trim)
                let branch = (git rev-parse --abbrev-ref HEAD | str trim)
                let dirty = ((do { git diff --quiet HEAD } | complete).exit_code != 0)
                $meta = ($meta | upsert git_root $git_root | upsert git_branch $branch | upsert git_dirty $dirty)
            }

            # Command duration from last command
            if ($env.CMD_DURATION_MS? | is-not-empty) {
                $meta = ($meta | upsert last_duration_ms ($env.CMD_DURATION_MS | into int))
            }

            let hex = ($meta | to json -r | encode hex)
            print -n $"\e]7777;carrot-precmd;($hex)\u{07}"
        })
    })
}

if "sudo" in $features {
    # Wrap sudo to preserve TERMINFO for proper terminal rendering
    def --wrapped carrot-sudo [...args: string] {
        if ("-e" in $args) or ("--edit" in $args) {
            ^sudo ...$args
        } else {
            let terminfo = ($env.TERMINFO? | default "")
            ^sudo $"TERMINFO=($terminfo)" ...$args
        }
    }
}

# TUI hint: emit OSC 7777 carrot-tui-hint for known TUI commands via the
# pre_execution hook, before the command writes its first byte. Matches
# the colon-separated $env.CARROT_KNOWN_TUIS list set by pty.rs.
$env.config = ($env.config | upsert hooks {|config|
    let existing = ($config.hooks? | default {})
    let existing_pre_exec = ($existing.pre_execution? | default [])

    $existing | upsert pre_execution ($existing_pre_exec | append {|cmd|
        let tuis = ($env.CARROT_KNOWN_TUIS? | default "" | split row ":")
        if ($tuis | is-empty) {
            return
        }
        # Skip when the command carries a non-interactive flag.
        if ($cmd =~ ' (--version|--help|-V|-h|-\?)( |$)') {
            return
        }
        let first_token = ($cmd | split row ' ' | get 0? | default "")
        if ($first_token | is-empty) {
            return
        }
        let bare = ($first_token | split row '/' | last)
        if ($bare in $tuis) {
            let hex = ('{"tui_mode":true}' | encode hex)
            print -n $"\e]7777;carrot-tui-hint;($hex)\u{07}"
        }
    })
})
