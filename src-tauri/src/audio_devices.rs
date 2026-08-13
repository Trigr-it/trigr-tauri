//! Audio output device switching. Used by the "Change Audio Output" macro step.
//!
//! Windows exposes no documented API for setting the default audio endpoint.
//! Third-party audio switchers (SoundSwitch, EarTrumpet, AudioSwitcher, NirCmd,
//! Logitech G Hub) all use the internal `IPolicyConfig` COM interface, present
//! since Vista, declared in `Policyconfig.h` inside the Windows SDK but not
//! exposed by `windows` / `windows-sys`. We declare it manually here — the
//! CLSID and IID are stable since Windows 7 and used by every open-source
//! audio switcher on GitHub.
//!
//! COM lifetime: mirrors `volume.rs` — each public call runs `CoInitializeEx`
//! (idempotent per thread), re-acquires the enumerator, drops everything at
//! function exit. Cheap (single-digit ms) and keeps us from caching pointers
//! across device changes.

use log::{info, warn};
use serde::Serialize;

use windows::core::{Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, IMMDeviceEnumerator,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantToString, PROPVARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};

/// One enumerated output endpoint, shipped to the frontend picker.
#[derive(Serialize, Clone, Debug)]
pub struct AudioOutputDevice {
    /// Windows endpoint ID — stable across reboots; the truth we save to config.
    /// Example: `{0.0.0.00000000}.{5f9e...}`
    pub id: String,
    /// Human-readable name shown in the picker: "Headphones (High Definition Audio Device)".
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    /// True for the endpoint Windows currently treats as the default (Console role).
    #[serde(rename = "isDefault")]
    pub is_default: bool,
}

/// Failure modes for `set_default_output_device`. Serialized so the frontend
/// / macro step can distinguish "device gone" (toast the user) from "COM broke"
/// (log + hush).
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", content = "message")]
pub enum SetOutputError {
    /// Device ID not present in the enumeration — unplugged, disabled, or the
    /// user renamed a virtual cable so the ID no longer resolves.
    #[serde(rename = "device_not_found")]
    DeviceNotFound(String),
    /// IPolicyConfig::SetDefaultEndpoint returned a non-success HRESULT.
    #[serde(rename = "policy_config_failed")]
    PolicyConfigFailed(String),
    /// CoCreateInstance / MMDevice enumeration failed. Usually catastrophic
    /// (audio subsystem missing), not the user's problem.
    #[serde(rename = "com_error")]
    ComError(String),
}

impl std::fmt::Display for SetOutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetOutputError::DeviceNotFound(id)      => write!(f, "device not found: {}", id),
            SetOutputError::PolicyConfigFailed(msg) => write!(f, "policy config failed: {}", msg),
            SetOutputError::ComError(msg)           => write!(f, "COM error: {}", msg),
        }
    }
}

// ── IPolicyConfig ────────────────────────────────────────────────────────────
// Undocumented COM interface, present since Vista, used by every third-party
// audio switcher. GUIDs from the Windows SDK's Policyconfig.h header.
//
// Method-order note: `SetDefaultEndpoint` is the 11th method in the interface
// (after 10 stubs we never call: GetMixFormat, GetDeviceFormat, ResetDeviceFormat,
// SetDeviceFormat, GetProcessingPeriod, SetProcessingPeriod, GetShareMode,
// SetShareMode, GetPropertyValue, SetPropertyValue). We declare all preceding
// methods as `usize` placeholders so the vtable slot for SetDefaultEndpoint
// lines up at the correct offset. Do NOT reorder — the vtable layout is ABI.

// CLSID_CPolicyConfigClient
const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
// IID_IPolicyConfig
const IID_IPOLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

#[repr(C)]
struct IPolicyConfigVtbl {
    // IUnknown
    query_interface: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        iid: *const GUID,
        interface: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    add_ref:  unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
    release:  unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
    // IPolicyConfig methods we don't call — placeholder function-pointer slots.
    _get_mix_format:         usize,
    _get_device_format:      usize,
    _reset_device_format:    usize,
    _set_device_format:      usize,
    _get_processing_period:  usize,
    _set_processing_period:  usize,
    _get_share_mode:         usize,
    _set_share_mode:         usize,
    _get_property_value:     usize,
    _set_property_value:     usize,
    // The one we care about — ERole passed as i32 to match Windows ERole enum ABI.
    set_default_endpoint: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        device_id: PCWSTR,
        role: i32,
    ) -> HRESULT,
    _set_endpoint_visibility: usize,
}

#[repr(transparent)]
struct IPolicyConfig(*mut core::ffi::c_void);

impl IPolicyConfig {
    unsafe fn vtable(&self) -> &IPolicyConfigVtbl {
        &**(self.0 as *const *const IPolicyConfigVtbl)
    }

    unsafe fn set_default_endpoint(&self, device_id: PCWSTR, role: i32) -> HRESULT {
        (self.vtable().set_default_endpoint)(self.0, device_id, role)
    }
}

impl Drop for IPolicyConfig {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (self.vtable().release)(self.0); }
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Enumerate all active render (playback) endpoints. Returns friendly names +
/// stable IDs suitable for feeding the frontend picker. Empty on any COM
/// failure — never panics.
pub fn list_output_devices() -> Vec<AudioOutputDevice> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER) {
                Ok(e) => e,
                Err(e) => {
                    warn!("[Keyfire] audio_devices: MMDeviceEnumerator CoCreateInstance failed: {}", e);
                    return Vec::new();
                }
            };

        // Current default (Console role) — used to mark isDefault on each row.
        let default_id: Option<String> = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|d| d.GetId().ok())
            .and_then(|pw| pw.to_string().ok());

        let collection = match enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            Ok(c) => c,
            Err(e) => {
                warn!("[Keyfire] audio_devices: EnumAudioEndpoints failed: {}", e);
                return Vec::new();
            }
        };

        let count = match collection.GetCount() {
            Ok(n) => n,
            Err(e) => {
                warn!("[Keyfire] audio_devices: GetCount failed: {}", e);
                return Vec::new();
            }
        };

        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(device) = collection.Item(i) else { continue; };
            let Ok(id_pw)  = device.GetId()     else { continue; };
            let Ok(id_str) = id_pw.to_string()  else { continue; };
            let Ok(store)  = device.OpenPropertyStore(STGM_READ) else {
                // Push with fallback name so users still see the row.
                out.push(AudioOutputDevice {
                    is_default: default_id.as_deref() == Some(id_str.as_str()),
                    friendly_name: format!("Audio device {}", i),
                    id: id_str,
                });
                continue;
            };
            let friendly = match store.GetValue(&PKEY_Device_FriendlyName) {
                Ok(pv) => propvariant_string(&pv).unwrap_or_else(|| format!("Audio device {}", i)),
                Err(_) => format!("Audio device {}", i),
            };
            out.push(AudioOutputDevice {
                is_default: default_id.as_deref() == Some(id_str.as_str()),
                friendly_name: friendly,
                id: id_str,
            });
        }

        // Sort by friendly name — enumeration order is not user-meaningful, and
        // the picker is easier to scan alphabetised.
        out.sort_by(|a, b| a.friendly_name.to_lowercase().cmp(&b.friendly_name.to_lowercase()));
        out
    }
}

/// Set the default output endpoint for all three roles (Console, Multimedia,
/// Communications). Setting all three matches Windows' Sound control panel
/// behaviour so the choice sticks across app types (games check eMultimedia,
/// VoIP checks eCommunications, everything else eConsole).
///
/// Returns Ok(friendly_name) on success. Errors:
/// - DeviceNotFound: device_id not in the active enumeration (unplugged / disabled).
/// - PolicyConfigFailed: SetDefaultEndpoint returned a non-success HRESULT.
/// - ComError: CoCreateInstance for MMDeviceEnumerator or IPolicyConfig failed.
pub fn set_default_output_device(device_id: &str) -> Result<String, SetOutputError> {
    // 1. Verify the device is actually present. Windows will happily let
    //    IPolicyConfig::SetDefaultEndpoint return S_OK for a stale ID and
    //    silently no-op — we'd rather report "not found" up front so the
    //    caller can toast the user.
    let devices = list_output_devices();
    let matched = devices.iter().find(|d| d.id == device_id).cloned();
    let Some(matched) = matched else {
        return Err(SetOutputError::DeviceNotFound(device_id.to_string()));
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // Build the CoCreateInstance call for IPolicyConfig manually because
        // the `windows` crate doesn't ship this undocumented interface.
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = windows::Win32::System::Com::CoCreateInstance::<Option<&windows::core::IUnknown>, windows::core::IUnknown>(
            &CLSID_POLICY_CONFIG_CLIENT,
            None,
            CLSCTX_ALL,
        );
        // Prefer the typed API to acquire the raw pointer via a QueryInterface
        // for our manually-declared IID. If CoCreateInstance succeeded we call
        // QueryInterface with IID_IPOLICY_CONFIG.
        let unknown = match hr {
            Ok(u) => u,
            Err(e) => {
                warn!("[Keyfire] audio_devices: CoCreateInstance(IPolicyConfig) failed: {}", e);
                return Err(SetOutputError::ComError(format!("CoCreateInstance: {}", e)));
            }
        };
        let unknown_ptr: *mut core::ffi::c_void = unknown.as_raw();
        // QueryInterface for IID_IPOLICY_CONFIG.
        let qi_fn: unsafe extern "system" fn(
            *mut core::ffi::c_void,
            *const GUID,
            *mut *mut core::ffi::c_void,
        ) -> HRESULT = {
            let vtbl = *(unknown_ptr as *const *const std::ffi::c_void);
            let qi_ptr = *(vtbl as *const *const std::ffi::c_void);
            std::mem::transmute(qi_ptr)
        };
        let qi_hr = qi_fn(unknown_ptr, &IID_IPOLICY_CONFIG, &mut raw);
        if qi_hr.is_err() || raw.is_null() {
            warn!("[Keyfire] audio_devices: QueryInterface(IPolicyConfig) failed: 0x{:08X}", qi_hr.0);
            return Err(SetOutputError::ComError(format!("QueryInterface: 0x{:08X}", qi_hr.0)));
        }
        let policy = IPolicyConfig(raw);

        // Encode device_id as a wide null-terminated string once.
        let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
        let pcwstr = PCWSTR(wide.as_ptr());

        // Set all three roles — matches how Sound Control Panel does it.
        for role in [eConsole.0, eMultimedia.0, eCommunications.0] {
            let hr = policy.set_default_endpoint(pcwstr, role);
            if hr.is_err() {
                warn!(
                    "[Keyfire] audio_devices: SetDefaultEndpoint role={} failed: 0x{:08X}",
                    role, hr.0
                );
                return Err(SetOutputError::PolicyConfigFailed(format!(
                    "role={} hr=0x{:08X}", role, hr.0
                )));
            }
        }

        info!(
            "[Keyfire] audio_devices: default output → \"{}\" ({})",
            matched.friendly_name, device_id
        );
        Ok(matched.friendly_name)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Read a PROPVARIANT (any string-shaped variant) into a Rust String via
/// PropVariantToString — layout-independent across `windows` crate versions
/// (we never touch the union). 512 wide chars covers every real-world audio
/// device name; the API truncates on overflow so worst case is a clipped label.
unsafe fn propvariant_string(pv: &PROPVARIANT) -> Option<String> {
    let mut buf = [0u16; 512];
    PropVariantToString(pv as *const PROPVARIANT, &mut buf).ok()?;
    // The buffer is null-terminated; find the terminator to slice the actual chars.
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}
