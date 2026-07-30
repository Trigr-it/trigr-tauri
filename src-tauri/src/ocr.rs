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
