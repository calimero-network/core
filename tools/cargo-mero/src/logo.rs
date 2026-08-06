//! Renders a bundle manifest's launcher icon as ANSI half-blocks for the
//! post-bundle summary. Pure decoration: any failure just means nothing prints.

use std::env;
use std::io::IsTerminal;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use png::{BitDepth, ColorType, Decoder};

const DATA_URI_PREFIX: &str = "data:image/png;base64,";
/// Cap on the character grid; each cell shows two source pixels of vertical
/// resolution via the upper/lower half-block trick.
const MAX_CELLS: u32 = 32;

/// One rendered icon. `cols` is the display width: the lines carry ANSI escapes,
/// so their byte length says nothing about how wide they print.
pub struct Logo {
    pub cols: usize,
    pub lines: Vec<String>,
}

/// Renders the icon for a terminal that can show it. `None` when it cannot.
pub fn render(uri: &str) -> Option<Logo> {
    let tty = std::io::stdout().is_terminal();
    let no_color = env::var_os("NO_COLOR").is_some();
    render_with(uri, tty, no_color)
}

/// Testable core: `tty` and `no_color` are injected so tests never depend on
/// the ambient terminal.
fn render_with(uri: &str, tty: bool, no_color: bool) -> Option<Logo> {
    if no_color || !tty {
        return None;
    }
    let payload = uri.strip_prefix(DATA_URI_PREFIX)?;
    let bytes = STANDARD.decode(payload).ok()?;
    let (width, height, pixels) = decode_png(&bytes)?;
    let (cols, _) = target_cells(width, height);
    Some(Logo {
        cols: cols as usize,
        lines: render_pixels(width, height, &pixels),
    })
}

/// Decodes an 8-bit PNG into `(width, height, RGB pixels)`; 16-bit and indexed
/// PNGs aren't worth the extra decode paths for a decorative preview.
fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<[u8; 3]>)> {
    let decoder = Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.bit_depth != BitDepth::Eight || info.width == 0 || info.height == 0 {
        return None;
    }
    let data = &buf[..info.buffer_size()];
    let channels: usize = match info.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => return None,
    };
    // Composited over black, not discarded: a logo's transparent margin carries
    // arbitrary RGB, which would otherwise average in as opaque colour.
    let over_black = |c: u8, a: u8| ((u32::from(c) * u32::from(a)) / 255) as u8;
    let pixels = data
        .chunks_exact(channels)
        .map(|p| match channels {
            1 => [p[0], p[0], p[0]],
            2 => {
                let v = over_black(p[0], p[1]);
                [v, v, v]
            }
            3 => [p[0], p[1], p[2]],
            _ => [
                over_black(p[0], p[3]),
                over_black(p[1], p[3]),
                over_black(p[2], p[3]),
            ],
        })
        .collect();
    Some((info.width, info.height, pixels))
}

/// Output grid in half-block cells. A cell stacks two pixels but is drawn about
/// twice as tall as it is wide, so a square image needs half as many rows as
/// columns to come out square on screen.
fn target_cells(width: u32, height: u32) -> (u32, u32) {
    let cols = width.min(MAX_CELLS);
    let rows = (u64::from(cols) * u64::from(height) / (u64::from(width) * 2)).max(1) as u32;
    (cols, rows)
}

/// Mean of the source pixels covering one output cell. Averaging rather than
/// point-sampling keeps thin strokes legible when a 512px logo lands on 16 cells.
fn sample(pixels: &[[u8; 3]], width: u32, height: u32, box_: (u32, u32, u32, u32)) -> [u8; 3] {
    let (x0, y0, x1, y1) = box_;
    let (x1, y1) = (x1.min(width).max(x0 + 1), y1.min(height).max(y0 + 1));
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let px = pixels[(y * width + x) as usize];
            r += u32::from(px[0]);
            g += u32::from(px[1]);
            b += u32::from(px[2]);
            n += 1;
        }
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

fn render_pixels(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<String> {
    let (cols, rows) = target_cells(width, height);
    let sub_rows = rows * 2;
    let span = |i: u32, out_len: u32, in_len: u32| {
        let lo = i * in_len / out_len;
        ((i + 1) * in_len / out_len).max(lo + 1)
    };

    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut out = String::new();
        for col in 0..cols {
            let (x0, x1) = (col * width / cols, span(col, cols, width));
            let top = sample(
                pixels,
                width,
                height,
                (
                    x0,
                    row * 2 * height / sub_rows,
                    x1,
                    span(row * 2, sub_rows, height),
                ),
            );
            let bottom = sample(
                pixels,
                width,
                height,
                (
                    x0,
                    (row * 2 + 1) * height / sub_rows,
                    x1,
                    span(row * 2 + 1, sub_rows, height),
                ),
            );
            out.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
            ));
        }
        out.push_str("\x1b[0m");
        lines.push(out);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a tiny real RGBA PNG in memory and wraps it as a data URI,
    /// so tests exercise the actual PNG decoder instead of a fixed blob.
    fn valid_uri() -> String {
        let mut png_bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut png_bytes, 2, 2);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().expect("write PNG header");
        #[rustfmt::skip]
        let pixels = [
            255, 0, 0, 255,    0, 255, 0, 255,
            0, 0, 255, 255,    255, 255, 0, 255,
        ];
        writer.write_image_data(&pixels).expect("write PNG data");
        drop(writer);
        format!("data:image/png;base64,{}", STANDARD.encode(&png_bytes))
    }

    #[test]
    fn renders_nothing_when_colour_is_disabled() {
        assert!(render_with(&valid_uri(), true, true).is_none());
    }

    #[test]
    fn renders_half_blocks_for_a_valid_png() {
        let out = render_with(&valid_uri(), true, false).expect("renders");
        assert!(out.lines.iter().any(|l| l.contains('\u{2580}')));
        assert_eq!(out.cols, 2, "a 2x2 source renders two columns");
    }

    #[test]
    fn a_square_image_renders_half_as_many_rows_as_columns() {
        // Cells are about twice as tall as they are wide, so equal counts would
        // stretch a square logo to double height. Asserted as a ratio so tuning
        // MAX_CELLS does not silently reintroduce the stretch.
        let (cols, rows) = target_cells(512, 512);
        assert_eq!(rows, cols / 2, "square source must halve the row count");
        let (cols, rows) = target_cells(512, 256);
        assert_eq!(rows, cols / 4, "a 2:1 source is half again as short");
        // Never zero rows, however wide the source.
        assert_eq!(target_cells(512, 8).1, 1);
    }

    #[test]
    fn renders_nothing_when_not_a_tty() {
        assert!(render_with(&valid_uri(), false, false).is_none());
    }

    #[test]
    fn renders_nothing_for_a_non_png_payload() {
        let uri = format!("data:image/png;base64,{}", STANDARD.encode(b"not a png"));
        assert!(render_with(&uri, true, false).is_none());
    }

    #[test]
    fn renders_nothing_for_a_malformed_data_uri() {
        assert!(render_with("not-a-data-uri", true, false).is_none());
    }
}
