use log::{error, info};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
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
    UpdateItem {
        id: i64,
        new_text: String,
        reply: mpsc::Sender<Option<String>>, // returns new content_tag on success
    },
    Prune,
}

pub struct FullClipItem {
    pub content_type: String,
    pub text_content: Option<String>,
    pub image_blob: Option<Vec<u8>>,
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

            info!("[Trigr] Clipboard DB ready: {}", db_path.display());

            for msg in rx {
                match msg {
                    ClipboardMsg::NewEntry(entry) => handle_new_entry(&conn, entry),
                    ClipboardMsg::GetHistory { page, per_page, reply } => {
                        let result = handle_get_history(&conn, page, per_page);
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
                    ClipboardMsg::UpdateItem { id, new_text, reply } => {
                        let result = handle_update_item(&conn, id, &new_text);
                        let _ = reply.send(result);
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

pub fn get_history(page: u32, per_page: u32) -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetHistory { page, per_page, reply: reply_tx }).is_ok() {
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

pub fn get_retention() -> u32 {
    retention_days()
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

fn handle_get_history(conn: &Connection, page: u32, per_page: u32) -> Value {
    let offset = page.saturating_sub(1) * per_page;
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM clipboard_history", [], |row| row.get(0))
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, content_type, text_content, image_width, image_height, preview, pinned, source_app, content_tag
             FROM clipboard_history ORDER BY pinned DESC, id DESC LIMIT ?1 OFFSET ?2",
        )
        .unwrap();

    let items: Vec<Value> = stmt
        .query_map(rusqlite::params![per_page, offset], |row| {
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
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    serde_json::json!({ "items": items, "total": total })
}

fn handle_get_item_full(conn: &Connection, id: i64) -> Option<FullClipItem> {
    conn.query_row(
        "SELECT content_type, text_content, image_blob FROM clipboard_history WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(FullClipItem {
                content_type: row.get::<_, String>(0).unwrap_or_default(),
                text_content: row.get::<_, Option<String>>(1).unwrap_or(None),
                image_blob: row.get::<_, Option<Vec<u8>>>(2).unwrap_or(None),
            })
        },
    )
    .ok()
}

fn handle_delete_item(conn: &Connection, id: i64) -> bool {
    conn.execute("DELETE FROM clipboard_history WHERE id = ?1", rusqlite::params![id]).is_ok()
}

fn handle_clear_all(conn: &Connection) -> bool {
    match conn.execute("DELETE FROM clipboard_history", []) {
        Ok(_) => { info!("[Trigr] Clipboard history cleared"); true }
        Err(e) => { error!("[Trigr] Failed to clear clipboard history: {}", e); false }
    }
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
    let mut stmt = conn
        .prepare("SELECT DISTINCT source_app FROM clipboard_history WHERE source_app != '' ORDER BY source_app ASC")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
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
    let days = retention_days();
    let query = format!(
        "DELETE FROM clipboard_history WHERE pinned = 0 AND timestamp < datetime('now', '-{} days')",
        days
    );
    let _ = conn.execute(&query, []);
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
    if crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.load(Ordering::SeqCst) {
        return;
    }

    // Capture source app BEFORE opening clipboard
    let source_app = get_foreground_process_name();

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
