//! Non-Windows stub — no system volume control.
#![allow(dead_code, unused_variables)]

pub fn set_master_volume_scalar(_scalar: f32) -> bool { false }
pub fn get_master_volume_scalar() -> Option<f32> { None }
