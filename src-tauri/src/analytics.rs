use log::{error, info, warn};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;

// ── Analytics event ────────────────────────────────────────────────────────

struct AnalyticsEvent {
    action_type: String,
    char_count: u32,
    trigger: String,
    label: String,
    /// For macro sequences: JSON array of step types, e.g. ["Type Text","Open App","Press Key"]
    macro_step_types: Option<Vec<String>>,
    /// Foreground app when the action fired (e.g. "revit", "chrome")
    target_app: String,
}

// ── Writer thread channel ──────────────────────────────────────────────────

static ANALYTICS_TX: OnceLock<Mutex<mpsc::Sender<AnalyticsMsg>>> = OnceLock::new();
/// Stored at init() so `telemetry.rs` can open its own read-only connection
/// without re-resolving the app data dir. Set once; never overwritten.
static ANALYTICS_DB_PATH: OnceLock<PathBuf> = OnceLock::new();

enum AnalyticsMsg {
    Log(AnalyticsEvent),
    GetStats(mpsc::Sender<serde_json::Value>),
    GetDailyChart(u32, mpsc::Sender<serde_json::Value>),
    GetAssignmentBreakdown(u32, mpsc::Sender<serde_json::Value>), // days (0 = all time)
    GetTypeBreakdown(u32, mpsc::Sender<serde_json::Value>),       // days (0 = all time)
    GetHourlyHeatmap(u32, mpsc::Sender<serde_json::Value>),
    GetTopApps(u32, mpsc::Sender<serde_json::Value>),              // days (0 = all time)
    GetExpansionEfficiency(mpsc::Sender<serde_json::Value>),
    GetExpansionCounts(mpsc::Sender<serde_json::Value>),
    GetStreaks(mpsc::Sender<serde_json::Value>),
    ExportCsv(mpsc::Sender<String>),
    Reset(mpsc::Sender<bool>),
    /// One-time migration: recalculate time_saved for old entries using current assignments.
    MigrateTimeSaved(std::collections::HashMap<String, serde_json::Value>),
    // ── Telemetry writes (read-only queries open their own connection in telemetry.rs) ──
    /// Insert a finalised daily aggregate row. Idempotent via INSERT OR IGNORE.
    TelemetryInsertRow {
        date: String,
        triggers: i64,
        expansions: i64,
        macros: i64,
    },
    /// Mark a telemetry row as successfully sent and bump send_attempts.
    TelemetryMarkSent { date: String, sent_at: String },
    /// Bump send_attempts on a row that failed to send (telemetry-side bookkeeping).
    TelemetryRecordAttempt { date: String },
    /// Purge rows whose date is older than `before_date` (local YYYY-MM-DD).
    TelemetryPurgeOlderThan { before_date: String },
}

// ── Initialise ─────────────────────────────────────────────────────────────

pub fn init(app_data_dir: PathBuf) {
    let db_path = app_data_dir.join("trigr-analytics.db");
    let _ = ANALYTICS_DB_PATH.set(db_path.clone());
    let (tx, rx) = mpsc::channel::<AnalyticsMsg>();
    let _ = ANALYTICS_TX.set(Mutex::new(tx));

    thread::Builder::new()
        .name("trigr-analytics".to_string())
        .spawn(move || {
            let conn = match Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("[Trigr] Failed to open analytics DB: {}", e);
                    return;
                }
            };

            // WAL mode for better concurrent read performance
            let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

            if let Err(e) = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS action_log (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp   TEXT NOT NULL,
                    action_type TEXT NOT NULL,
                    char_count  INTEGER DEFAULT 0,
                    time_saved  REAL NOT NULL
                );",
            ) {
                error!("[Trigr] Failed to create analytics table: {}", e);
                return;
            }

            // Schema migrations: add columns if missing
            let _ = conn.execute_batch("ALTER TABLE action_log ADD COLUMN trigger_key TEXT NOT NULL DEFAULT '';");
            let _ = conn.execute_batch("ALTER TABLE action_log ADD COLUMN label TEXT NOT NULL DEFAULT '';");
            let _ = conn.execute_batch("ALTER TABLE action_log ADD COLUMN target_app TEXT NOT NULL DEFAULT '';");

            // Version tracking for one-time migrations
            let _ = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS analytics_meta (key TEXT PRIMARY KEY, value TEXT);"
            );

            // Daily aggregate counts for the telemetry POST. One row per past
            // local-calendar day, finalised at local-midnight rollover. Today's
            // row is never inserted while it is still "today" (counts stay
            // mutable in action_log). The telemetry thread reads pending rows
            // (sent_at IS NULL) and POSTs them; on 2xx the writer thread sets
            // sent_at. 30-day retention is enforced by purge_old_rows in
            // telemetry.rs on every tick. Counts source: aggregate over
            // action_log grouped by action_type, anchored to local date.
            //
            // NOTE: no hotkeys column. Hotkey-remap actions are deliberately
            // excluded from action_log (see lines 173-178 below); adding them
            // here would require a separate counter pipeline for a category
            // we already decided is noise. If a future need arises we'll add
            // a column then.
            let _ = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS telemetry_sync (
                    date            TEXT PRIMARY KEY,
                    triggers        INTEGER NOT NULL DEFAULT 0,
                    expansions      INTEGER NOT NULL DEFAULT 0,
                    macros          INTEGER NOT NULL DEFAULT 0,
                    sent_at         TEXT,
                    send_attempts   INTEGER NOT NULL DEFAULT 0
                );"
            );

            // One-time migration (v0.4.0+): drop legacy key-to-key remap rows.
            // Simple "hotkey" action_type entries are no longer logged at runtime
            // (see log_action_ext). Existing rows from prior versions are removed
            // so totals/breakdowns reflect only meaningful automation. Idempotent.
            let migration_done: Result<String, _> = conn.query_row(
                "SELECT value FROM analytics_meta WHERE key = 'hotkey_rows_purged'",
                [],
                |row| row.get(0),
            );
            if migration_done.is_err() {
                if let Ok(n) = conn.execute("DELETE FROM action_log WHERE action_type = 'hotkey'", []) {
                    info!("[Trigr] Analytics migration: removed {} legacy hotkey-remap rows", n);
                }
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO analytics_meta (key, value) VALUES ('hotkey_rows_purged', '1')",
                    [],
                );
            }

            info!("[Trigr] Analytics DB ready: {}", db_path.display());

            for msg in rx {
                match msg {
                    AnalyticsMsg::Log(event) => {
                        handle_log(&conn, event);
                    }
                    AnalyticsMsg::GetStats(reply) => {
                        let stats = handle_get_stats(&conn);
                        let _ = reply.send(stats);
                    }
                    AnalyticsMsg::GetDailyChart(days, reply) => {
                        let data = handle_daily_chart(&conn, days);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetAssignmentBreakdown(days, reply) => {
                        let data = handle_assignment_breakdown(&conn, days);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetTypeBreakdown(days, reply) => {
                        let data = handle_type_breakdown(&conn, days);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetHourlyHeatmap(days, reply) => {
                        let data = handle_hourly_heatmap(&conn, days);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetTopApps(days, reply) => {
                        let data = handle_top_apps(&conn, days);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetExpansionEfficiency(reply) => {
                        let data = handle_expansion_efficiency(&conn);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetExpansionCounts(reply) => {
                        let data = handle_expansion_counts(&conn);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetStreaks(reply) => {
                        let data = handle_streaks(&conn);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::ExportCsv(reply) => {
                        let csv = handle_export_csv(&conn);
                        let _ = reply.send(csv);
                    }
                    AnalyticsMsg::Reset(reply) => {
                        let ok = handle_reset(&conn);
                        let _ = reply.send(ok);
                    }
                    AnalyticsMsg::MigrateTimeSaved(assignments) => {
                        handle_migrate_time_saved(&conn, &assignments);
                    }
                    AnalyticsMsg::TelemetryInsertRow { date, triggers, expansions, macros } => {
                        // INSERT OR IGNORE so re-runs after a partial backfill are safe.
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO telemetry_sync \
                             (date, triggers, expansions, macros, sent_at, send_attempts) \
                             VALUES (?1, ?2, ?3, ?4, NULL, 0)",
                            rusqlite::params![date, triggers, expansions, macros],
                        );
                    }
                    AnalyticsMsg::TelemetryMarkSent { date, sent_at } => {
                        let _ = conn.execute(
                            "UPDATE telemetry_sync \
                             SET sent_at = ?2, send_attempts = send_attempts + 1 \
                             WHERE date = ?1",
                            rusqlite::params![date, sent_at],
                        );
                    }
                    AnalyticsMsg::TelemetryRecordAttempt { date } => {
                        let _ = conn.execute(
                            "UPDATE telemetry_sync SET send_attempts = send_attempts + 1 WHERE date = ?1",
                            rusqlite::params![date],
                        );
                    }
                    AnalyticsMsg::TelemetryPurgeOlderThan { before_date } => {
                        let _ = conn.execute(
                            "DELETE FROM telemetry_sync WHERE date < ?1",
                            rusqlite::params![before_date],
                        );
                    }
                }
            }
        })
        .expect("Failed to spawn analytics writer thread");
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Log an action. Non-blocking — sends to writer thread via channel.
pub fn log_action(action_type: &str, char_count: u32, trigger: &str, label: &str) {
    log_action_ext(action_type, char_count, trigger, label, None);
}

/// Log an action with optional macro step types for accurate time calculation.
pub fn log_action_ext(action_type: &str, char_count: u32, trigger: &str, label: &str, macro_step_types: Option<Vec<String>>) {
    // Skip simple key-to-key remaps (action_type "hotkey"). They're a passthrough,
    // not a meaningful action — counting them inflates totals and dilutes the
    // signal for macros, expansions, and other real automation.
    if action_type == "hotkey" {
        return;
    }
    let target_app = crate::foreground::get_current_fg_proc();
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::Log(AnalyticsEvent {
                action_type: action_type.to_string(),
                char_count,
                trigger: trigger.to_string(),
                label: label.to_string(),
                macro_step_types,
                target_app,
            }));
        }
    }
}

// ── Telemetry write API (called from telemetry.rs) ────────────────────────
// Read-side queries open their own SQLite connection in telemetry.rs so
// they don't contend with the writer thread; writes are routed through the
// channel below so all mutations on telemetry_sync stay serialized through
// the same connection as action_log writes.

/// Path to the analytics SQLite file. Used by `telemetry.rs` to open its
/// own read-only connection. Set by `init()`.
pub fn db_path() -> Option<PathBuf> {
    ANALYTICS_DB_PATH.get().cloned()
}

pub(crate) fn telemetry_insert_row(date: &str, triggers: i64, expansions: i64, macros: i64) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::TelemetryInsertRow {
                date: date.to_string(),
                triggers,
                expansions,
                macros,
            });
        }
    }
}

pub(crate) fn telemetry_mark_sent(date: &str, sent_at: &str) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::TelemetryMarkSent {
                date: date.to_string(),
                sent_at: sent_at.to_string(),
            });
        }
    }
}

pub(crate) fn telemetry_record_attempt(date: &str) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::TelemetryRecordAttempt {
                date: date.to_string(),
            });
        }
    }
}

pub(crate) fn telemetry_purge_older_than(before_date: &str) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::TelemetryPurgeOlderThan {
                before_date: before_date.to_string(),
            });
        }
    }
}

/// Get aggregate stats. Blocks briefly while the writer thread queries.
pub fn get_stats() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetStats(reply), empty_stats())
}

/// Get daily chart data for the last N days.
pub fn get_daily_chart(days: u32) -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetDailyChart(days, reply), serde_json::json!([]))
}

/// Get type breakdown (expansions/hotkeys/macros) for a time range. days=0 means all time.
pub fn get_type_breakdown(days: u32) -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetTypeBreakdown(days, reply), serde_json::json!({}))
}

/// Get per-assignment breakdown (top 50 by usage). days=0 means all time.
pub fn get_assignment_breakdown(days: u32) -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetAssignmentBreakdown(days, reply), serde_json::json!([]))
}

/// Get hourly heatmap for the given number of days x 24 hours.
pub fn get_hourly_heatmap(days: u32) -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetHourlyHeatmap(days, reply), serde_json::json!([]))
}

/// Get top apps by action count. days=0 means all time.
pub fn get_top_apps(days: u32) -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetTopApps(days, reply), serde_json::json!([]))
}

/// Get expansion efficiency stats (chars typed vs chars expanded).
pub fn get_expansion_efficiency() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetExpansionEfficiency(reply), serde_json::json!({}))
}

/// Get per-expansion fire counts (trigger_key → count).
pub fn get_expansion_counts() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetExpansionCounts(reply), serde_json::json!({}))
}

/// Get current and longest streaks.
pub fn get_streaks() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetStreaks(reply), serde_json::json!({"current": 0, "longest": 0}))
}

/// Export all analytics as CSV string.
pub fn export_csv() -> String {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(AnalyticsMsg::ExportCsv(reply_tx)).is_ok() {
                if let Ok(csv) = reply_rx.recv_timeout(std::time::Duration::from_secs(10)) {
                    return csv;
                }
            }
        }
    }
    String::new()
}

/// Retroactively recalculate time_saved for old entries using current assignments.
/// Fire-and-forget — does not block.
pub fn migrate_time_saved(assignments: std::collections::HashMap<String, serde_json::Value>) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::MigrateTimeSaved(assignments));
        }
    }
}

/// Delete all analytics data. Returns true on success.
pub fn reset_stats() -> bool {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(AnalyticsMsg::Reset(reply_tx)).is_ok() {
                if let Ok(ok) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return ok;
                }
            }
        }
    }
    false
}

// ── Helper ─────────────────────────────────────────────────────────────────

fn send_and_recv<T: Send + 'static>(
    build_msg: impl FnOnce(mpsc::Sender<T>) -> AnalyticsMsg,
    default: T,
) -> T {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(build_msg(reply_tx)).is_ok() {
                if let Ok(val) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return val;
                }
            }
        }
    }
    default
}

// ── Writer thread handlers ─────────────────────────────────────────────────

/// Compute time saved (seconds) for a single macro step type.
fn step_time_saved(step_type: &str) -> f64 {
    match step_type {
        "Open App" | "Open URL" | "Open Folder" => 3.0,
        // Type Text, Press Key, Click Mouse, Click at Position, Focus Window, etc.
        _ => 1.0,
    }
}

fn handle_log(conn: &Connection, event: AnalyticsEvent) {
    let time_saved = match event.action_type.as_str() {
        "expansion" => event.char_count as f64 * 0.3,
        "macro" => {
            // Sum time for each step in the macro sequence
            match &event.macro_step_types {
                Some(steps) if !steps.is_empty() => {
                    steps.iter().map(|s| step_time_saved(s)).sum()
                }
                _ => 3.0, // fallback for legacy entries without step info
            }
        }
        "hotkey" => 0.0,  // key-for-key remap, no time saved
        "text" => 3.0,    // type text action
        "app" => 3.0,     // open app
        "url" => 3.0,     // open URL
        "folder" => 3.0,  // open folder
        "search_template" => 3.0,
        _ => 0.0,
    };

    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = conn.execute(
        "INSERT INTO action_log (timestamp, action_type, char_count, time_saved, trigger_key, label, target_app) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![now, event.action_type, event.char_count, time_saved, event.trigger, event.label, event.target_app],
    ) {
        error!("[Trigr] Failed to log analytics event: {}", e);
    }
}

fn handle_get_stats(conn: &Connection) -> serde_json::Value {
    // All windowed queries use SQLite's 'localtime' modifier so "today" and
    // "last N days" are anchored to the user's local calendar day. Stored
    // timestamps remain UTC (see handle_log) — only the comparison is local.
    let (total_actions, total_time_saved) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_today, time_saved_today) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log \
             WHERE DATE(timestamp, 'localtime') = DATE('now', 'localtime')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_last_7_days, time_saved_last_7_days) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log \
             WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-6 days')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_last_14_days, time_saved_last_14_days) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log \
             WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-13 days')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_last_30_days, time_saved_last_30_days) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log \
             WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-29 days')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let best_day = conn
        .query_row(
            "SELECT COALESCE(MAX(day_total), 0.0) FROM (
                SELECT SUM(time_saved) AS day_total FROM action_log
                GROUP BY DATE(timestamp, 'localtime')
            )",
            [],
            |row| row.get::<_, f64>(0),
        )
        .unwrap_or(0.0);

    let best_7_days = conn
        .query_row(
            "SELECT COALESCE(MAX(window_total), 0.0) FROM (
                SELECT SUM(a2.time_saved) AS window_total
                FROM (SELECT DISTINCT DATE(timestamp, 'localtime') AS d FROM action_log) days
                JOIN action_log a2
                  ON DATE(a2.timestamp, 'localtime') BETWEEN DATE(days.d, '-6 days') AND days.d
                GROUP BY days.d
            )",
            [],
            |row| row.get::<_, f64>(0),
        )
        .unwrap_or(0.0);

    let mut stmt = conn
        .prepare("SELECT action_type, COUNT(*) FROM action_log GROUP BY action_type")
        .unwrap();
    let mut expansions: i64 = 0;
    let mut hotkeys: i64 = 0;
    let mut macros: i64 = 0;
    let mut search_templates: i64 = 0;
    if let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0).unwrap_or_default(),
            row.get::<_, i64>(1).unwrap_or(0),
        ))
    }) {
        for row in rows.flatten() {
            match row.0.as_str() {
                "expansion" => expansions = row.1,
                "macro" => macros = row.1,
                "search_template" => search_templates = row.1,
                _ => hotkeys += row.1,
            }
        }
    }

    serde_json::json!({
        "total_actions": total_actions,
        "total_time_saved_seconds": total_time_saved,
        "actions_today": actions_today,
        "time_saved_today_seconds": time_saved_today,
        "actions_last_7_days": actions_last_7_days,
        "time_saved_last_7_days_seconds": time_saved_last_7_days,
        "actions_last_14_days": actions_last_14_days,
        "time_saved_last_14_days_seconds": time_saved_last_14_days,
        "actions_last_30_days": actions_last_30_days,
        "time_saved_last_30_days_seconds": time_saved_last_30_days,
        "best_day_time_saved_seconds": best_day,
        "best_7_days_time_saved_seconds": best_7_days,
        "expansions": expansions,
        "hotkeys": hotkeys,
        "macros": macros,
        "search_templates": search_templates,
    })
}

// ── Pro analytics handlers ─────────────────────────────────────────────────

fn handle_daily_chart(conn: &Connection, days: u32) -> serde_json::Value {
    // Bucket by local calendar day so bar keys match the frontend's local-date
    // fill-in loop. Window = today + (days-1) prior local days.
    let mut stmt = match conn.prepare(
        "SELECT DATE(timestamp, 'localtime') AS day, COUNT(*) AS actions, COALESCE(SUM(time_saved), 0.0) AS saved
         FROM action_log
         WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', ?1)
         GROUP BY day
         ORDER BY day ASC"
    ) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Daily chart query failed: {}", e);
            return serde_json::json!([]);
        }
    };

    let offset = format!("-{} days", days.saturating_sub(1));
    let rows: Vec<serde_json::Value> = match stmt.query_map(rusqlite::params![offset], |row| {
        Ok(serde_json::json!({
            "date": row.get::<_, String>(0).unwrap_or_default(),
            "actions": row.get::<_, i64>(1).unwrap_or(0),
            "time_saved": row.get::<_, f64>(2).unwrap_or(0.0),
        }))
    }) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    };

    serde_json::json!(rows)
}

fn handle_type_breakdown(conn: &Connection, days: u32) -> serde_json::Value {
    // days=0 → all time; days=N → today + (N-1) prior local calendar days.
    let query = if days == 0 {
        "SELECT action_type, COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log GROUP BY action_type".to_string()
    } else {
        format!(
            "SELECT action_type, COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log \
             WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-{} days') \
             GROUP BY action_type",
            days - 1
        )
    };

    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(e) => { warn!("[Trigr] Type breakdown query failed: {}", e); return serde_json::json!({}); }
    };

    let mut expansions: i64 = 0;
    let mut hotkeys: i64 = 0;
    let mut macros: i64 = 0;
    let mut total: i64 = 0;
    let mut time_saved: f64 = 0.0;
    if let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0).unwrap_or_default(),
            row.get::<_, i64>(1).unwrap_or(0),
            row.get::<_, f64>(2).unwrap_or(0.0),
        ))
    }) {
        for row in rows.flatten() {
            total += row.1;
            time_saved += row.2;
            match row.0.as_str() {
                "expansion" => expansions += row.1,
                "macro" => macros += row.1,
                _ => hotkeys += row.1,
            }
        }
    }

    serde_json::json!({
        "total": total,
        "expansions": expansions,
        "hotkeys": hotkeys,
        "macros": macros,
        "time_saved": time_saved,
    })
}

fn handle_assignment_breakdown(conn: &Connection, days: u32) -> serde_json::Value {
    // days=0 → all time; days=N → today + (N-1) prior local calendar days.
    let (query, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if days == 0 {
        ("SELECT trigger_key, label, action_type, COUNT(*) AS count, COALESCE(SUM(time_saved), 0.0) AS saved, MAX(timestamp) AS last_fired
          FROM action_log WHERE trigger_key != ''
          GROUP BY trigger_key ORDER BY count DESC LIMIT 50",
         vec![])
    } else {
        ("SELECT trigger_key, label, action_type, COUNT(*) AS count, COALESCE(SUM(time_saved), 0.0) AS saved, MAX(timestamp) AS last_fired
          FROM action_log WHERE trigger_key != ''
          AND DATE(timestamp, 'localtime') >= DATE('now', 'localtime', ?1)
          GROUP BY trigger_key ORDER BY count DESC LIMIT 50",
         vec![Box::new(format!("-{} days", days - 1)) as Box<dyn rusqlite::types::ToSql>])
    };

    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Assignment breakdown query failed: {}", e);
            return serde_json::json!([]);
        }
    };

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows: Vec<serde_json::Value> = match stmt.query_map(param_refs.as_slice(), |row| {
        Ok(serde_json::json!({
            "trigger": row.get::<_, String>(0).unwrap_or_default(),
            "label": row.get::<_, String>(1).unwrap_or_default(),
            "type": row.get::<_, String>(2).unwrap_or_default(),
            "count": row.get::<_, i64>(3).unwrap_or(0),
            "time_saved": row.get::<_, f64>(4).unwrap_or(0.0),
            "last_fired": row.get::<_, String>(5).unwrap_or_default(),
        }))
    }) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    };

    serde_json::json!(rows)
}

fn handle_top_apps(conn: &Connection, days: u32) -> serde_json::Value {
    // days=0 → all time; days=N → today + (N-1) prior local calendar days.
    let query = if days == 0 {
        "SELECT target_app, COUNT(*) AS count, COALESCE(SUM(time_saved), 0.0) AS saved
         FROM action_log WHERE target_app != '' AND LOWER(target_app) != 'trigr'
         GROUP BY target_app ORDER BY count DESC LIMIT 20".to_string()
    } else {
        format!(
            "SELECT target_app, COUNT(*) AS count, COALESCE(SUM(time_saved), 0.0) AS saved
             FROM action_log WHERE target_app != '' AND LOWER(target_app) != 'trigr'
             AND DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-{} days')
             GROUP BY target_app ORDER BY count DESC LIMIT 20", days - 1
        )
    };

    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(e) => { warn!("[Trigr] Top apps query failed: {}", e); return serde_json::json!([]); }
    };

    let rows: Vec<serde_json::Value> = match stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "app": row.get::<_, String>(0).unwrap_or_default(),
            "count": row.get::<_, i64>(1).unwrap_or(0),
            "time_saved": row.get::<_, f64>(2).unwrap_or(0.0),
        }))
    }) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    };

    serde_json::json!(rows)
}

fn compute_efficiency(conn: &Connection, where_clause: &str) -> serde_json::Value {
    let count_query = format!(
        "SELECT COUNT(*), COALESCE(SUM(char_count), 0) FROM action_log WHERE action_type = 'expansion'{}",
        where_clause
    );
    let (total_exp, chars_expanded) = conn.query_row(&count_query, [], |row| {
        Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, i64>(1).unwrap_or(0)))
    }).unwrap_or((0, 0));

    let trigger_query = format!(
        "SELECT trigger_key, COUNT(*) FROM action_log WHERE action_type = 'expansion' AND trigger_key != ''{} GROUP BY trigger_key",
        where_clause
    );
    let mut trigger_chars: i64 = 0;
    if let Ok(mut stmt) = conn.prepare(&trigger_query) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0).unwrap_or_default(), row.get::<_, i64>(1).unwrap_or(0)))
        }) {
            for row in rows.flatten() {
                let trigger = row.0.strip_prefix("GLOBAL::EXPANSION::").unwrap_or(&row.0);
                trigger_chars += (trigger.len() as i64 + 1) * row.1;
            }
        }
    }

    serde_json::json!({
        "total_expansions": total_exp,
        "chars_expanded": chars_expanded,
        "chars_typed": trigger_chars,
        "ratio": if trigger_chars > 0 { chars_expanded as f64 / trigger_chars as f64 } else { 0.0 },
    })
}

fn handle_expansion_efficiency(conn: &Connection) -> serde_json::Value {
    // Week/month windows are local calendar days: today + 6/29 prior days.
    let week = compute_efficiency(conn, " AND DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-6 days')");
    let month = compute_efficiency(conn, " AND DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-29 days')");
    let all = compute_efficiency(conn, "");

    serde_json::json!({
        "week": week,
        "month": month,
        "all": all,
    })
}

fn handle_expansion_counts(conn: &Connection) -> serde_json::Value {
    let query = "SELECT trigger_key, COUNT(*) AS count FROM action_log \
                 WHERE action_type = 'expansion' AND trigger_key != '' \
                 GROUP BY trigger_key";
    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({}),
    };
    let mut map = serde_json::Map::new();
    let _ = stmt
        .query_map([], |row| {
            let trigger: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((trigger, count))
        })
        .map(|rows| {
            for row in rows.flatten() {
                map.insert(row.0, serde_json::json!(row.1));
            }
        });
    serde_json::Value::Object(map)
}

fn handle_hourly_heatmap(conn: &Connection, days: u32) -> serde_json::Value {
    // Returns array of { dow (0=Sun..6=Sat), hour (0-23), count, time_saved }.
    // Both day-of-week and hour are in local time, and the date window is
    // today + (days-1) prior local calendar days.
    let query = format!(
        "SELECT CAST(strftime('%w', timestamp, 'localtime') AS INTEGER) AS dow,
                CAST(strftime('%H', timestamp, 'localtime') AS INTEGER) AS hour,
                COUNT(*) AS count,
                COALESCE(SUM(time_saved), 0.0) AS saved
         FROM action_log
         WHERE DATE(timestamp, 'localtime') >= DATE('now', 'localtime', '-{} days')
         GROUP BY dow, hour
         ORDER BY dow, hour", days.saturating_sub(1)
    );
    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Heatmap query failed: {}", e);
            return serde_json::json!([]);
        }
    };

    let rows: Vec<serde_json::Value> = match stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "dow": row.get::<_, i64>(0).unwrap_or(0),
            "hour": row.get::<_, i64>(1).unwrap_or(0),
            "count": row.get::<_, i64>(2).unwrap_or(0),
            "time_saved": row.get::<_, f64>(3).unwrap_or(0.0),
        }))
    }) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    };

    serde_json::json!(rows)
}

fn handle_streaks(conn: &Connection) -> serde_json::Value {
    // Get all distinct dates with at least one action, sorted ascending
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT DATE(timestamp, 'localtime') AS d FROM action_log ORDER BY d ASC"
    ) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Streaks query failed: {}", e);
            return serde_json::json!({"current": 0, "longest": 0});
        }
    };

    let dates: Vec<String> = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_) => Vec::new(),
    };

    if dates.is_empty() {
        return serde_json::json!({"current": 0, "longest": 0});
    }

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut longest = 1u32;
    let mut streak = 1u32;

    for i in 1..dates.len() {
        if let (Ok(prev), Ok(curr)) = (
            chrono::NaiveDate::parse_from_str(&dates[i - 1], "%Y-%m-%d"),
            chrono::NaiveDate::parse_from_str(&dates[i], "%Y-%m-%d"),
        ) {
            if (curr - prev).num_days() == 1 {
                streak += 1;
            } else {
                streak = 1;
            }
            if streak > longest {
                longest = streak;
            }
        }
    }

    // Current streak: only counts if the last date is today or yesterday
    let mut current_streak: u32 = 0;
    if let Some(last) = dates.last() {
        if last == &today || is_yesterday(last) {
            current_streak = 1;
            for i in (0..dates.len().saturating_sub(1)).rev() {
                if let (Ok(prev), Ok(curr)) = (
                    chrono::NaiveDate::parse_from_str(&dates[i], "%Y-%m-%d"),
                    chrono::NaiveDate::parse_from_str(&dates[i + 1], "%Y-%m-%d"),
                ) {
                    if (curr - prev).num_days() == 1 {
                        current_streak += 1;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    serde_json::json!({
        "current": current_streak,
        "longest": longest,
    })
}

fn is_yesterday(date_str: &str) -> bool {
    if let Ok(d) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
        return d == yesterday;
    }
    false
}

fn handle_export_csv(conn: &Connection) -> String {
    let mut csv = String::from("timestamp,action_type,trigger,label,char_count,time_saved\n");

    let mut stmt = match conn.prepare(
        "SELECT timestamp, action_type, trigger_key, label, char_count, time_saved FROM action_log ORDER BY id ASC"
    ) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] CSV export query failed: {}", e);
            return csv;
        }
    };

    if let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0).unwrap_or_default(),
            row.get::<_, String>(1).unwrap_or_default(),
            row.get::<_, String>(2).unwrap_or_default(),
            row.get::<_, String>(3).unwrap_or_default(),
            row.get::<_, i64>(4).unwrap_or(0),
            row.get::<_, f64>(5).unwrap_or(0.0),
        ))
    }) {
        for row in rows.flatten() {
            // Escape CSV fields that contain commas or quotes
            let escape = |s: &str| {
                if s.contains(',') || s.contains('"') || s.contains('\n') {
                    format!("\"{}\"", s.replace('"', "\"\""))
                } else {
                    s.to_string()
                }
            };
            csv.push_str(&format!(
                "{},{},{},{},{},{:.1}\n",
                escape(&row.0), escape(&row.1), escape(&row.2), escape(&row.3), row.4, row.5
            ));
        }
    }

    csv
}

fn handle_reset(conn: &Connection) -> bool {
    match conn.execute("DELETE FROM action_log", []) {
        Ok(_) => {
            info!("[Trigr] Analytics data reset");
            true
        }
        Err(e) => {
            error!("[Trigr] Failed to reset analytics: {}", e);
            false
        }
    }
}

/// Retroactively recalculate time_saved for old "hotkey" and "macro" entries
/// using the current assignment map.  Runs once, updates in-place.
fn handle_migrate_time_saved(conn: &Connection, assignments: &std::collections::HashMap<String, serde_json::Value>) {
    // Check if already migrated
    let already_done: bool = conn
        .query_row(
            "SELECT value FROM analytics_meta WHERE key = 'time_saved_v2'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(|v| v == "done")
        .unwrap_or(false);

    if already_done {
        return;
    }

    // Collect rows that need updating: old "hotkey" or "macro" entries
    let mut stmt = match conn.prepare("SELECT id, action_type, trigger_key, char_count FROM action_log WHERE action_type IN ('hotkey', 'macro')") {
        Ok(s) => s,
        Err(e) => { error!("[Trigr] Migration query failed: {}", e); return; }
    };

    let rows: Vec<(i64, String, String, u32)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get::<_, u32>(3).unwrap_or(0)))
        })
        .unwrap_or_else(|_| panic!("query_map"))
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        info!("[Trigr] Analytics migration: no old entries to update");
        return;
    }

    let mut updated = 0u32;
    for (id, old_type, trigger_key, _char_count) in &rows {
        // Look up the current assignment to determine actual type
        let (new_type, new_time) = if let Some(macro_val) = assignments.get(trigger_key.as_str()) {
            let at = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
            match at {
                "macro" => {
                    // Compute from steps
                    let time: f64 = macro_val.get("data")
                        .and_then(|d| d.get("steps"))
                        .and_then(|s| s.as_array())
                        .map(|arr| arr.iter().map(|s| {
                            let st = s.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            step_time_saved(st)
                        }).sum())
                        .unwrap_or(3.0);
                    ("macro".to_string(), time)
                }
                "hotkey" => (at.to_string(), 0.0),
                "text" => (at.to_string(), 3.0),
                "app" | "url" | "folder" => (at.to_string(), 3.0),
                _ => (at.to_string(), 0.0),
            }
        } else {
            // Assignment no longer exists — use old type with new rules
            if old_type == "macro" {
                ("macro".to_string(), 3.0) // can't resolve steps, keep a reasonable default
            } else {
                ("hotkey".to_string(), 0.0) // assume key-for-key
            }
        };

        if let Err(e) = conn.execute(
            "UPDATE action_log SET action_type = ?1, time_saved = ?2 WHERE id = ?3",
            rusqlite::params![new_type, new_time, id],
        ) {
            error!("[Trigr] Migration update failed for id {}: {}", id, e);
        } else {
            updated += 1;
        }
    }
    info!("[Trigr] Analytics migration: updated {}/{} entries", updated, rows.len());

    // Mark migration as done
    let _ = conn.execute(
        "INSERT OR REPLACE INTO analytics_meta (key, value) VALUES ('time_saved_v2', 'done')",
        [],
    );
}

fn empty_stats() -> serde_json::Value {
    serde_json::json!({
        "total_actions": 0,
        "total_time_saved_seconds": 0.0,
        "actions_today": 0,
        "time_saved_today_seconds": 0.0,
        "actions_last_7_days": 0,
        "time_saved_last_7_days_seconds": 0.0,
        "actions_last_14_days": 0,
        "time_saved_last_14_days_seconds": 0.0,
        "actions_last_30_days": 0,
        "time_saved_last_30_days_seconds": 0.0,
        "best_day_time_saved_seconds": 0.0,
        "best_7_days_time_saved_seconds": 0.0,
        "expansions": 0,
        "hotkeys": 0,
        "macros": 0,
    })
}
