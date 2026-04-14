use log::{error, info};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;

// ── Analytics event ────────────────────────────────────────────────────────

struct AnalyticsEvent {
    action_type: String,
    char_count: u32,
}

// ── Writer thread channel ──────────────────────────────────────────────────

static ANALYTICS_TX: OnceLock<Mutex<mpsc::Sender<AnalyticsMsg>>> = OnceLock::new();

enum AnalyticsMsg {
    Log(AnalyticsEvent),
    GetStats(mpsc::Sender<serde_json::Value>),
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
pub fn log_action(action_type: &str, char_count: u32) {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(AnalyticsMsg::Log(AnalyticsEvent {
                action_type: action_type.to_string(),
                char_count,
            }));
        }
    }
}

/// Get aggregate stats. Blocks briefly while the writer thread queries.
pub fn get_stats() -> serde_json::Value {
    if let Some(tx) = ANALYTICS_TX.get() {
        if let Ok(tx) = tx.lock() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(AnalyticsMsg::GetStats(reply_tx)).is_ok() {
                if let Ok(stats) = reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    return stats;
                }
            }
        }
    }
    empty_stats()
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

// ── Writer thread handlers ─────────────────────────────────────────────────

fn handle_log(conn: &Connection, event: AnalyticsEvent) {
    let time_saved = match event.action_type.as_str() {
        "expansion" => event.char_count as f64 * 0.3,
        "macro" => 5.0,
        _ => 3.0, // hotkey and any other type
    };

    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = conn.execute(
        "INSERT INTO action_log (timestamp, action_type, char_count, time_saved) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![now, event.action_type, event.char_count, time_saved],
    ) {
        error!("[Trigr] Failed to log analytics event: {}", e);
    }
}

fn handle_get_stats(conn: &Connection) -> serde_json::Value {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Total stats
    let (total_actions, total_time_saved) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    // Today stats
    let (actions_today, time_saved_today) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log WHERE timestamp LIKE ?1",
            rusqlite::params![format!("{}%", today)],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    // Last 7 days stats
    let (actions_last_7_days, time_saved_last_7_days) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(time_saved), 0.0) FROM action_log WHERE timestamp >= datetime('now', '-7 days')",
            [],
            |row| Ok((row.get::<_, i64>(0).unwrap_or(0), row.get::<_, f64>(1).unwrap_or(0.0))),
        )
        .unwrap_or((0, 0.0));

    // Best day — highest time_saved on any single calendar day
    let best_day = conn
        .query_row(
            "SELECT COALESCE(MAX(day_total), 0.0) FROM (
                SELECT SUM(time_saved) AS day_total FROM action_log GROUP BY DATE(timestamp)
            )",
            [],
            |row| row.get::<_, f64>(0),
        )
        .unwrap_or(0.0);

    // Best 7 days — highest rolling 7-day window total
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

    // Breakdown by type
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
