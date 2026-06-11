use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use log::{debug, error, info, warn};
use rand::RngCore;
use rand::rngs::OsRng;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use tauri::AppHandle;
use zeroize::Zeroizing;

use windows_sys::Win32::Foundation::{HWND, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};
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

const KEY_FILE_NAME: &str = "trigr-clipboard.key.dpapi";

// ── Encryption: DPAPI-wrapped AES-256-GCM master key ─────────────────────────
//
// Master key (32 random bytes) is generated on first launch and stored at
// %APPDATA%\com.nodescaffold.trigr\trigr-clipboard.key.dpapi after being
// wrapped with Windows DPAPI in user-scope. Only the logged-in Windows user
// can unwrap it; other local users and offline disk attackers cannot.
//
// The unwrapped key is held in a Zeroizing<[u8; 32]> just long enough to build
// the Aes256Gcm instance, then dropped (the cipher keeps its own expanded
// round keys internally, which aes-gcm zeroes on drop).
//
// Phase 2: key generation, load, and cipher init only. Encryption of writes
// and decryption of reads land in Phase 3.

// RwLock (not OnceLock) because Reset Clipboard Storage replaces the key at
// runtime: it wipes the db + key file, generates a fresh key, and installs the
// new cipher without an app restart. None = cipher unavailable (DPAPI failure
// or pre-init); encrypt/decrypt fall back per their own contracts.
static CLIPBOARD_CIPHER: RwLock<Option<Aes256Gcm>> = RwLock::new(None);

fn cipher_ready() -> bool {
    CLIPBOARD_CIPHER.read().map(|g| g.is_some()).unwrap_or(false)
}

/// Wrap arbitrary bytes with DPAPI in user-scope. NEVER pass
/// CRYPTPROTECT_LOCAL_MACHINE — that would let other users on the same
/// machine unwrap the key.
unsafe fn dpapi_protect(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut out_blob: CRYPT_INTEGER_BLOB = std::mem::zeroed();

    let ok = CryptProtectData(
        &mut in_blob,
        ptr::null(),       // szDataDescr
        ptr::null_mut(),   // pOptionalEntropy (none — DPAPI's user-scope is the secret)
        ptr::null_mut(),   // pvReserved
        ptr::null_mut(),   // pPromptStruct (no UI)
        0,                 // dwFlags — user-scope, no UI
        &mut out_blob,
    );

    if ok == 0 {
        return Err("CryptProtectData failed".to_string());
    }

    let ciphertext =
        std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
    LocalFree(out_blob.pbData as _);
    Ok(ciphertext)
}

/// Unwrap DPAPI-wrapped bytes. Returns Err if the calling user isn't the one
/// who wrapped them (or if the blob is corrupted).
unsafe fn dpapi_unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut out_blob: CRYPT_INTEGER_BLOB = std::mem::zeroed();

    let ok = CryptUnprotectData(
        &mut in_blob,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        0,
        &mut out_blob,
    );

    if ok == 0 {
        return Err("CryptUnprotectData failed".to_string());
    }

    let plaintext =
        std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
    LocalFree(out_blob.pbData as _);
    Ok(plaintext)
}

/// Load the existing master key from disk, or generate a fresh one if none
/// exists. Returns the unwrapped 32-byte key inside a zeroizing wrapper.
fn load_or_generate_master_key(app_data_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let key_path = app_data_dir.join(KEY_FILE_NAME);
    if key_path.exists() {
        load_master_key(&key_path)
    } else {
        generate_and_save_master_key(&key_path)
    }
}

fn load_master_key(key_path: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let protected = std::fs::read(key_path)
        .map_err(|e| format!("read key file: {}", e))?;
    let unwrapped = unsafe { dpapi_unprotect(&protected) }?;
    if unwrapped.len() != 32 {
        return Err(format!(
            "unexpected unwrapped key length: {} (expected 32)",
            unwrapped.len()
        ));
    }
    let mut key: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&unwrapped);
    Ok(key)
}

fn generate_and_save_master_key(key_path: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut_slice());

    let protected = unsafe { dpapi_protect(key.as_slice()) }?;

    std::fs::write(key_path, &protected)
        .map_err(|e| format!("write key file: {}", e))?;

    // Mark the file read-only so accidental edits don't corrupt it. Best-effort;
    // failure here is logged but not fatal — the file is still safe at rest
    // because the master key inside is DPAPI-wrapped.
    match std::fs::metadata(key_path) {
        Ok(meta) => {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            if let Err(e) = std::fs::set_permissions(key_path, perms) {
                warn!("[Trigr] Clipboard: failed to set key file read-only: {}", e);
            }
        }
        Err(e) => warn!("[Trigr] Clipboard: failed to read key file metadata: {}", e),
    }

    info!("[Trigr] Clipboard: generated new master key, wrapped with DPAPI");
    Ok(key)
}

/// Set up the AES-256-GCM cipher from the on-disk DPAPI-wrapped master key.
/// Returns true on success, false on failure (logged). Phase 3 reads/writes
/// will check `clipboard_cipher().is_some()` before encrypt/decrypt.
fn init_cipher(app_data_dir: &Path) -> bool {
    match load_or_generate_master_key(app_data_dir) {
        Ok(key) => {
            match Aes256Gcm::new_from_slice(key.as_slice()) {
                Ok(aead) => {
                    match CLIPBOARD_CIPHER.write() {
                        Ok(mut guard) => *guard = Some(aead),
                        Err(_) => {
                            error!("[Trigr] Clipboard: cipher lock poisoned during init");
                            return false;
                        }
                    }
                    info!("[Trigr] Clipboard: AES-256-GCM cipher ready");
                    true
                }
                Err(e) => {
                    error!("[Trigr] Clipboard: failed to build cipher: {}", e);
                    false
                }
            }
        }
        Err(e) => {
            error!("[Trigr] Clipboard: failed to load/generate master key: {}", e);
            false
        }
    }
}

/// Encrypt arbitrary bytes with the clipboard's master key. Returns the
/// ciphertext (which includes the GCM auth tag) and a freshly generated
/// 12-byte IV. None if the cipher hasn't been initialised yet — callers
/// should fall back to writing plaintext (matching the legacy on-disk shape)
/// when this happens.
///
/// IMPORTANT: never reuse an IV with the same key. This function generates a
/// fresh random IV from OsRng on every call. Storing the IV in the row's
/// iv_* column is the caller's responsibility.
fn encrypt_blob(plaintext: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let guard = CLIPBOARD_CIPHER.read().ok()?;
    let cipher = guard.as_ref()?;
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let nonce = Nonce::from_slice(&iv);
    match cipher.encrypt(nonce, plaintext) {
        Ok(ct) => Some((ct, iv.to_vec())),
        Err(e) => {
            warn!("[Trigr] Clipboard: encrypt failed: {:?}", e);
            None
        }
    }
}

/// Decrypt ciphertext using the stored IV. None on auth-tag mismatch, wrong
/// key, corrupted blob, or if the cipher isn't initialised. Per-row decrypt
/// failures MUST be skipped silently in the panel (caller's responsibility);
/// never expose ciphertext to the UI.
fn decrypt_blob(ciphertext: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
    let guard = CLIPBOARD_CIPHER.read().ok()?;
    let cipher = guard.as_ref()?;
    if iv.len() != 12 {
        warn!(
            "[Trigr] Clipboard: decrypt called with invalid iv length {} (expected 12)",
            iv.len()
        );
        return None;
    }
    let nonce = Nonce::from_slice(iv);
    cipher.decrypt(nonce, ciphertext).ok()
}

/// Resolve a nullable text column to its plaintext value, handling both
/// encrypted (iv non-NULL) and legacy plaintext (iv NULL) cases.
///
/// - iv == Some AND ciphertext == Some → decrypt; None on failure (skip row).
/// - iv == None  AND ciphertext == Some → legacy plaintext; UTF-8 lossy decode.
/// - ciphertext == None                → no value at all.
fn resolve_optional_text(ciphertext: Option<Vec<u8>>, iv: Option<Vec<u8>>) -> Option<String> {
    let ct = ciphertext?;
    match iv {
        Some(iv_bytes) => decrypt_blob(&ct, &iv_bytes).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        None => Some(String::from_utf8_lossy(&ct).into_owned()),
    }
}

/// Resolve a nullable byte-blob column (image_blob) — same iv/legacy logic as
/// resolve_optional_text but returns raw bytes.
fn resolve_optional_bytes(ciphertext: Option<Vec<u8>>, iv: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let ct = ciphertext?;
    match iv {
        Some(iv_bytes) => decrypt_blob(&ct, &iv_bytes),
        None => Some(ct),
    }
}

/// Resolve a NOT NULL text column (preview) — always returns a String, falling
/// back to empty on decrypt failure so the panel still renders the row.
fn resolve_required_text(ciphertext: Vec<u8>, iv: Option<Vec<u8>>) -> String {
    match iv {
        Some(iv_bytes) => match decrypt_blob(&ciphertext, &iv_bytes) {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::new(),
        },
        None => String::from_utf8_lossy(&ciphertext).into_owned(),
    }
}

/// Read a nullable column's raw bytes regardless of SQLite storage class.
/// Legacy (pre-v0.5) rows stored text columns with TEXT storage; encrypted
/// rows store BLOB ciphertext. rusqlite's `Vec<u8>` FromSql accepts BLOB only,
/// so `row.get::<_, Vec<u8>>()` fails with InvalidColumnType on legacy rows.
/// Every content-column read (history, item-full, image, migration) MUST go
/// through this helper, not `row.get::<_, Vec<u8>>()`.
fn get_optional_bytes(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    match row.get_ref(idx)? {
        ValueRef::Null => Ok(None),
        ValueRef::Blob(b) => Ok(Some(b.to_vec())),
        ValueRef::Text(t) => Ok(Some(t.to_vec())),
        other => Err(rusqlite::Error::InvalidColumnType(
            idx,
            format!("column {}", idx),
            other.data_type(),
        )),
    }
}

// ── Phase 3b: one-time migration of legacy plaintext rows ────────────────────
//
// On the first v0.5 launch (or any launch where plaintext rows still exist) we
// encrypt every row where `iv_preview IS NULL` and store the IVs alongside.
// A copy of the pre-migration .db is saved to `trigr-clipboard.db.plaintext-backup`
// BEFORE any UPDATE runs, so the user has a known-good rollback for 7 days.
//
// Safety invariants:
//   1. Backup file copy is atomic (write to .tmp, rename to final). A crash
//      mid-copy leaves only the .tmp — the migration retries on next launch.
//   2. Encryption happens inside a single SQLite transaction. A crash mid-
//      transaction triggers automatic rollback; legacy rows stay plaintext.
//   3. Before COMMIT, we decrypt-sample 3 rows we just encrypted and compare
//      to the captured plaintext. A mismatch aborts COMMIT and the migration
//      retries on next launch.
//   4. Cipher unavailable (init failed) → migration skipped, plaintext rows
//      stay plaintext, app keeps working with iv-NULL fallback.

const PLAINTEXT_BACKUP_NAME: &str = "trigr-clipboard.db.plaintext-backup";
const PLAINTEXT_BACKUP_TMP_NAME: &str = "trigr-clipboard.db.plaintext-backup.tmp";
const PLAINTEXT_BACKUP_EXPIRES_NAME: &str = "trigr-clipboard.plaintext-backup-expires";
const PLAINTEXT_BACKUP_RETENTION_DAYS: i64 = 7;

/// Delete the plaintext-backup file and its expiry stamp if the 7-day retention
/// window has passed. Runs at writer thread startup BEFORE the migration check.
fn cleanup_expired_plaintext_backup(db_path: &Path) {
    let backup_path = db_path.with_file_name(PLAINTEXT_BACKUP_NAME);
    let expiry_path = db_path.with_file_name(PLAINTEXT_BACKUP_EXPIRES_NAME);

    if !expiry_path.exists() {
        return;
    }

    let expiry_str = match std::fs::read_to_string(&expiry_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Clipboard: expiry file read failed: {}", e);
            return;
        }
    };
    let expiry = match chrono::DateTime::parse_from_rfc3339(expiry_str.trim()) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(e) => {
            warn!("[Trigr] Clipboard: expiry file unparseable ({}); leaving in place", e);
            return;
        }
    };

    if chrono::Utc::now() < expiry {
        return;
    }

    if backup_path.exists() {
        match std::fs::remove_file(&backup_path) {
            Ok(()) => info!("[Trigr] Clipboard: expired plaintext backup deleted"),
            Err(e) => {
                warn!("[Trigr] Clipboard: failed to delete expired backup: {}", e);
                return;
            }
        }
    }
    let _ = std::fs::remove_file(&expiry_path);
}

/// Run the one-time migration. Returns the number of rows migrated (0 = nothing
/// to do or migration skipped because cipher unavailable). Returns Err only on
/// errors that should be visible in the log; the caller logs and continues —
/// the app keeps running with legacy rows still plaintext-readable.
fn run_phase3b_migration(conn: &Connection, db_path: &Path) -> Result<usize, String> {
    if !cipher_ready() {
        info!("[Trigr] Clipboard: Phase 3b migration skipped (cipher unavailable)");
        return Ok(0);
    }

    let needs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE iv_preview IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if needs == 0 {
        info!("[Trigr] Clipboard: Phase 3b migration not needed (no plaintext rows)");
        return Ok(0);
    }

    info!(
        "[Trigr] Clipboard: Phase 3b migration starting ({} plaintext row(s))",
        needs
    );

    // STEP 1: copy plaintext .db to .plaintext-backup BEFORE any UPDATE.
    // Force a WAL checkpoint first so the .db file is a complete snapshot
    // (otherwise the WAL sidecar would hold pages the backup misses).
    let backup_path = db_path.with_file_name(PLAINTEXT_BACKUP_NAME);
    let backup_tmp = db_path.with_file_name(PLAINTEXT_BACKUP_TMP_NAME);

    if !backup_path.exists() {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        // Remove a stale .tmp from a prior interrupted run if present.
        let _ = std::fs::remove_file(&backup_tmp);
        std::fs::copy(db_path, &backup_tmp)
            .map_err(|e| format!("plaintext backup copy: {}", e))?;
        std::fs::rename(&backup_tmp, &backup_path)
            .map_err(|e| format!("plaintext backup rename: {}", e))?;
        info!(
            "[Trigr] Clipboard: plaintext backup saved to {}",
            backup_path.display()
        );
    } else {
        info!("[Trigr] Clipboard: reusing existing plaintext backup from prior attempt");
    }

    // STEP 2: snapshot up to 3 row previews for post-migration sample verification.
    let mut sample: Vec<(i64, Vec<u8>)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, preview FROM clipboard_history WHERE iv_preview IS NULL LIMIT 3",
            )
            .map_err(|e| format!("sample prepare: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, get_optional_bytes(row, 1)?.unwrap_or_default()))
            })
            .map_err(|e| format!("sample query: {}", e))?;
        for r in rows {
            sample.push(r.map_err(|e| format!("sample row: {}", e))?);
        }
    }

    // STEP 3: collect plaintext rows, then encrypt + UPDATE inside a transaction.
    let plain_rows: Vec<(i64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>, Vec<u8>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, text_content, image_blob, ocr_text, preview
                 FROM clipboard_history WHERE iv_preview IS NULL",
            )
            .map_err(|e| format!("collect prepare: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    get_optional_bytes(row, 1)?,
                    get_optional_bytes(row, 2)?,
                    get_optional_bytes(row, 3)?,
                    get_optional_bytes(row, 4)?.unwrap_or_default(),
                ))
            })
            .map_err(|e| format!("collect query: {}", e))?;
        let mut collected = Vec::new();
        for r in rows {
            collected.push(r.map_err(|e| format!("collect row: {}", e))?);
        }
        collected
    };

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("begin transaction: {}", e))?;

    let mut migrated: usize = 0;
    for (id, text_pt, image_pt, ocr_pt, preview_pt) in plain_rows {
        let (text_ct, iv_text): (Option<Vec<u8>>, Option<Vec<u8>>) = match text_pt {
            Some(pt) => match encrypt_blob(&pt) {
                Some((ct, iv)) => (Some(ct), Some(iv)),
                None => return Err(format!("encrypt text_content for row {}", id)),
            },
            None => (None, None),
        };
        let (image_ct, iv_image): (Option<Vec<u8>>, Option<Vec<u8>>) = match image_pt {
            Some(pt) => match encrypt_blob(&pt) {
                Some((ct, iv)) => (Some(ct), Some(iv)),
                None => return Err(format!("encrypt image_blob for row {}", id)),
            },
            None => (None, None),
        };
        let (ocr_ct, iv_ocr): (Option<Vec<u8>>, Option<Vec<u8>>) = match ocr_pt {
            Some(pt) => match encrypt_blob(&pt) {
                Some((ct, iv)) => (Some(ct), Some(iv)),
                None => return Err(format!("encrypt ocr_text for row {}", id)),
            },
            None => (None, None),
        };
        let (preview_ct, iv_preview): (Vec<u8>, Vec<u8>) = match encrypt_blob(&preview_pt) {
            Some((ct, iv)) => (ct, iv),
            None => return Err(format!("encrypt preview for row {}", id)),
        };

        tx.execute(
            "UPDATE clipboard_history
             SET text_content=?1, image_blob=?2, ocr_text=?3, preview=?4,
                 iv_text=?5, iv_image=?6, iv_ocr=?7, iv_preview=?8
             WHERE id=?9",
            rusqlite::params![
                text_ct, image_ct, ocr_ct, preview_ct, iv_text, iv_image, iv_ocr, iv_preview, id
            ],
        )
        .map_err(|e| format!("update row {}: {}", id, e))?;
        migrated += 1;
    }

    // Hard invariant: every counted plaintext row must have been encrypted.
    // A mismatch means the collect query dropped rows — abort so the
    // transaction rolls back and the migration retries next launch, rather
    // than committing a partial (or empty) migration as success.
    if migrated as i64 != needs {
        return Err(format!(
            "row-count mismatch: counted {} plaintext row(s) but encrypted {} — rolling back",
            needs, migrated
        ));
    }

    // STEP 4: sample-verify before COMMIT. Re-fetch each sampled row and decrypt
    // the new ciphertext using the new iv; compare to the plaintext we captured
    // BEFORE encryption. Any mismatch aborts COMMIT and the next launch retries
    // (the backup file is left intact for recovery).
    for (id, expected_preview) in &sample {
        let (ct, iv): (Vec<u8>, Vec<u8>) = tx
            .query_row(
                "SELECT preview, iv_preview FROM clipboard_history WHERE id=?1",
                rusqlite::params![id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(|e| format!("sample verify select row {}: {}", id, e))?;
        let decrypted = decrypt_blob(&ct, &iv)
            .ok_or_else(|| format!("sample verify decrypt failed for row {}", id))?;
        if &decrypted != expected_preview {
            return Err(format!(
                "sample verify MISMATCH on row {} — rolling back migration",
                id
            ));
        }
    }

    tx.commit()
        .map_err(|e| format!("commit migration: {}", e))?;

    info!(
        "[Trigr] Clipboard: Phase 3b migration committed ({} row(s) encrypted, {} sample(s) verified)",
        migrated,
        sample.len()
    );

    // STEP 5: write the 7-day expiry stamp.
    let expiry_path = db_path.with_file_name(PLAINTEXT_BACKUP_EXPIRES_NAME);
    let expiry = chrono::Utc::now() + chrono::Duration::days(PLAINTEXT_BACKUP_RETENTION_DAYS);
    if let Err(e) = std::fs::write(&expiry_path, expiry.to_rfc3339()) {
        warn!("[Trigr] Clipboard: failed to write expiry file: {}", e);
        // Non-fatal: the backup will just live forever until manually deleted.
    } else {
        info!(
            "[Trigr] Clipboard: plaintext backup expires {}",
            expiry.to_rfc3339()
        );
    }

    Ok(migrated)
}

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
    /// Phase 4: wipe db + key + backup files and start fresh. The handler
    /// swaps the writer loop's connection for a new one.
    ResetStorage {
        reply: mpsc::Sender<bool>,
    },
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

// ── DB open + schema ─────────────────────────────────────────────────────────

/// Open the clipboard DB and apply the full schema (CREATE + additive ALTERs).
/// Called at writer-thread startup and again by Reset Clipboard Storage after
/// it deletes the files. Keep ALL schema statements here so a reset-created db
/// is identical to a startup-created one.
fn open_clipboard_db(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("open: {}", e))?;

    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

    conn.execute_batch(
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
    )
    .map_err(|e| format!("create table: {}", e))?;

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

    // Phase 3a (v0.5): per-column 12-byte AES-GCM IVs. NULL on legacy rows
    // (those columns are still plaintext until the Phase 3b one-time
    // migration encrypts them). NON-NULL means the corresponding content
    // column holds ciphertext + GCM auth tag and must be decrypted with
    // resolve_optional_text / resolve_optional_bytes / resolve_required_text.
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_text BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_image BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_ocr BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_preview BLOB", []);

    Ok(conn)
}

/// Phase 4: Reset Clipboard Storage. Wipes the db (+WAL/SHM sidecars), the
/// DPAPI key file, and any plaintext backup, generates a fresh master key,
/// and reopens an empty db. Per [[feedback_data_hygiene]] files are deleted
/// outright so disk space is actually reclaimed. Returns the connection to
/// continue the writer loop with (None only if no db could be reopened at
/// all) and whether the reset fully succeeded.
fn handle_reset_storage(conn: Connection, db_path: &Path) -> (Option<Connection>, bool) {
    info!("[Trigr] Clipboard: storage reset requested");
    let mut ok = true;

    // 1. Checkpoint + close so Windows releases the file locks.
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    if let Err((c, e)) = conn.close() {
        warn!("[Trigr] Clipboard: close before reset failed: {}", e);
        drop(c);
    }

    // 2. Delete db + sidecars + key file + plaintext backup + expiry stamp.
    let dir = match db_path.parent() {
        Some(d) => d.to_path_buf(),
        None => PathBuf::new(),
    };
    let targets: Vec<PathBuf> = vec![
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
        dir.join(KEY_FILE_NAME),
        db_path.with_file_name(PLAINTEXT_BACKUP_NAME),
        db_path.with_file_name(PLAINTEXT_BACKUP_TMP_NAME),
        db_path.with_file_name(PLAINTEXT_BACKUP_EXPIRES_NAME),
    ];
    for path in targets {
        if !path.exists() {
            continue;
        }
        // The key file is written READONLY — clear the attribute first or
        // remove_file fails on Windows.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.permissions().readonly() {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            error!("[Trigr] Clipboard: reset failed to delete {}: {}", path.display(), e);
            ok = false;
        }
    }

    // 3. Fresh master key + cipher. Failure is non-fatal: the empty db keeps
    // working via the plaintext fallback and the Settings status line shows
    // encryption as unavailable.
    let key_path = dir.join(KEY_FILE_NAME);
    let new_cipher = match generate_and_save_master_key(&key_path)
        .and_then(|key| Aes256Gcm::new_from_slice(key.as_slice()).map_err(|e| e.to_string()))
    {
        Ok(c) => Some(c),
        Err(e) => {
            error!("[Trigr] Clipboard: reset failed to generate new key: {}", e);
            ok = false;
            None
        }
    };
    if let Ok(mut guard) = CLIPBOARD_CIPHER.write() {
        *guard = new_cipher;
    }

    // 4. Reopen a fresh, empty db.
    match open_clipboard_db(db_path) {
        Ok(c) => {
            if ok {
                info!("[Trigr] Clipboard: storage reset complete (fresh db + key)");
            }
            (Some(c), ok)
        }
        Err(e) => {
            error!("[Trigr] Clipboard: reset could not reopen db: {}", e);
            (None, false)
        }
    }
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

    // Phase 2: build the AES-256-GCM cipher from the DPAPI-wrapped master key
    // BEFORE spawning the writer thread. If this fails, the writer still spawns
    // and the clipboard still works (read/write paths fall back to plaintext
    // until Phase 3 wires encryption into the SQL paths); a follow-up error
    // toast surfaces from the Phase 5 startup path.
    let _ = init_cipher(&app_data_dir);

    let db_path = app_data_dir.join("trigr-clipboard.db");
    let _ = DB_PATH.set(db_path.clone());
    let (tx, rx) = mpsc::channel::<ClipboardMsg>();
    let _ = CLIPBOARD_TX.set(Mutex::new(tx));

    thread::Builder::new()
        .name("trigr-clipboard-writer".to_string())
        .spawn(move || {
            // `mut` because Reset Clipboard Storage closes this connection,
            // deletes the db files, and swaps in a fresh one mid-loop.
            let mut conn = match open_clipboard_db(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("[Trigr] Failed to open clipboard DB: {}", e);
                    return;
                }
            };

            // Phase 3b (v0.5): clean up an expired plaintext backup from a prior
            // upgrade, then run the one-time migration of any remaining legacy
            // plaintext rows. Both are no-ops on most launches. Errors are logged
            // but non-fatal — the app keeps running with iv-NULL fallback if
            // migration can't proceed.
            cleanup_expired_plaintext_backup(&db_path);
            match run_phase3b_migration(&conn, &db_path) {
                Ok(0) => {}
                Ok(n) => info!("[Trigr] Clipboard: Phase 3b migrated {} row(s)", n),
                Err(e) => error!("[Trigr] Clipboard: Phase 3b migration failed: {}", e),
            }

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
                        // Phase 3a: encrypt ocr_text with a fresh IV before storing.
                        let (ocr_ct, iv_ocr): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(text.as_bytes()) {
                            Some((ct, iv)) => (ct, Some(iv)),
                            None => (text.as_bytes().to_vec(), None),
                        };
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET ocr_text = ?1, iv_ocr = ?2 WHERE id = ?3",
                            rusqlite::params![ocr_ct, iv_ocr, id],
                        );
                    }
                    ClipboardMsg::IncrementPasteCount { id } => {
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET paste_count = paste_count + 1 WHERE id = ?1",
                            rusqlite::params![id],
                        );
                    }
                    ClipboardMsg::Prune => handle_prune(&conn),
                    ClipboardMsg::ResetStorage { reply } => {
                        let (new_conn, ok) = handle_reset_storage(conn, &db_path);
                        match new_conn {
                            Some(c) => conn = c,
                            None => {
                                // No usable db — same terminal state as a failed
                                // open at startup. Reply, log, end the thread.
                                error!("[Trigr] Clipboard: writer thread exiting after failed storage reset");
                                let _ = reply.send(false);
                                return;
                            }
                        }
                        let _ = reply.send(ok);
                    }
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

/// Phase 4: Reset Clipboard Storage (Settings → Privacy & Security). Longer
/// timeout than the other ops — the writer may be mid-migration or mid-write
/// when the message lands, and the reset itself does file I/O.
pub fn reset_storage() -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::ResetStorage { reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(15)) {
                    return ok;
                }
            }
        }
    }
    false
}

/// Phase 4: encryption status for the Settings status line. Pure file/lock
/// inspection — safe to call from any thread, no writer round-trip.
pub fn encryption_status() -> Value {
    let encrypted = cipher_ready();
    let (backup_exists, backup_expires) = match data_dir() {
        Some(dir) => {
            let exists = dir.join(PLAINTEXT_BACKUP_NAME).exists();
            let expires = if exists {
                std::fs::read_to_string(dir.join(PLAINTEXT_BACKUP_EXPIRES_NAME))
                    .ok()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
                    .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
            } else {
                None
            };
            (exists, expires)
        }
        None => (false, None),
    };
    serde_json::json!({
        "encrypted": encrypted,
        "backup_exists": backup_exists,
        "backup_expires": backup_expires,
    })
}

/// Phase 4: "Delete now" for the plaintext migration backup. Also removes the
/// expiry stamp so the startup cleanup has nothing left to track.
pub fn delete_plaintext_backup_now() -> bool {
    let dir = match data_dir() {
        Some(d) => d,
        None => return false,
    };
    let backup = dir.join(PLAINTEXT_BACKUP_NAME);
    if backup.exists() {
        if let Err(e) = std::fs::remove_file(&backup) {
            error!("[Trigr] Clipboard: failed to delete plaintext backup: {}", e);
            return false;
        }
        info!("[Trigr] Clipboard: plaintext backup deleted via Settings");
    }
    let _ = std::fs::remove_file(dir.join(PLAINTEXT_BACKUP_EXPIRES_NAME));
    true
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

    // Phase 3a: encrypt sensitive content columns before INSERT. If the cipher
    // isn't initialised (encrypt_blob returns None), each column falls back to
    // plaintext with iv_* = NULL — matching the legacy on-disk shape so the
    // app degrades gracefully rather than failing to write. The clipboard-new-item
    // event payload below stays plaintext because the panel needs to display it.
    let (text_ct, iv_text): (Option<Vec<u8>>, Option<Vec<u8>>) = match entry.text_content.as_deref() {
        Some(plain) => match encrypt_blob(plain.as_bytes()) {
            Some((ct, iv)) => (Some(ct), Some(iv)),
            None => (Some(plain.as_bytes().to_vec()), None),
        },
        None => (None, None),
    };
    let (image_ct, iv_image): (Option<Vec<u8>>, Option<Vec<u8>>) = match entry.image_blob.as_deref() {
        Some(plain) => match encrypt_blob(plain) {
            Some((ct, iv)) => (Some(ct), Some(iv)),
            None => (Some(plain.to_vec()), None),
        },
        None => (None, None),
    };
    let (preview_ct, iv_preview): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(entry.preview.as_bytes()) {
        Some((ct, iv)) => (ct, Some(iv)),
        None => (entry.preview.as_bytes().to_vec(), None),
    };

    let result = conn.execute(
        "INSERT INTO clipboard_history (timestamp, content_type, text_content, image_blob, image_width, image_height, preview, pinned, source_app, content_tag, iv_text, iv_image, iv_preview)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            now,
            entry.content_type,
            text_ct,
            image_ct,
            entry.image_width,
            entry.image_height,
            preview_ct,
            entry.source_app,
            entry.content_tag,
            iv_text,
            iv_image,
            iv_preview,
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

    // Toolbar filters (app, tag) layer on top of the date clause via
    // AND-joined predicates with unnumbered `?` placeholders — SQLite binds
    // them positionally from rusqlite::params_from_iter so we don't have to
    // track numbering across two queries. User input goes through binds, not
    // SQL string interpolation.
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
    let where_clause = clauses.join(" AND ");

    // Phase 3c: search can't be a WHERE clause — previews are ciphertext, so
    // SQL LIKE has nothing to match against. A non-empty query routes to the
    // decrypt-and-scan path instead, which applies the same date/app/tag
    // window and substring-matches decrypted previews in memory.
    if let Some(needle) = search.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return search_history(conn, &where_clause, &where_binds, &needle.to_lowercase(), per_page, offset);
    }

    // COUNT — same WHERE, just the toolbar binds.
    let count_sql = format!("SELECT COUNT(*) FROM clipboard_history WHERE {}", where_clause);
    let count_refs: Vec<&dyn rusqlite::ToSql> = where_binds.iter().map(|p| p.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, rusqlite::params_from_iter(count_refs.iter()), |row| row.get(0))
        .unwrap_or(0);

    // LIST — same WHERE, then LIMIT/OFFSET appended after the toolbar binds.
    // text_content/preview/ocr_text are bound as BLOB and resolved per-row by
    // the helpers below: NON-NULL iv_* → decrypt; NULL iv_* → legacy plaintext.
    let list_sql = format!(
        "SELECT {} FROM clipboard_history WHERE {} ORDER BY pinned DESC, id DESC LIMIT ? OFFSET ?",
        HISTORY_LIST_COLUMNS, where_clause
    );
    let mut list_binds: Vec<Box<dyn rusqlite::ToSql>> = where_binds;
    list_binds.push(Box::new(per_page as i64));
    list_binds.push(Box::new(offset as i64));
    let list_refs: Vec<&dyn rusqlite::ToSql> = list_binds.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&list_sql).unwrap();

    let items: Vec<Value> = stmt
        .query_map(rusqlite::params_from_iter(list_refs.iter()), |row| history_row_to_json(row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    serde_json::json!({ "items": items, "total": total })
}

/// Column list for both history-list SELECTs (normal + search page fetch).
/// history_row_to_json reads by position — keep order in sync.
const HISTORY_LIST_COLUMNS: &str = "id, timestamp, content_type, text_content, image_width, image_height, preview, pinned, source_app, content_tag, paste_count, ocr_text, iv_text, iv_preview, iv_ocr";

/// Shared row → JSON mapping for the history list (normal + search paths).
/// Reads HISTORY_LIST_COLUMNS by position.
fn history_row_to_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let text_ct = get_optional_bytes(row, 3).unwrap_or(None);
    let iv_text = row.get::<_, Option<Vec<u8>>>(12).unwrap_or(None);
    let preview_ct = get_optional_bytes(row, 6).ok().flatten().unwrap_or_default();
    let iv_preview = row.get::<_, Option<Vec<u8>>>(13).unwrap_or(None);
    let ocr_ct = get_optional_bytes(row, 11).unwrap_or(None);
    let iv_ocr = row.get::<_, Option<Vec<u8>>>(14).unwrap_or(None);
    Ok(serde_json::json!({
        "id": row.get::<_, i64>(0).unwrap_or(0),
        "timestamp": row.get::<_, String>(1).unwrap_or_default(),
        "content_type": row.get::<_, String>(2).unwrap_or_default(),
        "text_content": resolve_optional_text(text_ct, iv_text),
        "image_width": row.get::<_, u32>(4).unwrap_or(0),
        "image_height": row.get::<_, u32>(5).unwrap_or(0),
        "preview": resolve_required_text(preview_ct, iv_preview),
        "pinned": row.get::<_, i32>(7).unwrap_or(0) != 0,
        "source_app": row.get::<_, String>(8).unwrap_or_default(),
        "content_tag": row.get::<_, String>(9).unwrap_or("Text".to_string()),
        "paste_count": row.get::<_, i64>(10).unwrap_or(0),
        "ocr_text": resolve_optional_text(ocr_ct, iv_ocr),
    }))
}

/// Phase 3c: decrypt-and-scan search. SQL LIKE can't see into ciphertext, so
/// we scan the filtered window in two passes:
///   1. Fetch only (id, preview, iv_preview), decrypt each preview in memory
///      (previews are small truncated strings), lowercase substring match.
///   2. Fetch full rows for just the requested page of matched ids.
/// Two passes keep memory flat — large text_content/ocr blobs are only
/// decrypted for the page actually returned, never for the whole scan.
/// `needle` must already be lowercased by the caller.
fn search_history(
    conn: &Connection,
    where_clause: &str,
    where_binds: &[Box<dyn rusqlite::ToSql>],
    needle: &str,
    per_page: u32,
    offset: u32,
) -> Value {
    let started = std::time::Instant::now();

    let scan_sql = format!(
        "SELECT id, preview, iv_preview FROM clipboard_history WHERE {} ORDER BY pinned DESC, id DESC",
        where_clause
    );
    let bind_refs: Vec<&dyn rusqlite::ToSql> = where_binds.iter().map(|p| p.as_ref()).collect();
    let mut stmt = match conn.prepare(&scan_sql) {
        Ok(s) => s,
        Err(e) => {
            error!("[Trigr] Clipboard: search scan prepare failed: {}", e);
            return serde_json::json!({ "items": [], "total": 0 });
        }
    };
    let mut scanned: usize = 0;
    let matched_ids: Vec<i64> = stmt
        .query_map(rusqlite::params_from_iter(bind_refs.iter()), |row| {
            let id = row.get::<_, i64>(0)?;
            let preview_ct = get_optional_bytes(row, 1)?.unwrap_or_default();
            let iv_preview = row.get::<_, Option<Vec<u8>>>(2)?;
            Ok((id, resolve_required_text(preview_ct, iv_preview)))
        })
        .map(|iter| {
            iter.filter_map(|r| r.ok())
                .inspect(|_| scanned += 1)
                .filter(|(_, preview)| preview.to_lowercase().contains(needle))
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default();

    let total = matched_ids.len() as i64;
    let page_ids: Vec<i64> = matched_ids
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .collect();

    let items: Vec<Value> = if page_ids.is_empty() {
        Vec::new()
    } else {
        // page_ids are i64s we produced ourselves — safe to inline. The same
        // ORDER BY re-applied to the id subset preserves the scan's order.
        let id_list = page_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let list_sql = format!(
            "SELECT {} FROM clipboard_history WHERE id IN ({}) ORDER BY pinned DESC, id DESC",
            HISTORY_LIST_COLUMNS, id_list
        );
        match conn.prepare(&list_sql) {
            Ok(mut stmt) => stmt
                .query_map([], |row| history_row_to_json(row))
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            Err(e) => {
                error!("[Trigr] Clipboard: search page fetch prepare failed: {}", e);
                Vec::new()
            }
        }
    };

    debug!(
        "[Trigr] Clipboard: search scanned {} row(s), {} match(es) in {}ms",
        scanned,
        total,
        started.elapsed().as_millis()
    );

    serde_json::json!({ "items": items, "total": total })
}

fn handle_get_item_full(conn: &Connection, id: i64) -> Option<FullClipItem> {
    conn.query_row(
        "SELECT content_type, text_content, image_blob, ocr_text, iv_text, iv_image, iv_ocr FROM clipboard_history WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            let text_ct = get_optional_bytes(row, 1).unwrap_or(None);
            let iv_text = row.get::<_, Option<Vec<u8>>>(4).unwrap_or(None);
            let image_ct = get_optional_bytes(row, 2).unwrap_or(None);
            let iv_image = row.get::<_, Option<Vec<u8>>>(5).unwrap_or(None);
            let ocr_ct = get_optional_bytes(row, 3).unwrap_or(None);
            let iv_ocr = row.get::<_, Option<Vec<u8>>>(6).unwrap_or(None);
            Ok(FullClipItem {
                content_type: row.get::<_, String>(0).unwrap_or_default(),
                text_content: resolve_optional_text(text_ct, iv_text),
                image_blob: resolve_optional_bytes(image_ct, iv_image),
                ocr_text: resolve_optional_text(ocr_ct, iv_ocr),
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
        "SELECT image_blob, iv_image FROM clipboard_history WHERE id = ?1 AND content_type = 'image'",
        rusqlite::params![id],
        |row| {
            let image_ct = get_optional_bytes(row, 0)?;
            let iv_image = row.get::<_, Option<Vec<u8>>>(1)?;
            Ok(resolve_optional_bytes(image_ct, iv_image))
        },
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
    // Phase 3a: re-encrypt both columns with fresh IVs on every UPDATE. NEVER
    // reuse an IV with the same key (catastrophic with AES-GCM), so each
    // edit generates new IVs. Cipher-unavailable fallback writes plaintext +
    // NULL ivs, matching the legacy on-disk shape.
    let (text_ct, iv_text): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(new_text.as_bytes()) {
        Some((ct, iv)) => (ct, Some(iv)),
        None => (new_text.as_bytes().to_vec(), None),
    };
    let (preview_ct, iv_preview): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(preview.as_bytes()) {
        Some((ct, iv)) => (ct, Some(iv)),
        None => (preview.as_bytes().to_vec(), None),
    };
    match conn.execute(
        "UPDATE clipboard_history SET text_content = ?1, preview = ?2, content_tag = ?3, iv_text = ?4, iv_preview = ?5 WHERE id = ?6 AND content_type = 'text'",
        rusqlite::params![text_ct, preview_ct, new_tag, iv_text, iv_preview, id],
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
        // Writers like Word and Snipping Tool re-open the clipboard immediately
        // after copying (delayed-render formats, Office clipboard, auto-save).
        // A single OpenClipboard attempt silently loses that race and the copy
        // never reaches history — retry briefly before giving up. Runs on the
        // dedicated listener thread, so the sleeps block nothing else.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            log::warn!("[Trigr] Clipboard: OpenClipboard still locked after 10 attempts — copy not captured");
            return;
        }

        if IsClipboardFormatAvailable(CF_HDROP) != 0 {
            log::info!("[CLIP-DIAG] skip: CF_HDROP present (file copy)");
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
                    if *last == hash {
                        log::info!("[CLIP-DIAG] skip: duplicate image content");
                        return;
                    }
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
            log::warn!("[Trigr] Clipboard: image read failed (DIB advertised) — falling through to text");
        }

        if has_text {
            let mut handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                // Delayed-render sources (Word, Office clipboard) can need a
                // beat to synthesize the text on demand — one short retry.
                std::thread::sleep(std::time::Duration::from_millis(30));
                handle = GetClipboardData(CF_UNICODETEXT);
            }
            if handle.is_null() {
                log::warn!("[Trigr] Clipboard: CF_UNICODETEXT advertised but GetClipboardData returned null — copy not captured");
            } else {
                let ptr = GlobalLock(handle) as *const u16;
                if ptr.is_null() {
                    log::warn!("[Trigr] Clipboard: GlobalLock failed on text handle — copy not captured");
                } else {
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
                        if *last == hash {
                            log::info!("[CLIP-DIAG] skip: duplicate text content");
                            return;
                        }
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
