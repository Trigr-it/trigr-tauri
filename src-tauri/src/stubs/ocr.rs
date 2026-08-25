//! Non-Windows stub for OCR. The real ocr.rs uses the WinRT OCR engine.
//! Apple's Vision framework is the Phase 2 candidate on macOS.
#![allow(dead_code, unused_variables)]

pub fn ocr_png_bytes(png: &[u8]) -> Result<String, String> {
    Err("OCR is not available on this platform yet".to_string())
}

#[derive(Debug, Clone)]
pub struct OcrLineRect {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn ocr_png_bytes_with_rects(png: &[u8]) -> Result<Vec<OcrLineRect>, String> {
    Err("OCR is not available on this platform yet".to_string())
}

pub fn capture_screen_region_png(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    None
}
