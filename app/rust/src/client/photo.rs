//! Photo preparation (protocol/README.md §6.3): decode whatever the picker returned, apply
//! the EXIF orientation to the pixels, re-encode without metadata.
//!
//! Photos become JPEG. A PNG (screenshots, transparency) stays PNG when the caller allows it,
//! at a larger size so text stays readable, and falls back to JPEG if it can't fit.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgba, RgbaImage};

/// Longest side of a sent photo (JPEG).
pub const MAX_SIDE: u32 = 1600;
/// Longest sides tried for a PNG, largest first; screenshots need the pixels.
const PNG_SIDES: [u32; 4] = [2560, 1920, 1600, 1280];
/// Longest side of the inline preview.
pub const THUMB_SIDE: u32 = 64;
/// The preview travels inside the E2E message.
pub const MAX_THUMB: usize = 8 * 1024;
/// Anything bigger than this is not a photo we want to decode on a phone.
const MAX_INPUT: usize = 64 * 1024 * 1024;

pub struct Prepared {
    /// JPEG or PNG; receivers tell them apart by the magic bytes.
    pub data: Vec<u8>,
    pub thumb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// `keep_png`: a PNG input may stay PNG (false when the PNG is only an intermediate, e.g. a
/// phone photo the app already decoded and re-encoded losslessly).
pub fn prepare(input: &[u8], keep_png: bool) -> anyhow::Result<Prepared> {
    if input.len() > MAX_INPUT {
        anyhow::bail!("файл слишком большой");
    }
    let reader = ImageReader::new(Cursor::new(input)).with_guessed_format()?;
    let is_png = reader.format() == Some(ImageFormat::Png);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| anyhow::anyhow!("это не картинка (поддерживаются JPEG, PNG, WebP, GIF)"))?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);

    let thumb = thumbnail(&img)?;
    if is_png && keep_png {
        for side in PNG_SIDES {
            let fitted = fit(&img, side);
            let data = encode_png(&fitted)?;
            if data.len() <= proto::file::MAX_PLAINTEXT {
                return Ok(Prepared { data, thumb, w: fitted.width(), h: fitted.height() });
            }
        }
        // A noisy PNG that no size fits: send it as a photo.
    }

    let rgb = DynamicImage::ImageRgb8(flatten(&fit(&img, MAX_SIDE)).into());
    let mut data = vec![];
    for quality in [82, 70, 55, 40] {
        data = encode_jpeg(&rgb, quality)?;
        if data.len() <= proto::file::MAX_PLAINTEXT {
            break;
        }
    }
    if data.len() > proto::file::MAX_PLAINTEXT {
        anyhow::bail!("фото не ужимается до 2 МБ");
    }
    Ok(Prepared { data, thumb, w: rgb.width(), h: rgb.height() })
}

fn fit(img: &DynamicImage, side: u32) -> DynamicImage {
    if img.width() > side || img.height() > side {
        img.resize(side, side, FilterType::Triangle)
    } else {
        img.clone()
    }
}

/// Transparent pixels over white (JPEG has no alpha; black would hide dark content).
fn flatten(img: &DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let Rgba([r, g, b, a]) = *rgba.get_pixel(x, y);
        let over = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        image::Rgb([over(r), over(g), over(b)])
    })
}

fn thumbnail(img: &DynamicImage) -> anyhow::Result<Vec<u8>> {
    let small = DynamicImage::ImageRgb8(flatten(&img.thumbnail(THUMB_SIDE, THUMB_SIDE)));
    let mut thumb = vec![];
    for quality in [50, 30] {
        thumb = encode_jpeg(&small, quality)?;
        if thumb.len() <= MAX_THUMB {
            break;
        }
    }
    Ok(thumb)
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    let mut out = vec![];
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))?;
    Ok(out)
}

/// RGB when fully opaque (smaller), RGBA otherwise. No metadata chunks are written.
fn encode_png(img: &DynamicImage) -> anyhow::Result<Vec<u8>> {
    let opaque = !img.color().has_alpha() || img.to_rgba8().pixels().all(|p| p.0[3] == 255);
    let img = if opaque {
        DynamicImage::ImageRgb8(img.to_rgb8())
    } else {
        DynamicImage::ImageRgba8(RgbaImage::from(img.to_rgba8()))
    };
    let mut out = vec![];
    img.write_with_encoder(PngEncoder::new_with_quality(&mut out, CompressionType::Default, PngFilter::Adaptive))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn png_of(img: DynamicImage) -> Vec<u8> {
        let mut png = vec![];
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        png
    }

    #[test]
    fn photos_become_jpeg_with_thumbs() {
        let big: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_fn(3000, 1000, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        let p = prepare(&png_of(DynamicImage::ImageRgb8(big)), false).unwrap();
        assert_eq!((p.w, p.h), (1600, 533));
        assert_eq!(&p.data[..2], &[0xff, 0xd8]);
        assert!(p.thumb.len() <= MAX_THUMB);
        let t = image::load_from_memory(&p.thumb).unwrap();
        assert_eq!(t.width(), THUMB_SIDE);
        assert!(prepare(b"not an image", true).is_err());
    }

    #[test]
    fn png_stays_png_with_alpha_and_more_pixels() {
        // A "screenshot": flat colors compress well, transparency in one corner.
        let shot = RgbaImage::from_fn(3000, 1500, |x, y| {
            if x < 100 && y < 100 { Rgba([0, 0, 0, 0]) } else { Rgba([(x / 300) as u8 * 20, 200, 30, 255]) }
        });
        let p = prepare(&png_of(DynamicImage::ImageRgba8(shot)), true).unwrap();
        assert_eq!(&p.data[..4], b"\x89PNG");
        assert_eq!((p.w, p.h), (2560, 1280));
        let back = image::load_from_memory(&p.data).unwrap();
        assert!(back.color().has_alpha());
        assert_eq!(back.to_rgba8().get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn noisy_png_falls_back_to_jpeg() {
        let mut seed = 1u32;
        let noise = RgbaImage::from_fn(1400, 1400, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgba([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8, 255])
        });
        let p = prepare(&png_of(DynamicImage::ImageRgba8(noise)), true).unwrap();
        assert_eq!(&p.data[..2], &[0xff, 0xd8]);
        assert!(p.data.len() <= proto::file::MAX_PLAINTEXT);
    }
}
