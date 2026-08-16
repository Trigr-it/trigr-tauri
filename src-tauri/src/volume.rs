//! System master audio volume control via IAudioEndpointVolume COM.
//! Used by the Change Volume macro step.
//!
//! Each public call re-acquires the endpoint interface — cheap (single-digit
//! ms) and avoids caching an IAudioEndpointVolume pointer across audio-device
//! swaps (headphones plugged in / out, default endpoint changed). CoInit is
//! idempotent on the calling thread; the S_FALSE from repeat init is ignored.
//!
//! `windows` crate 0.61 (rather than windows-sys 0.59) is used here because
//! windows-sys doesn't expose the audio COM interfaces at all — only the raw
//! CoCreateInstance / CoInitializeEx functions. The `windows` crate is already
//! a transitive dep so binary-size cost is essentially zero.

use log::warn;

use windows::Win32::Media::Audio::{
    eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};

/// Acquire an IAudioEndpointVolume for the default render endpoint.
/// Returns None on any COM failure (missing audio device, denied by policy,
/// etc.) — callers log a warn! and treat the step as no-op.
fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    unsafe {
        // Idempotent — returns S_FALSE if already initialised on this thread.
        // Return code intentionally ignored via `_ =` (must_use lint suppression).
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER) {
                Ok(e) => e,
                Err(e) => {
                    warn!("[Keyfire] volume: MMDeviceEnumerator CoCreateInstance failed: {}", e);
                    return None;
                }
            };
        let device = match enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
            Ok(d) => d,
            Err(e) => {
                warn!("[Keyfire] volume: GetDefaultAudioEndpoint failed: {}", e);
                return None;
            }
        };
        match device.Activate::<IAudioEndpointVolume>(CLSCTX_INPROC_SERVER, None) {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("[Keyfire] volume: IAudioEndpointVolume Activate failed: {}", e);
                None
            }
        }
    }
}

/// Set master volume to an exact scalar in [0.0, 1.0].
pub fn set_master_volume_scalar(scalar: f32) -> bool {
    let scalar = scalar.clamp(0.0, 1.0);
    let Some(vol) = endpoint_volume() else { return false; };
    unsafe {
        match vol.SetMasterVolumeLevelScalar(scalar, std::ptr::null()) {
            Ok(()) => true,
            Err(e) => {
                warn!("[Keyfire] volume: SetMasterVolumeLevelScalar failed: {}", e);
                false
            }
        }
    }
}

/// Read the current master volume as a scalar in [0.0, 1.0].
pub fn get_master_volume_scalar() -> Option<f32> {
    let vol = endpoint_volume()?;
    unsafe {
        match vol.GetMasterVolumeLevelScalar() {
            Ok(level) => Some(level.clamp(0.0, 1.0)),
            Err(e) => {
                warn!("[Keyfire] volume: GetMasterVolumeLevelScalar failed: {}", e);
                None
            }
        }
    }
}

