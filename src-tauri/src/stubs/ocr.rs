//! Non-Windows stub for OCR. The real ocr.rs uses the WinRT OCR engine.
//! Apple's Vision framework is the Phase 2 candidate on macOS.
#![allow(dead_code, unused_variables)]

pub fn ocr_png_bytes(png: &[u8]) -> Result<String, String> {
    Err("OCR is not available on this platform yet".to_string())
}
