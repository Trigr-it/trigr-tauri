//! Windows.Media.Ocr-based offline OCR for clipboard images.
//!
//! Uses TryCreateFromUserProfileLanguages — works on systems with at least
//! one OCR language pack installed. English ships by default on en-* Windows.
//! Returns Err with a user-friendly message if OCR is unavailable so the UI
//! can show "OCR not available on this system" rather than crashing.
//!
//! Blocks the calling thread — call from spawn_blocking / a dedicated thread.

use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows_core::Interface;

/// Run OCR over PNG bytes. Blocks until complete. Returns recognised text
/// (joined whitespace-separated by Windows.Media.Ocr) or a user-friendly
/// error string.
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

    // Step 2: decode the PNG into a SoftwareBitmap.
    let decoder = BitmapDecoder::CreateAsync(&stream_iras)
        .map_err(|e| format!("BitmapDecoder::CreateAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("BitmapDecoder await failed: {}", e))?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| format!("GetSoftwareBitmapAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("GetSoftwareBitmap await failed: {}", e))?;

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

    let text = result
        .Text()
        .map_err(|e| format!("Failed to read OCR text: {}", e))?
        .to_string();

    Ok(text)
}
