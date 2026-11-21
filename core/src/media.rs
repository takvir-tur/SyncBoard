use std::fmt;
use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::imageops::FilterType as ResizeFilter;
use image::{DynamicImage, ImageEncoder, ImageFormat, Rgb, RgbImage};

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const SMALL_SCREENSHOT_MAX_PIXELS: u64 = 1_500_000;
const SMALL_PNG_MAX_BYTES: usize = 512 * 1024;
const WEBP_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedImage {
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    EmptyInput,
    DecodeFailed(String),
    EncodeFailed(String),
    ExceedsMaxSize,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty image input"),
            Self::DecodeFailed(e) => write!(f, "failed to decode image: {e}"),
            Self::EncodeFailed(e) => write!(f, "failed to encode image: {e}"),
            Self::ExceedsMaxSize => write!(f, "image exceeds 8 MiB maximum after compression"),
        }
    }
}

impl std::error::Error for MediaError {}

pub fn normalize_and_compress_image(raw: &[u8]) -> Result<CompressedImage, MediaError> {
    if raw.is_empty() {
        return Err(MediaError::EmptyInput);
    }

    let img = image::load_from_memory(raw).map_err(|e| MediaError::DecodeFailed(e.to_string()))?;
    let pixels = (img.width() as u64) * (img.height() as u64);
    let has_transparency = image_has_transparency(&img);

    let mut png_fallback: Option<Vec<u8>> = None;

    if pixels <= SMALL_SCREENSHOT_MAX_PIXELS {
        let png_bytes = encode_png(&img)?;
        if png_bytes.len() <= SMALL_PNG_MAX_BYTES {
            return Ok(CompressedImage {
                mime: "image/png".to_string(),
                data: png_bytes,
            });
        }

        if has_transparency {
            if let Ok(webp_bytes) = encode_lossless_webp(&img) {
                if webp_bytes.len() <= WEBP_MAX_BYTES && webp_bytes.len() < png_bytes.len() {
                    return Ok(CompressedImage {
                        mime: "image/webp".to_string(),
                        data: webp_bytes,
                    });
                }
            }
            if png_bytes.len() <= MAX_IMAGE_BYTES {
                return Ok(CompressedImage {
                    mime: "image/png".to_string(),
                    data: png_bytes,
                });
            }
        }

        png_fallback = Some(png_bytes);
    }

    let mut working = img;
    for scale_step in 0..6 {
        let rgb = flatten_to_rgb8(&working);
        for quality in [85u8, 74, 60, 46] {
            let jpeg_bytes = encode_jpeg(&rgb, quality)?;
            if jpeg_bytes.len() <= MAX_IMAGE_BYTES {
                if scale_step == 0 {
                    if let Some(ref png_bytes) = png_fallback {
                        if png_bytes.len() <= jpeg_bytes.len() && png_bytes.len() <= MAX_IMAGE_BYTES {
                            return Ok(CompressedImage {
                                mime: "image/png".to_string(),
                                data: png_bytes.clone(),
                            });
                        }
                    }
                }
                return Ok(CompressedImage {
                    mime: "image/jpeg".to_string(),
                    data: jpeg_bytes,
                });
            }
        }

        let next_w = ((working.width() as f32) * 0.75).round().max(64.0) as u32;
        let next_h = ((working.height() as f32) * 0.75).round().max(64.0) as u32;
        if next_w == working.width() && next_h == working.height() {
            break;
        }
        working = working.resize_exact(next_w, next_h, ResizeFilter::Triangle);
    }

    Err(MediaError::ExceedsMaxSize)
}

fn image_has_transparency(img: &DynamicImage) -> bool {
    if !img.color().has_alpha() {
        return false;
    }
    img.to_rgba8().pixels().any(|p| p.0[3] < 255)
}

fn flatten_to_rgb8(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = RgbImage::new(w, h);
    for (x, y, px) in rgba.enumerate_pixels() {
        let a = px.0[3] as u16;
        if a == 255 {
            rgb.put_pixel(x, y, Rgb([px.0[0], px.0[1], px.0[2]]));
        } else if a == 0 {
            rgb.put_pixel(x, y, Rgb([255, 255, 255]));
        } else {
            let inv = 255 - a;
            let r = ((px.0[0] as u16 * a + 255 * inv) / 255) as u8;
            let g = ((px.0[1] as u16 * a + 255 * inv) / 255) as u8;
            let b = ((px.0[2] as u16 * a + 255 * inv) / 255) as u8;
            rgb.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    rgb
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let rgba = img.to_rgba8();
    let mut out = Vec::new();
    let encoder = PngEncoder::new_with_quality(
        &mut out,
        CompressionType::Default,
        FilterType::Adaptive,
    );
    encoder
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| MediaError::EncodeFailed(e.to_string()))?;
    Ok(out)
}

fn encode_lossless_webp(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::WebP)
        .map_err(|e| MediaError::EncodeFailed(e.to_string()))?;
    Ok(out.into_inner())
}

fn encode_jpeg(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| MediaError::EncodeFailed(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn small_screenshot_stays_lossless_png() {
        let mut img = RgbaImage::new(320, 180);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if (x + y) % 16 < 8 {
                Rgba([24, 120, 220, 255])
            } else {
                Rgba([245, 245, 245, 255])
            };
        }
        let mut raw = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img)
            .write_to(&mut raw, ImageFormat::Bmp)
            .expect("bmp encode");

        let compressed = normalize_and_compress_image(raw.get_ref()).expect("compress");
        assert_eq!(compressed.mime, "image/png");
        assert!(compressed.data.len() <= SMALL_PNG_MAX_BYTES);
    }

    #[test]
    fn large_noisy_photo_compresses_to_jpeg_under_limit() {
        let mut img = RgbImage::new(1600, 1200);
        let mut state: u32 = 0x1234_5678;
        for (_, _, p) in img.enumerate_pixels_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let r = (state >> 24) as u8;
            let g = (state >> 16) as u8;
            let b = (state >> 8) as u8;
            *p = Rgb([r, g, b]);
        }
        let mut raw = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut raw, ImageFormat::Png)
            .expect("png encode");

        let compressed = normalize_and_compress_image(raw.get_ref()).expect("compress");
        assert_eq!(compressed.mime, "image/jpeg");
        assert!(compressed.data.len() <= MAX_IMAGE_BYTES);
    }
}
