use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64_STD;
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
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

// ── Phase 5: encryption error surfacing ──────────────────────────────────────
//
// Two failure modes reach the user as a one-time toast pointing at
// Settings → Reset clipboard storage. NEVER auto-wipe on either:
//   1. Key file exists but DPAPI can't unwrap it at startup (KEY_UNREADABLE,
//      picked up by App.jsx via get_clipboard_encryption_status on mount —
//      an emit here would race the frontend listener registration).
//   2. 5+ row decrypt failures in one session (key/data mismatch, e.g. the
//      key file was deleted and silently regenerated) — emitted as a
//      "clipboard-encryption-error" event the moment the threshold is hit;
//      the frontend is necessarily mounted by then since decrypts only run
//      on panel/overlay fetches.

static KEY_UNREADABLE: AtomicBool = AtomicBool::new(false);
static DECRYPT_FAILURES: AtomicU32 = AtomicU32::new(0);
static DECRYPT_TOAST_SENT: AtomicBool = AtomicBool::new(false);
const DECRYPT_FAILURE_TOAST_THRESHOLD: u32 = 5;

fn note_decrypt_failure() {
    let n = DECRYPT_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
    if n == DECRYPT_FAILURE_TOAST_THRESHOLD && !DECRYPT_TOAST_SENT.swap(true, Ordering::SeqCst) {
        warn!(
            "[Keyfire] Clipboard: {} row decrypt failures this session — key/data mismatch likely",
            n
        );
        if let Some(app) = APP_HANDLE.get() {
            use tauri::Emitter;
            let _ = app.emit(
                "clipboard-encryption-error",
                serde_json::json!({ "reason": "decrypt_failures" }),
            );
        }
    }
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
                warn!("[Keyfire] Clipboard: failed to set key file read-only: {}", e);
            }
        }
        Err(e) => warn!("[Keyfire] Clipboard: failed to read key file metadata: {}", e),
    }

    info!("[Keyfire] Clipboard: generated new master key, wrapped with DPAPI");
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
                            error!("[Keyfire] Clipboard: cipher lock poisoned during init");
                            return false;
                        }
                    }
                    info!("[Keyfire] Clipboard: AES-256-GCM cipher ready");
                    true
                }
                Err(e) => {
                    error!("[Keyfire] Clipboard: failed to build cipher: {}", e);
                    false
                }
            }
        }
        Err(e) => {
            error!("[Keyfire] Clipboard: failed to load/generate master key: {}", e);
            // Distinguish "key file exists but DPAPI can't unwrap it" (corrupt
            // file, copied from another user/machine) — the recovery path is
            // Settings → Reset clipboard storage. NEVER auto-wipe here.
            if app_data_dir.join(KEY_FILE_NAME).exists() {
                error!("[Keyfire] Clipboard: encryption key unreadable");
                KEY_UNREADABLE.store(true, Ordering::SeqCst);
            }
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
            warn!("[Keyfire] Clipboard: encrypt failed: {:?}", e);
            None
        }
    }
}

/// Encode a raw image blob (any format the `image` crate can decode — PNG or
/// JPEG in practice for clipboard captures) into a small WebP thumbnail
/// suitable for inline delivery in the history list payload. Preserves aspect
/// ratio, fits inside a 200x200 box, WebP-compressed to keep base64 payload
/// small. Returns None on decode/encode failure or if the input is empty —
/// callers store None and fall back to full-res lazy load.
///
/// The output is INTENTIONALLY not encrypted here — the caller wraps it with
/// encrypt_blob so thumb_blob lives on-disk with the same protection as
/// image_blob. Keeping the helper cipher-free lets the backfill worker reuse
/// it without touching the cipher path more than once per row.
const THUMB_MAX_DIMENSION: u32 = 200;

fn make_thumb_webp(source_bytes: &[u8]) -> Option<Vec<u8>> {
    if source_bytes.is_empty() { return None; }
    let img = match image::load_from_memory(source_bytes) {
        Ok(i) => i,
        Err(e) => {
            debug!("[Keyfire] Clipboard: thumb decode failed: {}", e);
            return None;
        }
    };
    // `thumbnail` preserves aspect ratio, fitting inside the given box —
    // exactly what we want for the tile-slot in the card. It uses a fast
    // nearest-neighbour path for order-of-magnitude downscales which is fine
    // at this size; the visible tile is ~120x90 in the UI so even Lanczos
    // wouldn't move the needle perceptibly.
    let thumb = img.thumbnail(THUMB_MAX_DIMENSION, THUMB_MAX_DIMENSION);
    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = thumb.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::WebP) {
        debug!("[Keyfire] Clipboard: thumb WebP encode failed: {}", e);
        return None;
    }
    Some(buf)
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
            "[Keyfire] Clipboard: decrypt called with invalid iv length {} (expected 12)",
            iv.len()
        );
        return None;
    }
    let nonce = Nonce::from_slice(iv);
    match cipher.decrypt(nonce, ciphertext) {
        Ok(pt) => Some(pt),
        Err(_) => {
            // Auth-tag mismatch: wrong key or tampered row. Debug level — a
            // mismatched key fails EVERY row on every fetch, info would flood
            // the log. The aggregate warn lives in note_decrypt_failure.
            debug!("[Keyfire] Clipboard: row decrypt failed (auth-tag mismatch)");
            note_decrypt_failure();
            None
        }
    }
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
            warn!("[Keyfire] Clipboard: expiry file read failed: {}", e);
            return;
        }
    };
    let expiry = match chrono::DateTime::parse_from_rfc3339(expiry_str.trim()) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(e) => {
            warn!("[Keyfire] Clipboard: expiry file unparseable ({}); leaving in place", e);
            return;
        }
    };

    if chrono::Utc::now() < expiry {
        return;
    }

    if backup_path.exists() {
        match std::fs::remove_file(&backup_path) {
            Ok(()) => info!("[Keyfire] Clipboard: expired plaintext backup deleted"),
            Err(e) => {
                warn!("[Keyfire] Clipboard: failed to delete expired backup: {}", e);
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
        info!("[Keyfire] Clipboard: Phase 3b migration skipped (cipher unavailable)");
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
        info!("[Keyfire] Clipboard: Phase 3b migration not needed (no plaintext rows)");
        return Ok(0);
    }

    info!(
        "[Keyfire] Clipboard: Phase 3b migration starting ({} plaintext row(s))",
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
            "[Keyfire] Clipboard: plaintext backup saved to {}",
            backup_path.display()
        );
    } else {
        info!("[Keyfire] Clipboard: reusing existing plaintext backup from prior attempt");
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
        "[Keyfire] Clipboard: Phase 3b migration committed ({} row(s) encrypted, {} sample(s) verified)",
        migrated,
        sample.len()
    );

    // STEP 5: write the 7-day expiry stamp.
    let expiry_path = db_path.with_file_name(PLAINTEXT_BACKUP_EXPIRES_NAME);
    let expiry = chrono::Utc::now() + chrono::Duration::days(PLAINTEXT_BACKUP_RETENTION_DAYS);
    if let Err(e) = std::fs::write(&expiry_path, expiry.to_rfc3339()) {
        warn!("[Keyfire] Clipboard: failed to write expiry file: {}", e);
        // Non-fatal: the backup will just live forever until manually deleted.
    } else {
        info!(
            "[Keyfire] Clipboard: plaintext backup expires {}",
            expiry.to_rfc3339()
        );
    }

    Ok(migrated)
}

// ── Clipboard entry ──────────────────────────────────────────────────────────

struct ClipEntry {
    content_type: String,
    text_content: Option<String>,
    /// Unwrapped CF_HTML fragment when the source app put HTML on the clipboard
    /// alongside CF_UNICODETEXT. `None` = plain-text-only capture. Store the
    /// fragment only, NOT the CF_HTML wrapper — the wrapper is rebuilt at paste
    /// time via expansions::build_cf_html.
    html_content: Option<String>,
    image_blob: Option<Vec<u8>>,
    image_width: u32,
    image_height: u32,
    preview: String,
    source_app: String,
    /// v0.8.4: full exe path for the source process. Empty when Free-tier or
    /// when the foreground exe couldn't be resolved. Encrypted per row — full
    /// paths often include %USERPROFILE% so treat as PII, matching the other
    /// content-column encryption tier.
    source_app_path: String,
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
        /// Some("starred")    → only starred items, ignoring age
        /// Some("YYYY-MM-DD") → only rows whose local date matches
        date_filter: Option<String>,
        /// Exact match on source_app column (e.g. "chrome.exe"). None = no filter.
        app_filter: Option<String>,
        /// Exact match on content_tag column ("Text", "Image", ...). None = no filter.
        tag_filter: Option<String>,
        /// Case-insensitive substring match against the preview column. None / empty = no filter.
        search: Option<String>,
        /// Main UI sorts starred items above pinned; popup ignores starred
        /// (only pinned promotes). True = Main UI ordering, false = popup ordering.
        promote_starred: bool,
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
    StarItem {
        id: i64,
        starred: bool,
        reply: mpsc::Sender<bool>,
    },
    ReorderPinned {
        ids: Vec<i64>,
        reply: mpsc::Sender<bool>,
    },
    ReorderStarred {
        ids: Vec<i64>,
        reply: mpsc::Sender<bool>,
    },
    CreateFolder {
        name: String,
        reply: mpsc::Sender<Option<i64>>,
    },
    RenameFolder {
        id: i64,
        name: String,
        reply: mpsc::Sender<bool>,
    },
    DeleteFolder {
        id: i64,
        reply: mpsc::Sender<bool>,
    },
    MoveToFolder {
        id: i64,
        folder_id: Option<i64>,
        reply: mpsc::Sender<bool>,
    },
    GetFolders {
        reply: mpsc::Sender<Value>,
    },
    GetImageBlob {
        id: i64,
        reply: mpsc::Sender<Option<Vec<u8>>>,
    },
    GetDistinctSourceApps {
        reply: mpsc::Sender<Vec<String>>,
    },
    GetDateBuckets {
        app_filter: Option<String>,
        tag_filter: Option<String>,
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
    /// One-off backfill helper: return every image row that hasn't been OCR'd
    /// yet. Used by `run_ocr_backfill` after a fresh Pro upgrade to catch up
    /// on the existing clipboard history.
    GetPendingOcrIds {
        reply: mpsc::Sender<Vec<i64>>,
    },
    /// v0.8.4 thumbnail backfill: write a pre-decoded WebP thumbnail into a
    /// row's thumb_blob column (encrypted alongside image_blob). Only called
    /// by run_thumb_backfill for legacy image rows.
    SetThumbBlob {
        id: i64,
        thumb: Vec<u8>,
    },
    /// Return every image row that has an image_blob but no thumb_blob yet.
    /// Backfill worker iterates these once on first v0.8.4 launch.
    GetPendingThumbIds {
        reply: mpsc::Sender<Vec<i64>>,
    },
    IncrementPasteCount {
        id: i64,
    },
    /// Promote-on-use: copying a row from the panel floats it to the top of
    /// the timeline without creating a duplicate entry.
    TouchItem {
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
    /// CF_HTML fragment (unwrapped). Populated when the source app copied rich
    /// content. paste_clipboard_item routes through write_clipboard_dual when
    /// this is Some so rich-text-aware target apps receive formatting; plain-
    /// text-only apps automatically fall back to text_content.
    pub html_content: Option<String>,
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

// ── Auto-OCR (Pro) + search-inside-images (Pro) ─────────────────────────────
//
// Both default ON. The Pro gate is checked at dispatch time in handle_new_entry
// (auto-OCR) and in search_history (search-inside-images), so a licence
// transition takes effect on the next capture / search without restart.
//
// Auto-OCR dispatches to a dedicated worker thread via `OCR_TX`. The worker
// receives (id, plaintext_png_bytes) — cheaper than round-tripping through
// the writer thread + decryption. It calls `ocr_png_bytes` (blocking WinRT),
// then sends the recognised text back through the writer thread via
// `set_ocr_text` so it lands in the DB encrypted on the same path the manual
// Extract text button uses.
//
// Size floor (64x64) skips icons + tiny sprites (OCR would just be noise).
// Size cap (4000x4000) skips huge photos where OCR takes multiple seconds and
// is unlikely to yield useful text — user can still trigger Extract text
// manually for those.

static AUTO_OCR_ENABLED: AtomicBool = AtomicBool::new(true);
static SEARCH_INSIDE_IMAGES_ENABLED: AtomicBool = AtomicBool::new(true);

const OCR_MIN_DIMENSION: u32 = 64;
const OCR_MAX_DIMENSION: u32 = 4000;

pub fn auto_ocr_enabled() -> bool {
    AUTO_OCR_ENABLED.load(Ordering::SeqCst)
}

pub fn set_auto_ocr_enabled(enabled: bool) {
    AUTO_OCR_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn search_inside_images_enabled() -> bool {
    SEARCH_INSIDE_IMAGES_ENABLED.load(Ordering::SeqCst)
}

pub fn set_search_inside_images_enabled(enabled: bool) {
    SEARCH_INSIDE_IMAGES_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Job dispatched to the OCR worker thread. Holds the plaintext PNG bytes so
/// the worker doesn't need DB access. `id` is the row we'll update once the
/// text is recognised.
struct OcrJob {
    id: i64,
    png: Vec<u8>,
}

static OCR_TX: OnceLock<Mutex<mpsc::Sender<OcrJob>>> = OnceLock::new();

/// Dispatch an OCR job to the worker thread. No-op if the worker hasn't been
/// spawned or the channel has closed. Never blocks the caller.
fn dispatch_ocr_job(id: i64, png: Vec<u8>) {
    if let Some(tx) = OCR_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(OcrJob { id, png });
        }
    }
}

/// Spawn the single OCR worker thread. Called once from `init()`. Serial
/// processing keeps CPU usage bounded even under a paste-burst of images.
fn spawn_ocr_worker() {
    let (tx, rx) = mpsc::channel::<OcrJob>();
    let _ = OCR_TX.set(Mutex::new(tx));
    thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            let started = std::time::Instant::now();
            match crate::ocr::ocr_png_bytes(&job.png) {
                Ok(text) => {
                    let has_text = !text.trim().is_empty();
                    debug!(
                        "[Keyfire] Clipboard: auto-OCR id={} {} chars in {}ms",
                        job.id,
                        text.len(),
                        started.elapsed().as_millis()
                    );
                    // Always write, even for empty results — a NULL ocr_text
                    // still means "not tried"; empty string means "tried, no
                    // text found" and prevents redundant re-OCR.
                    set_ocr_text(job.id, text);
                    if let Some(app) = APP_HANDLE.get() {
                        use tauri::Emitter;
                        let _ = app.emit(
                            "clipboard-item-ocred",
                            serde_json::json!({ "id": job.id, "has_text": has_text }),
                        );
                    }
                }
                Err(e) => {
                    // OCR engine missing / no language pack / decode failed.
                    // Log at debug so we don't spam trigr.log on systems
                    // without OCR — user will still see "OCR not available"
                    // if they click Extract text manually.
                    debug!("[Keyfire] Clipboard: auto-OCR id={} failed: {}", job.id, e);
                }
            }
        }
    });
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

/// Returns `(basename, full_path)` for the foreground process. basename is
/// the exe filename with extension (e.g. "chrome.exe") — matches the historic
/// source_app column shape and stays the filter key. full_path is the
/// absolute path used by the frontend to look up an app icon via
/// SHGetFileInfoW. Either half is String::new() on failure so the caller can
/// null-check without unwrapping options.
fn get_foreground_process_info() -> (String, String) {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return (String::new(), String::new());
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return (String::new(), String::new());
        }
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            let process2 = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if process2.is_null() {
                return (String::new(), String::new());
            }
            let info = query_process_info(process2);
            windows_sys::Win32::Foundation::CloseHandle(process2);
            return info;
        }
        let info = query_process_info(process);
        windows_sys::Win32::Foundation::CloseHandle(process);
        info
    }
}

unsafe fn query_process_info(process: *mut std::ffi::c_void) -> (String, String) {
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
        return (String::new(), String::new());
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    let name = path.rsplit('\\').next().unwrap_or("").to_string();
    (name, path)
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

    // starred: second tier above pinned, Main UI only. Independent of `pinned`
    // — an item can be both. Popup UI ignores `starred` (popup only promotes
    // pinned). pinned_order / starred_order: nullable rank within each tier;
    // NULL means "unranked, fall back to id DESC tiebreaker". COALESCE(...,
    // 999999) in ORDER BY pushes NULLs to the bottom of their tier.
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN starred INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN pinned_order INTEGER", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN starred_order INTEGER", []);

    // html_content: CF_HTML fragment (unwrapped) captured alongside CF_UNICODETEXT
    // when the source app put both on the clipboard. NULL for plain-text-only
    // copies and for all pre-v0.6.4 rows — those paste as plain text, matching
    // pre-existing behaviour. Encrypted per row with iv_html following the
    // Phase 3a AES-256-GCM pattern (fresh IV per write, NULL iv_html means the
    // ciphertext column is a legacy plaintext fallback).
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN html_content BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_html BLOB", []);

    // v0.8.4: pre-decoded WebP thumbnail (~200x200) inlined into the list
    // payload so image tiles paint without a full-res IPC round-trip. NULL
    // for legacy rows until the one-shot backfill runs (see run_thumb_backfill).
    // Encrypted with a fresh IV like image_blob — iv_thumb NULL means the
    // ciphertext is legacy plaintext (in practice never — the column shipped
    // after the encryption path). Non-image rows also carry NULL.
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN thumb_blob BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_thumb BLOB", []);

    // v0.8.4: full exe path for the source process, used by the frontend to
    // fetch an app icon via SHGetFileInfoW. Encrypted per row — %USERPROFILE%
    // often appears in the path. NULL for Free-tier + all pre-v0.8.4 rows,
    // which fall back to the text badge.
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN source_app_path BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN iv_source_app_path BLOB", []);

    // Saved folders: flat (no nesting) user-created folders that organise the
    // Saved tier (internally still `starred` — the rename is UI-only).
    // folder_id NULL = root of Saved. Folder names are user-typed and can be
    // sensitive ("Passwords", "Client X"), so they follow the Phase 3a
    // AES-256-GCM pattern: name holds ciphertext when iv_name is non-NULL,
    // legacy/cipher-unavailable plaintext when NULL. Un-saving an item clears
    // its folder_id; deleting a folder moves its items back to the Saved
    // root, never deletes them.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS clipboard_folders (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            sort_order INTEGER
        );",
    )
    .map_err(|e| format!("create folders table: {}", e))?;
    let _ = conn.execute("ALTER TABLE clipboard_folders ADD COLUMN iv_name BLOB", []);
    let _ = conn.execute("ALTER TABLE clipboard_history ADD COLUMN folder_id INTEGER", []);

    Ok(conn)
}

/// Phase 4: Reset Clipboard Storage. Wipes the db (+WAL/SHM sidecars), the
/// DPAPI key file, and any plaintext backup, generates a fresh master key,
/// and reopens an empty db. Per [[feedback_data_hygiene]] files are deleted
/// outright so disk space is actually reclaimed. Returns the connection to
/// continue the writer loop with (None only if no db could be reopened at
/// all) and whether the reset fully succeeded.
fn handle_reset_storage(conn: Connection, db_path: &Path) -> (Option<Connection>, bool) {
    info!("[Keyfire] Clipboard: storage reset requested");
    let mut ok = true;

    // 1. Checkpoint + close so Windows releases the file locks.
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    if let Err((c, e)) = conn.close() {
        warn!("[Keyfire] Clipboard: close before reset failed: {}", e);
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
            error!("[Keyfire] Clipboard: reset failed to delete {}: {}", path.display(), e);
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
            error!("[Keyfire] Clipboard: reset failed to generate new key: {}", e);
            ok = false;
            None
        }
    };
    if let Ok(mut guard) = CLIPBOARD_CIPHER.write() {
        *guard = new_cipher;
    }

    // Reset wipes db + key together, so any prior error state is stale now.
    // Clearing DECRYPT_TOAST_SENT lets a future, genuinely new failure toast
    // again instead of being swallowed by this session's earlier one.
    KEY_UNREADABLE.store(false, Ordering::SeqCst);
    DECRYPT_FAILURES.store(0, Ordering::SeqCst);
    DECRYPT_TOAST_SENT.store(false, Ordering::SeqCst);

    // 4. Reopen a fresh, empty db.
    match open_clipboard_db(db_path) {
        Ok(c) => {
            if ok {
                info!("[Keyfire] Clipboard: storage reset complete (fresh db + key)");
            }
            (Some(c), ok)
        }
        Err(e) => {
            error!("[Keyfire] Clipboard: reset could not reopen db: {}", e);
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
        // Auto-OCR settings default ON. Only override if the user has explicitly
        // saved a value — a missing key on a fresh install (or an install that
        // predates this feature) keeps the default enabled experience.
        if let Some(v) = cfg.get("clipboardAutoOcr").and_then(|v| v.as_bool()) {
            AUTO_OCR_ENABLED.store(v, Ordering::SeqCst);
        }
        if let Some(v) = cfg.get("clipboardSearchInsideImages").and_then(|v| v.as_bool()) {
            SEARCH_INSIDE_IMAGES_ENABLED.store(v, Ordering::SeqCst);
        }
    }

    // Spawn the OCR worker thread. Idle until the first capture-with-image
    // arrives and the Pro / setting gates pass in handle_new_entry.
    spawn_ocr_worker();

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
                    error!("[Keyfire] Failed to open clipboard DB: {}", e);
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
                Ok(n) => info!("[Keyfire] Clipboard: Phase 3b migrated {} row(s)", n),
                Err(e) => error!("[Keyfire] Clipboard: Phase 3b migration failed: {}", e),
            }

            info!("[Keyfire] Clipboard DB ready: {}", db_path.display());

            for msg in rx {
                match msg {
                    ClipboardMsg::NewEntry(entry) => handle_new_entry(&conn, entry),
                    ClipboardMsg::GetHistory { page, per_page, date_filter, app_filter, tag_filter, search, promote_starred, reply } => {
                        let result = handle_get_history(
                            &conn, page, per_page,
                            date_filter.as_deref(),
                            app_filter.as_deref(),
                            tag_filter.as_deref(),
                            search.as_deref(),
                            promote_starred,
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
                    ClipboardMsg::StarItem { id, starred, reply } => {
                        let ok = handle_star_item(&conn, id, starred);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::ReorderPinned { ids, reply } => {
                        let ok = handle_reorder_tier(&mut conn, &ids, "pinned_order");
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::ReorderStarred { ids, reply } => {
                        let ok = handle_reorder_tier(&mut conn, &ids, "starred_order");
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::CreateFolder { name, reply } => {
                        let id = handle_create_folder(&conn, &name);
                        let _ = reply.send(id);
                    }
                    ClipboardMsg::RenameFolder { id, name, reply } => {
                        let ok = handle_rename_folder(&conn, id, &name);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::DeleteFolder { id, reply } => {
                        let ok = handle_delete_folder(&mut conn, id);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::MoveToFolder { id, folder_id, reply } => {
                        let ok = handle_move_to_folder(&conn, id, folder_id);
                        let _ = reply.send(ok);
                    }
                    ClipboardMsg::GetFolders { reply } => {
                        let folders = handle_get_folders(&conn);
                        let _ = reply.send(folders);
                    }
                    ClipboardMsg::GetImageBlob { id, reply } => {
                        let blob = handle_get_image_blob(&conn, id);
                        let _ = reply.send(blob);
                    }
                    ClipboardMsg::GetDistinctSourceApps { reply } => {
                        let apps = handle_get_distinct_source_apps(&conn);
                        let _ = reply.send(apps);
                    }
                    ClipboardMsg::GetDateBuckets { app_filter, tag_filter, reply } => {
                        let buckets = handle_get_date_buckets(&conn, app_filter.as_deref(), tag_filter.as_deref());
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
                    ClipboardMsg::GetPendingOcrIds { reply } => {
                        let ids: Vec<i64> = conn
                            .prepare("SELECT id FROM clipboard_history WHERE content_type = 'image' AND ocr_text IS NULL AND image_blob IS NOT NULL ORDER BY id DESC")
                            .and_then(|mut stmt| {
                                let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
                                Ok(rows.filter_map(|r| r.ok()).collect())
                            })
                            .unwrap_or_default();
                        let _ = reply.send(ids);
                    }
                    ClipboardMsg::SetThumbBlob { id, thumb } => {
                        // Encrypt with a fresh IV, then UPDATE both columns.
                        // On cipher-not-initialised, store plaintext with
                        // iv_thumb NULL — same graceful degrade as image_blob.
                        let (thumb_ct, iv_thumb): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(&thumb) {
                            Some((ct, iv)) => (ct, Some(iv)),
                            None => (thumb, None),
                        };
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET thumb_blob = ?1, iv_thumb = ?2 WHERE id = ?3",
                            rusqlite::params![thumb_ct, iv_thumb, id],
                        );
                    }
                    ClipboardMsg::GetPendingThumbIds { reply } => {
                        let ids: Vec<i64> = conn
                            .prepare("SELECT id FROM clipboard_history WHERE content_type = 'image' AND thumb_blob IS NULL AND image_blob IS NOT NULL ORDER BY id DESC")
                            .and_then(|mut stmt| {
                                let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
                                Ok(rows.filter_map(|r| r.ok()).collect())
                            })
                            .unwrap_or_default();
                        let _ = reply.send(ids);
                    }
                    ClipboardMsg::IncrementPasteCount { id } => {
                        // Counter only — pasting must NOT reorder the list.
                        // Sequential workflows (copy 5 items, paste them back
                        // in order from the popup) break if each paste floats
                        // its row to the top and shuffles the list underneath
                        // the user (design reversal 2026-07-28; promote-on-
                        // paste shipped in v0.6.7 and was walked back).
                        // Explicit panel Copy still promotes via TouchItem.
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET paste_count = paste_count + 1 WHERE id = ?1",
                            rusqlite::params![id],
                        );
                    }
                    ClipboardMsg::TouchItem { id } => {
                        let now = chrono::Utc::now().to_rfc3339();
                        let _ = conn.execute(
                            "UPDATE clipboard_history SET timestamp = ?1 WHERE id = ?2",
                            rusqlite::params![now, id],
                        );
                        emit_item_touched(id, &now);
                    }
                    ClipboardMsg::Prune => handle_prune(&conn),
                    ClipboardMsg::ResetStorage { reply } => {
                        let (new_conn, ok) = handle_reset_storage(conn, &db_path);
                        match new_conn {
                            Some(c) => conn = c,
                            None => {
                                // No usable db — same terminal state as a failed
                                // open at startup. Reply, log, end the thread.
                                error!("[Keyfire] Clipboard: writer thread exiting after failed storage reset");
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
    promote_starred: bool,
) -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        let (reply_tx, reply_rx) = mpsc::channel();
        // Lock scoped to the send — see get_image_blob for why the guard
        // must not be held across the reply wait.
        let sent = tx
            .lock()
            .map(|tx| {
                tx.send(ClipboardMsg::GetHistory {
                    page, per_page, date_filter, app_filter, tag_filter, search, promote_starred,
                    reply: reply_tx,
                })
                .is_ok()
            })
            .unwrap_or(false);
        if sent {
            if let Ok(result) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                return result;
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
        // Phase 5: error surfacing. key_unreadable = DPAPI unwrap failed at
        // startup; decrypt_failures = auth-tag mismatches this session.
        "key_unreadable": KEY_UNREADABLE.load(Ordering::SeqCst),
        "decrypt_failures": DECRYPT_FAILURES.load(Ordering::SeqCst),
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
            error!("[Keyfire] Clipboard: failed to delete plaintext backup: {}", e);
            return false;
        }
        info!("[Keyfire] Clipboard: plaintext backup deleted via Settings");
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

pub fn star_item(id: i64, starred: bool) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::StarItem { id, starred, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn reorder_pinned(ids: Vec<i64>) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::ReorderPinned { ids, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn reorder_starred(ids: Vec<i64>) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::ReorderStarred { ids, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn create_folder(name: String) -> Option<i64> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::CreateFolder { name, reply: reply_tx }).is_ok() {
                if let Ok(id) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return id;
                }
            }
        }
    }
    None
}

pub fn rename_folder(id: i64, name: String) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::RenameFolder { id, name, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn delete_folder(id: i64) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::DeleteFolder { id, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn move_to_folder(id: i64, folder_id: Option<i64>) -> bool {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::MoveToFolder { id, folder_id, reply: reply_tx }).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

pub fn get_folders() -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetFolders { reply: reply_tx }).is_ok() {
                if let Ok(folders) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return folders;
                }
            }
        }
    }
    serde_json::json!([])
}

pub fn get_image_blob(id: i64) -> Option<Vec<u8>> {
    if let Some(tx) = CLIPBOARD_TX.get() {
        let (reply_tx, reply_rx) = mpsc::channel();
        // Send under the lock, wait for the reply AFTER releasing it.
        // Holding the guard across recv_timeout made every concurrent
        // requester serialise on the mutex for the full round-trip
        // (seconds per image in debug) — under a thumbnail burst that
        // stalled every other clipboard caller, main thread included.
        let sent = tx
            .lock()
            .map(|tx| tx.send(ClipboardMsg::GetImageBlob { id, reply: reply_tx }).is_ok())
            .unwrap_or(false);
        if sent {
            if let Ok(blob) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                return blob;
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

pub fn get_date_buckets(app_filter: Option<String>, tag_filter: Option<String>) -> Value {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(ClipboardMsg::GetDateBuckets { app_filter, tag_filter, reply: reply_tx }).is_ok() {
                if let Ok(buckets) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return buckets;
                }
            }
        }
    }
    serde_json::json!({ "dates": [], "pinned_count": 0, "starred_count": 0 })
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

/// One-off OCR backfill for existing image rows. Called from the frontend on
/// first launch after a Pro upgrade (guarded by a localStorage flag so it
/// only runs once). Runs on a dedicated thread so we don't block the caller.
///
/// Emits `clipboard-ocr-backfill-progress` with `{processed, total}` after
/// each item, and `clipboard-ocr-backfill-done` when the queue drains. The
/// status bar listens for both. Silent no-op if the user isn't Pro or auto-
/// OCR is disabled — the frontend guard should skip in those cases anyway.
///
/// Aborts after `MAX_CONSECUTIVE_FAILURES` OCR errors in a row — usually
/// means the OCR engine or language pack is missing and every subsequent
/// call would fail the same way.
pub fn run_ocr_backfill() {
    if !crate::licence::is_pro() || !auto_ocr_enabled() {
        return;
    }
    const MAX_CONSECUTIVE_FAILURES: u32 = 3;

    thread::spawn(move || {
        let ids: Vec<i64> = if let Some(tx) = CLIPBOARD_TX.get() {
            if let Ok(tx) = tx.lock() {
                let (reply_tx, reply_rx) = mpsc::channel();
                if tx.send(ClipboardMsg::GetPendingOcrIds { reply: reply_tx }).is_ok() {
                    reply_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap_or_default()
                } else {
                    return;
                }
            } else {
                return;
            }
        } else {
            return;
        };

        let total = ids.len();
        if total == 0 {
            return;
        }

        info!("[Keyfire] Clipboard: OCR backfill starting, {} image(s) pending", total);
        emit_backfill_progress(0, total);

        let mut consecutive_failures: u32 = 0;
        for (i, id) in ids.iter().enumerate() {
            // If the user disables auto-OCR mid-backfill, bail out cleanly.
            if !auto_ocr_enabled() || !crate::licence::is_pro() {
                info!("[Keyfire] Clipboard: OCR backfill cancelled (setting/licence change)");
                break;
            }
            let blob = get_image_blob(*id);
            if let Some(blob) = blob {
                match crate::ocr::ocr_png_bytes(&blob) {
                    Ok(text) => {
                        consecutive_failures = 0;
                        set_ocr_text(*id, text);
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        debug!("[Keyfire] Clipboard: backfill OCR id={} failed: {}", id, e);
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            warn!(
                                "[Keyfire] Clipboard: OCR backfill aborting after {} consecutive failures — engine or language pack likely missing",
                                consecutive_failures
                            );
                            break;
                        }
                    }
                }
            }
            emit_backfill_progress(i + 1, total);
        }

        info!("[Keyfire] Clipboard: OCR backfill done");
        if let Some(app) = APP_HANDLE.get() {
            use tauri::Emitter;
            let _ = app.emit("clipboard-ocr-backfill-done", serde_json::json!({ "total": total }));
        }
    });
}

fn emit_backfill_progress(processed: usize, total: usize) {
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-ocr-backfill-progress",
            serde_json::json!({ "processed": processed, "total": total }),
        );
    }
}

pub fn set_thumb_blob(id: i64, thumb: Vec<u8>) {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::SetThumbBlob { id, thumb });
        }
    }
}

/// v0.8.4 one-off thumbnail backfill for existing image rows. Called from
/// the frontend on first launch after the perf patch lands (guarded by a
/// localStorage flag so it only runs once per install). Runs on a dedicated
/// thread so the caller isn't blocked.
///
/// For each pending image row: fetch the full-res image_blob, downscale +
/// WebP-encode via make_thumb_webp, then send the plaintext bytes back to the
/// writer via SetThumbBlob (which encrypts + UPDATEs). Rows that fail to
/// decode are skipped silently — a corrupted image_blob still lets the row
/// live, it just keeps falling back to the getClipboardImage lazy-fetch.
///
/// Emits `clipboard-thumb-backfill-progress` with `{processed, total}` after
/// each item, and `clipboard-thumb-backfill-done` when the queue drains.
pub fn run_thumb_backfill() {
    thread::spawn(move || {
        let ids: Vec<i64> = if let Some(tx) = CLIPBOARD_TX.get() {
            if let Ok(tx) = tx.lock() {
                let (reply_tx, reply_rx) = mpsc::channel();
                if tx.send(ClipboardMsg::GetPendingThumbIds { reply: reply_tx }).is_ok() {
                    reply_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap_or_default()
                } else {
                    return;
                }
            } else {
                return;
            }
        } else {
            return;
        };

        let total = ids.len();
        if total == 0 {
            emit_thumb_backfill_done(0);
            return;
        }

        info!("[Keyfire] Clipboard: thumb backfill starting, {} image(s) pending", total);
        emit_thumb_backfill_progress(0, total);

        for (i, id) in ids.iter().enumerate() {
            let blob = get_image_blob(*id);
            if let Some(blob) = blob {
                if let Some(thumb) = make_thumb_webp(&blob) {
                    set_thumb_blob(*id, thumb);
                }
            }
            emit_thumb_backfill_progress(i + 1, total);
        }

        info!("[Keyfire] Clipboard: thumb backfill done ({} row(s))", total);
        emit_thumb_backfill_done(total);
    });
}

fn emit_thumb_backfill_progress(processed: usize, total: usize) {
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-thumb-backfill-progress",
            serde_json::json!({ "processed": processed, "total": total }),
        );
    }
}

fn emit_thumb_backfill_done(total: usize) {
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-thumb-backfill-done",
            serde_json::json!({ "total": total }),
        );
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

/// Promote-on-use without a paste-count bump — the panel Copy paths. Fire and
/// forget, matching increment_paste_count.
pub fn touch_item(id: i64) {
    if let Some(tx) = CLIPBOARD_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ClipboardMsg::TouchItem { id });
        }
    }
}

/// Broadcast that a row's timestamp changed so the main panel can float it to
/// the top of the timeline without a full reload. Also fired for popup pastes
/// while the main window sits in the tray — the main webview is never
/// suspended, so the panel state stays current for the next open.
fn emit_item_touched(id: i64, timestamp: &str) {
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-item-touched",
            serde_json::json!({ "id": id, "timestamp": timestamp }),
        );
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
    let (html_ct, iv_html): (Option<Vec<u8>>, Option<Vec<u8>>) = match entry.html_content.as_deref() {
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
    // v0.8.4: encrypt source_app_path — full exe path is PII (username in
    // %USERPROFILE%). Empty string ⇒ NULL, same handling as text_content.
    let (path_ct, iv_source_app_path): (Option<Vec<u8>>, Option<Vec<u8>>) = if entry.source_app_path.is_empty() {
        (None, None)
    } else {
        match encrypt_blob(entry.source_app_path.as_bytes()) {
            Some((ct, iv)) => (Some(ct), Some(iv)),
            None => (Some(entry.source_app_path.as_bytes().to_vec()), None),
        }
    };
    // v0.8.4: pre-decode a small WebP thumbnail from the plaintext image bytes
    // so the list payload can inline it. Skipped for non-image rows and for
    // decode failures (row still saves; list falls back to full-res lazy
    // load). Encrypted separately with a fresh IV — same tier as image_blob.
    // The plaintext bytes are also kept in `thumb_plain` so the new-item event
    // can emit thumb_b64 without a decrypt round-trip.
    let thumb_plain: Option<Vec<u8>> = if entry.content_type == "image" {
        entry.image_blob.as_deref().and_then(make_thumb_webp)
    } else {
        None
    };
    let (thumb_ct, iv_thumb): (Option<Vec<u8>>, Option<Vec<u8>>) = match thumb_plain.as_deref() {
        Some(bytes) => match encrypt_blob(bytes) {
            Some((ct, iv)) => (Some(ct), Some(iv)),
            None => (Some(bytes.to_vec()), None),
        },
        None => (None, None),
    };

    let result = conn.execute(
        "INSERT INTO clipboard_history (timestamp, content_type, text_content, html_content, image_blob, image_width, image_height, preview, pinned, source_app, content_tag, iv_text, iv_html, iv_image, iv_preview, thumb_blob, iv_thumb, source_app_path, iv_source_app_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        rusqlite::params![
            now,
            entry.content_type,
            text_ct,
            html_ct,
            image_ct,
            entry.image_width,
            entry.image_height,
            preview_ct,
            entry.source_app,
            entry.content_tag,
            iv_text,
            iv_html,
            iv_image,
            iv_preview,
            thumb_ct,
            iv_thumb,
            path_ct,
            iv_source_app_path,
        ],
    );

    if let Err(e) = result {
        error!("[Keyfire] Failed to insert clipboard entry: {}", e);
        return;
    }

    let new_id = conn.last_insert_rowid();
    handle_prune(conn);

    // Auto-OCR dispatch (Pro, setting-gated, size-gated). Uses plaintext PNG
    // bytes from the entry directly — cheaper than round-tripping through DB
    // decrypt. Skips icons (<64px) and huge images (>4000px) that either lack
    // meaningful text or would monopolise the OCR worker.
    if entry.content_type == "image" && crate::licence::is_pro() && auto_ocr_enabled() {
        if let Some(png) = entry.image_blob.as_deref() {
            let w = entry.image_width;
            let h = entry.image_height;
            let within_floor = w >= OCR_MIN_DIMENSION && h >= OCR_MIN_DIMENSION;
            let within_cap = w <= OCR_MAX_DIMENSION && h <= OCR_MAX_DIMENSION;
            if within_floor && within_cap {
                dispatch_ocr_job(new_id, png.to_vec());
            }
        }
    }

    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "clipboard-new-item",
            serde_json::json!({
                "id": new_id,
                "timestamp": now,
                "content_type": entry.content_type,
                // text_content and html_content are intentionally omitted —
                // fresh copies of huge text (thousands of lines pasted over
                // and over) would otherwise pile into React state item by
                // item. The frontend lazy-fetches full text on selection via
                // get_clipboard_item_text_full; preview is enough for the
                // card render.
                "preview": entry.preview,
                "image_width": entry.image_width,
                "image_height": entry.image_height,
                "pinned": false,
                "starred": false,
                "pinned_order": serde_json::Value::Null,
                "starred_order": serde_json::Value::Null,
                "folder_id": serde_json::Value::Null,
                "source_app": entry.source_app,
                "content_tag": entry.content_tag,
                // has_html only — the full fragment can be large (Word/Excel
                // paste blobs are ~KBs each) and the UI just needs the boolean
                // to decide whether to surface "Paste as plain".
                "has_html": entry.html_content.is_some(),
                // v0.8.4: inline the plaintext WebP thumbnail so a fresh image
                // copy's tile paints without a getClipboardImage round-trip.
                "thumb_b64": thumb_plain.as_deref().map(|b| B64_STD.encode(b)),
                // v0.8.4: full exe path so the frontend can look up an app
                // icon via SHGetFileInfoW. Empty string emitted as null.
                "source_app_path": if entry.source_app_path.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(entry.source_app_path.clone())
                },
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
    promote_starred: bool,
) -> Value {
    let offset = page.saturating_sub(1) * per_page;
    // Pro-gated visibility window (used by the default + per-date views).
    // Pinned + starred rows always bypass age. Per [[feedback_sqlite_localtime_pattern]]
    // we compare local-time dates via DATE(timestamp, 'localtime').
    let days = effective_retention_days();
    let date_clause = match date_filter {
        // Sidebar "Pinned" bucket — every pinned row, ignoring age.
        Some("pinned") => "pinned = 1".to_string(),
        // Sidebar "Starred" bucket — every starred row, ignoring age. Main UI only.
        Some("starred") => "starred = 1".to_string(),
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
        _ => format!("(starred = 1 OR pinned = 1 OR timestamp >= datetime('now', '-{} days'))", days),
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
        return search_history(conn, &where_clause, &where_binds, &needle.to_lowercase(), per_page, offset, promote_starred);
    }

    // COUNT — same WHERE, just the toolbar binds.
    let count_sql = format!("SELECT COUNT(*) FROM clipboard_history WHERE {}", where_clause);
    let count_refs: Vec<&dyn rusqlite::ToSql> = where_binds.iter().map(|p| p.as_ref()).collect();
    let total: i64 = conn
        .query_row(&count_sql, rusqlite::params_from_iter(count_refs.iter()), |row| row.get(0))
        .unwrap_or(0);

    // ORDER BY: Main UI promotes starred above pinned; popup ignores starred
    // (only pinned promotes). COALESCE pushes NULL ranks to the bottom of
    // their tier so unranked items fall back to id DESC ordering within tier.
    let order_clause = order_by_clause(promote_starred);

    // LIST — same WHERE, then LIMIT/OFFSET appended after the toolbar binds.
    // text_content/preview/ocr_text are bound as BLOB and resolved per-row by
    // the helpers below: NON-NULL iv_* → decrypt; NULL iv_* → legacy plaintext.
    let list_sql = format!(
        "SELECT {} FROM clipboard_history WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
        HISTORY_LIST_COLUMNS, where_clause, order_clause
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
/// history_row_to_json reads by position — keep order in sync. New columns
/// are APPENDED to preserve existing positions for the iv_* / ocr_* indices.
///
/// The list query intentionally SELECTs iv_html but NOT html_content itself.
/// html_content can be large (KB per row) and the list view only needs the
/// has_html boolean to render the "Paste as plain" affordance; the full
/// fragment is fetched later via handle_get_item_full when the user actually
/// pastes. Every write path either sets both iv_html + html_content
/// (cipher available) or both NULL (no html captured), so iv_html alone is
/// a faithful presence signal.
///
/// text_content is likewise dropped from the list SELECT — cards render from
/// `preview` (truncated at write time, ~200 chars) and the full body is
/// fetched via get_item_full on selection / edit. Prevents multi-MB pastes
/// from stalling the list load.
///
/// thumb_blob + iv_thumb are appended — decrypted inline in history_row_to_json
/// and emitted as `thumb_b64`. Small WebP inlined avoids the per-image IPC
/// round-trip getClipboardImage would otherwise fire for every visible tile.
/// Legacy rows have thumb_blob NULL and fall back to full-res lazy load until
/// the one-shot backfill worker fills them in.
///
/// source_app_path + iv_source_app_path are appended — decrypted inline and
/// emitted as `source_app_path`. Frontend uses it to look up the source
/// process's icon via SHGetFileInfoW. Legacy + Free-tier rows have NULL and
/// fall back to the text badge.
const HISTORY_LIST_COLUMNS: &str = "id, timestamp, content_type, image_width, image_height, preview, pinned, source_app, content_tag, paste_count, ocr_text, iv_preview, iv_ocr, starred, pinned_order, starred_order, iv_html, folder_id, thumb_blob, iv_thumb, source_app_path, iv_source_app_path";

/// Shared row → JSON mapping for the history list (normal + search paths).
/// Reads HISTORY_LIST_COLUMNS by position.
fn history_row_to_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let preview_ct = get_optional_bytes(row, 5).ok().flatten().unwrap_or_default();
    let iv_preview = row.get::<_, Option<Vec<u8>>>(11).unwrap_or(None);
    let ocr_ct = get_optional_bytes(row, 10).unwrap_or(None);
    let iv_ocr = row.get::<_, Option<Vec<u8>>>(12).unwrap_or(None);
    let iv_html = row.get::<_, Option<Vec<u8>>>(16).unwrap_or(None);
    let thumb_ct = get_optional_bytes(row, 18).unwrap_or(None);
    let iv_thumb = row.get::<_, Option<Vec<u8>>>(19).unwrap_or(None);
    // Decrypt-and-base64 inline. Rows without a thumbnail (non-image + legacy
    // pre-backfill) emit thumb_b64 = null; frontend ImageThumb falls back to
    // getClipboardImage for those.
    let thumb_b64 = resolve_optional_bytes(thumb_ct, iv_thumb)
        .map(|bytes| B64_STD.encode(&bytes));
    let path_ct = get_optional_bytes(row, 20).unwrap_or(None);
    let iv_source_app_path = row.get::<_, Option<Vec<u8>>>(21).unwrap_or(None);
    let source_app_path = resolve_optional_text(path_ct, iv_source_app_path);
    Ok(serde_json::json!({
        "id": row.get::<_, i64>(0).unwrap_or(0),
        "timestamp": row.get::<_, String>(1).unwrap_or_default(),
        "content_type": row.get::<_, String>(2).unwrap_or_default(),
        "image_width": row.get::<_, u32>(3).unwrap_or(0),
        "image_height": row.get::<_, u32>(4).unwrap_or(0),
        "preview": resolve_required_text(preview_ct, iv_preview),
        "pinned": row.get::<_, i32>(6).unwrap_or(0) != 0,
        "source_app": row.get::<_, String>(7).unwrap_or_default(),
        "content_tag": row.get::<_, String>(8).unwrap_or("Text".to_string()),
        "paste_count": row.get::<_, i64>(9).unwrap_or(0),
        "ocr_text": resolve_optional_text(ocr_ct, iv_ocr),
        "starred": row.get::<_, i32>(13).unwrap_or(0) != 0,
        "pinned_order": row.get::<_, Option<i64>>(14).unwrap_or(None),
        "starred_order": row.get::<_, Option<i64>>(15).unwrap_or(None),
        "has_html": iv_html.is_some(),
        "folder_id": row.get::<_, Option<i64>>(17).unwrap_or(None),
        "thumb_b64": thumb_b64,
        "source_app_path": source_app_path,
    }))
}

/// ORDER BY for the history list. Two variants share the same NULL-handling
/// pattern via COALESCE so unranked items fall to the bottom of their tier.
///   Main UI: saved items above pinned, then by tier-rank, then recency.
///   Popup: only pinned promote, by pinned_order, then recency.
/// Recency = datetime(timestamp) DESC (promote-on-use rewrites timestamp, so
/// last-used floats to the top like Win+V / Paste / Ditto), id DESC tiebreak.
/// datetime() normalises the stored string — RFC3339 and any legacy
/// "YYYY-MM-DD HH:MM:SS" rows compare correctly instead of lexically.
fn order_by_clause(promote_starred: bool) -> &'static str {
    if promote_starred {
        "starred DESC, pinned DESC, COALESCE(starred_order, 999999) ASC, COALESCE(pinned_order, 999999) ASC, datetime(timestamp) DESC, id DESC"
    } else {
        "pinned DESC, COALESCE(pinned_order, 999999) ASC, datetime(timestamp) DESC, id DESC"
    }
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
    promote_starred: bool,
) -> Value {
    let started = std::time::Instant::now();
    let order_clause = order_by_clause(promote_starred);

    // Search-inside-images (Pro + setting): also decrypt and scan ocr_text on
    // image rows. Gated at scan time so a licence transition or setting flip
    // takes effect on the next query without restart. Non-image rows have
    // ocr_text = NULL so pulling those columns is cheap.
    let search_images = crate::licence::is_pro() && search_inside_images_enabled();

    // Full-text scan (added with the clipboard perf patch). Preview is
    // truncated to 200 chars at write time, so a substring that only appears
    // past that offset used to be invisible to search. The scan now also
    // pulls text_content + iv_text; on preview-miss (and ocr-miss for image
    // rows) we decrypt the full body and retry the match. Preview hits
    // short-circuit — the extra decrypt only fires when the cheaper checks
    // failed, so common-word searches still return quickly for rows whose
    // preview already contains the needle.
    let scan_sql = format!(
        "SELECT id, preview, iv_preview, ocr_text, iv_ocr, text_content, iv_text FROM clipboard_history WHERE {} ORDER BY {}",
        where_clause, order_clause
    );
    let bind_refs: Vec<&dyn rusqlite::ToSql> = where_binds.iter().map(|p| p.as_ref()).collect();
    let mut stmt = match conn.prepare(&scan_sql) {
        Ok(s) => s,
        Err(e) => {
            error!("[Keyfire] Clipboard: search scan prepare failed: {}", e);
            return serde_json::json!({ "items": [], "total": 0 });
        }
    };
    let mut scanned: usize = 0;
    let mut ocr_matches: HashSet<i64> = HashSet::new();
    let mut text_matches: HashSet<i64> = HashSet::new();
    let matched_ids: Vec<i64> = stmt
        .query_map(rusqlite::params_from_iter(bind_refs.iter()), |row| {
            let id = row.get::<_, i64>(0)?;
            let preview_ct = get_optional_bytes(row, 1)?.unwrap_or_default();
            let iv_preview = row.get::<_, Option<Vec<u8>>>(2)?;
            let ocr_ct = get_optional_bytes(row, 3)?;
            let iv_ocr = row.get::<_, Option<Vec<u8>>>(4)?;
            let text_ct = get_optional_bytes(row, 5)?;
            let iv_text = row.get::<_, Option<Vec<u8>>>(6)?;
            Ok((
                id,
                resolve_required_text(preview_ct, iv_preview),
                ocr_ct.and_then(|ct| resolve_optional_text(Some(ct), iv_ocr)),
                text_ct,
                iv_text,
            ))
        })
        .map(|iter| {
            iter.filter_map(|r| r.ok())
                .inspect(|_| scanned += 1)
                .filter_map(|(id, preview, ocr, text_ct, iv_text)| {
                    let preview_hit = preview.to_lowercase().contains(needle);
                    let ocr_hit = !preview_hit
                        && search_images
                        && ocr
                            .as_deref()
                            .map(|s| s.to_lowercase().contains(needle))
                            .unwrap_or(false);
                    // Full text_content check on preview + ocr miss. Decrypt
                    // is skipped when either lighter check already hit and
                    // when text_content is NULL (image rows / empty rows).
                    let text_hit = if preview_hit || ocr_hit {
                        false
                    } else {
                        resolve_optional_text(text_ct, iv_text)
                            .as_deref()
                            .map(|s| s.to_lowercase().contains(needle))
                            .unwrap_or(false)
                    };
                    if preview_hit || ocr_hit || text_hit {
                        // Track hits that came from OCR or full text only.
                        // Preview hits are the default and need no tag —
                        // the frontend filter will pass them naturally.
                        if ocr_hit {
                            ocr_matches.insert(id);
                        } else if text_hit {
                            text_matches.insert(id);
                        }
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let total = matched_ids.len() as i64;
    let page_ids: Vec<i64> = matched_ids
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .collect();

    let mut items: Vec<Value> = if page_ids.is_empty() {
        Vec::new()
    } else {
        // page_ids are i64s we produced ourselves — safe to inline. The same
        // ORDER BY re-applied to the id subset preserves the scan's order.
        let id_list = page_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let list_sql = format!(
            "SELECT {} FROM clipboard_history WHERE id IN ({}) ORDER BY {}",
            HISTORY_LIST_COLUMNS, id_list, order_clause
        );
        match conn.prepare(&list_sql) {
            Ok(mut stmt) => stmt
                .query_map([], |row| history_row_to_json(row))
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            Err(e) => {
                error!("[Keyfire] Clipboard: search page fetch prepare failed: {}", e);
                Vec::new()
            }
        }
    };

    // Tag rows whose match came from OCR text or full text_content only
    // (preview did not match). The panel uses "ocr" to render a small "in
    // image" chip; "text" is a signal to the frontend filter that the row
    // was matched deeper than the 200-char preview so it doesn't get
    // dropped by the client-side preview substring re-check.
    for item in items.iter_mut() {
        if let Some(id) = item.get("id").and_then(|v| v.as_i64()) {
            let tag = if ocr_matches.contains(&id) {
                Some("ocr")
            } else if text_matches.contains(&id) {
                Some("text")
            } else {
                None
            };
            if let Some(t) = tag {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("search_source".to_string(), serde_json::json!(t));
                }
            }
        }
    }

    debug!(
        "[Keyfire] Clipboard: search scanned {} row(s), {} match(es) ({} via OCR, {} via text) in {}ms",
        scanned,
        total,
        ocr_matches.len(),
        text_matches.len(),
        started.elapsed().as_millis()
    );

    serde_json::json!({ "items": items, "total": total })
}

fn handle_get_item_full(conn: &Connection, id: i64) -> Option<FullClipItem> {
    conn.query_row(
        "SELECT content_type, text_content, image_blob, ocr_text, iv_text, iv_image, iv_ocr, html_content, iv_html FROM clipboard_history WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            let text_ct = get_optional_bytes(row, 1).unwrap_or(None);
            let iv_text = row.get::<_, Option<Vec<u8>>>(4).unwrap_or(None);
            let image_ct = get_optional_bytes(row, 2).unwrap_or(None);
            let iv_image = row.get::<_, Option<Vec<u8>>>(5).unwrap_or(None);
            let ocr_ct = get_optional_bytes(row, 3).unwrap_or(None);
            let iv_ocr = row.get::<_, Option<Vec<u8>>>(6).unwrap_or(None);
            let html_ct = get_optional_bytes(row, 7).unwrap_or(None);
            let iv_html = row.get::<_, Option<Vec<u8>>>(8).unwrap_or(None);
            Ok(FullClipItem {
                content_type: row.get::<_, String>(0).unwrap_or_default(),
                text_content: resolve_optional_text(text_ct, iv_text),
                html_content: resolve_optional_text(html_ct, iv_html),
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
    // Preserve pinned + starred tiers. Users opt items into a tier expecting them
    // to survive a Clear All; only ephemeral history should go.
    if let Err(e) = conn.execute(
        "DELETE FROM clipboard_history WHERE pinned = 0 AND starred = 0",
        [],
    ) {
        error!("[Keyfire] Failed to clear clipboard history: {}", e);
        return false;
    }
    // Reclaim disk space. DELETE alone leaves the file at its high-water mark, and in
    // WAL mode VACUUM alone leaves the .db-wal file large. Both steps are needed:
    //   1. VACUUM         — rebuild .db, freeing pages held by deleted rows.
    //   2. wal_checkpoint — flush WAL into .db and truncate .db-wal back to zero bytes.
    let mut vacuum_ok = true;
    if let Err(e) = conn.execute("VACUUM", []) {
        error!("[Keyfire] VACUUM after clear failed: {}", e);
        vacuum_ok = false;
    }
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        error!("[Keyfire] WAL truncate after clear failed: {}", e);
        vacuum_ok = false;
    }
    if vacuum_ok {
        info!("[Keyfire] Clipboard history cleared, database vacuumed and WAL truncated");
    }
    // Always return true — the table is empty either way; only file size may not have shrunk.
    true
}

fn handle_pin_item(conn: &Connection, id: i64, pinned: bool) -> bool {
    let val: i32 = if pinned { 1 } else { 0 };
    // Unpinning also clears the explicit rank — next pin starts unranked at
    // the bottom of the tier (id DESC tiebreaker), matching the popup default.
    let sql = if pinned {
        "UPDATE clipboard_history SET pinned = ?1 WHERE id = ?2"
    } else {
        "UPDATE clipboard_history SET pinned = ?1, pinned_order = NULL WHERE id = ?2"
    };
    conn.execute(sql, rusqlite::params![val, id]).is_ok()
}

fn handle_star_item(conn: &Connection, id: i64, starred: bool) -> bool {
    let val: i32 = if starred { 1 } else { 0 };
    // Un-saving clears the rank AND the folder assignment — a re-saved item
    // starts unranked at the Saved root, matching the unpin behaviour.
    let sql = if starred {
        "UPDATE clipboard_history SET starred = ?1 WHERE id = ?2"
    } else {
        "UPDATE clipboard_history SET starred = ?1, starred_order = NULL, folder_id = NULL WHERE id = ?2"
    };
    conn.execute(sql, rusqlite::params![val, id]).is_ok()
}

// ── Saved folders ────────────────────────────────────────────────────────────

fn handle_create_folder(conn: &Connection, name: &str) -> Option<i64> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Phase 3a treatment: encrypt the user-typed name with a fresh IV;
    // cipher-unavailable fallback writes plaintext + NULL iv, matching the
    // content columns' legacy on-disk shape.
    let (name_ct, iv_name): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(trimmed.as_bytes()) {
        Some((ct, iv)) => (ct, Some(iv)),
        None => (trimmed.as_bytes().to_vec(), None),
    };
    // Append below existing folders: next sort_order = max + 1.
    let next_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM clipboard_folders",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    match conn.execute(
        "INSERT INTO clipboard_folders (name, iv_name, sort_order) VALUES (?1, ?2, ?3)",
        rusqlite::params![name_ct, iv_name, next_order],
    ) {
        Ok(_) => Some(conn.last_insert_rowid()),
        Err(e) => {
            error!("[Keyfire] Clipboard: create folder failed: {}", e);
            None
        }
    }
}

fn handle_rename_folder(conn: &Connection, id: i64, name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Fresh IV on every rename — never reuse an IV with the same key.
    let (name_ct, iv_name): (Vec<u8>, Option<Vec<u8>>) = match encrypt_blob(trimmed.as_bytes()) {
        Some((ct, iv)) => (ct, Some(iv)),
        None => (trimmed.as_bytes().to_vec(), None),
    };
    matches!(
        conn.execute(
            "UPDATE clipboard_folders SET name = ?1, iv_name = ?2 WHERE id = ?3",
            rusqlite::params![name_ct, iv_name, id],
        ),
        Ok(rows) if rows > 0
    )
}

/// Deleting a folder moves its items back to the Saved root — folder deletion
/// must never delete clipboard content. Transactional so the folder row and
/// its item reassignments can't diverge.
fn handle_delete_folder(conn: &mut Connection, id: i64) -> bool {
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            error!("[Keyfire] Clipboard: delete folder begin transaction failed: {}", e);
            return false;
        }
    };
    if let Err(e) = tx.execute(
        "UPDATE clipboard_history SET folder_id = NULL WHERE folder_id = ?1",
        rusqlite::params![id],
    ) {
        error!("[Keyfire] Clipboard: delete folder unassign failed: {}", e);
        return false;
    }
    if let Err(e) = tx.execute(
        "DELETE FROM clipboard_folders WHERE id = ?1",
        rusqlite::params![id],
    ) {
        error!("[Keyfire] Clipboard: delete folder failed: {}", e);
        return false;
    }
    tx.commit().is_ok()
}

/// Assign an item to a folder (or back to the Saved root with None). Moving
/// into a folder also saves the item if it wasn't already — dragging a
/// timeline item straight into a folder is a save + file in one gesture.
fn handle_move_to_folder(conn: &Connection, id: i64, folder_id: Option<i64>) -> bool {
    // Reject moves into a folder that doesn't exist (stale UI state) so the
    // item can't end up invisibly filed under a dangling folder_id.
    if let Some(fid) = folder_id {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM clipboard_folders WHERE id = ?1",
                rusqlite::params![fid],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            return false;
        }
    }
    let sql = if folder_id.is_some() {
        "UPDATE clipboard_history SET folder_id = ?1, starred = 1 WHERE id = ?2"
    } else {
        "UPDATE clipboard_history SET folder_id = ?1 WHERE id = ?2"
    };
    conn.execute(sql, rusqlite::params![folder_id, id]).is_ok()
}

/// Folder list for the sidebar + Saved section sub-headers:
///   [{ "id": N, "name": "...", "count": M }, ...]
/// count = saved items currently in the folder. Ordered by sort_order (append
/// order), id as tiebreaker.
fn handle_get_folders(conn: &Connection) -> Value {
    let sql = "SELECT f.id, f.name, f.iv_name,
                      (SELECT COUNT(*) FROM clipboard_history h
                        WHERE h.folder_id = f.id AND h.starred = 1) AS cnt
               FROM clipboard_folders f
               ORDER BY COALESCE(f.sort_order, 999999) ASC, f.id ASC";
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            error!("[Keyfire] Clipboard: get folders prepare failed: {}", e);
            return serde_json::json!([]);
        }
    };
    let folders: Vec<Value> = stmt
        .query_map([], |row| {
            // Same iv-NULL plaintext fallback as the content columns; a
            // decrypt failure yields an empty name rather than ciphertext.
            let name_ct = get_optional_bytes(row, 1)?.unwrap_or_default();
            let iv_name = row.get::<_, Option<Vec<u8>>>(2)?;
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "name": resolve_required_text(name_ct, iv_name),
                "count": row.get::<_, i64>(3).unwrap_or(0),
            }))
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    serde_json::json!(folders)
}

/// Rewrites the ranks for one tier (`pinned_order` or `starred_order`) so the
/// passed `ids` slice becomes the visual order (index 0 = top). Single
/// transaction — either every row updates or none, so the tier never enters a
/// partially-reordered state. `column` MUST be a static column name (caller
/// passes a literal), never user input.
fn handle_reorder_tier(conn: &mut Connection, ids: &[i64], column: &str) -> bool {
    if ids.is_empty() {
        return true;
    }
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            error!("[Keyfire] Clipboard: reorder begin transaction failed: {}", e);
            return false;
        }
    };
    {
        let sql = format!("UPDATE clipboard_history SET {} = ?1 WHERE id = ?2", column);
        let mut stmt = match tx.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                error!("[Keyfire] Clipboard: reorder prepare failed: {}", e);
                return false;
            }
        };
        for (rank, id) in ids.iter().enumerate() {
            if let Err(e) = stmt.execute(rusqlite::params![rank as i64, id]) {
                error!("[Keyfire] Clipboard: reorder execute failed at rank {}: {}", rank, e);
                return false;
            }
        }
    }
    if let Err(e) = tx.commit() {
        error!("[Keyfire] Clipboard: reorder commit failed: {}", e);
        return false;
    }
    true
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
         WHERE source_app != '' AND (starred = 1 OR pinned = 1 OR timestamp >= datetime('now', '-{} days'))
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
///   { "dates": [{ "date": "YYYY-MM-DD", "count": N }, ...], "pinned_count": M, "starred_count": K }
/// One row per distinct local-calendar date that has timeline content (not
/// pinned and not starred) within the effective Pro-gated retention window.
/// Pinned + starred items are bucketed separately (sidebar shortcuts ignore age)
/// so they're excluded from the date rows. Per [[feedback_sqlite_localtime_pattern]]
/// we store UTC and convert with DATE(timestamp, 'localtime') for grouping.
fn handle_get_date_buckets(
    conn: &Connection,
    app_filter: Option<&str>,
    tag_filter: Option<&str>,
) -> Value {
    let days = effective_retention_days();

    // Toolbar filters (app, tag) layer on top via positional `?` binds, same
    // pattern as handle_get_history. Search is intentionally NOT applied here —
    // it would force a decrypt-and-scan per refresh and the buckets are meant
    // to be cheap. GROUP BY naturally drops dates with zero matching rows, so
    // the sidebar hides empty dates without extra filtering on the JS side.
    let mut clauses: Vec<String> = vec![
        "pinned = 0".to_string(),
        "starred = 0".to_string(),
        format!("timestamp >= datetime('now', '-{} days')", days),
    ];
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(app) = app_filter.filter(|s| !s.is_empty()) {
        clauses.push("source_app = ?".to_string());
        binds.push(Box::new(app.to_string()));
    }
    if let Some(tag) = tag_filter.filter(|s| !s.is_empty() && *s != "All") {
        clauses.push("content_tag = ?".to_string());
        binds.push(Box::new(tag.to_string()));
    }
    let where_clause = clauses.join(" AND ");

    let dates_sql = format!(
        "SELECT DATE(timestamp, 'localtime') AS local_date, COUNT(*) AS cnt
         FROM clipboard_history
         WHERE {}
         GROUP BY local_date
         ORDER BY local_date DESC",
        where_clause
    );

    let mut stmt = match conn.prepare(&dates_sql) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({ "dates": [], "pinned_count": 0, "starred_count": 0 }),
    };
    let dates_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|p| p.as_ref()).collect();
    let dates: Vec<Value> = stmt
        .query_map(rusqlite::params_from_iter(dates_refs.iter()), |row| {
            Ok(serde_json::json!({
                "date": row.get::<_, String>(0).unwrap_or_default(),
                "count": row.get::<_, i64>(1).unwrap_or(0),
            }))
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    // Pinned + starred counts also respect the active filters so each sidebar
    // bucket reflects what the user would actually see if they clicked it.
    let tier_count = |flag_clause: &str| -> i64 {
        let mut tier_clauses: Vec<String> = vec![flag_clause.to_string()];
        let mut tier_binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(app) = app_filter.filter(|s| !s.is_empty()) {
            tier_clauses.push("source_app = ?".to_string());
            tier_binds.push(Box::new(app.to_string()));
        }
        if let Some(tag) = tag_filter.filter(|s| !s.is_empty() && *s != "All") {
            tier_clauses.push("content_tag = ?".to_string());
            tier_binds.push(Box::new(tag.to_string()));
        }
        let sql = format!(
            "SELECT COUNT(*) FROM clipboard_history WHERE {}",
            tier_clauses.join(" AND ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = tier_binds.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, rusqlite::params_from_iter(refs.iter()), |row| row.get(0))
            .unwrap_or(0)
    };
    let pinned_count = tier_count("pinned = 1");
    let starred_count = tier_count("starred = 1");

    serde_json::json!({ "dates": dates, "pinned_count": pinned_count, "starred_count": starred_count })
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
    // A user text edit produces plain text, so the accompanying HTML fragment
    // (if any) is now stale — clear it so the row pastes as plain going forward.
    match conn.execute(
        "UPDATE clipboard_history SET text_content = ?1, preview = ?2, content_tag = ?3, iv_text = ?4, iv_preview = ?5, html_content = NULL, iv_html = NULL WHERE id = ?6 AND content_type = 'text'",
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
        "DELETE FROM clipboard_history WHERE pinned = 0 AND starred = 0 AND timestamp < datetime('now', '-{} days')",
        days
    );
    match conn.execute(&query, []) {
        Ok(deleted) if deleted > 0 => {
            info!("[Keyfire] Pruned {} expired clipboard items", deleted);
            // Reclaim space — VACUUM rebuilds .db, wal_checkpoint(TRUNCATE) shrinks .db-wal.
            // Both are skipped when nothing was deleted (common case — handle_prune runs
            // after every new clipboard entry).
            if let Err(e) = conn.execute("VACUUM", []) {
                error!("[Keyfire] VACUUM after prune failed: {}", e);
            }
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                error!("[Keyfire] WAL truncate after prune failed: {}", e);
            }
        }
        Ok(_) => {} // nothing pruned — no space to reclaim
        Err(e) => error!("[Keyfire] Prune query failed: {}", e),
    }
}

// ── Clipboard HTML helper ────────────────────────────────────────────────────
//
// The CF_HTML clipboard format is a UTF-8 ASCII header + UTF-8 HTML body in
// one blob. Header keys StartFragment / EndFragment give byte offsets from the
// start of the blob to the meaningful fragment (the bit between the
// `<!--StartFragment-->` and `<!--EndFragment-->` markers that Windows expects
// paste targets to consume). Anything outside the fragment is boilerplate
// wrapper we deliberately strip — expansions::build_cf_html rebuilds a valid
// wrapper at paste time from the raw fragment, so re-storing the wrapper here
// would just double-nest it on paste.
//
// Caller MUST hold the clipboard open (OpenClipboard succeeded, not yet
// CloseClipboard'd). Returns None when CF_HTML isn't available, when GlobalLock
// fails, or when the header is malformed / fragment offsets are nonsense.

/// Parse the "Key:number" header pattern out of a CF_HTML blob. Returns the
/// number after the given key, or None if missing / unparseable. Offsets are
/// zero-padded 10-digit decimals in practice but the parser is lenient:
/// scans forward past the ':' and reads consecutive ASCII digits.
fn parse_cf_html_header_offset(header: &[u8], key: &str) -> Option<usize> {
    let needle = format!("{}:", key);
    let pos = header
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())?;
    let mut i = pos + needle.len();
    while i < header.len() && (header[i] == b' ' || header[i] == b'\t') {
        i += 1;
    }
    let start = i;
    while i < header.len() && header[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    std::str::from_utf8(&header[start..i]).ok()?.parse().ok()
}

unsafe fn read_clipboard_html() -> Option<String> {
    let format_id = crate::expansions::cf_html_format_id();
    if IsClipboardFormatAvailable(format_id) == 0 {
        return None;
    }
    let handle = GetClipboardData(format_id);
    if handle.is_null() {
        return None;
    }
    let size = GlobalSize(handle);
    // A well-formed CF_HTML wrapper is at least 100 bytes just for the header —
    // if the blob is smaller than that, StartFragment/EndFragment can't fit.
    if size < 100 {
        return None;
    }
    let ptr = GlobalLock(handle) as *const u8;
    if ptr.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
    GlobalUnlock(handle);

    // Only the first ~500 bytes are header. Restrict the offset search to that
    // window so a StartFragment-like substring appearing in the HTML body
    // itself can't hijack the parse.
    let header_scan_len = std::cmp::min(bytes.len(), 512);
    let start_fragment = parse_cf_html_header_offset(&bytes[..header_scan_len], "StartFragment")?;
    let end_fragment = parse_cf_html_header_offset(&bytes[..header_scan_len], "EndFragment")?;
    if end_fragment <= start_fragment || end_fragment > bytes.len() {
        return None;
    }
    let fragment_bytes = &bytes[start_fragment..end_fragment];
    // CF_HTML is defined as UTF-8. Non-UTF-8 input is treated as corrupt and
    // falls back to plain-text-only capture.
    std::str::from_utf8(fragment_bytes).ok().map(|s| s.to_string())
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
        let class_name: Vec<u16> = "KEYFIREClipboardListener\0".encode_utf16().collect();
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
            error!("[Keyfire] Failed to register clipboard window class");
            return;
        }

        let hwnd = CreateWindowExW(
            0, class_name.as_ptr(), std::ptr::null(), WS_OVERLAPPED,
            0, 0, 0, 0, HWND_MESSAGE,
            std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null(),
        );
        if hwnd.is_null() {
            error!("[Keyfire] Failed to create clipboard message-only window");
            return;
        }
        if AddClipboardFormatListener(hwnd) == 0 {
            error!("[Keyfire] Failed to add clipboard format listener");
            DestroyWindow(hwnd);
            return;
        }

        info!("[Keyfire] Clipboard listener started (message-only HWND)");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
            DispatchMessageW(&msg);
        }

        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
        info!("[Keyfire] Clipboard listener stopped");
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

/// Registered clipboard formats that mark a copy as private. Password managers
/// (1Password, Bitwarden, KeePass) and "copy without history" features stamp
/// these alongside the real content so clipboard monitors skip the copy —
/// Windows' own Win+V history honours them, and so must we.
///
/// - `ExcludeClipboardContentFromMonitorProcessing` — presence alone means skip.
/// - `Clipboard Viewer Ignore` — older convention (KeePass etc.), presence = skip.
/// - `CanIncludeInClipboardHistory` — value semantics: a DWORD 0 means exclude
///   (a 1 explicitly ALLOWS history, so presence alone is not a skip).
fn privacy_format_atoms() -> (u32, u32, u32) {
    static ATOMS: std::sync::OnceLock<(u32, u32, u32)> = std::sync::OnceLock::new();
    *ATOMS.get_or_init(|| {
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        unsafe {
            (
                RegisterClipboardFormatW(wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr()),
                RegisterClipboardFormatW(wide("Clipboard Viewer Ignore").as_ptr()),
                RegisterClipboardFormatW(wide("CanIncludeInClipboardHistory").as_ptr()),
            )
        }
    })
}

/// True when the current clipboard contents are marked private by the source
/// app. MUST be called with the clipboard open (between OpenClipboard and
/// CloseClipboard). Never logs content.
unsafe fn clipboard_marked_private() -> bool {
    let (exclude, viewer_ignore, can_include) = privacy_format_atoms();

    if exclude != 0 && IsClipboardFormatAvailable(exclude) != 0 {
        return true;
    }
    if viewer_ignore != 0 && IsClipboardFormatAvailable(viewer_ignore) != 0 {
        return true;
    }
    if can_include != 0 && IsClipboardFormatAvailable(can_include) != 0 {
        let handle = GetClipboardData(can_include);
        if !handle.is_null() {
            let ptr = GlobalLock(handle) as *const u32;
            if !ptr.is_null() {
                let allowed = *ptr != 0;
                GlobalUnlock(handle);
                if !allowed {
                    return true;
                }
            }
        } else {
            // Format advertised but unreadable — treat as private rather than
            // risk capturing something the source app tried to protect.
            return true;
        }
    }
    false
}

fn handle_clipboard_update() {
    // Skip Keyfire's own injected writes. Two layers: the level flag covers the
    // synchronous write window, and the per-write sequence-number record covers
    // the async tail (a WM_CLIPBOARDUPDATE delivered after the flag was cleared —
    // the H3 leak). A real user copy, or a `Copy to Clipboard` macro step (the
    // target app performs that copy), has a seqnum Keyfire never recorded, so it is
    // always still captured. Checked first so the self-seqnum is consumed even
    // when a later gate (capture-off / excluded app) would return early.
    let cur_seq = crate::expansions::clipboard_sequence_number();
    let was_self = crate::actions::is_self_clipboard_seq(cur_seq);
    let was_suppress = crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.load(Ordering::SeqCst);

    if was_self || was_suppress {
        return;
    }

    // Master capture toggle. When off, the listener keeps running so re-enabling
    // takes effect on the very next clipboard event without restarting Keyfire.
    if !CAPTURE_ENABLED.load(Ordering::SeqCst) {
        return;
    }

    // App exclusion list: skip capture when the user has opted out of recording
    // clipboard from this process. Comparison is case-insensitive and ignores
    // the `.exe` suffix on both sides.
    // Resolve foreground process ONCE — used for both the exclusion filter
    // and (Pro) the source_app + source_app_path capture below.
    let (fg_name, fg_path) = get_foreground_process_info();
    if !fg_name.is_empty() && is_app_excluded(&fg_name) {
        return;
    }

    // Capture source app for the row (Pro feature — Free users get empty source).
    // v0.8.4: also capture the full exe path so the frontend can render the
    // app's real icon in the list. Path is encrypted at write time — see the
    // source_app_path column comment in open_clipboard_db.
    let (source_app, source_app_path) = if crate::licence::is_pro() {
        (fg_name, fg_path)
    } else {
        (String::new(), String::new())
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
            log::warn!("[Keyfire] Clipboard: OpenClipboard still locked after 10 attempts — copy not captured");
            return;
        }

        // Source app marked this copy private (password managers, "copy
        // without history"). Same treatment Win+V gives it: never captured.
        if clipboard_marked_private() {
            CloseClipboard();
            log::info!("[Keyfire] Clipboard: copy marked private by source app — not captured");
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
                    if *last == hash {
                        return;
                    }
                    *last = hash;
                }

                send_entry(ClipEntry {
                    content_type: "image".to_string(),
                    text_content: None,
                    html_content: None,
                    image_blob: Some(png_bytes),
                    image_width: width,
                    image_height: height,
                    preview: format!("{}×{} image", width, height),
                    source_app: source_app.clone(),
                    source_app_path: source_app_path.clone(),
                    content_tag: "Image".to_string(),
                });
                return;
            }
            log::warn!("[Keyfire] Clipboard: image read failed (DIB advertised) — falling through to text");
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
                log::warn!("[Keyfire] Clipboard: CF_UNICODETEXT advertised but GetClipboardData returned null — copy not captured");
            } else {
                let ptr = GlobalLock(handle) as *const u16;
                if ptr.is_null() {
                    log::warn!("[Keyfire] Clipboard: GlobalLock failed on text handle — copy not captured");
                } else {
                    let mut len = 0usize;
                    while *ptr.add(len) != 0 { len += 1; }
                    let slice = std::slice::from_raw_parts(ptr, len);
                    let text = String::from_utf16_lossy(slice);
                    GlobalUnlock(handle);

                    // Read CF_HTML *before* CloseClipboard — one clipboard-open
                    // covers both format reads. Rich-text sources (Word, Outlook,
                    // Chrome, Slack composer, Notion) put CF_UNICODETEXT +
                    // CF_HTML on the clipboard together; capturing the HTML now
                    // lets paste_clipboard_item reproduce bullets, links, bold
                    // and colour instead of stripping to plain text. None on
                    // any failure — plain text is still authoritative.
                    let html_fragment = read_clipboard_html();
                    CloseClipboard();

                    if text.trim().is_empty() { return; }

                    let hash = compute_hash(text.as_bytes());
                    {
                        let mut last = last_hash().lock().unwrap();
                        if *last == hash {
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
                        html_content: html_fragment,
                        image_blob: None,
                        image_width: 0,
                        image_height: 0,
                        preview,
                        source_app,
                        source_app_path,
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
