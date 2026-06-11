use log::{error, info};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use tauri::AppHandle;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, IsClipboardFormatAvailable,
    OpenClipboard, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetForegroundWindow, GetWindowThreadProcessId,
    RegisterClassW, HWND_MESSAGE, MSG, WNDCLASSW, WS_OVERLAPPED,
};

// ── Clipboard formats ────────────────────────────────────────────────────────

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const DEFAULT_RETENTION_DAYS: u32 = 7;

// ── Clipboard entry ──────────────────────────────────────────────────────────

struct ClipEntry {
    content_type: String,
    text_content: Option<String>,
    image_blob: Option<Vec<u8>>,
    image_width: u32,
    image_height: u32,
    preview: String,
    source_app: String,
    content_tag: String,
}

// ── Writer thread channel ────────────────────────────────────────────────────

static CLIPBOARD_TX: OnceLock<Mutex<mpsc::Sender<ClipboardMsg>>> = OnceLock::new();
static DB_PATH: OnceLock<PathBuf> = OnceLock::new();

enum ClipboardMsg {
    NewEntry(ClipEntry),
    GetHistory {
        page: u32,
        per_page: u32,
        /// None       → default view (all visible rows in effective window)
        /// Some("pinned")     → only pinned items, ignoring age
        /// Some("YYYY-MM-DD") → only rows whose local date matches
        date_filter: Option<String>,
        /// Exact match on source_app column (e.g. "chrome.exe"). None = no filter.
        app_filter: Option<String>,
        /// Exact match on content_tag column ("Text", "Image", ...). None = no filter.
        tag_filter: Option<String>,
        /// Case-insensitive substring match against the preview column. None / empty = no filter.
        search: Option<String>,
        reply: mpsc::Sender<Value>,
    },
    GetItemFull {
        id: i64,
        reply: mpsc::Sender<Option<FullClipItem>>,
    },
    DeleteItem {
        id: i64,
        reply: mpsc::Sender<bool>,
    },
    ClearAll {
        reply: mpsc::Sender<bool>,
    },
    PinItem {
        id: i64,
        pinned: bool,
        reply: mpsc::Sender<bool>,
    },
    GetImageBlob {
        id: i64,
        reply: mpsc::Sender<Option<Vec<u8>>>,
    },
    GetDistinctSourceApps {
        reply: mpsc::Sender<Vec<String>>,
    },
    GetDateBuckets {
        reply: mpsc::Sender<Value>,
    },
    UpdateItem {
        id: i64,
        new_text: String,
        reply: mpsc::Sender<Option<String>>, // returns new content_tag on success
    },
    SetOcrText {
        id: i64,
        text: String,
    },
    IncrementPasteCount {
        id: i64,
    },
    Prune,
}

pub struct FullClipItem {
    pub content_type: String,
    pub text_content: Option<String>,
    pub image_blob: Option<Vec<u8>>,
    pub ocr_text: Option<String>,
}

// ── App handle for Tauri events ──────────────────────────────────────────────

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

// ── Retention days ───────────────────────────────────────────────────────────

static RETENTION_DAYS: OnceLock<Mutex<u32>> = OnceLock::new();

fn retention_days() -> u32 {
    RETENTION_DAYS
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| *g)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Pro-gated retention: raw stored value clamped by current licence tier.
/// Free max is 7 days, Pro max is 30. The stored preference (from config)
/// is preserved as-is so a Pro user who downgrades and then re-upgrades
/// gets their original setting back automatically. Prune + UI both read
/// through here, so a runtime licence transition (e.g. trial expiry while
/// the app is running) takes effect on the next prune cycle without restart.
fn effective_retention_days() -> u32 {
    let raw = retention_days();
    let max = if crate::licence::is_pro() { 30 } else { 7 };
    raw.min(max)
}

// ── Capture-enabled gate + per-app exclusion list ───────────────────────────
//
// Both default to permissive (capture on, no exclusions) so existing installs
// keep behaving as before. Users opt in via Settings — no apps are excluded
// out of the box, a deliberate design call.

static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(true);
static EXCLUDED_APPS: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn excluded_apps() -> &'static RwLock<HashSet<String>> {
    EXCLUDED_APPS.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Normalize a process name for comparison: lowercase + strip `.exe`. Matches
/// the convention used by foreground.rs so picker output and live foreground
/// detection compare equal.
fn normalize_proc_name(name: &str) -> String {
    name.to_lowercase().trim_end_matches(".exe").to_string()
}

fn is_app_excluded(proc_name: &str) -> bool {
    let normalized = normalize_proc_name(proc_name);
    if normalized.is_empty() {
        return false;
    }
    excluded_apps()
        .read()
        .map(|set| set.contains(&normalized))
        .unwrap_or(false)
}

// ── Deduplication ────────────────────────────────────────────────────────────

static LAST_HASH: OnceLock<Mutex<u64>> = OnceLock::new();

fn last_hash() -> &'static Mutex<u64> {
    LAST_HASH.get_or_init(|| Mutex::new(0))
}

fn compute_hash(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

// ── Auto-tagging ─────────────────────────────────────────────────────────────

fn auto_tag(content_type: &str, text: Option<&str>) -> String {
    if content_type == "image" {
        return "Image".to_string();
    }
    let t = match text {
        Some(s) => s.trim(),
        None => return "Text".to_string(),
    };
    if t.is_empty() {
        return "Text".to_string();
    }
    // Link
    if t.starts_with("http://") || t.starts_with("https://") {
        return "Link".to_string();
    }
    // Email — contains @ with a dot after it
    if let Some(at_pos) = t.find('@') {
        if t[at_pos..].contains('.') {
            return "Email".to_string();
        }
    }
    // Colour — #hex or rgb( or rgba(
    if (t.starts_with('#') && t.len() >= 4 && t.len() <= 7
        && t[1..].chars().all(|c| c.is_ascii_hexdigit()))
        || t.starts_with("rgb(")
        || t.starts_with("rgba(")
    {
        return "Colour".to_string();
    }
    // Number — purely numeric with optional currency/percent
    {
        let stripped = t.replace(|c: char| "£$€%,. \t".contains(c), "");
        if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
            return "Number".to_string();
        }
    }
    "Text".to_string()
}

// ── Source app capture ────────────────────────────────────────────────────────

fn get_foreground_process_name() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return String::new();
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return String::new();
        }
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            // Try with limited access
            let process2 = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if process2.is_null() {
                return String::new();
            }
            let name = query_process_name(process2);
            windows_sys::Win32::Foundation::CloseHandle(process2);
            return name;
        }
        let name = query_process_name(process);
        windows_sys::Win32::Foundation::CloseHandle(process);
        name
    }
}

unsafe fn query_process_name(process: *mut std::ffi::c_void) -> String {
    // Use QueryFullProcessImageNameW which works across sessions
    let mut buf = [0u16; 260];
    let mut size: u32 = 260;
    let ok = windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
        process,
        0,
        buf.as_mut_ptr(),
        &mut size,
    );
    if ok == 0 || size == 0 {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    // Extract just the filename
    path.rsplit('\\').next().unwrap_or("").to_string()
}

// ── Initialise ───────────────────────────────────────────────────────────────

pub fn init(app_data_dir: PathBuf, app_handle: AppHandle) {
    let _ = APP_HANDLE.set(app_handle);
    let _ = RETENTION_DAYS.set(Mutex::new(DEFAULT_RETENTION_DAYS));

    if let Some(cfg) = crate::config::load_config() {
        if let Some(days) = cfg.get("clipboardRetentionDays").and_then(|v| v.as_u64()) {
            if let Ok(mut g) = RETENTION_DAYS.get().unwrap().lock() {
                *g = (days as u32).clamp(1, 30);
            }
        }
        if let Some(enabled) = cfg.get("clipboardCaptureEnabled").and_then(|v| v.as_bool()) {
            CAPTURE_ENABLED.store(enabled, Ordering::SeqCst);
        }
        if let Some(arr) = cfg.get("clipboardExcludedApps").and_then(|v| v.as_array()) {
            let apps: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            set_excluded_apps(apps);
        }
    }

    let db_path = app_data_dir.join("trigr-clipboard.db");
    let _ = DB_PATH.set(db_path.clone());
    let (tx, rx) = mpsc::channel::<ClipboardMsg>();
    let _ = CLIPBOARD_TX.set(Mutex::new(tx));

    thread::Builder::new()
        .name("trigr-clipboard-writer".to_string())
        .spawn(move || {
            let conn = match Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("[Trigr] Failed to open clipboard DB: {}", e);
                    return;
                }
            };

            let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

            if let Err(e) = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS clipboard_history (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp    TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    text_content TEXT,
                    image_blob   BLOB,
                    image_width  INTEGER DEFAULT 0,
                    image_height INTEGER DEFAULT 0,
                    preview      TEXT NOT NULL DEFAULT '',
                    pinned       INTEGER DEFAULT 0
                );",
            ) {
                error!("[Trigr] Failed to create clipboard table: {}", e);
                return;
            }

            // Schema migration: add source_app and content_tag columns if missing
            let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN source_app TEXT NOT NULL DEFAULT ''", []);
            let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN content_tag TEXT NOT NULL DEFAULT 'Text'", []);
            // paste_count: number of times this entry has been pasted via the main UI.
            // DEFAULT 0 — existing rows get 0 cleanly, no data loss.
            let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN paste_count INTEGER NOT NULL DEFAULT 0", []);
            // ocr_text: cached OCR result for image rows. NULL until the user runs
            // Extract Text. Populated by `set_ocr_text` after `ocr_clipboard_image`
            // succeeds so re-selecting the same image shows the text without re-OCR.
            let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN ocr_text TEXT", []);

            info!("[Trigr] Clipboard DB ready: {}", db_path.display());

            for msg in rx {
                match msg {
                    ClipboardMsg::NewEntry(entry) => handle_new_entry(&conn, entry),
                    ClipboardMsg::GetHistory { page, per_page, date_filter, app_filter, tag_filter, search, reply } => {
                        let result = handle_get_history(
                            &conn, page, per_page,
                            date_filter.as_deref(),
                            app_filter.as_deref(),
                            tag_filter.as_deref(),
                            search.as_deref(),
                        );
                        let _ = reply.send(result);
                    }
                    ClipboardMsg::GetItemFull { id, reply } => {
                        let item = handle_get_item_full(&conn, id);
                        let _ = reply.send(item);
                    }
                    ClipboardMsg::DeleteItem { id, reply } => {
                        let ok = handle_delete_item(&conn, id);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::ClearAll { reply } => {
                        let ok = handle_clear_all(&conn);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::PinItem { id, pinned, reply } => {
                        let ok = handle_pin_item(&conn, id, pinned);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::GetImageBlob { id, reply } => {
                        let blob = handle_get_image_blob(&conn, id);
                        let _ = reply.send(blob);
                    }
                    ClipboardMsg::GetDistinctSourceApps { reply } => {
                        let apps = handle_get_distinct_source_apps(&conn);
                        let _ = reply.send(apps);
                    }
                    ClipboardMsg::GetDateBuckets { reply } => {
                        let buckets = handle_get_date_buckets(&conn);
                        let _ = reply.send(buckets);
                    }
                    ClipboardMsg::UpdateItem { id, new_text, reply } => {
                        let result = handle_update_item(&conn, id, &new_text);
                        let _ = reply.send(result);
                    }
                    ClipboardMsg::SetOcrText { id, text } => {
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET ocr_text = ?1 WHERE id = ?2",
                            rusqlite::params![text, id],
                        );
                    }
                    ClipboardMsg::IncrementPasteCount { id } => {
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET paste_count = paste_count + 1 WHERE id = ?1",
                            rusqlite::params![id],
                        );
                    }
                    ClipboardMsg::Prune => handle_prune(&conn),
                }
            }
        })
        .expect("Failed to spawn clipboard writer thread");

    thread::Builder::new()
        .name("trigr-clipboard-listener".to_string())
        .spawn(|| run_clipboard_listener())
        .expect("Failed to spawn clipboard listener thread");
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn get_history(
    page: u32,
    per_page: u32,
    date_filter: Option<String>,
    app_filter: Option<String>,
    tag_filter: Option<String>,
    search: Option<String>,
) -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetHistory {
                page, per_page, date_filter, app_filter, tag_filter, search,
                reply: reply_tx,
            }).is_ok() {
                if let Ok(result) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return result;
                }
            }
        }
    }
    serde_json::json!({ "items": [], "total": 0 })
}

pub fn get_item_full(id: i64) -> Option<FullClipItem> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetItemFull { id, reply: reply_tx }).is_ok() {
                if let Ok(item) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return item;
                }
            }
        }
    }
    None
}

pub fn delete_item(id: i64) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::DeleteItem { id, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn clear_all() -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::ClearAll { reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn pin_item(id: i64, pinned: bool) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::PinItem { id, pinned, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn get_image_blob(id: i64) -> Option<Vec<u8>> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetImageBlob { id, reply: reply_tx }).is_ok() {
                if let Ok(blob) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return blob;
                }
            }
        }
    }
    None
}

pub fn get_distinct_source_apps() -> Vec<String> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetDistinctSourceApps { reply: reply_tx }).is_ok() {
                if let Ok(apps) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return apps;
                }
            }
        }
    }
    Vec::new()
}

pub fn get_date_buckets() -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetDateBuckets { reply: reply_tx }).is_ok() {
                if let Ok(buckets) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return buckets;
                }
            }
        }
    }
    serde_json::json!({ "dates": [], "pinned_count": 0 })
}

pub fn update_item(id: i64, new_text: String) -> Option<String> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::UpdateItem { id, new_text, reply: reply_tx }).is_ok() {
                if let Ok(tag) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return tag;
                }
            }
        }
    }
    None
}

/// Cache OCR text on an image row. Fire-and-forget; failures are logged at the
/// writer thread but not surfaced — re-running OCR is cheap and the displayed
/// text is preserved in frontend state for the current session either way.
pub fn set_ocr_text(id: i64, text: String) {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::SetOcrText { id, text });
        }
    }
}

pub fn set_retention_days(days: u32) {
    let clamped = days.clamp(1, 30);
    if let Some(m) = RETENTION_DAYS.get() {
        if let Ok(mut g) = m.lock() {
            *g = clamped;
        }
    }
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::Prune);
        }
    }
}

pub fn is_capture_enabled() -> bool {
    CAPTURE_ENABLED.load(Ordering::SeqCst)
}

pub fn set_capture_enabled(enabled: bool) {
    CAPTURE_ENABLED.store(enabled, Ordering::SeqCst);
    // Sync the clipboard paste hotkey in the hook suppress set so the combo
    // (default Ctrl+Shift+V) is freed for normal OS use when capture is
    // disabled, and reclaimed when it is re-enabled. Without this the user
    // could turn off clipboard but the hotkey would still be hijacked.
    crate::hotkeys::refresh_clipboard_paste_suppress();
}

pub fn set_excluded_apps(apps: Vec<String>) {
    let normalized: HashSet<String> = apps
        .into_iter()
        .map(|a| normalize_proc_name(&a))
        .filter(|a| !a.is_empty())
        .collect();
    if let Ok(mut g) = excluded_apps().write() {
        *g = normalized;
    }
}

pub fn get_retention() -> u32 {
    effective_retention_days()
}

/// Extracts up to `n` dominant RGB colours from PNG bytes via the color-thief
/// crate. Returns empty Vec on decode/extraction failure.
pub fn dominant_colors(png_bytes: &[u8], n: usize) -> Vec<[u8; 3]> {
    use color_thief::ColorFormat;

    if png_bytes.is_empty() {
        return Vec::new();
    }
    let img = match image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png) {
        Ok(i) => i,
        Err(_) => return Vec::new(),
    };
    let rgba = img.to_rgba8();
    let pixels = rgba.as_raw();

    // color-thief requires count >= 2 and quality 1..=10 (lower = more accurate, slower).
    let count = n.clamp(2, 10) as u8;
    let palette = match color_thief::get_palette(pixels, ColorFormat::Rgba, 10, count) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    palette.into_iter().take(n).map(|c| [c.r, c.g, c.b]).collect()
}

/// Increments the paste_count for a given clipboard entry. Fire-and-forget —
/// no reply channel; failures are silently dropped (best-effort counter).
pub fn increment_paste_count(id: i64) {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::IncrementPasteCount { id });
        }
    }
}

/// Returns the directory containing trigr-clipboard.db (and its WAL/SHM files).
/// Used by the "Open clipboard folder" settings button so it always opens the
/// real folder regardless of which AppData root the app picked at init.
pub fn data_dir() -> Option<std::path::PathBuf> {
    DB_PATH.get().and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

pub fn get_storage_size() -> u64 {
    if let Some(path) = DB_PATH.get() {
        // Include WAL and SHM files in total size
        let mut total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let wal = path.with_extension("db-wal");
        let shm = path.with_extension("db-shm");
        total += std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        total += std::fs::metadata(&shm).map(|m| m.len()).unwrap_or(0);
        total
    } else {
        0
    }
}

// ── Writer thread handlers ───────────────────────────────────────────────────

fn handle_new_entry(conn: &Connection, entry: ClipEntry) {
    let now = chrono::Utc::now().to_rfc3339();

    let result = conn.execute(
        "INSERT INTO clipboard_history (timestamp, content_type, text_content, image_blob, image_width, image_height, preview, pinned, source_app, content_tag)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
        rusqlite::params![
            now,
            entry.content_type,
            entry.text_content,
            entry.image_blob,
            entry.image_width,
            entry.image_height,
            entry.preview,
            entry.source_app,
            entry.content_tag,
        ],
    );

    if let Err(e) = result {
        error!("[Trigr] Failed to insert clipboard entry: {}", e);
        return;
    }

    let new_id = conn.last_insert_rowid();
    handle_prune(conn);

    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-new-item",
            serde_json::json!({
                "id": new_id,
                "timestamp": now,
                "content_type": entry.content_type,
                "text_content": entry.text_content,
                "preview": entry.preview,
                "image_width": entry.image_width,
                "image_height": entry.image_height,
                "pinned": false,
                "source_app": entry.source_app,
                "content_tag": entry.content_tag,
            }),
        );
    }
}

fn handle_get_history(
    conn: &Connection,
    page: u32,
    per_page: u32,
    date_filter: Option<&str>,
    app_filter: Option<&str>,
    tag_filter: Option<&str>,
    search: Option<&str>,
) -> Value {
    let offset = page.saturating_sub(1) * per_page;
    // Pro-gated visibility window (used by the default + per-date views).
    // Pinned rows always bypass age. Per [[feedback_sqlite_localtime_pattern]]
    // we compare local-time dates via DATE(timestamp, 'localtime').
    let days = effective_retention_days();
    let date_clause = match date_filter {
        // Sidebar "Pinned" bucket — every pinned row, ignoring age.
        Some("pinned") => "pinned = 1".to_string(),
        // Sidebar single-date bucket — match a specific local calendar date.
        // No Pro-gate filter here because the bucket query already excluded
        // dates outside the effective window before listing them.
        Some(d) if d.len() == 10 && d.chars().nth(4) == Some('-') => {
            // Input shape is enforced by the date-bucket query that produced it
            // (YYYY-MM-DD). We strip stray quotes defensively.
            let safe = d.replace('\'', "");
            format!("DATE(timestamp, 'localtime') = '{}'", safe)
        }
        // Default / unrecognised filter — Pro-gated default view.
        _ => format!("(pinned = 1 OR timestamp >= datetime('now', '-{} days'))", days),
    };

    // Toolbar filters (app, tag, search) layer on top of the date clause via
    // AND-joined predicates with unnumbered `?` placeholders — SQLite binds
    // them positionally from rusqlite::params_from_iter so we don't have to
    // track numbering across two queries. User input goes through binds, not
    // SQL string interpolation. LIKE wildcards in search are escaped so a
    // literal % or _ in the query doesn't broaden the match.
    let mut clauses: Vec<String> = vec![format!("({})", date_clause)];
    let mut where_binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(app) = app_filter.filter(|s| !s.is_empty()) {
        clauses.push("source_app = ?".to_string());
        where_binds.push(Box::new(app.to_string()));
    }
    if let Some(tag) = tag_filter.filter(|s| !s.is_empty() && *s != "All") {
        clauses.push("content_tag = ?".to_string());
        where_binds.push(Box::new(tag.to_string()));
    }
    if let Some(q) = search.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let escaped = q.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_");
        clauses.push("LOWER(preview) LIKE ? ESCAPE '\\'".to_string());
        where_binds.push(Box::new(format!("%{}%", escaped.to_lowercase())));
    }
    let where_clause = clauses.join(" AND ");

    // COUNT — same WHERE, just the toolbar binds.
    let count_sql = format!("SELECT COUNT(*) FROM clipboard_history WHERE {}", where_clause);
    let count_refs: Vec<&dyn rusqlite::ToSql> = where_binds.iter().map(|p| p.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, rusqlite::params_from_iter(count_refs.iter()), |row| row.get(0))
        .unwrap_or(0);

    // LIST — same WHERE, then LIMIT/OFFSET appended after the toolbar binds.
    let list_sql = format!(
        "SELECT id, timestamp, content_type, text_content, image_width, image_height, preview, pinned, source_app, content_tag, paste_count, ocr_text
         FROM clipboard_history WHERE {} ORDER BY pinned DESC, id DESC LIMIT ? OFFSET ?",
        where_clause
    );
    let mut list_binds: Vec<Box<dyn rusqlite::ToSql>> = where_binds;
    list_binds.push(Box::new(per_page as i64));
    list_binds.push(Box::new(offset as i64));
    let list_refs: Vec<&dyn rusqlite::ToSql> = list_binds.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&list_sql).unwrap();

    let items: Vec<Value> = stmt
        .query_map(rusqlite::params_from_iter(list_refs.iter()), |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0).unwrap_or(0),
                "timestamp": row.get::<_, String>(1).unwrap_or_default(),
                "content_type": row.get::<_, String>(2).unwrap_or_default(),
                "text_content": row.get::<_, Option<String>>(3).unwrap_or(None),
                "image_width": row.get::<_, u32>(4).unwrap_or(0),
                "image_height": row.get::<_, u32>(5).unwrap_or(0),
                "preview": row.get::<_, String>(6).unwrap_or_default(),
                "pinned": row.get::<_, i32>(7).unwrap_or(0) != 0,
                "source_app": row.get::<_, String>(8).unwrap_or_default(),
                "content_tag": row.get::<_, String>(9).unwrap_or("Text".to_string()),
                "paste_count": row.get::<_, i64>(10).unwrap_or(0),
                "ocr_text": row.get::<_, Option<String>>(11).unwrap_or(None),
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    serde_json::json!({ "items": items, "total": total })
}

fn handle_get_item_full(conn: &Connection, id: i64) -> Option<FullClipItem> {
    conn.query_row(
        "SELECT content_type, text_content, image_blob, ocr_text FROM clipboard_history WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(FullClipItem {
                content_type: row.get::<_, String>(0).unwrap_or_default(),
                text_content: row.get::<_, Option<String>>(1).unwrap_or(None),
                image_blob: row.get::<_, Option<Vec<u8>>>(2).unwrap_or(None),
                ocr_text: row.get::<_, Option<String>>(3).unwrap_or(None),
            })
        },
    )
    .ok()
}

fn handle_delete_item(conn: &Connection, id: i64) -> bool {
    conn.execute("DELETE FROM clipboard_history WHERE id = ?1", rusqlite::params![id]).is_ok()
}

fn handle_clear_all(conn: &Connection) -> bool {
    if let Err(e) = conn.execute("DELETE FROM clipboard_history", []) {
        error!("[Trigr] Failed to clear clipboard history: {}", e);
        return false;
    }
    // Reclaim disk space. DELETE alone leaves the file at its high-water mark, and in
    // WAL mode VACUUM alone leaves the .db-wal file large. Both steps are needed:
    //   1. VACUUM         — rebuild .db, freeing pages held by deleted rows.
    //   2. wal_checkpoint — flush WAL into .db and truncate .db-wal back to zero bytes.
    let mut vacuum_ok = true;
    if let Err(e) = conn.execute("VACUUM", []) {
        error!("[Trigr] VACUUM after clear failed: {}", e);
        vacuum_ok = false;
    }
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        error!("[Trigr] WAL truncate after clear failed: {}", e);
        vacuum_ok = false;
    }
    if vacuum_ok {
        info!("[Trigr] Clipboard history cleared, database vacuumed and WAL truncated");
    }
    // Always return true — the table is empty either way; only file size may not have shrunk.
    true
}

fn handle_pin_item(conn: &Connection, id: i64, pinned: bool) -> bool {
    let val: i32 = if pinned { 1 } else { 0 };
    conn.execute("UPDATE clipboard_history SET pinned = ?1 WHERE id = ?2", rusqlite::params![val, id]).is_ok()
}

fn handle_get_image_blob(conn: &Connection, id: i64) -> Option<Vec<u8>> {
    conn.query_row(
        "SELECT image_blob FROM clipboard_history WHERE id = ?1 AND content_type = 'image'",
        rusqlite::params![id],
        |row| row.get::<_, Option<Vec<u8>>>(0),
    ).ok().flatten()
}

fn handle_get_distinct_source_apps(conn: &Connection) -> Vec<String> {
    // Mirror the Pro-gated visibility from handle_get_history: only return
    // source apps that appear in rows the Free user can actually see. Without
    // this filter, the source-filter dropdown would list apps from hidden rows.
    let days = effective_retention_days();
    let sql = format!(
        "SELECT DISTINCT source_app FROM clipboard_history
         WHERE source_app != '' AND (pinned = 1 OR timestamp >= datetime('now', '-{} days'))
         ORDER BY source_app ASC",
        days
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// Returns the date buckets used by the ClipboardPanel sidebar:
///   { "dates": [{ "date": "YYYY-MM-DD", "count": N }, ...], "pinned_count": M }
/// One row per distinct local-calendar date that has non-pinned content within
/// the effective Pro-gated retention window. Pinned items are bucketed
/// separately (the sidebar shows them under a "Pinned" entry that ignores age),
/// so they're not counted in the date rows. Per [[feedback_sqlite_localtime_pattern]]
/// we store UTC and convert with DATE(timestamp, 'localtime') for grouping.
fn handle_get_date_buckets(conn: &Connection) -> Value {
    let days = effective_retention_days();
    let dates_sql = format!(
        "SELECT DATE(timestamp, 'localtime') AS local_date, COUNT(*) AS cnt
         FROM clipboard_history
         WHERE pinned = 0 AND timestamp >= datetime('now', '-{} days')
         GROUP BY local_date
         ORDER BY local_date DESC",
        days
    );

    let mut stmt = match conn.prepare(&dates_sql) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({ "dates": [], "pinned_count": 0 }),
    };
    let dates: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "date": row.get::<_, String>(0).unwrap_or_default(),
                "count": row.get::<_, i64>(1).unwrap_or(0),
            }))
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let pinned_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE pinned = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    serde_json::json!({ "dates": dates, "pinned_count": pinned_count })
}

fn handle_update_item(conn: &Connection, id: i64, new_text: &str) -> Option<String> {
    let new_tag = auto_tag("text", Some(new_text));
    let preview = if new_text.len() > 200 {
        let end = new_text.char_indices().nth(200).map(|(i, _)| i).unwrap_or(new_text.len());
        format!("{}…", &new_text[..end])
    } else {
        new_text.to_string()
    };
    match conn.execute(
        "UPDATE clipboard_history SET text_content = ?1, preview = ?2, content_tag = ?3 WHERE id = ?4 AND content_type = 'text'",
        rusqlite::params![new_text, preview, new_tag, id],
    ) {
        Ok(rows) if rows > 0 => Some(new_tag),
        _ => None,
    }
}

fn handle_prune(conn: &Connection) {
    // Prune uses the RAW stored preference, not the Pro-gated effective value.
    // This preserves a downgraded Pro user's data on disk so it reappears on
    // re-upgrade. The Free user's UI is gated separately at query time below,
    // so they only see the most recent 7 days even when more rows exist.
    // Always-Free users still naturally cap at 7 because raw = 7 default.
    let days = retention_days();
    let query = format!(
        "DELETE FROM clipboard_history WHERE pinned = 0 AND timestamp < datetime('now', '-{} days')",
        days
    );
    match conn.execute(&query, []) {
        Ok(deleted) if deleted > 0 => {
            info!("[Trigr] Pruned {} expired clipboard items", deleted);
            // Reclaim space — VACUUM rebuilds .db, wal_checkpoint(TRUNCATE) shrinks .db-wal.
            // Both are skipped when nothing was deleted (common case — handle_prune runs
            // after every new clipboard entry).
            if let Err(e) = conn.execute("VACUUM", []) {
                error!("[Trigr] VACUUM after prune failed: {}", e);
            }
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                error!("[Trigr] WAL truncate after prune failed: {}", e);
            }
        }
        Ok(_) => {} // nothing pruned — no space to reclaim
        Err(e) => error!("[Trigr] Prune query failed: {}", e),
    }
}

// ── Clipboard image helper ───────────────────────────────────────────────────

unsafe fn read_clipboard_image() -> Option<(Vec<u8>, u32, u32)> {
    let handle = GetClipboardData(CF_DIB);
    if handle.is_null() { return None; }
    let size = GlobalSize(handle);
    if size < 40 { return None; }
    let ptr = GlobalLock(handle) as *const u8;
    if ptr.is_null() { return None; }

    let header = ptr as *const u32;
    let width = *header.add(1);
    let height_raw = *header.add(2) as i32;
    let height = height_raw.unsigned_abs();
    let planes_bits = *header.add(3);
    let bit_count = (planes_bits >> 16) as u16;
    let compression = *header.add(4);

    if (compression != 0 && compression != 3) || (bit_count != 24 && bit_count != 32) {
        GlobalUnlock(handle); return None;
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        GlobalUnlock(handle); return None;
    }

    let bpp = (bit_count / 8) as usize;
    let row_stride = ((width as usize * bpp + 3) / 4) * 4;
    let pixel_offset = if compression == 3 { 52usize } else { 40usize };
    let data_size = row_stride * height as usize;
    if size < (pixel_offset + data_size) {
        GlobalUnlock(handle); return None;
    }
    let pixels = std::slice::from_raw_parts(ptr.add(pixel_offset), data_size);
    let is_bottom_up = height_raw > 0;

    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        let src_y = if is_bottom_up { (height as usize - 1) - y } else { y };
        let src_row = &pixels[src_y * row_stride..];
        for x in 0..width as usize {
            let si = x * bpp;
            let di = (y * width as usize + x) * 4;
            rgba[di] = src_row[si + 2];
            rgba[di + 1] = src_row[si + 1];
            rgba[di + 2] = src_row[si];
            rgba[di + 3] = if bit_count == 32 { src_row[si + 3] } else { 255 };
        }
    }
    GlobalUnlock(handle);

    use image::{ImageBuffer, RgbaImage};
    let img: RgbaImage = ImageBuffer::from_raw(width, height, rgba)?;
    let dyn_img = image::DynamicImage::ImageRgba8(img);
    let mut buf = std::io::Cursor::new(Vec::new());
    if dyn_img.write_to(&mut buf, image::ImageFormat::Png).is_err() { return None; }
    Some((buf.into_inner(), width, height))
}

// ── Clipboard listener thread ────────────────────────────────────────────────

fn run_clipboard_listener() {
    unsafe {
        let class_name: Vec<u16> = "TRIGRClipboardListener\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(clipboard_wnd_proc),
            cbClsExtra: 0, cbWndExtra: 0,
            hInstance: std::ptr::null_mut(),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            error!("[Trigr] Failed to register clipboard window class");
            return;
        }

        let hwnd = CreateWindowExW(
            0, class_name.as_ptr(), std::ptr::null(), WS_OVERLAPPED,
            0, 0, 0, 0, HWND_MESSAGE,
            std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null(),
        );
        if hwnd.is_null() {
            error!("[Trigr] Failed to create clipboard message-only window");
            return;
        }
        if AddClipboardFormatListener(hwnd) == 0 {
            error!("[Trigr] Failed to add clipboard format listener");
            DestroyWindow(hwnd);
            return;
        }

        info!("[Trigr] Clipboard listener started (message-only HWND)");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
            DispatchMessageW(&msg);
        }

        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
        info!("[Trigr] Clipboard listener stopped");
    }
}

unsafe extern "system" fn clipboard_wnd_proc(
    hwnd: HWND, msg: u32, w_param: usize, l_param: isize,
) -> isize {
    if msg == WM_CLIPBOARDUPDATE {
        handle_clipboard_update();
        return 0;
    }
    DefWindowProcW(hwnd, msg, w_param, l_param)
}

fn handle_clipboard_update() {
    // Skip Trigr's own injected writes. Two layers: the level flag covers the
    // synchronous write window, and the per-write sequence-number record covers
    // the async tail (a WM_CLIPBOARDUPDATE delivered after the flag was cleared —
    // the H3 leak). A real user copy, or a `Copy to Clipboard` macro step (the
    // target app performs that copy), has a seqnum Trigr never recorded, so it is
    // always still captured. Checked first so the self-seqnum is consumed even
    // when a later gate (capture-off / excluded app) would return early.
    let cur_seq = crate::expansions::clipboard_sequence_number();
    let was_self = crate::actions::is_self_clipboard_seq(cur_seq);
    let was_suppress = crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.load(Ordering::SeqCst);

    // ── TEMP DIAGNOSTIC [CLIP-DIAG]: clipboard-flood investigation ───────────
    // Logs one line per WM_CLIPBOARDUPDATE so we can correlate seqnums + the
    // self-skip / suppress gates with the rows actually landing in the DB.
    // Remove this block (and the early get_foreground_process_name call below
    // that supports it) once the flood writer is identified.
    let fg_proc = get_foreground_process_name();
    log::info!(
        "[CLIP-DIAG] seq={} self={} suppress={} capture_on={} fg={}",
        cur_seq,
        was_self,
        was_suppress,
        CAPTURE_ENABLED.load(Ordering::SeqCst),
        if fg_proc.is_empty() { "<unknown>" } else { fg_proc.as_str() }
    );

    if was_self || was_suppress {
        return;
    }

    // Master capture toggle. When off, the listener keeps running so re-enabling
    // takes effect on the very next clipboard event without restarting Trigr.
    if !CAPTURE_ENABLED.load(Ordering::SeqCst) {
        return;
    }

    // fg_proc already resolved above for [CLIP-DIAG]; reuse it here. (When the
    // diagnostic is removed, restore the original call site at the line below.)

    // App exclusion list: skip capture when the user has opted out of recording
    // clipboard from this process. Comparison is case-insensitive and ignores
    // the `.exe` suffix on both sides.
    if !fg_proc.is_empty() && is_app_excluded(&fg_proc) {
        return;
    }

    // Capture source app for the row (Pro feature — Free users get empty source).
    let source_app = if crate::licence::is_pro() {
        fg_proc
    } else {
        String::new()
    };

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }

        if IsClipboardFormatAvailable(CF_HDROP) != 0 {
            CloseClipboard();
            return;
        }

        let has_dib = IsClipboardFormatAvailable(CF_DIB) != 0;
        let has_text = IsClipboardFormatAvailable(CF_UNICODETEXT) != 0;

        if has_dib {
            if let Some((png_bytes, width, height)) = read_clipboard_image() {
                CloseClipboard();

                let hash = compute_hash(&png_bytes);
                {
                    let mut last = last_hash().lock().unwrap();
                    if *last == hash { return; }
                    *last = hash;
                }

                send_entry(ClipEntry {
                    content_type: "image".to_string(),
                    text_content: None,
                    image_blob: Some(png_bytes),
                    image_width: width,
                    image_height: height,
                    preview: format!("{}×{} image", width, height),
                    source_app: source_app.clone(),
                    content_tag: "Image".to_string(),
                });
                return;
            }
        }

        if has_text {
            let handle = GetClipboardData(CF_UNICODETEXT);
            if !handle.is_null() {
                let ptr = GlobalLock(handle) as *const u16;
                if !ptr.is_null() {
                    let mut len = 0usize;
                    while *ptr.add(len) != 0 { len += 1; }
                    let slice = std::slice::from_raw_parts(ptr, len);
                    let text = String::from_utf16_lossy(slice);
                    GlobalUnlock(handle);
                    CloseClipboard();

                    if text.trim().is_empty() { return; }

                    let hash = compute_hash(text.as_bytes());
                    {
                        let mut last = last_hash().lock().unwrap();
                        if *last == hash { return; }
                        *last = hash;
                    }

                    let tag = auto_tag("text", Some(&text));
                    let preview = if text.len() > 200 {
                        let end = text.char_indices().nth(200).map(|(i, _)| i).unwrap_or(text.len());
                        format!("{}…", &text[..end])
                    } else {
                        text.clone()
                    };

                    send_entry(ClipEntry {
                        content_type: "text".to_string(),
                        text_content: Some(text),
                        image_blob: None,
                        image_width: 0,
                        image_height: 0,
                        preview,
                        source_app,
                        content_tag: tag,
                    });
                    return;
                }
            }
        }

        CloseClipboard();
    }
}

fn send_entry(entry: ClipEntry) {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::NewEntry(entry));
        }
    }
}
