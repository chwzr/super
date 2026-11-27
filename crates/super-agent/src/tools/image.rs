//! Images for `Read`: a large image is made smaller before it is sent.
//!
//! An image may be at most [`MAX_IMAGE_SIDE`] pixels on each side and at most
//! [`MAX_IMAGE_BASE64_BYTES`] once encoded as base64 (3.75 MB of image data).
//! A larger one is scaled down in proportion and saved again in its own
//! family (PNG, or JPEG for a JPEG); if that is still too heavy it is saved
//! as JPEG at falling quality. If nothing brings it under the limits, the
//! read fails.

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use std::io::Cursor;

/// The most pixels an image sent to the model has on either side.
pub const MAX_IMAGE_SIDE: u32 = 2000;
/// The most bytes an image sent to the model takes as base64: 5 MB, which is
/// 3.75 MB of image data.
pub const MAX_IMAGE_BASE64_BYTES: usize = 5 * 1024 * 1024;
/// JPEG qualities tried, in order, when an image is still too heavy.
const JPEG_QUALITIES: [u8; 4] = [80, 60, 40, 20];
/// The JPEG quality of a scaled JPEG on the first try.
const SCALED_JPEG_QUALITY: u8 = 85;

/// The image as it is sent.
#[derive(Debug, Clone)]
pub struct FittedImage {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    /// The size of the file's image, when it could be read.
    pub original: Option<(u32, u32)>,
    /// The size of the image that is sent, when known.
    pub size: Option<(u32, u32)>,
    /// Whether the image was scaled or saved again.
    pub changed: bool,
}

/// Why an image could not be brought under the limits.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageFitError {
    /// The image is over the limits and could not be decoded to scale it.
    Decode(String),
    /// Even scaled down and saved as JPEG it is too heavy.
    TooLarge { width: u32, height: u32 },
}

/// The length of `bytes` bytes once encoded as base64.
pub fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3).saturating_mul(4)
}

fn within_limits(width: u32, height: u32, bytes: usize) -> bool {
    width <= MAX_IMAGE_SIDE
        && height <= MAX_IMAGE_SIDE
        && base64_len(bytes) <= MAX_IMAGE_BASE64_BYTES
}

/// Bring the image in `bytes` (of type `mime_type`) under the limits, or say
/// why that is not possible. An image within them is returned as it is.
pub fn fit_image(bytes: Vec<u8>, mime_type: &'static str) -> Result<FittedImage, ImageFitError> {
    let dimensions = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok());
    match dimensions {
        Some((width, height)) if within_limits(width, height, bytes.len()) => {
            return Ok(FittedImage {
                data: bytes,
                mime_type,
                original: Some((width, height)),
                size: Some((width, height)),
                changed: false,
            });
        }
        // An image whose size cannot be read is sent as it is while it is
        // light enough; the model's provider judges it.
        None if base64_len(bytes.len()) <= MAX_IMAGE_BASE64_BYTES => {
            return Ok(FittedImage {
                data: bytes,
                mime_type,
                original: None,
                size: None,
                changed: false,
            });
        }
        _ => {}
    }

    let image = decode(&bytes).map_err(ImageFitError::Decode)?;
    let original = (image.width(), image.height());
    let scaled = if original.0 > MAX_IMAGE_SIDE || original.1 > MAX_IMAGE_SIDE {
        image.resize(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE, FilterType::Triangle)
    } else {
        image
    };
    let size = (scaled.width(), scaled.height());
    let fitted = |data: Vec<u8>, mime_type: &'static str| FittedImage {
        data,
        mime_type,
        original: Some(original),
        size: Some(size),
        changed: true,
    };

    let first = if mime_type == "image/jpeg" {
        encode_jpeg(&scaled, SCALED_JPEG_QUALITY).map(|data| (data, "image/jpeg"))
    } else {
        encode_png(&scaled).map(|data| (data, "image/png"))
    };
    if let Some((data, mime_type)) = first
        && base64_len(data.len()) <= MAX_IMAGE_BASE64_BYTES
    {
        return Ok(fitted(data, mime_type));
    }
    for quality in JPEG_QUALITIES {
        if let Some(data) = encode_jpeg(&scaled, quality)
            && base64_len(data.len()) <= MAX_IMAGE_BASE64_BYTES
        {
            return Ok(fitted(data, "image/jpeg"));
        }
    }
    Err(ImageFitError::TooLarge {
        width: size.0,
        height: size.1,
    })
}

fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Ok(image)
}

fn encode_png(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
        .ok()?;
    Some(data)
}

/// JPEG has no transparency: a transparent part is laid on white, so dark
/// drawings on a clear background stay readable.
fn opaque_rgb(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
        let alpha = u16::from(alpha);
        let over_white =
            |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        image::Rgb([over_white(red), over_white(green), over_white(blue)])
    })
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let rgb = opaque_rgb(image);
    let mut data = Vec::new();
    rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut data, quality))
        .ok()?;
    Some(data)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        let mut data = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        data
    }

    /// Noise that PNG cannot compress, so a moderate size is heavy.
    pub(crate) fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut state = 0x2545_f491_u32;
        let image = RgbImage::from_fn(width, height, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [a, b, c, _] = state.to_le_bytes();
            image::Rgb([a, b, c])
        });
        let mut data = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        data
    }

    fn size_of(data: &[u8]) -> (u32, u32) {
        ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn an_image_within_the_limits_is_sent_as_it_is() {
        let data = png(40, 30);
        let fitted = fit_image(data.clone(), "image/png").unwrap();
        assert!(!fitted.changed);
        assert_eq!(fitted.data, data);
        assert_eq!(fitted.size, Some((40, 30)));
    }

    #[test]
    fn a_wide_image_is_scaled_in_proportion() {
        let fitted = fit_image(png(3000, 1000), "image/png").unwrap();
        assert!(fitted.changed);
        assert_eq!(fitted.mime_type, "image/png");
        assert_eq!(fitted.original, Some((3000, 1000)));
        assert_eq!(fitted.size, Some((2000, 667)));
        assert_eq!(size_of(&fitted.data), (2000, 667));
    }

    #[test]
    fn a_heavy_image_is_saved_again_as_jpeg() {
        // About 5 MB of noise: within the side limit, over the weight limit.
        let data = noisy_png(1500, 1100);
        assert!(base64_len(data.len()) > MAX_IMAGE_BASE64_BYTES);
        let fitted = fit_image(data, "image/png").unwrap();
        assert!(fitted.changed);
        assert_eq!(fitted.mime_type, "image/jpeg");
        assert_eq!(fitted.size, Some((1500, 1100)));
        assert!(base64_len(fitted.data.len()) <= MAX_IMAGE_BASE64_BYTES);
        assert_eq!(size_of(&fitted.data), (1500, 1100));
    }

    #[test]
    fn an_image_that_cannot_be_decoded_fails_once_it_is_too_heavy() {
        let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let small = fit_image(data.clone(), "image/png").unwrap();
        assert!(!small.changed);
        data.resize(MAX_IMAGE_BASE64_BYTES, 0);
        assert!(matches!(
            fit_image(data, "image/png"),
            Err(ImageFitError::Decode(_))
        ));
    }

    #[test]
    fn transparency_is_laid_on_white_for_jpeg() {
        let clear = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 0])));
        assert_eq!(opaque_rgb(&clear).get_pixel(0, 0).0, [255, 255, 255]);
        let solid = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])));
        assert_eq!(opaque_rgb(&solid).get_pixel(1, 1).0, [10, 20, 30]);
    }
}
