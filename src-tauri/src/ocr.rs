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
pub fn preprocess_for_ocr(png: &[u8]) -> Option<Vec<u8>> {
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
    // Pick the largest integer scale (up to 4×) that keeps the image within
    // the OCR engine's MaxImageDimension cap. Small button-sized captures —
    // typical of Wait for Text on a UI control — benefit disproportionately
    // from more upscale: Windows.Media.Ocr wants glyphs ~20 px tall to be
    // reliable, so a 12-15px button label needs at least 2× and ideally 3×
    // to land in that zone.
    let max_side = w.max(h);
    let upscale_factor: u32 = if max_side * 4 <= cap { 4 }
        else if max_side * 3 <= cap { 3 }
        else if max_side * 2 <= cap { 2 }
        else { 1 };
    let needs_upscale = upscale_factor > 1;

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
            w * upscale_factor,
            h * upscale_factor,
            image::imageops::FilterType::CatmullRom,
        );
    }

    log::debug!(
        "[Keyfire] OCR preprocess: {}x{} lo={} hi={} stretch={} upscale={}x",
        w, h, lo, hi, needs_stretch, upscale_factor
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

/// One OCR line with its bounding rect in SOURCE-IMAGE pixel coords
/// (already unscaled from any BitmapTransform or preprocess upscale).
#[derive(Debug, Clone)]
pub struct OcrLineRect {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// OCR-plus-bounding-rects variant of [`ocr_png_bytes`]. Used by the "Wait
/// for Text" macro step so clickOnMatch can drop a click at the centre of
/// the matched line. Text output matches Line.Text() verbatim (no paragraph
/// gap insertion — the caller does its own substring matching per line).
///
/// The returned rects are ALWAYS in source-image coord space regardless of
/// whether preprocess_for_ocr's 2× upscale or BitmapTransform's downscale
/// fired internally: we track both scale factors and divide out at the end.
pub fn ocr_png_bytes_with_rects(png: &[u8]) -> Result<Vec<OcrLineRect>, String> {
    if png.is_empty() {
        return Err("Empty image data".to_string());
    }

    // Preprocess exactly like the plain path — accuracy matters for button
    // labels — but remember whether the upscale ran so we can reverse it.
    // preprocess_for_ocr's upscale is always 2× (integer), so the reverse
    // factor is trivial.
    let src_dims_before_preprocess = image::load_from_memory(png)
        .ok()
        .map(|img| (img.width(), img.height()));
    let preprocessed = preprocess_for_ocr(png);
    let png: &[u8] = preprocessed.as_deref().unwrap_or(png);
    let mut preprocess_scale: f64 = 1.0;
    if let (Some(pp), Some((sw, _sh))) = (preprocessed.as_deref(), src_dims_before_preprocess) {
        if let Ok(pp_img) = image::load_from_memory(pp) {
            if sw > 0 {
                preprocess_scale = (pp_img.width() as f64) / (sw as f64);
            }
        }
    }

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
    stream_iras
        .Seek(0)
        .map_err(|e| format!("Stream seek failed: {}", e))?;

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
    let mut transform_scale: f64 = 1.0;

    let bitmap = if needs_scale {
        let m = max_dim.unwrap() as f64;
        let scale = (m / src_w.max(src_h) as f64).min(1.0);
        transform_scale = scale;
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

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        format!(
            "OCR not available on this system (no language pack?): {}",
            e
        )
    })?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("RecognizeAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Recognize await failed: {}", e))?;

    // Total scale from source-image coords to the coord space of the rects
    // WinRT emits: preprocess_scale × transform_scale. Both default to 1.0
    // when their respective step didn't fire, so a single divide reverses
    // whichever combination actually ran (usually just one, often neither).
    let total_scale = preprocess_scale * transform_scale;
    let inv_scale = if total_scale > 0.0 { 1.0 / total_scale } else { 1.0 };

    let lines = result
        .Lines()
        .map_err(|e| format!("Failed to read OCR lines: {}", e))?;
    let line_count = lines
        .Size()
        .map_err(|e| format!("Failed to size OCR lines: {}", e))?;
    let mut out = Vec::with_capacity(line_count as usize);
    for i in 0..line_count {
        let line = lines
            .GetAt(i)
            .map_err(|e| format!("OCR line {} fetch failed: {}", i, e))?;
        let text = line
            .Text()
            .map_err(|e| format!("OCR line {} text failed: {}", i, e))?
            .to_string();
        // Line bounding rect = union of its words' rects. WinRT gives us
        // per-word bounds but not a Line.BoundingRect, so we roll it up.
        let (mut left, mut top, mut right, mut bottom) =
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        if let Ok(words) = line.Words() {
            if let Ok(wc) = words.Size() {
                for j in 0..wc {
                    if let Ok(word) = words.GetAt(j) {
                        if let Ok(r) = word.BoundingRect() {
                            let x = r.X as f64;
                            let y = r.Y as f64;
                            let w = r.Width as f64;
                            let h = r.Height as f64;
                            if x < left { left = x; }
                            if y < top { top = y; }
                            if x + w > right { right = x + w; }
                            if y + h > bottom { bottom = y + h; }
                        }
                    }
                }
            }
        }
        if left == f64::MAX {
            // No word rects — skip this line rather than emit garbage
            // coords the click-on-match arm would faithfully click.
            continue;
        }
        let x = (left * inv_scale).round() as i32;
        let y = (top * inv_scale).round() as i32;
        let w = ((right - left) * inv_scale).round().max(1.0) as i32;
        let h = ((bottom - top) * inv_scale).round().max(1.0) as i32;
        out.push(OcrLineRect { text, x, y, w, h });
    }
    Ok(out)
}

/// Screen-region PNG capture for the Wait for Text macro step. BitBlts a
/// virtual-desktop rectangle into a 32-bit DIB then encodes as PNG so the
/// existing OCR pipeline (which speaks PNG in/text+rects out) can consume
/// it without a second pixel path. Returns None on any GDI failure — the
/// caller treats that as "no OCR this poll" and retries next tick.
pub fn capture_screen_region_png(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, SRCCOPY,
    };
    if w <= 0 || h <= 0 { return None; }
    unsafe {
        let screen_dc = GetDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND);
        if screen_dc.is_null() { return None; }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_null() {
            ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen_dc);
            return None;
        }
        let bmp = CreateCompatibleBitmap(screen_dc, w, h);
        if bmp.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen_dc);
            return None;
        }
        let old = SelectObject(mem_dc, bmp);
        let blt_ok = BitBlt(mem_dc, 0, 0, w, h, screen_dc, x, y, SRCCOPY);
        if blt_ok == 0 {
            SelectObject(mem_dc, old);
            DeleteObject(bmp);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen_dc);
            return None;
        }

        // Pull pixels out as 32bpp top-down (negative biHeight) so the row
        // order matches image::RgbaImage's expectations directly.
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB as u32;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let scanlines = GetDIBits(
            mem_dc,
            bmp,
            0,
            h as u32,
            buf.as_mut_ptr() as *mut _,
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem_dc, old);
        DeleteObject(bmp);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, screen_dc);
        if scanlines == 0 { return None; }

        // GDI 32bpp is BGRA (alpha undefined) — swap to RGBA and force
        // alpha to 255 so the PNG encoder writes a fully opaque image.
        for chunk in buf.chunks_exact_mut(4) {
            let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
            chunk[0] = r;
            chunk[1] = g;
            chunk[2] = b;
            chunk[3] = 255;
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, buf)?;
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(out)
    }
}
