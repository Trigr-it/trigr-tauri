//! App-specific profile templates.
//!
//! Content lives in `resources/app-profile-templates.json` (compile-time
//! embedded). Each template describes one app (`exe`), a set of assignments
//! keyed `"<Modifier>::<KeyId>"` (the profile name is prefixed by the frontend
//! on import) and an optional per-profile radial wheel. This module only adds
//! the *detection* layer: which of those exes exist on this machine, resolved
//! through the same App Paths / running-process / System32 chain the Open App
//! action uses. The frontend (AppProfilesModal, TemplatesPanel) does the import
//! against live React state so the usual save + engine-sync paths run.

use serde_json::Value;

const TEMPLATES_JSON: &str = include_str!("../resources/app-profile-templates.json");

/// Parsed `templates` array, or empty on a parse error (logged once per call;
/// the JSON is a checked-in resource so this should never fire in the field).
fn templates() -> Vec<Value> {
    match serde_json::from_str::<Value>(TEMPLATES_JSON) {
        Ok(v) => v
            .get("templates")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default(),
        Err(e) => {
            log::error!("[Keyfire] app-profile-templates.json parse error: {}", e);
            Vec::new()
        }
    }
}

#[cfg(windows)]
fn detect(exe: &str) -> Option<String> {
    crate::resolve_exe_path_for_name(exe)
}

#[cfg(not(windows))]
fn detect(_exe: &str) -> Option<String> {
    None
}

/// Every template with two extra fields: `installed` (bool) and `path` (the
/// resolved exe path, or null). Blocking (registry + process walk per exe);
/// the command wrapper in lib.rs runs it on `spawn_blocking`.
pub fn list_with_detection() -> Value {
    let mut out = Vec::new();
    for mut t in templates() {
        let exe = t.get("exe").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let path = if exe.is_empty() { None } else { detect(&exe) };
        if let Some(obj) = t.as_object_mut() {
            obj.insert("installed".to_string(), Value::Bool(path.is_some()));
            obj.insert(
                "path".to_string(),
                path.map(Value::String).unwrap_or(Value::Null),
            );
        }
        out.push(t);
    }
    let found = out.iter().filter(|t| t.get("installed") == Some(&Value::Bool(true))).count();
    log::info!("[Keyfire] App profile templates: {} of {} apps installed", found, out.len());
    Value::Array(out)
}
