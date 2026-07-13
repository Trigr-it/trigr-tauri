//! Non-Windows stub — no system volume control.
#![allow(dead_code, unused_variables)]

pub fn set_master_volume_scalar(_scalar: f32) -> bool { false }
pub fn get_master_volume_scalar() -> Option<f32> { None }
pub fn adjust_master_volume(_delta_units: i32) -> bool { false }
pub fn set_master_mute(_mute: bool) -> bool { false }
pub fn get_master_mute() -> Option<bool> { None }
pub fn toggle_master_mute() -> bool { false }
