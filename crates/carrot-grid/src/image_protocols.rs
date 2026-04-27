//! Image-protocol payload parsers — terminal-image bytes → [`DecodedImage`].
//!
//! Three terminal image protocols share this module:
//!
//! - **iTerm2 Inline Images** (OSC 1337) — `\e]1337;File=key=value,…:base64\a`
//!   This module's [`parse_iterm2_payload`] handles the key-value header
//!   plus base64 + format-autodetect via the `image` crate.
//! - **Kitty Graphics** (APC `\e_G…`) — uses [`decode_image_bytes`] on the
//!   reassembled chunked payload (see Plan 31 A7.2 caller).
//! - **Sixel** (DCS) — owns its own state-machine-driven decoder (Plan 31
//!   A7.1), so it doesn't go through this module — it builds the
//!   `DecodedImage::Rgba8` pixel buffer directly.
//!
//! All three end up writing into the per-block [`ImageStore`].

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::image::{DecodedImage, ImageFormat, Placement};

/// Parsed iTerm2 OSC 1337 `File=` payload — key/value header + decoded
/// image. Built by [`parse_iterm2_payload`]; the caller pushes the
/// `image` plus a synthesised [`Placement`] into the active block's
/// [`crate::ImageStore`].
#[derive(Debug)]
pub struct ITerm2Image {
    pub image: Arc<DecodedImage>,
    /// `File=name=BASE64` — base64-encoded filename (informational; some
    /// clients render it as alt-text).
    pub name: Option<String>,
    /// `inline=1` flag. Almost always `true`; iTerm2 honours it as a
    /// hint to display the image rather than offering download. We
    /// preserve it so a future "save as" UI can branch on it.
    pub inline: bool,
    /// Cell width hint from `width=N` / `width=Npx` / `width=Nch` /
    /// `width=auto`. `None` if the header didn't specify it.
    pub width_cells: Option<u16>,
    /// Cell height hint from `height=…`. Same units as `width_cells`.
    pub height_cells: Option<u16>,
    /// `preserveAspectRatio=1` flag. Defaults to `true` when absent.
    pub preserve_aspect_ratio: bool,
}

/// Parse an iTerm2 OSC 1337 `File=…:base64` payload.
///
/// `payload` is the OSC body **after** the `1337;File=` prefix has been
/// stripped — i.e. the `key=value,key=value,…:base64` part. The OSC
/// terminator (BEL or `\e\\`) must already be removed.
///
/// Returns `None` if the payload is malformed: missing `:` separator,
/// invalid base64, unrecognisable image format, or the decoded buffer
/// is too small to be an image.
///
/// Reference: <https://iterm2.com/documentation-images.html>
pub fn parse_iterm2_payload(payload: &[u8]) -> Option<ITerm2Image> {
    // Header / data split on the first `:` byte. Some clients emit
    // bare `1337;File=base64` without any header; treat that as
    // empty-header + payload.
    let split = payload.iter().position(|&b| b == b':');
    let (header, data) = match split {
        Some(ix) => (&payload[..ix], &payload[ix + 1..]),
        None => (&[][..], payload),
    };

    let mut name: Option<String> = None;
    let mut inline = false;
    let mut width_cells: Option<u16> = None;
    let mut height_cells: Option<u16> = None;
    let mut preserve_aspect_ratio = true;

    if !header.is_empty() {
        let header_str = std::str::from_utf8(header).ok()?;
        for kv in header_str.split([',', ';']) {
            let Some(eq) = kv.find('=') else {
                continue;
            };
            let key = kv[..eq].trim();
            let value = kv[eq + 1..].trim();
            match key {
                "name" => {
                    if let Ok(decoded) = STANDARD.decode(value) {
                        name = String::from_utf8(decoded).ok();
                    }
                }
                "inline" => inline = matches!(value, "1" | "true" | "yes"),
                "width" => width_cells = parse_dimension_hint(value),
                "height" => height_cells = parse_dimension_hint(value),
                "preserveAspectRatio" => {
                    preserve_aspect_ratio = !matches!(value, "0" | "false" | "no");
                }
                // size, type and other keys are advisory and ignored.
                _ => {}
            }
        }
    }

    // base64 decode then format-detect via the `image` crate. iTerm2
    // sends PNG / JPEG / GIF / TIFF / BMP — `image::guess_format` covers
    // them all from the magic bytes.
    let bytes = STANDARD.decode(data).ok()?;
    let image = decode_image_bytes(&bytes)?;

    Some(ITerm2Image {
        image: Arc::new(image),
        name,
        inline,
        width_cells,
        height_cells,
        preserve_aspect_ratio,
    })
}

/// Decode raw image-format bytes (PNG / JPEG / GIF / BMP / TIFF / WebP)
/// into a [`DecodedImage`] with `Rgba8` pixels — the canonical format
/// the GPU upload pipeline expects. Returns `None` on unsupported
/// formats or decode errors.
///
/// Used by both iTerm2 and Kitty Graphics paths once the protocol's
/// transport (base64 / chunked) has been unwrapped to the raw image
/// bytes.
pub fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let dyn_img = image::load_from_memory(bytes).ok()?;
    let rgba = dyn_img.into_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba.into_raw();
    Some(DecodedImage::new(
        width,
        height,
        ImageFormat::Rgba8,
        pixels,
    ))
}

/// Build a [`Placement`] from a top-of-block anchor + cell dims +
/// optional iTerm2 width/height hints.
///
/// The terminal-side caller (where the image marker lands in the
/// active block) holds the current cursor row + viewport metrics.
/// Hints are clamped: at most `max_cols` wide, at most `max_rows`
/// tall. When both hints are absent the image consumes its native
/// pixel size mapped through `cell_w_px` / `cell_h_px`.
pub fn placement_from_iterm2(
    img: &ITerm2Image,
    row_start: u32,
    col_start: u16,
    cell_w_px: u16,
    cell_h_px: u16,
    max_rows: u16,
    max_cols: u16,
) -> Placement {
    let native_cols = ((img.image.width as u16).saturating_add(cell_w_px.saturating_sub(1)))
        / cell_w_px.max(1);
    let native_rows = ((img.image.height as u16).saturating_add(cell_h_px.saturating_sub(1)))
        / cell_h_px.max(1);
    let cols = img.width_cells.unwrap_or(native_cols).clamp(1, max_cols);
    let rows = img.height_cells.unwrap_or(native_rows).clamp(1, max_rows);
    Placement {
        row_start,
        col_start,
        rows,
        cols,
        offset_x: 0,
        offset_y: 0,
        external_id: 0,
    }
}

/// Parse iTerm2's width/height tokens. Accepts:
///
/// - `N` (raw cell count, decimal),
/// - `Nch` (cell count),
/// - `Npx` (pixel count, divided into cells later by the caller),
/// - `auto` / `N%` (percentage) — currently treated as "no hint".
fn parse_dimension_hint(value: &str) -> Option<u16> {
    if value == "auto" || value.is_empty() || value.ends_with('%') {
        return None;
    }
    let trimmed = value
        .strip_suffix("ch")
        .or_else(|| value.strip_suffix("px"))
        .unwrap_or(value);
    trimmed.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 transparent PNG (smallest legal IHDR + IDAT). base64-encoded.
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNgYAAAAAMAASsJTYQAAAAASUVORK5CYII=";

    #[test]
    fn parse_minimal_payload_with_no_header() {
        let payload = format!(":{TINY_PNG_B64}");
        let img = parse_iterm2_payload(payload.as_bytes()).expect("decoded");
        assert_eq!(img.image.width, 1);
        assert_eq!(img.image.height, 1);
        assert_eq!(img.image.format, ImageFormat::Rgba8);
        assert!(!img.inline);
        assert!(img.preserve_aspect_ratio);
    }

    #[test]
    fn parse_full_header_with_inline_and_dimensions() {
        let payload = format!(
            "name={};inline=1;width=4;height=2:{}",
            STANDARD.encode("hello.png"),
            TINY_PNG_B64
        );
        let img = parse_iterm2_payload(payload.as_bytes()).expect("decoded");
        assert_eq!(img.name.as_deref(), Some("hello.png"));
        assert!(img.inline);
        assert_eq!(img.width_cells, Some(4));
        assert_eq!(img.height_cells, Some(2));
    }

    #[test]
    fn header_separators_can_be_comma_or_semicolon() {
        let payload = format!("inline=1,width=3,height=1:{TINY_PNG_B64}");
        let img = parse_iterm2_payload(payload.as_bytes()).unwrap();
        assert!(img.inline);
        assert_eq!(img.width_cells, Some(3));
    }

    #[test]
    fn invalid_base64_returns_none() {
        let payload = b":not_valid_base64!!!".to_vec();
        assert!(parse_iterm2_payload(&payload).is_none());
    }

    #[test]
    fn empty_payload_returns_none() {
        assert!(parse_iterm2_payload(b":").is_none());
    }

    #[test]
    fn dimension_hint_accepts_ch_and_px_suffix() {
        assert_eq!(parse_dimension_hint("12"), Some(12));
        assert_eq!(parse_dimension_hint("8ch"), Some(8));
        assert_eq!(parse_dimension_hint("64px"), Some(64));
        assert_eq!(parse_dimension_hint("auto"), None);
        assert_eq!(parse_dimension_hint("50%"), None);
        assert_eq!(parse_dimension_hint(""), None);
    }

    #[test]
    fn placement_uses_explicit_hints_when_present() {
        let payload = format!("inline=1;width=5;height=3:{TINY_PNG_B64}");
        let img = parse_iterm2_payload(payload.as_bytes()).unwrap();
        let p = placement_from_iterm2(&img, 0, 2, 8, 16, 100, 200);
        assert_eq!(p.cols, 5);
        assert_eq!(p.rows, 3);
        assert_eq!(p.col_start, 2);
        assert_eq!(p.row_start, 0);
    }

    #[test]
    fn placement_clamps_to_viewport_caps() {
        let payload = format!("inline=1;width=99;height=99:{TINY_PNG_B64}");
        let img = parse_iterm2_payload(payload.as_bytes()).unwrap();
        let p = placement_from_iterm2(&img, 0, 0, 8, 16, 10, 12);
        assert_eq!(p.cols, 12);
        assert_eq!(p.rows, 10);
    }

    #[test]
    fn placement_falls_back_to_native_pixel_dims() {
        let payload = format!(":{TINY_PNG_B64}");
        let img = parse_iterm2_payload(payload.as_bytes()).unwrap();
        // 1×1 px image at 8×16 cell px ⇒ 1 col × 1 row (ceil division).
        let p = placement_from_iterm2(&img, 0, 0, 8, 16, 100, 200);
        assert_eq!(p.cols, 1);
        assert_eq!(p.rows, 1);
    }
}
