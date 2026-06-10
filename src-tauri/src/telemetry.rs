//! Anonymous aggregate telemetry — daily summary POST.
//!
//! Sends one row per past local-calendar day to a small Railway-hosted
//! backend. Payload contains only:
//!   - date (local YYYY-MM-DD)
//!   - triggers (total count of logged actions that day)
//!   - expansions / macros (breakdowns)
//!   - app_version (the running Trigr version)
//!   - request_id (one-shot UUID, NOT persisted, lets the backend dedupe
//!     duplicate retries without us needing a persistent install ID)
//!
//! Zero identifiers, zero user content, zero hardware fingerprint. Default ON
//! during beta with a one-click off toggle in Settings (machine-local flag in
//! trigr-local-settings.json). When opted out, this module never sends and
//! never inserts new rows.
//!
//! Aggregation strategy:
//!   - Today's row stays mutable in `action_log` (never inserted into
//!     telemetry_sync while it is still "today" locally).
//!   - Yesterday and older: finalised by inserting into telemetry_sync on
//!     the next tick, then POSTed.
//!   - Failed POSTs leave `sent_at = NULL` so the next tick retries.
//!   - Rows older than 30 days are deleted on every tick.
//!
//! Threading:
//!   - This module owns ONE read-only rusqlite connection to
//!     trigr-analytics.db, opened on first use. All writes route through the
//!     analytics writer thread via `crate::analytics::telemetry_*` helpers
//!     so the writer's exclusive connection stays the only mutator.
//!   - HTTP POST runs on the telemetry timer thread itself (spawned from
//!     lib.rs:.setup() — see Stage 3 wiring).

use chrono::{Datelike, Duration, Local, NaiveDate};
use log::{info, warn};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

// ── Endpoint + auth ─────────────────────────────────────────────────────────
// Live Railway service (repo: Trigr-it/trigr-telemetry-backend). Raw Railway
// domain for now; if we later front it with telemetry.usetrigr.com, swapping
// this constant is a client release. The X-Ingest-Key ships in this
// source-available binary, so it is spam filtering, not security — the
// backend's validation and rate limiting are the real defences. Rotating it
// is a redeploy of both client and backend, acceptable while we're pre-1.0.
pub const TELEMETRY_ENDPOINT: &str =
    "https://trigr-telemetry-backend-production.up.railway.app/telemetry/v1/daily";
pub const TELEMETRY_INGEST_KEY: &str = "7752cb9bf95388ab36da0adb9c9a952645693dd9d3773245";

const RETAIN_DAYS: i64 = 30;

// ── Read-only connection cache ─────────────────────────────────────────────
// We can't share rusqlite::Connection between threads safely (not Send + Sync),
// but the telemetry thread is the only consumer here, so a thread-local cache
// would also work. Mutex<Option<Connection>> keeps the door open for an
// occasional reconnect if the DB file changes.
static READ_CONN: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn with_read_conn<F, R>(default: R, body: F) -> R
where
    F: FnOnce(&Connection) -> R,
{
    let lock = READ_CONN.get_or_init(|| Mutex::new(None));
    let mut guard = match lock.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        let Some(path) = crate::analytics::db_path() else {
            warn!("[Telemetry] analytics db_path not initialised yet");
            return default;
        };
        match open_read_conn(&path) {
            Ok(c) => *guard = Some(c),
            Err(e) => {
                warn!("[Telemetry] failed to open read connection: {}", e);
                return default;
            }
        }
    }
    body(guard.as_ref().unwrap())
}

fn open_read_conn(path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    // WAL is set globally by the writer; readers just inherit it. Setting
    // query_only is belt-and-braces — even a stray UPDATE would error out.
    let _ = conn.execute_batch("PRAGMA query_only = ON;");
    Ok(conn)
}

// ── Date helpers ───────────────────────────────────────────────────────────

/// Today's date in the user's local timezone. The cornerstone for "is this
/// day still mutable" decisions.
fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

fn fmt_date(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Lower bound for EVERY telemetry operation: nothing dated before this local
/// date is aggregated, kept, or sent. The later of two components wins:
///
///  - The telemetry epoch — the date this telemetry-enabled build first ran
///    on this machine (stamped here on first call). Usage recorded before
///    that predates the onboarding disclosure and must never leave the
///    machine — no historical backfill burst on upgrade.
///  - The 30-day retention horizon — local housekeeping so telemetry_sync
///    stays bounded. Applying the same floor to backfill, purge AND send
///    kills the churn loop (backfill re-creating rows the purge just
///    deleted) and stops the writer-thread purge race from leaking
///    expired rows into a send.
///
/// ISO dates compare correctly as plain strings.
fn send_floor() -> String {
    let retention = fmt_date(local_today() - Duration::days(RETAIN_DAYS));
    let mut epoch = crate::config::get_telemetry_epoch();
    if epoch.is_empty() {
        epoch = fmt_date(local_today());
        if crate::config::set_telemetry_epoch(&epoch) {
            info!("[Telemetry] epoch stamped: {} — only dates from here forward are ever sent", epoch);
        }
    }
    if epoch > retention { epoch } else { retention }
}

// ── Aggregation ────────────────────────────────────────────────────────────

/// Counts for a single local calendar day. Mirrors the telemetry_sync table
/// (minus the housekeeping columns) so the same struct flows through
/// aggregate → insert → POST.
#[derive(Debug, Clone, Copy)]
pub struct DayCounts {
    pub triggers: i64,
    pub expansions: i64,
    pub macros: i64,
}

/// Aggregate counts for one local date. Single SQL query with conditional
/// aggregations — one full scan of the row range matching `DATE(...,'localtime') = ?`.
/// Returns zeros if the date has no rows or the query fails (logged).
pub fn aggregate_for_date(date: &str) -> DayCounts {
    with_read_conn(
        DayCounts { triggers: 0, expansions: 0, macros: 0 },
        |conn| {
            let row: rusqlite::Result<(i64, i64, i64)> = conn.query_row(
                "SELECT \
                    COUNT(*) AS triggers, \
                    COUNT(CASE WHEN action_type = 'expansion' THEN 1 END) AS expansions, \
                    COUNT(CASE WHEN action_type = 'macro' THEN 1 END) AS macros \
                 FROM action_log \
                 WHERE DATE(timestamp, 'localtime') = ?1",
                rusqlite::params![date],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            );
            match row {
                Ok((t, e, m)) => DayCounts { triggers: t, expansions: e, macros: m },
                Err(e) => {
                    warn!("[Telemetry] aggregate_for_date({}) query failed: {}", date, e);
                    DayCounts { triggers: 0, expansions: 0, macros: 0 }
                }
            }
        },
    )
}

/// Find every local date strictly before today that has action_log rows but
/// does NOT yet have a corresponding telemetry_sync row, and queue inserts
/// for each via the analytics writer thread.
///
/// Implementation: a single LEFT JOIN-ish query finds the gap dates. We
/// deliberately use a sub-SELECT (`d NOT IN ...`) rather than a JOIN so the
/// query stays readable. The cost is fine — telemetry_sync is at most ~30
/// rows by design.
///
/// Zero-count days are NEVER produced here because they wouldn't appear in
/// the DISTINCT action_log scan in the first place. Spec P6: skip zero-count
/// days to keep backend signal clean.
pub fn ensure_rows_through_yesterday(floor: &str) {
    let today_str = fmt_date(local_today());
    let missing: Vec<String> = with_read_conn(Vec::new(), |conn| {
        let mut stmt = match conn.prepare(
            "SELECT DISTINCT DATE(timestamp, 'localtime') AS d \
             FROM action_log \
             WHERE DATE(timestamp, 'localtime') < ?1 \
               AND DATE(timestamp, 'localtime') >= ?2 \
               AND DATE(timestamp, 'localtime') NOT IN (SELECT date FROM telemetry_sync) \
             ORDER BY d ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("[Telemetry] ensure_rows: prepare failed: {}", e);
                return Vec::new();
            }
        };
        let rows = stmt.query_map(rusqlite::params![today_str, floor], |r| r.get::<_, String>(0));
        match rows {
            Ok(it) => it.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                warn!("[Telemetry] ensure_rows: query failed: {}", e);
                Vec::new()
            }
        }
    });

    if missing.is_empty() {
        return;
    }
    info!("[Telemetry] backfilling {} missing daily row(s)", missing.len());
    for date in missing {
        let c = aggregate_for_date(&date);
        // Defensive: skip if somehow zero (shouldn't happen given the
        // DISTINCT-from-action_log query above, but a race between two ticks
        // or a manual DELETE could leave a stale empty bucket).
        if c.triggers == 0 {
            continue;
        }
        crate::analytics::telemetry_insert_row(&date, c.triggers, c.expansions, c.macros);
    }
}

/// Delete telemetry_sync rows below the floor (retention horizon or the
/// telemetry epoch, whichever is later). Single DELETE through the analytics
/// writer thread. Also the one-time cleanup path for any pre-epoch rows left
/// behind by builds without the epoch rule.
pub fn purge_old_rows(floor: &str) {
    crate::analytics::telemetry_purge_older_than(floor);
}

// ── HTTP send ──────────────────────────────────────────────────────────────

/// One pending telemetry row pulled from the DB for sending.
struct PendingRow {
    date: String,
    triggers: i64,
    expansions: i64,
    macros: i64,
}

/// Pull every telemetry_sync row that has `sent_at IS NULL` and sits at or
/// above the floor. The floor filter here is belt-and-braces: the purge
/// DELETE travels async through the analytics writer thread, so without it a
/// send could race ahead of the purge and POST a row that's about to be
/// deleted.
fn select_pending_rows(floor: &str) -> Vec<PendingRow> {
    with_read_conn(Vec::new(), |conn| {
        let mut stmt = match conn.prepare(
            "SELECT date, triggers, expansions, macros \
             FROM telemetry_sync \
             WHERE sent_at IS NULL \
               AND date >= ?1 \
             ORDER BY date ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("[Telemetry] select_pending: prepare failed: {}", e);
                return Vec::new();
            }
        };
        let rows = stmt.query_map(rusqlite::params![floor], |r| {
            Ok(PendingRow {
                date: r.get(0)?,
                triggers: r.get(1)?,
                expansions: r.get(2)?,
                macros: r.get(3)?,
            })
        });
        match rows {
            Ok(it) => it.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                warn!("[Telemetry] select_pending: query failed: {}", e);
                Vec::new()
            }
        }
    })
}

/// POST every pending row. Build the reqwest blocking client once per tick.
/// 2xx response → mark the row sent. Anything else (timeout, non-2xx, network
/// error) → record an attempt (so we can see retry frequency in the DB) and
/// move on to the next row. The row stays `sent_at = NULL` and will be
/// retried on the next tick automatically.
///
/// `app_version` comes from `app.package_info().version` — a build-time
/// constant baked into the binary, so it's safe to pass once per tick.
pub fn send_pending(app_version: &str, floor: &str) {
    let pending = select_pending_rows(floor);
    if pending.is_empty() {
        return;
    }

    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("[Telemetry] failed to build HTTP client: {}", e);
            return;
        }
    };

    info!("[Telemetry] sending {} pending row(s)", pending.len());

    let total = pending.len();
    let mut sent_ok = 0usize;
    let mut last_err = String::new();
    for row in pending {
        // One-shot request_id (not persisted) lets the backend dedupe a
        // retry that lands after a successful POST whose response we
        // never received. Doesn't break the "no persistent identifier"
        // rule — re-generated every send attempt.
        let request_id = uuid::Uuid::new_v4().to_string();
        let payload = serde_json::json!({
            "date": row.date,
            "triggers": row.triggers,
            "expansions": row.expansions,
            "macros": row.macros,
            "app_version": app_version,
            "request_id": request_id,
        });

        let resp = client
            .post(TELEMETRY_ENDPOINT)
            .header("X-Ingest-Key", TELEMETRY_INGEST_KEY)
            .header("Content-Type", "application/json")
            .body(payload.to_string())
            .send();

        match resp {
            Ok(r) if r.status().is_success() => {
                let now = chrono::Utc::now().to_rfc3339();
                crate::analytics::telemetry_mark_sent(&row.date, &now);
                info!("[Telemetry] sent {} OK ({})", row.date, r.status());
                sent_ok += 1;
            }
            Ok(r) => {
                crate::analytics::telemetry_record_attempt(&row.date);
                last_err = format!("HTTP {}", r.status());
            }
            Err(e) => {
                crate::analytics::telemetry_record_attempt(&row.date);
                last_err = format!("network error: {}", e);
                // An unreachable endpoint fails identically for every row —
                // don't hammer it (or spam the log) once per pending row.
                // Unsent rows stay sent_at = NULL and retry next tick.
                break;
            }
        }
    }
    if sent_ok < total {
        warn!(
            "[Telemetry] {} of {} row(s) unsent ({}) — will retry next tick",
            total - sent_ok,
            total,
            last_err
        );
    }
}

// ── Orchestrator ───────────────────────────────────────────────────────────

/// Run one full telemetry cycle. Cheap and bounded — never blocks more than
/// ~20s per pending row in the worst case (connect + read timeouts).
///
/// Order matters:
///   1. Opt-out gate — first thing, so opted-out users incur no DB I/O.
///   2. send_floor — stamps the epoch on the very first tick; every later
///      step is bounded by it.
///   3. ensure_rows_through_yesterday — finalise any day that just rolled
///      over, in case the user hasn't restarted across local midnight.
///   4. purge_old_rows — keep the table within the retention/epoch floor.
///   5. send_pending — POST whatever is still NULL at or above the floor.
pub fn tick(app_version: &str) {
    if crate::config::get_telemetry_opt_out() {
        // Cheap noop log so verification can confirm the gate fires correctly.
        // Remove this line (or drop to debug!) once telemetry has been stable
        // for a few weeks of beta use.
        info!("[Telemetry] tick skipped — user opted out");
        return;
    }
    info!("[Telemetry] tick start (app_version={})", app_version);
    let floor = send_floor();
    ensure_rows_through_yesterday(&floor);
    purge_old_rows(&floor);
    send_pending(app_version, &floor);
}
