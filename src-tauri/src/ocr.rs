//! Windows.Media.Ocr-based offline OCR for clipboard images.
//!
//! Uses TryCreateFromUserProfileLanguages — works on systems with at least
//! one OCR language pack installed. English ships by default on en-* Windows.
//! Returns Err with a user-friendly message if OCR is unavailable so the UI
//! can show "OCR not available on this system" rather than crashing.
//!
//! Blocks the calling thread — call from spawn_blocking / a dedicated thread.
//!
//! Large-image handling: `OcrEngine.MaxImageDimension` is a hard cap (~2600
//! pixels historically) above which the engine silently truncates the input,
//! producing OCR output that's missing the bottom/right of the image. When
//! the source exceeds this, we scale down proportionally via BitmapTransform
//! during decode so the full image reaches the engine.
//!
//! Accuracy preprocessing: the engine has no tuning knobs and internally
//! binarizes, so low-contrast coloured text (red on white, grey on grey)
//! misreads. Before recognition we grayscale + contrast-stretch the image
//! (robust min/max, not hard threshold — mixed light/dark regions survive)
//! and 2x-upscale small captures, both of which measurably help it.

use std::io::Cursor;
use std::sync::OnceLock;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows_core::Interface;

/// Cached `OcrEngine.MaxImageDimension`. It's a static property on the class
/// and doesn't change at runtime, so querying once is enough. `None` means we
/// couldn't fetch it (Windows without the Media OCR component); in that case
/// we skip the scale-down path entirely and hope the engine handles the size.
static MAX_OCR_DIMENSION: OnceLock<Option<u32>> = OnceLock::new();

fn max_ocr_dimension() -> Option<u32> {
    *MAX_OCR_DIMENSION.get_or_init(|| OcrEngine::MaxImageDimension().ok())
}

/// Fallback engine cap when `MaxImageDimension` can't be queried. Matches the
/// value the API has returned on every Windows build to date.
const FALLBACK_MAX_DIMENSION: u32 = 2600;

/// Preprocess PNG bytes for better recognition accuracy. Returns re-encoded
/// PNG bytes, or `None` when preprocessing isn't needed (image already
/// high-contrast and large enough) or fails for any reason — the caller falls
/// back to the original bytes, so this can never make OCR worse than before.
///
/// Two transforms:
/// 1. Grayscale + linear contrast stretch. The black/white points come from
///    the darkest and lightest histogram bins holding at least 0.05% of
///    pixels — robust against single-pixel outliers, but a sparse run of
///    coloured text (~2% of pixels) still anchors the black point, so red
///    text on a white page maps to near-black. Deliberately NOT a hard
///    threshold: screenshots mixing dark and light regions keep their
///    mid-tones and dark-mode captures don't get noise blown up to full range.
/// 2. 2x upscale (Catmull-Rom) when the result still fits inside the engine's
///    MaxImageDimension — the engine is measurably more accurate once glyphs
///    are ~20px+ tall.
fn preprocess_for_ocr(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let mut gray = img.to_luma8();
    let (w, h) = gray.dimensions();

    // Histogram over luma values.
    let mut hist = [0u64; 256];
    for p in gray.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let total = (w as u64) * (h as u64);

    // Robust black/white points: darkest and lightest bins with at least
    // 0.05% of pixels (floor of 4 so tiny images aren't outlier-driven).
    let floor = (total / 2000).max(4);
    let lo = (0..256).find(|&i| hist[i] >= floor).unwrap_or(0) as u32;
    let hi = (0..256).rfind(|&i| hist[i] >= floor).unwrap_or(255) as u32;

    // Near-flat image: nothing meaningful to stretch (and dividing by a tiny
    // range would amplify noise). Leave the original alone.
    if hi <= lo || hi - lo < 16 {
        return None;
    }

    let needs_stretch = lo > 8 || hi < 247;
    let cap = max_ocr_dimension().unwrap_or(FALLBACK_MAX_DIMENSION);
    let needs_upscale = w.max(h) * 2 <= cap;

    if !needs_stretch && !needs_upscale {
        return None;
    }

    if needs_stretch {
        let mut lut = [0u8; 256];
        for (i, entry) in lut.iter_mut().enumerate() {
            let v = ((i as f32 - lo as f32) / (hi - lo) as f32 * 255.0).clamp(0.0, 255.0);
            *entry = v as u8;
        }
        for p in gray.pixels_mut() {
            p.0[0] = lut[p.0[0] as usize];
        }
    }

    if needs_upscale {
        gray = image::imageops::resize(
            &gray,
            w * 2,
            h * 2,
            image::imageops::FilterType::CatmullRom,
        );
    }

    log::debug!(
        "[Keyfire] OCR preprocess: {}x{} lo={} hi={} stretch={} upscale={}",
        w, h, lo, hi, needs_stretch, needs_upscale
    );

    let mut out = Vec::new();
    image::DynamicImage::ImageLuma8(gray)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Run OCR over PNG bytes. Blocks until complete. Returns recognised text or a
/// user-friendly error string.
///
/// Walks `OcrResult.Lines()` instead of using `OcrResult.Text()` so line breaks
/// in the source image are preserved. Inserts a blank line between consecutive
/// lines when their vertical gap exceeds the average line height — a rough
/// paragraph-break heuristic that handles screenshots of articles / emails /
/// chat logs reasonably without trying to be clever about columns.
pub fn ocr_png_bytes(png: &[u8]) -> Result<String, String> {
    if png.is_empty() {
        return Err("Empty image data".to_string());
    }

    // Step 0: contrast/scale preprocessing. Falls back to the original bytes
    // when unnecessary or on any decode/encode failure.
    let preprocessed = preprocess_for_ocr(png);
    let png: &[u8] = preprocessed.as_deref().unwrap_or(png);

    // Step 1: write PNG bytes into an InMemoryRandomAccessStream.
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| format!("Failed to create memory stream: {}", e))?;

    let stream_iras: windows::Storage::Streams::IRandomAccessStream = stream
        .cast()
        .map_err(|e| format!("Failed to cast stream to IRandomAccessStream: {}", e))?;

    let writer = DataWriter::CreateDataWriter(&stream_iras)
        .map_err(|e| format!("Failed to create DataWriter: {}", e))?;
    writer
        .WriteBytes(png)
        .map_err(|e| format!("WriteBytes failed: {}", e))?;
    writer
        .StoreAsync()
        .map_err(|e| format!("StoreAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("StoreAsync await failed: {}", e))?;
    writer
        .FlushAsync()
        .map_err(|e| format!("FlushAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("FlushAsync await failed: {}", e))?;
    let _ = writer.DetachStream();

    // Reset stream position to 0 before decoding.
    stream_iras
        .Seek(0)
        .map_err(|e| format!("Stream seek failed: {}", e))?;

    // Step 2: decode the PNG into a SoftwareBitmap. When the source exceeds
    // OcrEngine.MaxImageDimension we scale down at decode time (BitmapTransform)
    // so the whole image reaches OCR — the engine would otherwise crop
    // silently, dropping the bottom/right rows.
    let decoder = BitmapDecoder::CreateAsync(&stream_iras)
        .map_err(|e| format!("BitmapDecoder::CreateAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("BitmapDecoder await failed: {}", e))?;

    let src_w = decoder
        .PixelWidth()
        .map_err(|e| format!("PixelWidth failed: {}", e))?;
    let src_h = decoder
        .PixelHeight()
        .map_err(|e| format!("PixelHeight failed: {}", e))?;

    let max_dim = max_ocr_dimension();
    let needs_scale = match max_dim {
        Some(m) => src_w > m || src_h > m,
        None => false,
    };

    let bitmap = if needs_scale {
        // Preserve aspect ratio. floor() is safe — a stray fractional pixel
        // makes no OCR difference and matches how WinRT rounds internally.
        let m = max_dim.unwrap() as f64;
        let scale = (m / src_w.max(src_h) as f64).min(1.0);
        let dst_w = ((src_w as f64) * scale).floor().max(1.0) as u32;
        let dst_h = ((src_h as f64) * scale).floor().max(1.0) as u32;

        let transform = BitmapTransform::new()
            .map_err(|e| format!("BitmapTransform::new failed: {}", e))?;
        transform
            .SetScaledWidth(dst_w)
            .map_err(|e| format!("SetScaledWidth failed: {}", e))?;
        transform
            .SetScaledHeight(dst_h)
            .map_err(|e| format!("SetScaledHeight failed: {}", e))?;

        log::debug!(
            "[Keyfire] OCR: scaling {}x{} -> {}x{} (MaxImageDimension={})",
            src_w, src_h, dst_w, dst_h, max_dim.unwrap()
        );

        decoder
            .GetSoftwareBitmapTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Premultiplied,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::DoNotColorManage,
            )
            .map_err(|e| format!("GetSoftwareBitmapTransformedAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("GetSoftwareBitmapTransformed await failed: {}", e))?
    } else {
        decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| format!("GetSoftwareBitmapAsync failed: {}", e))?
            .get()
            .map_err(|e| format!("GetSoftwareBitmap await failed: {}", e))?
    };

    // Step 3: try to create an OCR engine for the user's installed languages.
    // If no language packs are installed, RecognizeAsync will fail or the
    // engine call returns an error — surface it as a user-friendly message.
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        format!(
            "OCR not available on this system (no language pack?): {}",
            e
        )
    })?;

    // Step 4: recognize.
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("RecognizeAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Recognize await failed: {}", e))?;

    // Step 5: walk lines top-to-bottom. For each line, compute its vertical
    // extent from its words' bounding rects. Between consecutive lines, if the
    // gap is wider than the average line height, treat that as a paragraph
    // break (insert a blank line); otherwise just join with a newline.
    let lines = result
        .Lines()
        .map_err(|e| format!("Failed to read OCR lines: {}", e))?;
    let line_count = lines
        .Size()
        .map_err(|e| format!("Failed to size OCR lines: {}", e))?;

    if line_count == 0 {
        return Ok(String::new());
    }

    let mut out = String::new();
    let mut prev_bottom: Option<f64> = None;
    let mut prev_height: Option<f64> = None;

    for i in 0..line_count {
        let line = lines
            .GetAt(i)
            .map_err(|e| format!("OCR line {} fetch failed: {}", i, e))?;
        let text = line
            .Text()
            .map_err(|e| format!("OCR line {} text failed: {}", i, e))?
            .to_string();

        // Compute the line's top/bottom from its words' bounding rects.
        // Falls back to (0, 0) if Words is empty or read errors — paragraph
        // detection will degrade to "every line gets a single \n" in that case.
        let (line_top, line_bottom) = match line.Words() {
            Ok(words) => match words.Size() {
                Ok(wc) if wc > 0 => {
                    let mut top = f64::MAX;
                    let mut bottom = f64::MIN;
                    for j in 0..wc {
                        if let Ok(word) = words.GetAt(j) {
                            if let Ok(rect) = word.BoundingRect() {
                                let y = rect.Y as f64;
                                let h = rect.Height as f64;
                                if y < top { top = y; }
                                if y + h > bottom { bottom = y + h; }
                            }
                        }
                    }
                    if top == f64::MAX { (0.0, 0.0) } else { (top, bottom) }
                }
                _ => (0.0, 0.0),
            },
            Err(_) => (0.0, 0.0),
        };
        let line_height = (line_bottom - line_top).max(1.0);

        if i > 0 {
            if let (Some(pb), Some(ph)) = (prev_bottom, prev_height) {
                let gap = line_top - pb;
                let avg_h = ((line_height + ph) / 2.0).max(1.0);
                // Threshold of 0.6× average line height empirically separates
                // standard line-spacing (~0.2-0.3×) from paragraph breaks (~1.0×+).
                if gap > avg_h * 0.6 {
                    out.push_str("\n\n");
                } else {
                    out.push('\n');
                }
            } else {
                out.push('\n');
            }
        }
        out.push_str(&text);

        prev_bottom = Some(line_bottom);
        prev_height = Some(line_height);
    }

    Ok(out)
}
