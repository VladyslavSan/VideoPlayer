//! Pictures from Sonarr, Radarr and the metadata sites, made safe to show.
//!
//! Some JPEGs keep each colour in its own scan (Sonarr and Radarr's resized
//! copies often do). The decoder behind the window's pictures (zune-jpeg
//! 0.5.15) gets those wrong when the colour is at half size, and the picture
//! comes out green and striped. Those are decoded here with another decoder
//! and saved as an ordinary JPEG; every other picture is kept as it came.
//! (zune-jpeg 0.5.16 fixes it; this can go once `image` picks that up.)

use anyhow::{Context, Result, bail};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};

/// The picture as it should be saved.
pub fn readable(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if !scans_split(&bytes) {
        return Ok(bytes);
    }
    let mut decoder = jpeg_decoder::Decoder::new(bytes.as_slice());
    let pixels = decoder.decode().context("couldn't read the picture")?;
    let info = decoder.info().context("couldn't read the picture")?;
    let color = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => ExtendedColorType::Rgb8,
        jpeg_decoder::PixelFormat::L8 => ExtendedColorType::L8,
        other => bail!("a picture in {other:?}"),
    };
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90).encode(
        &pixels,
        u32::from(info.width),
        u32::from(info.height),
        color,
    )?;
    Ok(out)
}

/// A baseline JPEG whose first scan doesn't hold all of its colours.
/// (Progressive ones are made of several scans anyway, and decode fine.)
fn scans_split(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return false;
    }
    let mut colours = None;
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return false;
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            // Padding before a marker.
            at += 1;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let body = &bytes[(at + 4).min(bytes.len())..(at + 2 + length).min(bytes.len())];
        match marker {
            // Baseline and extended sequential frames.
            0xC0 | 0xC1 => colours = body.get(5).copied(),
            // Any other frame type: not one of these.
            0xC2..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => return false,
            // The first scan.
            0xDA => return matches!((colours, body.first()), (Some(all), Some(&n)) if n < all),
            _ => {}
        }
        at += 2 + length;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageDecoder, codecs::jpeg::JpegDecoder};

    /// 64x32, colour at half size, each colour in its own scan; reddish,
    /// brighter on the left (made by `testdata/make_split_scans.py`).
    const SPLIT: &[u8] = include_bytes!("testdata/split-scans.jpg");

    /// How the window would decode it.
    fn decode(jpeg: &[u8]) -> (u32, Vec<u8>) {
        let decoder = JpegDecoder::new(std::io::Cursor::new(jpeg)).unwrap();
        let width = decoder.dimensions().0;
        let mut pixels = vec![0; decoder.total_bytes() as usize];
        decoder.read_image(&mut pixels).unwrap();
        (width, pixels)
    }

    fn ordinary() -> Vec<u8> {
        let rgb: Vec<u8> = [30, 160, 60].repeat(64 * 48);
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 95)
            .encode(&rgb, 64, 48, ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    #[test]
    fn finds_split_scans() {
        assert!(scans_split(SPLIT));
        assert!(!scans_split(&ordinary()));
        assert!(!scans_split(b"\x89PNG\r\n\x1a\n"));
        assert!(!scans_split(&[0xFF, 0xD8, 0xFF]));
        assert!(!scans_split(&SPLIT[..40]));
    }

    #[test]
    fn keeps_ordinary_pictures() {
        let jpeg = ordinary();
        assert_eq!(readable(jpeg.clone()).unwrap(), jpeg);
    }

    #[test]
    fn redoes_split_pictures() {
        let out = readable(SPLIT.to_vec()).unwrap();
        assert!(!scans_split(&out));
        let (width, pixels) = decode(&out);
        // Red everywhere, not green: the left is (251, 108, 100), the
        // right (211, 68, 60).
        for (i, px) in pixels.chunks(3).enumerate() {
            let left = (i as u32 % width) < width / 2;
            let want: [i32; 3] = if left { [251, 108, 100] } else { [211, 68, 60] };
            for (got, want) in px.iter().zip(want) {
                assert!((i32::from(*got) - want).abs() < 12, "pixel {i}: {px:?}");
            }
        }
    }
}
