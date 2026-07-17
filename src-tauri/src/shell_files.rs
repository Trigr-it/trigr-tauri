//! Explorer-integrated file management for the Files macro steps
//! (Create Folder / Copy Files / Move Files).
//!
//! Two capabilities live here:
//!
//! 1. `explorer_context()` — reads the folder path and selected items of a
//!    File Explorer window (or the desktop) via the IShellWindows COM
//!    collection hosted by explorer.exe. Same chain AutoHotkey and PowerToys
//!    use: ShellWindows → IServiceProvider → SID_STopLevelBrowser →
//!    IShellBrowser → active IShellView → folder path + SVGIO_SELECTION.
//!
//! 2. `transfer_files()` — copy/move through IFileOperation so users get the
//!    native shell semantics: progress dialog on big transfers, the standard
//!    Replace / Skip / Keep-both conflict prompt, Recycle-Bin undo,
//!    cross-volume moves, and OneDrive placeholder hydration. Raw std::fs
//!    would silently clobber on name collisions — wrong default for a
//!    user-facing tool.
//!
//! Only real Explorer windows (and the desktop) register in ShellWindows —
//! third-party file managers (Total Commander, Files) and file-open dialogs
//! don't, so "current folder" / "selected files" modes fail with a log line
//! there and the macro aborts rather than acting on the wrong folder.
//!
//! `windows` crate (not windows-sys) for the same reason as volume.rs: the
//! shell COM interfaces don't exist in windows-sys. CoInit is idempotent on
//! the calling (macro executor) thread; the S_FALSE from repeat init is
//! ignored, matching the volume.rs pattern.

use log::{info, warn};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IServiceProvider, CLSCTX_ALL,
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IFolderView, IShellBrowser, IShellItem, IShellItemArray,
    IShellView, IShellWindows, SHCreateItemFromParsingName, ShellWindows, FILEOPERATION_FLAGS,
    SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetForegroundWindow, IsWindowVisible, GA_ROOT,
};

/// What the user is looking at in File Explorer: the open folder (None for
/// virtual locations like This PC that have no filesystem path) and the
/// filesystem paths of the current selection (files AND folders; virtual
/// items are skipped).
pub struct ExplorerContext {
    pub folder: Option<String>,
    pub selected: Vec<String>,
}

fn co_init() {
    // Idempotent — returns S_FALSE if already initialised on this thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Filesystem path of a shell item, or None for virtual items (This PC,
/// zip-folder contents, Control Panel, ...).
fn item_fs_path(item: &IShellItem) -> Option<String> {
    unsafe {
        match item.GetDisplayName(SIGDN_FILESYSPATH) {
            Ok(pw) => {
                let s = pw.to_string().ok();
                CoTaskMemFree(Some(pw.0 as *const _));
                s.filter(|p| !p.is_empty())
            }
            Err(_) => None,
        }
    }
}

/// Folder path + selection out of an active shell view.
fn view_context(view: &IShellView) -> ExplorerContext {
    let folder = view
        .cast::<IFolderView>()
        .ok()
        .and_then(|fv| unsafe { fv.GetFolder::<IShellItem>().ok() })
        .and_then(|item| item_fs_path(&item));

    // GetItemObject errors when the selection is empty — treat as no selection.
    let mut selected = Vec::new();
    if let Ok(arr) = unsafe { view.GetItemObject::<IShellItemArray>(SVGIO_SELECTION) } {
        if let Ok(count) = unsafe { arr.GetCount() } {
            for i in 0..count {
                if let Ok(item) = unsafe { arr.GetItemAt(i) } {
                    if let Some(p) = item_fs_path(&item) {
                        selected.push(p);
                    }
                }
            }
        }
    }
    ExplorerContext { folder, selected }
}

/// True when `hwnd` is the desktop's top-level window (what
/// GetForegroundWindow returns while desktop icons have focus).
fn is_desktop_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let mut buf = [0u16; 32];
    let len = unsafe { GetClassNameW(hwnd as _, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..len as usize]);
    class == "Progman" || class == "WorkerW"
}

/// Locate the Explorer window the user is working in and read its folder +
/// selection. `hwnd_hint` is the window that had focus when the macro
/// trigger fired (execute_macro_step's target_hwnd) — preferred over the
/// live foreground window so overlay-triggered macros (radial menu) still
/// resolve the Explorer window the user was in. Falls back to the desktop
/// when that's what's focused. None when neither matches an Explorer view.
pub fn explorer_context(hwnd_hint: isize) -> Option<ExplorerContext> {
    co_init();
    let fg = unsafe { GetForegroundWindow() } as isize;

    unsafe {
        let shell: IShellWindows = match CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
        {
            Ok(s) => s,
            Err(e) => {
                warn!("[Keyfire] shell_files: ShellWindows CoCreateInstance failed: {}", e);
                return None;
            }
        };

        let count = shell.Count().unwrap_or(0);
        // Two passes: exact match on the trigger-time window first, then the
        // live foreground window. Avoids grabbing an arbitrary Explorer
        // window when several are open.
        for wanted in [hwnd_hint, fg] {
            if wanted == 0 {
                continue;
            }
            for i in 0..count {
                let Ok(disp) = shell.Item(&VARIANT::from(i)) else { continue };
                let Ok(sp) = disp.cast::<IServiceProvider>() else { continue };
                let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                    continue;
                };
                let Ok(hwnd) = browser.GetWindow() else { continue };
                // Windows 11 tabbed Explorer: the shell browser's window is
                // a ShellTabWindowClass CHILD of the CabinetWClass frame,
                // while GetForegroundWindow returns the frame — so compare
                // top-level ancestors, not raw handles. Background tabs
                // share the same frame but their tab window is hidden, so
                // visibility picks the active tab.
                let tab = hwnd.0 as isize;
                let root = GetAncestor(tab as _, GA_ROOT) as isize;
                if tab != wanted && root != wanted {
                    continue;
                }
                if IsWindowVisible(tab as _) == 0 {
                    continue;
                }
                let Ok(view) = browser.QueryActiveShellView() else { continue };
                return Some(view_context(&view));
            }
        }

        // Desktop: its icons live in a shell view too, reachable via
        // FindWindowSW with SWC_DESKTOP rather than the window enumeration.
        if is_desktop_window(hwnd_hint) || is_desktop_window(fg) {
            let loc = VARIANT::from(0i32); // CSIDL_DESKTOP
            let root = VARIANT::default();
            let mut hwnd_out: i32 = 0;
            if let Ok(disp) =
                shell.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd_out, SWFO_NEEDDISPATCH)
            {
                if let Ok(sp) = disp.cast::<IServiceProvider>() {
                    if let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) {
                        if let Ok(view) = browser.QueryActiveShellView() {
                            return Some(view_context(&view));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Strip characters Windows forbids in path components and reject traversal.
/// `name` may contain `/` or `\` separators — each segment is sanitised so
/// "Invoices/{year}" style nested names work with create_dir_all.
fn sanitise_folder_name(name: &str) -> String {
    let segments: Vec<String> = name
        .split(['/', '\\'])
        .map(|seg| {
            seg.chars()
                .filter(|c| !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
                .collect::<String>()
                .trim()
                .trim_end_matches('.')
                .to_string()
        })
        .filter(|seg| !seg.is_empty() && seg != "." && seg != "..")
        .collect();
    segments.join("\\")
}

/// Resolve `name` as a subfolder of `base` for a transfer destination.
/// `create` = create it when missing; otherwise a missing subfolder is an
/// Err so the macro can abort — the "only file into .\Superceded\ when that
/// folder exists here" workflow.
pub fn resolve_subfolder(base: &str, name: &str, create: bool) -> Result<String, String> {
    let clean = sanitise_folder_name(name);
    if clean.is_empty() {
        return Err(format!("subfolder name '{}' is empty after sanitising", name));
    }
    let full = std::path::Path::new(base).join(&clean);
    if full.is_dir() {
        return Ok(full.to_string_lossy().into_owned());
    }
    if !create {
        return Err(format!("subfolder '{}' not found in {}", clean, base));
    }
    std::fs::create_dir_all(&full)
        .map_err(|e| format!("create {} failed: {}", full.display(), e))?;
    Ok(full.to_string_lossy().into_owned())
}

/// Create `parent\name` (nested names allowed). Idempotent — an existing
/// folder is a success, matching create_dir_all semantics. Returns the full
/// path created.
pub fn create_folder(parent: &str, name: &str) -> Result<String, String> {
    let clean = sanitise_folder_name(name);
    if clean.is_empty() {
        return Err(format!("folder name '{}' is empty after sanitising", name));
    }
    let full = std::path::Path::new(parent).join(&clean);
    std::fs::create_dir_all(&full)
        .map_err(|e| format!("create_dir_all {} failed: {}", full.display(), e))?;
    Ok(full.to_string_lossy().into_owned())
}

/// Resolve `{inc}` / `{inc:N}` tokens in a folder/file name against what
/// already exists in `parent`: scan for entries matching the name with the
/// token as a number, take max+1 (1 when nothing matches). `{inc:3}`
/// zero-pads to 3 digits ("Report {inc:3}" → "Report 007"). All token
/// occurrences get the same number. Names without the token pass through.
pub fn resolve_increment(parent: &str, name: &str) -> String {
    let Ok(token_re) = regex_lite::Regex::new(r"\{inc(?::(\d+))?\}") else {
        return name.to_string();
    };
    let Some(caps) = token_re.captures(name) else {
        return name.to_string();
    };
    let pad: usize = caps.get(1).and_then(|g| g.as_str().parse().ok()).unwrap_or(0);

    // Build a matcher for existing entries: literal name text with every
    // token occurrence matching digits (first one captured).
    let mut pattern = String::from("^");
    let mut rest = name;
    let mut first = true;
    while let Some(m) = token_re.find(rest) {
        pattern.push_str(&regex_escape(&rest[..m.start()]).to_lowercase());
        pattern.push_str(if first { "(\\d+)" } else { "\\d+" });
        first = false;
        rest = &rest[m.end()..];
    }
    pattern.push_str(&regex_escape(rest).to_lowercase());
    pattern.push('$');

    let mut max_n: u64 = 0;
    if let Ok(matcher) = regex_lite::Regex::new(&pattern) {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_lowercase();
                if let Some(c) = matcher.captures(&fname) {
                    if let Some(n) = c.get(1).and_then(|g| g.as_str().parse::<u64>().ok()) {
                        max_n = max_n.max(n);
                    }
                }
            }
        }
    }
    let next = max_n + 1;
    let formatted = if pad > 0 {
        format!("{:0width$}", next, width = pad)
    } else {
        next.to_string()
    };
    token_re.replace_all(name, formatted.as_str()).into_owned()
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(c, '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Top-level entries (files AND folders) of `dir`, as full paths. Used to
/// seed a newly created folder from a template folder — IFileOperation
/// copies folder entries recursively.
pub fn list_dir_entries(dir: &str) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            warn!("[Keyfire] shell_files: read_dir {} failed: {}", dir, e);
            Vec::new()
        }
    }
}

/// Case-insensitive `*` / `?` wildcard match.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Files (not subfolders) in `dir` whose names match any of the
/// `;`-separated wildcard patterns ("*.pdf; report-?.docx"). Empty pattern
/// list means match everything.
pub fn list_matching_files(dir: &str, patterns: &str) -> Vec<String> {
    let pats: Vec<String> = patterns
        .split(';')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("[Keyfire] shell_files: read_dir {} failed: {}", dir, e);
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_lowercase();
        if pats.is_empty() || pats.iter().any(|p| wildcard_match(p, &fname)) {
            out.push(path.to_string_lossy().into_owned());
        }
    }
    out
}

/// Breadth-first search under `root` (directories only, `max_depth` levels —
/// root's children are depth 1) for the first directory whose NAME contains
/// `key`, case-insensitively. BFS so shallow matches win over deep ones:
/// project folders are usually direct children of the base. Used by the
/// Sort Files step ("PRJ042" → "[PRJ042] Acme Office Fit-Out").
pub fn find_folder_by_key(root: &str, key: &str, max_depth: u32) -> Option<String> {
    let key_lc = key.to_lowercase();
    if key_lc.is_empty() {
        return None;
    }
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, u32)> =
        std::collections::VecDeque::new();
    queue.push_back((std::path::PathBuf::from(root), 0));
    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if entry.file_name().to_string_lossy().to_lowercase().contains(&key_lc) {
                return Some(path.to_string_lossy().into_owned());
            }
            queue.push_back((path, depth + 1));
        }
    }
    None
}

/// One planned Sort Files move: `src` into `dest_dir`, optionally renamed on
/// arrival (`new_name` carries the timestamp-suffixed name for collisions).
pub struct PlannedMove {
    pub src: String,
    pub dest_dir: String,
    pub new_name: Option<String>,
}

/// Execute a batch of per-item-destination moves as ONE IFileOperation —
/// one progress dialog, one undo unit. Same flag/cancel semantics as
/// transfer_files. Returns the number of items queued.
pub fn perform_moves(moves: &[PlannedMove]) -> Result<usize, String> {
    co_init();
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| format!("FileOperation CoCreateInstance failed: {}", e))?;
        let _ = op.SetOperationFlags(FILEOPERATION_FLAGS(0x40 | 0x200));

        // Destination folders repeat across items (many files → one project
        // folder) — cache the IShellItems. Clone is an AddRef, cheap.
        let mut dest_cache: std::collections::HashMap<String, IShellItem> =
            std::collections::HashMap::new();
        let mut queued = 0usize;
        for m in moves {
            let dest = match dest_cache.get(&m.dest_dir) {
                Some(i) => i.clone(),
                None => match shell_item(&m.dest_dir) {
                    Ok(i) => {
                        dest_cache.insert(m.dest_dir.clone(), i.clone());
                        i
                    }
                    Err(e) => {
                        warn!("[Keyfire] shell_files: destination unavailable — {}", e);
                        continue;
                    }
                },
            };
            let item = match shell_item(&m.src) {
                Ok(i) => i,
                Err(e) => {
                    warn!("[Keyfire] shell_files: source unavailable — {}", e);
                    continue;
                }
            };
            let r = match &m.new_name {
                Some(n) => op.MoveItem(&item, &dest, &HSTRING::from(n.as_str()), None),
                None => op.MoveItem(&item, &dest, PCWSTR::null(), None),
            };
            match r {
                Ok(()) => queued += 1,
                Err(e) => warn!("[Keyfire] shell_files: queue {} failed: {}", m.src, e),
            }
        }
        if queued == 0 {
            return Err("no items could be queued".to_string());
        }
        if let Err(e) = op.PerformOperations() {
            if (e.code().0 as u32) == 0x80270000 {
                info!("[Keyfire] shell_files: sort cancelled by user");
                return Ok(queued);
            }
            return Err(format!("PerformOperations failed: {}", e));
        }
        if op.GetAnyOperationsAborted().map(|b| b.as_bool()).unwrap_or(false) {
            info!("[Keyfire] shell_files: some operations were skipped/cancelled by user");
        }
        Ok(queued)
    }
}

fn shell_item(path: &str) -> Result<IShellItem, String> {
    unsafe {
        SHCreateItemFromParsingName(&HSTRING::from(path), None)
            .map_err(|e| format!("{}: {}", path, e))
    }
}

/// Copy or move `sources` into `dest_dir` via IFileOperation. Blocks until
/// the operation completes (the shell pumps its own progress dialog).
/// Returns the number of items queued; Err when nothing could be queued or
/// the operation itself failed. User-cancelled counts as Ok — the shell
/// already told them.
pub fn transfer_files(sources: &[String], dest_dir: &str, is_move: bool) -> Result<usize, String> {
    co_init();
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| format!("FileOperation CoCreateInstance failed: {}", e))?;
        // 0x40 = FOF_ALLOWUNDO (Recycle-Bin undo), 0x200 = FOF_NOCONFIRMMKDIR
        // (silently create the destination chain). Progress + conflict
        // dialogs stay on — that's the point of using IFileOperation.
        let _ = op.SetOperationFlags(FILEOPERATION_FLAGS(0x40 | 0x200));

        let dest = shell_item(dest_dir)?;
        let mut queued = 0usize;
        for src in sources {
            // Ctrl+A selections can include the destination folder itself —
            // copying a folder into itself raises a shell error dialog.
            if src.eq_ignore_ascii_case(dest_dir) {
                warn!(
                    "[Keyfire] shell_files: skipping {} — it is the destination folder",
                    src
                );
                continue;
            }
            match shell_item(src) {
                Ok(item) => {
                    let r = if is_move {
                        op.MoveItem(&item, &dest, None, None)
                    } else {
                        op.CopyItem(&item, &dest, None, None)
                    };
                    match r {
                        Ok(()) => queued += 1,
                        Err(e) => warn!("[Keyfire] shell_files: queue {} failed: {}", src, e),
                    }
                }
                Err(e) => warn!("[Keyfire] shell_files: source unavailable — {}", e),
            }
        }
        if queued == 0 {
            return Err("no items could be queued".to_string());
        }
        if let Err(e) = op.PerformOperations() {
            // 0x80270000 range = COPYENGINE_E_USER_CANCELLED and friends —
            // the user closed the dialog; not a step failure.
            if (e.code().0 as u32) == 0x80270000 {
                info!("[Keyfire] shell_files: transfer cancelled by user");
                return Ok(queued);
            }
            return Err(format!("PerformOperations failed: {}", e));
        }
        if op.GetAnyOperationsAborted().map(|b| b.as_bool()).unwrap_or(false) {
            info!("[Keyfire] shell_files: some operations were skipped/cancelled by user");
        }
        Ok(queued)
    }
}
