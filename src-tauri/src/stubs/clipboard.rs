//! Non-Windows stub for clipboard history. The real clipboard.rs listens via
//! a Win32 message-only window and stores to SQLite. Returns empty-but-shaped
//! payloads so the frontend panels render an empty state instead of erroring.
//! Native NSPasteboard listener replaces this in Phase 2 of the Mac port.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::path::PathBuf;
use tauri::AppHandle;

pub struct FullClipItem {
    pub content_type: String,
    pub text_content: Option<String>,
    pub html_content: Option<String>,
    pub image_blob: Option<Vec<u8>>,
    pub ocr_text: Option<String>,
}

pub fn init(app_data_dir: PathBuf, app_handle: AppHandle) {
    log::warn!("[stub] clipboard history is not available on this platform yet");
}

pub fn get_history(
    page: u32,
    per_page: u32,
    date_filter: Option<String>,
    app_filter: Option<String>,
    tag_filter: Option<String>,
    search: Option<String>,
    promote_starred: bool,
) -> Value {
    serde_json::json!({ "items": [], "total": 0 })
}

pub fn get_item_full(id: i64) -> Option<FullClipItem> {
    None
}

pub fn delete_item(id: i64) -> bool {
    false
}

pub fn clear_all() -> bool {
    false
}

pub fn reset_storage() -> bool {
    false
}

pub fn encryption_status() -> Value {
    serde_json::json!({
        "encrypted": false,
        "backup_exists": false,
        "backup_expires": null,
        "key_unreadable": false,
        "decrypt_failures": 0,
    })
}

pub fn delete_plaintext_backup_now() -> bool {
    false
}

pub fn pin_item(id: i64, pinned: bool) -> bool {
    false
}

pub fn star_item(id: i64, starred: bool) -> bool {
    false
}

pub fn reorder_pinned(ids: Vec<i64>) -> bool {
    false
}

pub fn reorder_starred(ids: Vec<i64>) -> bool {
    false
}

pub fn get_image_blob(id: i64) -> Option<Vec<u8>> {
    None
}

pub fn get_distinct_source_apps() -> Vec<String> {
    Vec::new()
}

pub fn get_date_buckets(app_filter: Option<String>, tag_filter: Option<String>) -> Value {
    serde_json::json!({ "dates": [], "pinned_count": 0, "starred_count": 0 })
}

pub fn update_item(id: i64, new_text: String) -> Option<String> {
    None
}

pub fn set_ocr_text(id: i64, text: String) {}

pub fn set_retention_days(days: u32) {}

pub fn set_capture_enabled(enabled: bool) {}

pub fn set_excluded_apps(apps: Vec<String>) {}

pub fn get_retention() -> u32 {
    30
}

pub fn dominant_colors(png_bytes: &[u8], n: usize) -> Vec<[u8; 3]> {
    Vec::new()
}

pub fn increment_paste_count(id: i64) {}

pub fn data_dir() -> Option<std::path::PathBuf> {
    None
}

pub fn get_storage_size() -> u64 {
    0
}
