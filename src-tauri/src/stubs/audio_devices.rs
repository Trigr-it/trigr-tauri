//! Non-Windows stub — no audio device switching.
#![allow(dead_code, unused_variables)]

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct AudioOutputDevice {
    pub id: String,
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", content = "message")]
pub enum SetOutputError {
    #[serde(rename = "device_not_found")]
    DeviceNotFound(String),
    #[serde(rename = "policy_config_failed")]
    PolicyConfigFailed(String),
    #[serde(rename = "com_error")]
    ComError(String),
}

impl std::fmt::Display for SetOutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "audio device switching unavailable on this platform")
    }
}

pub fn list_output_devices() -> Vec<AudioOutputDevice> { Vec::new() }

pub fn set_default_output_device(_device_id: &str) -> Result<String, SetOutputError> {
    Err(SetOutputError::ComError("unsupported on this platform".to_string()))
}
