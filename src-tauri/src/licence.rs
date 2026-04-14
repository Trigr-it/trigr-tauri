use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ── State ──────────────────────────────────────────────────────────────────

/// Fast atomic check — avoids locking the mutex on every feature gate query.
static IS_PRO: AtomicBool = AtomicBool::new(false);

/// Full cached licence state, loaded from local settings on startup.
static LICENCE_STATE: OnceLock<Mutex<LicenceState>> = OnceLock::new();

const GRACE_PERIOD_DAYS: i64 = 7;
const REVALIDATION_HOURS: i64 = 24;

// ── Data structures ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LicenceState {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub validated_at: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Returned to the frontend for UI display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicenceStatus {
    pub is_pro: bool,
    pub key_entered: bool,
    pub status: String,
    pub product_name: String,
    pub expires_at: Option<String>,
}

// ── LemonSqueezy API response types ────────────────────────────────────────

#[derive(Deserialize, Debug)]
struct LsValidateResponse {
    valid: bool,
    license_key: Option<LsLicenseKey>,
    instance: Option<LsInstance>,
    error: Option<String>,
}

#[derive(Deserialize, Debug)]
struct LsActivateResponse {
    activated: bool,
    license_key: Option<LsLicenseKey>,
    instance: Option<LsInstance>,
    error: Option<String>,
}

#[derive(Deserialize, Debug)]
struct LsDeactivateResponse {
    deactivated: bool,
    error: Option<String>,
}

#[derive(Deserialize, Debug)]
struct LsLicenseKey {
    status: Option<String>,
    expires_at: Option<String>,
}

#[derive(Deserialize, Debug)]
struct LsInstance {
    id: Option<String>,
}

// ── Initialization ─────────────────────────────────────────────────────────

/// Call once at startup from lib.rs setup, after config::init().
pub fn init() {
    let state = load_from_local_settings();
    let pro = compute_is_pro(&state);
    IS_PRO.store(pro, Ordering::SeqCst);
    let _ = LICENCE_STATE.set(Mutex::new(state));
    info!("[Trigr] Licence module initialized — is_pro: {}", pro);
}

/// Fast atomic check — safe to call from any thread.
pub fn is_pro() -> bool {
    IS_PRO.load(Ordering::SeqCst)
}

/// Returns full licence status for the frontend.
pub fn get_licence_status() -> LicenceStatus {
    let state = match LICENCE_STATE.get() {
        Some(m) => m.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        None => LicenceState::default(),
    };
    build_status(&state)
}

// ── API operations ─────────────────────────────────────────────────────────

/// Activate a licence key against LemonSqueezy API.
pub async fn activate_licence(key: String) -> Result<LicenceStatus, String> {
    let instance_name = get_instance_name();
    info!("[Trigr] Activating licence key (instance: {})", instance_name);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.lemonsqueezy.com/v1/licenses/activate")
        .header("Accept", "application/json")
        .form(&[
            ("license_key", key.as_str()),
            ("instance_name", instance_name.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let body: LsActivateResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid response: {}", e))?;

    if !body.activated {
        let err = body.error.unwrap_or_else(|| "Activation failed".to_string());
        warn!("[Trigr] Licence activation failed: {}", err);
        return Err(err);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let lk = body.license_key.as_ref();
    let state = LicenceState {
        key: Some(key),
        instance_id: body.instance.and_then(|i| i.id),
        valid: true,
        validated_at: Some(now),
        status: lk.and_then(|l| l.status.clone()),
        product_name: Some("Trigr Pro".to_string()),
        expires_at: lk.and_then(|l| l.expires_at.clone()),
    };

    update_state(state.clone());
    info!("[Trigr] Licence activated successfully");
    Ok(build_status(&state))
}

/// Deactivate the current licence.
pub async fn deactivate_licence() -> Result<LicenceStatus, String> {
    let (key, instance_id) = {
        let guard = LICENCE_STATE
            .get()
            .ok_or("Not initialized")?
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (guard.key.clone(), guard.instance_id.clone())
    };

    let key = key.ok_or("No licence key to deactivate")?;
    let instance_id = instance_id.ok_or("No instance ID — cannot deactivate")?;

    info!("[Trigr] Deactivating licence");

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.lemonsqueezy.com/v1/licenses/deactivate")
        .header("Accept", "application/json")
        .form(&[
            ("license_key", key.as_str()),
            ("instance_id", instance_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let body: LsDeactivateResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid response: {}", e))?;

    if !body.deactivated {
        let err = body.error.unwrap_or_else(|| "Deactivation failed".to_string());
        warn!("[Trigr] Licence deactivation failed: {}", err);
        return Err(err);
    }

    let state = LicenceState::default();
    update_state(state.clone());
    info!("[Trigr] Licence deactivated");
    Ok(build_status(&state))
}

/// Check if re-validation is due and perform it if needed.
/// Called from frontend on app mount and window focus.
pub async fn check_and_revalidate() -> LicenceStatus {
    let state = match LICENCE_STATE.get() {
        Some(m) => m.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        None => return build_status(&LicenceState::default()),
    };

    // No key entered — nothing to validate
    let key = match &state.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => return build_status(&state),
    };

    // Check if revalidation is needed (>24h since last check)
    if let Some(ref va) = state.validated_at {
        if let Ok(va_dt) = chrono::DateTime::parse_from_rfc3339(va) {
            let hours_since = (chrono::Utc::now() - va_dt.to_utc()).num_hours();
            if hours_since < REVALIDATION_HOURS {
                // Still fresh — return cached status
                return build_status(&state);
            }
        }
    }

    // Time to revalidate
    info!("[Trigr] Revalidating licence key");
    let instance_name = get_instance_name();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return build_status(&state), // Offline — use cached
    };

    let resp = client
        .post("https://api.lemonsqueezy.com/v1/licenses/validate")
        .header("Accept", "application/json")
        .form(&[
            ("license_key", key.as_str()),
            ("instance_name", instance_name.as_str()),
        ])
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            warn!("[Trigr] Licence revalidation failed (offline?): {}", e);
            return build_status(&state); // Offline — use cached + grace period
        }
    };

    let body: LsValidateResponse = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            warn!("[Trigr] Licence revalidation: invalid response: {}", e);
            return build_status(&state);
        }
    };

    let now = chrono::Utc::now().to_rfc3339();
    let lk = body.license_key.as_ref();
    let new_state = LicenceState {
        key: Some(key),
        instance_id: state.instance_id.clone().or_else(|| body.instance.and_then(|i| i.id)),
        valid: body.valid,
        validated_at: Some(now),
        status: lk.and_then(|l| l.status.clone()).or(state.status.clone()),
        product_name: state.product_name.clone(),
        expires_at: lk.and_then(|l| l.expires_at.clone()).or(state.expires_at.clone()),
    };

    update_state(new_state.clone());
    if body.valid {
        info!("[Trigr] Licence revalidated successfully");
    } else {
        warn!("[Trigr] Licence revalidation: key is no longer valid");
    }
    build_status(&new_state)
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn compute_is_pro(state: &LicenceState) -> bool {
    if !state.valid {
        return false;
    }
    if let Some(ref status) = state.status {
        if status != "active" {
            return false;
        }
    }
    // Check expiry
    if let Some(ref exp) = state.expires_at {
        if let Ok(exp_dt) = chrono::DateTime::parse_from_rfc3339(exp) {
            if exp_dt.to_utc() < chrono::Utc::now() {
                return false;
            }
        }
    }
    // Grace period: must have validated within last N days
    if let Some(ref va) = state.validated_at {
        if let Ok(va_dt) = chrono::DateTime::parse_from_rfc3339(va) {
            let days_since = (chrono::Utc::now() - va_dt.to_utc()).num_days();
            return days_since <= GRACE_PERIOD_DAYS;
        }
    }
    // No validation timestamp but marked valid — allow (first activation)
    true
}

fn build_status(state: &LicenceState) -> LicenceStatus {
    let is_pro = compute_is_pro(state);
    let key_entered = state.key.as_ref().map(|k| !k.is_empty()).unwrap_or(false);

    let status = if !key_entered {
        "no_key".to_string()
    } else if !state.valid {
        state
            .status
            .clone()
            .unwrap_or_else(|| "inactive".to_string())
    } else if is_pro {
        "active".to_string()
    } else {
        // Valid but not pro — grace period expired
        "offline_expired".to_string()
    };

    LicenceStatus {
        is_pro,
        key_entered,
        status,
        product_name: state.product_name.clone().unwrap_or_default(),
        expires_at: state.expires_at.clone(),
    }
}

fn update_state(state: LicenceState) {
    let pro = compute_is_pro(&state);
    IS_PRO.store(pro, Ordering::SeqCst);

    if let Some(m) = LICENCE_STATE.get() {
        if let Ok(mut guard) = m.lock() {
            *guard = state.clone();
        }
    }

    save_to_local_settings(&state);
}

fn load_from_local_settings() -> LicenceState {
    let val = crate::config::load_local_settings_json();
    match val.get("licence") {
        Some(licence_val) => {
            serde_json::from_value::<LicenceState>(licence_val.clone()).unwrap_or_else(|e| {
                warn!("[Trigr] Failed to deserialize licence state: {}", e);
                LicenceState::default()
            })
        }
        None => LicenceState::default(),
    }
}

fn save_to_local_settings(state: &LicenceState) {
    let mut val = crate::config::load_local_settings_json();
    if let Some(obj) = val.as_object_mut() {
        match serde_json::to_value(state) {
            Ok(licence_val) => {
                obj.insert("licence".to_string(), licence_val);
            }
            Err(e) => {
                error!("[Trigr] Failed to serialize licence state: {}", e);
                return;
            }
        }
    }
    crate::config::save_local_settings_json(&val);
}

/// Get a stable machine-specific instance name using the COMPUTERNAME environment variable.
fn get_instance_name() -> String {
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        if !name.is_empty() {
            return format!("trigr-{}", name);
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return format!("trigr-{}", name);
        }
    }
    "trigr-unknown".to_string()
}
