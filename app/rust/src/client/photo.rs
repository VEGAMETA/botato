//! Photo preparation (protocol/README.md §6.3): decode whatever the picker returned, apply
//! the EXIF orientation to the pixels, re-encode as JPEG without metadata.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader};

/// Longest side of a sent photo.
pub const MAX_SIDE: u32 = 1600;
/// Longest side of the inline preview.
pub const THUMB_SIDE: u32 = 64;
/// The preview travels inside the E2E message.
pub const MAX_THUMB: usize = 8 * 1024;
/// Anything bigger than this is not a photo we want to decode on a phone.
const MAX_INPUT: usize = 64 * 1024 * 1024;

pub struct Prepared {
    pub jpeg: Vec<u8>,
    pub thumb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub fn prepare(input: &[u8]) -> anyhow::Result<Prepared> {
    if input.len() > MAX_INPUT {
        anyhow::bail!("файл слишком большой");
    }
    let mut decoder = ImageReader::new(Cursor::new(input))
        .with_guessed_format()?
        .into_decoder()
        .map_err(|_| anyhow::anyhow!("это не картинка (поддерживаются JPEG, PNG, WebP, GIF)"))?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle);
    }
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8());

    let mut jpeg = vec![];
    for quality in [82, 70, 55, 40] {
        jpeg = encode(&rgb, quality)?;
        if jpeg.len() <= proto::file::MAX_PLAINTEXT {
            break;
        }
    }
    if jpeg.len() > proto::file::MAX_PLAINTEXT {
        anyhow::bail!("фото не ужимается до 2 МБ");
    }
    let mut thumb = vec![];
    for quality in [50, 30] {
        thumb = encode(&rgb.thumbnail(THUMB_SIDE, THUMB_SIDE), quality)?;
        if thumb.len() <= MAX_THUMB {
            break;
        }
    }
    Ok(Prepared { jpeg, thumb, w: rgb.width(), h: rgb.height() })
}

fn encode(img: &DynamicImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    let mut out = vec![];
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn resizes_reencodes_and_thumbs() {
        let big: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_fn(3000, 1000, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        let mut png = vec![];
        DynamicImage::ImageRgb8(big).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = prepare(&png).unwrap();
        assert_eq!((p.w, p.h), (1600, 533));
        assert_eq!(&p.jpeg[..2], &[0xff, 0xd8]);
        assert!(p.thumb.len() <= MAX_THUMB);
        let t = image::load_from_memory(&p.thumb).unwrap();
        assert_eq!(t.width(), THUMB_SIDE);
        assert!(prepare(b"not an image").is_err());
    }
}
