//! Non-Windows stub — no Explorer integration or shell file operations.
//! Mac implementation will read Finder's front window + selection via
//! Apple Events (requires the Automation TCC permission) when the engine
//! port reaches this module.
#![allow(dead_code, unused_variables)]

pub struct ExplorerContext {
    pub folder: Option<String>,
    pub selected: Vec<String>,
}

pub fn explorer_context(_hwnd_hint: isize) -> Option<ExplorerContext> {
    None
}

pub fn create_folder(_parent: &str, _name: &str) -> Result<String, String> {
    Err("not supported on this platform".to_string())
}

pub fn resolve_subfolder(_base: &str, _name: &str, _create: bool) -> Result<String, String> {
    Err("not supported on this platform".to_string())
}

pub fn resolve_increment(_parent: &str, name: &str) -> String {
    name.to_string()
}

pub fn list_dir_entries(_dir: &str) -> Vec<String> {
    Vec::new()
}

pub fn list_matching_files(_dir: &str, _patterns: &str) -> Vec<String> {
    Vec::new()
}

pub fn transfer_files(_sources: &[String], _dest_dir: &str, _is_move: bool) -> Result<usize, String> {
    Err("not supported on this platform".to_string())
}

pub fn find_folder_by_key(_root: &str, _key: &str, _max_depth: u32) -> Option<String> {
    None
}

pub struct PlannedMove {
    pub src: String,
    pub dest_dir: String,
    pub new_name: Option<String>,
}

pub fn perform_moves(_moves: &[PlannedMove], _silent_overwrite: bool) -> Result<usize, String> {
    Err("not supported on this platform".to_string())
}
