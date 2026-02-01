//! Keystroke → VT-byte encoding for the terminal pane.
//!
//! Consumed by the `KeyDown` handler on [`super::TerminalPane`].
//! Translates a single keystroke into the bytes the terminal wants
//! to see on its PTY, honouring current terminal mode flags
//! (`DECCKM` decides application vs normal cursor keys, etc.).

use carrot_term::term::TermMode;
use carrot_terminal::Terminal;

/// Encode one keystroke as the bytes the PTY expects. Returns an
/// empty vec for keystrokes that carry no payload (pure modifier
/// press, bare system key we don't map).
pub(crate) fn keystroke_to_bytes(keystroke: &inazuma::Keystroke, terminal: &Terminal) -> Vec<u8> {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();

    let handle = terminal.handle();
    let term = handle.lock();
    let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
    drop(term);

    let prefix = if app_cursor { "\x1bO" } else { "\x1b[" };

    if modifiers.control {
        if let Some(ch) = key.chars().next()
            && ch.is_ascii_alphabetic()
        {
            let ctrl_byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
            return vec![ctrl_byte];
        }
        if key == "space" {
            return vec![0x00];
        }
    }

    match key {
        "enter" | "return" => return b"\r".to_vec(),
        "backspace" => return vec![0x7f],
        "tab" => return b"\t".to_vec(),
        "escape" => return vec![0x1b],
        "up" => return format!("{}A", prefix).into_bytes(),
        "down" => return format!("{}B", prefix).into_bytes(),
        "right" => return format!("{}C", prefix).into_bytes(),
        "left" => return format!("{}D", prefix).into_bytes(),
        "home" => return b"\x1b[H".to_vec(),
        "end" => return b"\x1b[F".to_vec(),
        "delete" => return b"\x1b[3~".to_vec(),
        "pageup" => return b"\x1b[5~".to_vec(),
        "pagedown" => return b"\x1b[6~".to_vec(),
        "space" => {
            if modifiers.alt {
                return b"\x1b ".to_vec();
            }
            return b" ".to_vec();
        }
        _ => {}
    }

    if modifiers.alt
        && let Some(ref key_char) = keystroke.key_char
    {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(key_char.as_bytes());
        return bytes;
    }

    if let Some(ref key_char) = keystroke.key_char {
        return key_char.as_bytes().to_vec();
    }

    Vec::new()
}
