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
}

// ── Writer thread channel ──────────────────────────────────────────────────

static ANALYTICS_TX: OnceLock<Mutex<mpsc::Sender<AnalyticsMsg>>> = OnceLock::new();

enum AnalyticsMsg {
    Log(AnalyticsEvent),
    GetStats(mpsc::Sender<serde_json::Value>),
    GetDailyChart(u32, mpsc::Sender<serde_json::Value>),
    GetAssignmentBreakdown(mpsc::Sender<serde_json::Value>),
    GetHourlyHeatmap(mpsc::Sender<serde_json::Value>),
    GetStreaks(mpsc::Sender<serde_json::Value>),
    ExportCsv(mpsc::Sender<String>),
    Reset(mpsc::Sender<bool>),
}

// ── Initialise ─────────────────────────────────────────────────────────────

pub fn init(app_data_dir: PathBuf) {
    let db_path = app_data_dir.join("trigr-analytics.db");
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

            // Schema migration: add trigger and label columns if missing
            let _ = conn.execute_batch("ALTER TABLE action_log ADD COLUMN trigger_key TEXT NOT NULL DEFAULT '';");
            let _ = conn.execute_batch("ALTER TABLE action_log ADD COLUMN label TEXT NOT NULL DEFAULT '';");

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
                    AnalyticsMsg::GetAssignmentBreakdown(reply) => {
                        let data = handle_assignment_breakdown(&conn);
                        let _ = reply.send(data);
                    }
                    AnalyticsMsg::GetHourlyHeatmap(reply) => {
                        let data = handle_hourly_heatmap(&conn);
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
                }
            }
        })
        .expect("Failed to spawn analytics writer thread");
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Log an action. Non-blocking — sends to writer thread via channel.
pub fn log_action(action_type: &str, char_count: u32, trigger: &str, label: &str) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::Log(AnalyticsEvent {
                action_type: action_type.to_string(),
                char_count,
                trigger: trigger.to_string(),
                label: label.to_string(),
            }));
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

/// Get per-assignment breakdown (top 50 by usage).
pub fn get_assignment_breakdown() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetAssignmentBreakdown(reply), serde_json::json!([]))
}

/// Get hourly heatmap (7 days x 24 hours).
pub fn get_hourly_heatmap() -> serde_json::Value {
    send_and_recv(|reply| AnalyticsMsg::GetHourlyHeatmap(reply), serde_json::json!([]))
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

fn handle_log(conn: &Connection, event: AnalyticsEvent) {
    let time_saved = match event.action_type.as_str() {
        "expansion" => event.char_count as f64 * 0.3,
        "macro" => 5.0,
        _ => 3.0, // hotkey and any other type
    };

    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = conn.execute(
        "INSERT INTO action_log (timestamp, action_type, char_count, time_saved, trigger_key, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![now, event.action_type, event.char_count, time_saved, event.trigger, event.label],
    ) {
        error!("[Trigr] Failed to log analytics event: {}", e);
    }
}

fn handle_get_stats(conn: &Connection) -> serde_json::Value {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let (total_actions, total_time_saved) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_today, time_saved_today) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log WHERE timestamp LIKE ?1",
            rusqlite::params![format!("{}%", today)],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let (actions_last_7_days, time_saved_last_7_days) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log WHERE timestamp >= datetime('now', '-7 days')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    let best_day = conn
        .query_row(
            "SELECT COALESCE(MAX(day_total), 0.0) FROM (
                SELECT SUM(time_saved) AS day_total FROM action_log GROUP BY DATE(timestamp)
            )",
            [],
            |row| row.get::<_, f64>(0),
        )
        .unwrap_or(0.0);

    let best_7_days = conn
        .query_row(
            "SELECT COALESCE(MAX(window_total), 0.0) FROM (
                SELECT SUM(a2.time_saved) AS window_total
                FROM (SELECT DISTINCT DATE(timestamp) AS d FROM action_log) days
                JOIN action_log a2 ON DATE(a2.timestamp) BETWEEN DATE(days.d, '-6 days') AND days.d
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
        "best_day_time_saved_seconds": best_day,
        "best_7_days_time_saved_seconds": best_7_days,
        "expansions": expansions,
        "hotkeys": hotkeys,
        "macros": macros,
    })
}

// ── Pro analytics handlers ─────────────────────────────────────────────────

fn handle_daily_chart(conn: &Connection, days: u32) -> serde_json::Value {
    let mut stmt = match conn.prepare(
        "SELECT DATE(timestamp) AS day, COUNT(*) AS actions, COALESCE(SUM(time_saved), 0.0) AS saved
         FROM action_log
         WHERE timestamp >= datetime('now', ?1)
         GROUP BY day
         ORDER BY day ASC"
    ) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Daily chart query failed: {}", e);
            return serde_json::json!([]);
        }
    };

    let offset = format!("-{} days", days);
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

fn handle_assignment_breakdown(conn: &Connection) -> serde_json::Value {
    let mut stmt = match conn.prepare(
        "SELECT trigger_key, label, action_type, COUNT(*) AS count, COALESCE(SUM(time_saved), 0.0) AS saved, MAX(timestamp) AS last_fired
         FROM action_log
         WHERE trigger_key != ''
         GROUP BY trigger_key
         ORDER BY count DESC
         LIMIT 50"
    ) {
        Ok(s) => s,
        Err(e) => {
            warn!("[Trigr] Assignment breakdown query failed: {}", e);
            return serde_json::json!([]);
        }
    };

    let rows: Vec<serde_json::Value> = match stmt.query_map([], |row| {
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

fn handle_hourly_heatmap(conn: &Connection) -> serde_json::Value {
    // Returns array of { dow (0=Sun..6=Sat), hour (0-23), count }
    let mut stmt = match conn.prepare(
        "SELECT CAST(strftime('%w', timestamp) AS INTEGER) AS dow,
                CAST(strftime('%H', timestamp, 'localtime') AS INTEGER) AS hour,
                COUNT(*) AS count
         FROM action_log
         WHERE timestamp >= datetime('now', '-7 days')
         GROUP BY dow, hour
         ORDER BY dow, hour"
    ) {
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
        "SELECT DISTINCT DATE(timestamp) AS d FROM action_log ORDER BY d ASC"
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
    let mut current_streak = 1u32;
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
    current_streak = 0;
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

fn empty_stats() -> serde_json::Value {
    serde_json::json!({
        "total_actions": 0,
        "total_time_saved_seconds": 0.0,
        "actions_today": 0,
        "time_saved_today_seconds": 0.0,
        "actions_last_7_days": 0,
        "time_saved_last_7_days_seconds": 0.0,
        "best_day_time_saved_seconds": 0.0,
        "best_7_days_time_saved_seconds": 0.0,
        "expansions": 0,
        "hotkeys": 0,
        "macros": 0,
    })
}
