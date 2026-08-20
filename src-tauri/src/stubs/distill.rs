// Mac stub — distillation engine is Windows-only for the initial ship.
// Returns an empty Vec so the UI can compile and render on Mac; distilled
// playback is unavailable there.

use crate::recorder::RecordedEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetWindow {
    pub title: String,
    pub exe: String,
    pub class: String,
    pub client_w: i32,
    pub client_h: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DistilledStep {
    TypeText { text: String },
    PressKey { vks: Vec<u32>, #[serde(default)] cursor: Option<(i32, i32)> },
    #[serde(rename_all = "camelCase")]
    ClickAtPosition {
        x_abs: i32,
        y_abs: i32,
        x_rel: Option<i32>,
        y_rel: Option<i32>,
        button: String,
        target_window: Option<TargetWindow>,
        #[serde(default)]
        hold_ms: u64,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Drag {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        button: String,
        hold_ms: u64,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    Scroll { delta: i32, x: i32, y: i32 },
    FocusWindow { title: String, exe: String },
    Wait { ms: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetApp {
    pub exe: String,
    #[serde(default)]
    pub window_title_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordMacroValue {
    pub events: Vec<RecordedEvent>,
    #[serde(default)]
    pub distilled: Option<Vec<serde_json::Value>>,
    #[serde(default = "default_playback_mode")]
    pub playback_mode: String,
    #[serde(default)]
    pub target_app: Option<TargetApp>,
    #[serde(default)]
    pub disable_target_binding: bool,
}

fn default_playback_mode() -> String {
    "raw".into()
}

pub fn parse_record_macro_value(json: &str) -> Option<RecordMacroValue> {
    let trimmed = json.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_str::<RecordMacroValue>(json).ok()
    } else if trimmed.starts_with('[') {
        let events: Vec<RecordedEvent> = serde_json::from_str(json).ok()?;
        Some(RecordMacroValue {
            events,
            distilled: None,
            playback_mode: "raw".into(),
            target_app: None,
            disable_target_binding: false,
        })
    } else {
        None
    }
}

pub fn distill(_events: &[RecordedEvent]) -> Vec<serde_json::Value> {
    Vec::new()
}

pub fn extract_target_app(_events: &[RecordedEvent]) -> Option<TargetApp> {
    None
}

pub fn resolve_target_window(_target: &TargetWindow) -> Option<isize> {
    None
}

pub fn client_to_screen(_hwnd: isize, _rel_x: i32, _rel_y: i32) -> Option<(i32, i32)> {
    None
}
