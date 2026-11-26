//! Loads background images on a background thread (LLM-573).
//!
//! GPUI's own image loader decodes a file at its full size, and the
//! sprite atlas then holds every pixel. A 8000×6000 photo behind a
//! laptop window would take 190 MB of GPU memory for a picture that
//! shows at a fraction of that. This loader shrinks each image to the
//! largest size it can show at on the screens attached, and never
//! enlarges one. It also applies the filters GPUI cannot draw, blur and
//! brightness (LLM-572), and keeps every frame of an animated GIF, APNG
//! or WebP (LLM-569).

use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::{App, Asset, RenderImage, SharedString};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, Delay, DynamicImage, Frame, ImageDecoder, ImageFormat, RgbaImage};

use super::BgSize;

/// The largest texture the GPU takes on a side. Larger images shrink to
/// it, so they still show.
pub const MAX_TEXTURE: u32 = 16384;

/// The frames a file may keep. A long animation past this stops at the
/// last frame kept, so one GIF cannot take all memory.
pub const MAX_FRAMES: usize = 600;

/// What to load: the file, and what the loader does to it. Two slots
/// with the same values share one decoded image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageKey {
    pub path: PathBuf,
    /// The file's modification time when the app last looked. A changed
    /// file gets a new key, so it loads again (LLM-568).
    pub stamp: Option<SystemTime>,
    pub fit: Fit,
    pub effects: Effects,
}

impl Eq for ImageKey {}

impl Hash for ImageKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.stamp.hash(state);
        self.fit.hash(state);
        self.effects.hash(state);
    }
}

/// The size the image shows at, at most.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub size: BgSize,
    /// The largest screen, in logical px.
    pub screen: (f32, f32),
    /// Device px per logical px.
    pub scale: f32,
}

impl Hash for Fit {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.size {
            BgSize::Cover => 0u8.hash(state),
            BgSize::Contain => 1u8.hash(state),
            BgSize::Fill => 2u8.hash(state),
            BgSize::Scale(s) => (3u8, s.to_bits()).hash(state),
            BgSize::WidthShare(s) => (4u8, s.to_bits()).hash(state),
            BgSize::Width(s) => (5u8, s.to_bits()).hash(state),
        }
        self.screen.0.to_bits().hash(state);
        self.screen.1.to_bits().hash(state);
        self.scale.to_bits().hash(state);
    }
}

/// Filters baked into the pixels as the image loads (LLM-572).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effects {
    /// A radius in logical px. 0 is none.
    pub blur: f32,
    /// A factor on each color channel. 1 is none.
    pub brightness: f32,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            blur: 0.,
            brightness: 1.,
        }
    }
}

impl Hash for Effects {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.blur.to_bits().hash(state);
        self.brightness.to_bits().hash(state);
    }
}

/// A loaded image.
#[derive(Debug)]
pub struct Loaded {
    pub image: Arc<RenderImage>,
    /// The file's own size in px. Layout uses it, not the shrunk size,
    /// so `size = "auto"` keeps meaning the file's pixels.
    pub natural: (f32, f32),
}

pub type LoadResult = Result<Arc<Loaded>, SharedString>;

/// The asset GPUI caches. Its source is an [`ImageKey`].
pub enum BackdropLoader {}

impl Asset for BackdropLoader {
    type Source = ImageKey;
    type Output = LoadResult;

    fn load(key: ImageKey, cx: &mut App) -> impl Future<Output = LoadResult> + Send + 'static {
        let svg = cx.svg_renderer();
        async move {
            let bytes = std::fs::read(&key.path)
                .map_err(|e| SharedString::from(format!("{}: {e}", key.path.display())))?;
            let result = match load_bytes(&bytes, &key) {
                Err(DecodeError::NotAnImage) => svg
                    .render_single_frame(&bytes, 1.0)
                    .map(|image| {
                        let s = image.size(0);
                        let natural = (u32::from(s.width) as f32, u32::from(s.height) as f32);
                        Arc::new(Loaded { image, natural })
                    })
                    .map_err(|_| "not an image".to_string()),
                Err(DecodeError::Failed(e)) => Err(e),
                Ok(loaded) => Ok(loaded),
            };
            result.map_err(|e| {
                let message = format!("{}: {e}", key.path.display());
                eprintln!("llmtodo: background image {message}");
                SharedString::from(message)
            })
        }
    }
}

/// Decodes and processes a file's bytes. SVG is left to the caller.
pub fn load_bytes(bytes: &[u8], key: &ImageKey) -> Result<Arc<Loaded>, DecodeError> {
    process(decode(bytes)?, key).map_err(DecodeError::Failed)
}

#[derive(Debug)]
pub enum DecodeError {
    NotAnImage,
    Failed(String),
}

/// The file's frames, in RGBA. A still image is one frame.
fn decode(bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
    let format = image::guess_format(bytes).map_err(|_| DecodeError::NotAnImage)?;
    let failed = |e: image::ImageError| DecodeError::Failed(e.to_string());
    let frames = match format {
        ImageFormat::Gif => animated(GifDecoder::new(Cursor::new(bytes)).map_err(failed)?)?,
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(failed)?;
            if decoder.has_animation() {
                let _ = decoder.set_background_color(image::Rgba([0, 0, 0, 0]));
                animated(decoder)?
            } else {
                still(decoder)?
            }
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(bytes)).map_err(failed)?;
            if decoder.is_apng().map_err(failed)? {
                animated(decoder.apng().map_err(failed)?)?
            } else {
                still(decoder)?
            }
        }
        format => still(
            image::ImageReader::with_format(Cursor::new(bytes), format)
                .into_decoder()
                .map_err(failed)?,
        )?,
    };
    Ok(frames)
}

fn still(mut decoder: impl ImageDecoder) -> Result<Vec<Frame>, DecodeError> {
    let failed = |e: image::ImageError| DecodeError::Failed(e.to_string());
    let orientation = decoder.orientation().map_err(failed)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
    image.apply_orientation(orientation);
    Ok(vec![Frame::new(image.into_rgba8())])
}

fn animated<'a>(decoder: impl AnimationDecoder<'a>) -> Result<Vec<Frame>, DecodeError> {
    let frames: Vec<Frame> = decoder
        .into_frames()
        .filter_map(Result::ok)
        .take(MAX_FRAMES)
        .collect();
    if frames.is_empty() {
        return Err(DecodeError::Failed("no frame decodes".into()));
    }
    Ok(frames)
}

/// Device px per image px when the image shows at its largest: on the
/// largest screen, since a panel is never larger than the screen.
pub fn display_factor(natural: (f32, f32), fit: &Fit) -> f32 {
    let (iw, ih) = natural;
    let (sw, sh) = fit.screen;
    let shown_w = match fit.size {
        BgSize::Cover | BgSize::Fill => iw * (sw / iw).max(sh / ih),
        BgSize::Contain => iw * (sw / iw).min(sh / ih),
        BgSize::Scale(s) => iw * s,
        BgSize::WidthShare(p) => sw * p,
        BgSize::Width(w) => w,
    };
    let f = shown_w * fit.scale.max(1.) / iw;
    if f.is_finite() && f > 0. { f } else { 1. }
}

/// The factor the image shrinks by: at most 1, and small enough that the
/// image is no larger than it can show at, nor than the GPU takes.
pub fn shrink_factor(natural: (f32, f32), fit: &Fit) -> f32 {
    let (iw, ih) = natural;
    if iw <= 0. || ih <= 0. {
        return 1.;
    }
    let limit = MAX_TEXTURE as f32 / iw.max(ih);
    display_factor(natural, fit).min(limit).min(1.)
}

/// Shrinks, filters and converts each frame to the BGRA GPUI draws.
fn process(frames: Vec<Frame>, key: &ImageKey) -> Result<Arc<Loaded>, String> {
    let (w, h) = frames[0].buffer().dimensions();
    let natural = (w as f32, h as f32);
    let factor = shrink_factor(natural, &key.fit);
    let target = (
        ((w as f32 * factor).round() as u32).max(1),
        ((h as f32 * factor).round() as u32).max(1),
    );
    // Blur is in logical px on screen. One shrunk image px shows at
    // `display / factor` device px.
    let px_per_logical = factor * key.fit.scale.max(1.) / display_factor(natural, &key.fit);
    let sigma = key.effects.blur.max(0.) * px_per_logical;
    let out: Vec<Frame> = frames
        .into_iter()
        .map(|frame| {
            let delay = frame.delay();
            let mut buffer = frame.into_buffer();
            if target != buffer.dimensions() {
                buffer = image::imageops::resize(&buffer, target.0, target.1, FilterType::Triangle);
            }
            buffer = apply_effects(buffer, sigma, key.effects.brightness);
            for pixel in buffer.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Frame::from_parts(buffer, 0, 0, delay)
        })
        .collect();
    Ok(Arc::new(Loaded {
        image: Arc::new(RenderImage::new(out)),
        natural,
    }))
}

/// Blur by `sigma` image px, then scale each channel by `brightness`.
pub fn apply_effects(mut buffer: RgbaImage, sigma: f32, brightness: f32) -> RgbaImage {
    if sigma >= 0.3 {
        buffer = image::imageops::fast_blur(&buffer, sigma);
    }
    if (brightness - 1.).abs() > f32::EPSILON && brightness.is_finite() {
        let b = brightness.max(0.);
        for pixel in buffer.pixels_mut() {
            for c in &mut pixel.0[..3] {
                *c = (*c as f32 * b).round().min(255.) as u8;
            }
        }
    }
    buffer
}

/// How long frame `i` shows, in ms. GIFs often say 0, which browsers
/// show as 100 ms.
pub fn frame_ms(delay: Delay) -> u64 {
    let (n, d) = delay.numer_denom_ms();
    let ms = if d == 0 {
        0
    } else {
        u64::from(n) / u64::from(d)
    };
    if ms < 10 { 100 } else { ms }
}
