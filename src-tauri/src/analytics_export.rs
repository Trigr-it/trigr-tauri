//! Analytics XLSX export — one formatted sheet per analytics section.
//!
//! Runs on the analytics writer thread (it owns the SQLite connection), so
//! every query helper in analytics.rs is reachable directly. Pure-Rust
//! dependency chain (rust_xlsxwriter → zip → zlib-rs) — ARM64 safe.
//!
//! Sheet list mirrors the Analytics panel top-to-bottom: Overview, Daily
//! Activity (with embedded chart), Breakdown, Heatmap (conditional colour
//! scale), Key Mappings, Text Expansions, Top Apps, Expansion Efficiency,
//! and the raw Action Log that used to be the whole CSV export.

use rusqlite::Connection;
use rust_xlsxwriter::{
    Chart, ChartLine, ChartSolidFill, ChartType, Color, ConditionalFormat2ColorScale, Format,
    FormatAlign, Workbook, Worksheet,
};
use serde_json::Value;

// ── Brand palette ───────────────────────────────────────────────────────────
// Keyfire gold on dark ink, matching the app accent. Values here are the only
// hardcoded colours in the exporter — keep in sync with global.css --accent.
const GOLD: u32 = 0xE8A020;
const INK: u32 = 0x16161E;
const MUTED: u32 = 0x6B6B88;
const ROW_ALT: u32 = 0xF6F1E7; // warm off-white zebra stripe

struct Styles {
    title: Format,
    subtitle: Format,
    section: Format,
    header: Format,
    cell: Format,
    cell_alt: Format,
    int: Format,
    int_alt: Format,
    num1: Format,
    num1_alt: Format,
    dur: Format,
    dur_alt: Format,
    date: Format,
    date_alt: Format,
    label: Format,
    accent_value: Format,
}

fn styles() -> Styles {
    let title = Format::new()
        .set_bold()
        .set_font_size(18)
        .set_font_color(Color::RGB(INK));
    let subtitle = Format::new()
        .set_font_size(10)
        .set_font_color(Color::RGB(MUTED));
    let section = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::RGB(GOLD));
    let header = Format::new()
        .set_bold()
        .set_font_color(Color::RGB(INK))
        .set_background_color(Color::RGB(GOLD))
        .set_align(FormatAlign::Left);
    let cell = Format::new();
    let cell_alt = Format::new().set_background_color(Color::RGB(ROW_ALT));
    let int = Format::new().set_num_format("#,##0");
    let int_alt = Format::new()
        .set_num_format("#,##0")
        .set_background_color(Color::RGB(ROW_ALT));
    let num1 = Format::new().set_num_format("#,##0.0");
    let num1_alt = Format::new()
        .set_num_format("#,##0.0")
        .set_background_color(Color::RGB(ROW_ALT));
    let dur = Format::new().set_num_format("[h]:mm:ss");
    let dur_alt = Format::new()
        .set_num_format("[h]:mm:ss")
        .set_background_color(Color::RGB(ROW_ALT));
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let date_alt = Format::new()
        .set_num_format("yyyy-mm-dd")
        .set_background_color(Color::RGB(ROW_ALT));
    let label = Format::new().set_font_color(Color::RGB(MUTED));
    let accent_value = Format::new()
        .set_bold()
        .set_font_color(Color::RGB(GOLD))
        .set_num_format("[h]:mm:ss");
    Styles {
        title,
        subtitle,
        section,
        header,
        cell,
        cell_alt,
        int,
        int_alt,
        num1,
        num1_alt,
        dur,
        dur_alt,
        date,
        date_alt,
        label,
        accent_value,
    }
}

/// Seconds → Excel duration serial (days).
fn dur(seconds: f64) -> f64 {
    seconds / 86_400.0
}

fn jf64(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn jstr<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

use crate::analytics::Window;

/// The list of local calendar dates a window's daily sheet should show.
/// Presets anchor at today; custom ranges iterate from..=to (capped at 366
/// rows so a pathological range can't build a million-row sheet). "Today"
/// widens to a week — a 1-day chart is unreadable.
fn chart_dates(win: &Window) -> Vec<chrono::NaiveDate> {
    let today = chrono::Local::now().date_naive();
    let last_n = |n: i64| -> Vec<chrono::NaiveDate> {
        (0..n).rev().map(|i| today - chrono::Duration::days(i)).collect()
    };
    match win {
        Window::All => last_n(30),
        Window::Days(1) => last_n(7),
        Window::Days(n) => last_n((*n).clamp(7, 30) as i64),
        Window::Range(f, t) => {
            let from = chrono::NaiveDate::parse_from_str(f, "%Y-%m-%d");
            let to = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d");
            match (from, to) {
                (Ok(from), Ok(to)) if from <= to => {
                    let span = (to - from).num_days().min(365);
                    (0..=span).map(|i| from + chrono::Duration::days(i)).collect()
                }
                _ => last_n(30),
            }
        }
    }
}

/// Build and save the full workbook. Called on the analytics writer thread.
/// `win` scopes the data sheets; the Overview summary always shows the full
/// today/7/14/30 spread for orientation.
pub fn export_xlsx(conn: &Connection, path: &std::path::Path, win: &Window) -> Result<(), String> {
    let s = styles();
    let mut wb = Workbook::new();

    let dates = chart_dates(win);
    // The daily sheet's query window must cover the fill dates, not the raw
    // export window ("Today" widens the chart to a week).
    let chart_win = match win {
        Window::All => Window::Days(30),
        Window::Days(n) => Window::Days((*n).clamp(7, 30)),
        Window::Range(_, _) => win.clone(),
    };
    // All-time heatmaps stay a 30-day picture — a multi-year dow/hour
    // aggregate washes out to uniform noise.
    let heatmap_win = match win {
        Window::All => Window::Days(30),
        other => other.clone(),
    };

    let stats = crate::analytics::handle_get_stats(conn);
    let streaks = crate::analytics::handle_streaks(conn);
    let daily = crate::analytics::handle_daily_chart(conn, &chart_win);
    let bd_ranges: Vec<(String, Value)> = if matches!(win, Window::All) {
        [Window::All, Window::Days(7), Window::Days(14), Window::Days(30)]
            .iter()
            .map(|w| (w.label(), crate::analytics::handle_type_breakdown(conn, w)))
            .collect()
    } else {
        vec![(win.label(), crate::analytics::handle_type_breakdown(conn, win))]
    };
    let heatmap = crate::analytics::handle_hourly_heatmap(conn, &heatmap_win);
    let assignments = crate::analytics::handle_assignment_breakdown(conn, win);
    let top_apps = crate::analytics::handle_top_apps(conn, win);
    // Efficiency sheet keeps the classic week/month/all comparison regardless
    // of export scope — the ratios are trends, not period sums.
    let efficiency = crate::analytics::handle_expansion_efficiency(conn, &Window::All);

    sheet_overview(wb.add_worksheet(), &s, &stats, &streaks, win)?;
    sheet_daily(wb.add_worksheet(), &s, &daily, &dates)?;
    sheet_breakdown(wb.add_worksheet(), &s, &bd_ranges)?;
    sheet_heatmap(wb.add_worksheet(), &s, &heatmap, &heatmap_win)?;
    sheet_leaderboard(
        wb.add_worksheet(),
        &s,
        "Key Mappings",
        &assignments,
        LeaderboardKind::KeyMappings,
    )?;
    sheet_leaderboard(
        wb.add_worksheet(),
        &s,
        "Text Expansions",
        &assignments,
        LeaderboardKind::Expansions,
    )?;
    sheet_top_apps(wb.add_worksheet(), &s, &top_apps)?;
    sheet_efficiency(wb.add_worksheet(), &s, &efficiency)?;
    sheet_action_log(wb.add_worksheet(), &s, conn, win)?;

    wb.save(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn xe(e: rust_xlsxwriter::XlsxError) -> String {
    e.to_string()
}

// ── Overview ────────────────────────────────────────────────────────────────

fn sheet_overview(
    ws: &mut Worksheet,
    s: &Styles,
    stats: &Value,
    streaks: &Value,
    win: &Window,
) -> Result<(), String> {
    ws.set_name("Overview").map_err(xe)?;
    ws.set_column_width(0, 26).map_err(xe)?;
    ws.set_column_width(1, 16).map_err(xe)?;
    ws.set_column_width(2, 16).map_err(xe)?;

    ws.merge_range(0, 0, 0, 2, "Keyfire Analytics", &s.title)
        .map_err(xe)?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    ws.merge_range(
        1,
        0,
        1,
        2,
        &format!(
            "Generated {} - export period: {} - all data stored locally on this device",
            generated,
            win.label()
        ),
        &s.subtitle,
    )
    .map_err(xe)?;

    // Activity summary
    let mut r = 3;
    ws.write_string_with_format(r, 0, "ACTIVITY SUMMARY", &s.section)
        .map_err(xe)?;
    r += 1;
    for (c, h) in ["Period", "Actions", "Time saved"].iter().enumerate() {
        ws.write_string_with_format(r, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    r += 1;
    let periods = [
        ("Today", "actions_today", "time_saved_today_seconds"),
        ("Last 7 days", "actions_last_7_days", "time_saved_last_7_days_seconds"),
        ("Last 14 days", "actions_last_14_days", "time_saved_last_14_days_seconds"),
        ("Last 30 days", "actions_last_30_days", "time_saved_last_30_days_seconds"),
    ];
    for (label, ak, tk) in periods {
        ws.write_string(r, 0, label).map_err(xe)?;
        ws.write_number_with_format(r, 1, jf64(stats, ak), &s.int)
            .map_err(xe)?;
        ws.write_number_with_format(r, 2, dur(jf64(stats, tk)), &s.dur)
            .map_err(xe)?;
        r += 1;
    }

    // Records
    r += 1;
    ws.write_string_with_format(r, 0, "RECORDS", &s.section)
        .map_err(xe)?;
    r += 1;
    let records = [
        ("All-time actions", jf64(stats, "total_actions"), false),
        ("All-time time saved", jf64(stats, "total_time_saved_seconds"), true),
        ("Best day (time saved)", jf64(stats, "best_day_time_saved_seconds"), true),
        ("Best 7 days (time saved)", jf64(stats, "best_7_days_time_saved_seconds"), true),
    ];
    for (label, val, is_dur) in records {
        ws.write_string_with_format(r, 0, label, &s.label).map_err(xe)?;
        if is_dur {
            ws.write_number_with_format(r, 1, dur(val), &s.accent_value)
                .map_err(xe)?;
        } else {
            ws.write_number_with_format(r, 1, val, &s.int).map_err(xe)?;
        }
        r += 1;
    }

    // Streaks
    r += 1;
    ws.write_string_with_format(r, 0, "STREAKS", &s.section)
        .map_err(xe)?;
    r += 1;
    ws.write_string_with_format(r, 0, "Current streak (days)", &s.label)
        .map_err(xe)?;
    ws.write_number_with_format(r, 1, jf64(streaks, "current"), &s.int)
        .map_err(xe)?;
    r += 1;
    ws.write_string_with_format(r, 0, "Longest streak (days)", &s.label)
        .map_err(xe)?;
    ws.write_number_with_format(r, 1, jf64(streaks, "longest"), &s.int)
        .map_err(xe)?;
    r += 1;

    // Action totals
    r += 1;
    ws.write_string_with_format(r, 0, "ACTION TOTALS (ALL TIME)", &s.section)
        .map_err(xe)?;
    r += 1;
    let totals = [
        ("Text expansions", "expansions"),
        ("Hotkey actions", "hotkeys"),
        ("Macros", "macros"),
        ("Search templates", "search_templates"),
        ("Typos fixed", "autocorrects"),
    ];
    for (label, key) in totals {
        ws.write_string_with_format(r, 0, label, &s.label).map_err(xe)?;
        ws.write_number_with_format(r, 1, jf64(stats, key), &s.int)
            .map_err(xe)?;
        r += 1;
    }
    Ok(())
}

// ── Daily Activity ──────────────────────────────────────────────────────────

fn sheet_daily(
    ws: &mut Worksheet,
    s: &Styles,
    daily: &Value,
    dates: &[chrono::NaiveDate],
) -> Result<(), String> {
    ws.set_name("Daily Activity").map_err(xe)?;
    ws.set_column_width(0, 12).map_err(xe)?;
    ws.set_column_width(1, 10).map_err(xe)?;
    ws.set_column_width(2, 12).map_err(xe)?;
    ws.set_column_width(3, 14).map_err(xe)?;

    for (c, h) in ["Date", "Actions", "Time saved", "Minutes saved"].iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    ws.set_freeze_panes(1, 0).map_err(xe)?;

    // Fill every local day in the window, zeroing gaps so the chart is continuous.
    let mut map = std::collections::HashMap::new();
    if let Some(arr) = daily.as_array() {
        for row in arr {
            map.insert(jstr(row, "date").to_string(), (jf64(row, "actions"), jf64(row, "time_saved")));
        }
    }
    let mut r: u32 = 1;
    for d in dates {
        let key = d.format("%Y-%m-%d").to_string();
        let (actions, saved) = map.get(&key).copied().unwrap_or((0.0, 0.0));
        let (cf, intf, durf, numf) = if r % 2 == 0 {
            (&s.cell_alt, &s.int_alt, &s.dur_alt, &s.num1_alt)
        } else {
            (&s.cell, &s.int, &s.dur, &s.num1)
        };
        ws.write_string_with_format(r, 0, &key, cf).map_err(xe)?;
        ws.write_number_with_format(r, 1, actions, intf).map_err(xe)?;
        ws.write_number_with_format(r, 2, dur(saved), durf).map_err(xe)?;
        ws.write_number_with_format(r, 3, saved / 60.0, numf).map_err(xe)?;
        r += 1;
    }
    let last = r - 1;

    // Embedded chart: actions as columns, minutes saved as a line.
    let mut chart = Chart::new(ChartType::Column);
    chart
        .add_series()
        .set_name("Actions")
        .set_categories(("Daily Activity", 1, 0, last, 0))
        .set_values(("Daily Activity", 1, 1, last, 1))
        .set_format(ChartSolidFill::new().set_color(Color::RGB(GOLD)));
    let mut line = Chart::new(ChartType::Line);
    line.add_series()
        .set_name("Minutes saved")
        .set_categories(("Daily Activity", 1, 0, last, 0))
        .set_values(("Daily Activity", 1, 3, last, 3))
        .set_format(ChartLine::new().set_color(Color::RGB(INK)))
        .set_secondary_axis(true);
    chart.combine(&line);
    chart.title().set_name(&format!("Daily activity ({} days)", dates.len()));
    chart.set_width(720).set_height(320);
    ws.insert_chart(1, 5, &chart).map_err(xe)?;
    Ok(())
}

// ── Breakdown ───────────────────────────────────────────────────────────────

fn sheet_breakdown(
    ws: &mut Worksheet,
    s: &Styles,
    ranges: &[(String, Value)],
) -> Result<(), String> {
    ws.set_name("Breakdown").map_err(xe)?;
    ws.set_column_width(0, 18).map_err(xe)?;

    // Header: Type | <range> Actions | <range> Time saved | ...
    ws.write_string_with_format(0, 0, "Type", &s.header).map_err(xe)?;
    for (i, (name, _)) in ranges.iter().enumerate() {
        let base = 1 + (i as u16) * 2;
        ws.set_column_width(base, 14).map_err(xe)?;
        ws.set_column_width(base + 1, 14).map_err(xe)?;
        ws.write_string_with_format(0, base, &format!("{} - actions", name), &s.header)
            .map_err(xe)?;
        ws.write_string_with_format(0, base + 1, &format!("{} - saved", name), &s.header)
            .map_err(xe)?;
    }
    ws.set_freeze_panes(1, 1).map_err(xe)?;

    let rows = [
        ("Expansions", "expansions", "expansions_saved"),
        ("Hotkey actions", "hotkeys", "hotkeys_saved"),
        ("Macros", "macros", "macros_saved"),
        ("Typos fixed", "autocorrects", "autocorrects_saved"),
        ("Total", "total", "time_saved"),
    ];
    for (ri, (label, count_key, saved_key)) in rows.iter().enumerate() {
        let r = 1 + ri as u32;
        let (cf, intf, durf) = if r % 2 == 0 {
            (&s.cell_alt, &s.int_alt, &s.dur_alt)
        } else {
            (&s.cell, &s.int, &s.dur)
        };
        ws.write_string_with_format(r, 0, *label, cf).map_err(xe)?;
        for (i, (_, bd)) in ranges.iter().enumerate() {
            let base = 1 + (i as u16) * 2;
            ws.write_number_with_format(r, base, jf64(bd, count_key), intf)
                .map_err(xe)?;
            ws.write_number_with_format(r, base + 1, dur(jf64(bd, saved_key)), durf)
                .map_err(xe)?;
        }
    }
    Ok(())
}

// ── Heatmap ─────────────────────────────────────────────────────────────────

fn sheet_heatmap(ws: &mut Worksheet, s: &Styles, heatmap: &Value, win: &Window) -> Result<(), String> {
    ws.set_name("Heatmap").map_err(xe)?;
    const DOW: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    let window = win.label().to_lowercase();
    ws.write_string_with_format(0, 0, &format!("Actions by hour ({})", window), &s.section)
        .map_err(xe)?;

    // Header row: hours 0..23
    ws.write_string_with_format(1, 0, "Day", &s.header).map_err(xe)?;
    for h in 0..24u16 {
        ws.set_column_width(h + 1, 5).map_err(xe)?;
        ws.write_string_with_format(1, h + 1, &format!("{:02}", h), &s.header)
            .map_err(xe)?;
    }
    ws.set_column_width(0, 8).map_err(xe)?;

    let mut grid = [[0.0f64; 24]; 7];
    let mut saved_grid = [[0.0f64; 24]; 7];
    if let Some(arr) = heatmap.as_array() {
        for cell in arr {
            let d = jf64(cell, "dow") as usize;
            let h = jf64(cell, "hour") as usize;
            if d < 7 && h < 24 {
                grid[d][h] = jf64(cell, "count");
                saved_grid[d][h] = jf64(cell, "time_saved");
            }
        }
    }
    for (d, name) in DOW.iter().enumerate() {
        let r = 2 + d as u32;
        ws.write_string_with_format(r, 0, *name, &s.label).map_err(xe)?;
        for h in 0..24usize {
            ws.write_number_with_format(r, (h + 1) as u16, grid[d][h], &s.int)
                .map_err(xe)?;
        }
    }
    let scale = ConditionalFormat2ColorScale::new()
        .set_minimum_color(Color::RGB(0xFFFFFF))
        .set_maximum_color(Color::RGB(GOLD));
    ws.add_conditional_format(2, 1, 8, 24, &scale).map_err(xe)?;

    // Second grid: minutes saved by hour
    ws.write_string_with_format(11, 0, &format!("Minutes saved by hour ({})", window), &s.section)
        .map_err(xe)?;
    ws.write_string_with_format(12, 0, "Day", &s.header).map_err(xe)?;
    for h in 0..24u16 {
        ws.write_string_with_format(12, h + 1, &format!("{:02}", h), &s.header)
            .map_err(xe)?;
    }
    for (d, name) in DOW.iter().enumerate() {
        let r = 13 + d as u32;
        ws.write_string_with_format(r, 0, *name, &s.label).map_err(xe)?;
        for h in 0..24usize {
            ws.write_number_with_format(r, (h + 1) as u16, saved_grid[d][h] / 60.0, &s.num1)
                .map_err(xe)?;
        }
    }
    let scale2 = ConditionalFormat2ColorScale::new()
        .set_minimum_color(Color::RGB(0xFFFFFF))
        .set_maximum_color(Color::RGB(GOLD));
    ws.add_conditional_format(13, 1, 19, 24, &scale2).map_err(xe)?;
    Ok(())
}

// ── Leaderboards (Key Mappings / Text Expansions) ───────────────────────────

enum LeaderboardKind {
    KeyMappings,
    Expansions,
}

fn sheet_leaderboard(
    ws: &mut Worksheet,
    s: &Styles,
    name: &str,
    assignments: &Value,
    kind: LeaderboardKind,
) -> Result<(), String> {
    ws.set_name(name).map_err(xe)?;
    let headers = ["Rank", "Label", "Trigger", "Type", "Fires", "Time saved", "Last fired"];
    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    ws.set_column_width(0, 6).map_err(xe)?;
    ws.set_column_width(1, 32).map_err(xe)?;
    ws.set_column_width(2, 36).map_err(xe)?;
    ws.set_column_width(3, 14).map_err(xe)?;
    ws.set_column_width(4, 8).map_err(xe)?;
    ws.set_column_width(5, 12).map_err(xe)?;
    ws.set_column_width(6, 20).map_err(xe)?;
    ws.set_freeze_panes(1, 0).map_err(xe)?;

    // Mirrors the panel's filters: key-mapping types with time saved vs expansions.
    const KEY_TYPES: [&str; 8] = [
        "hotkey", "text", "app", "url", "folder", "macro", "search_template", "ahk",
    ];
    let empty = Vec::new();
    let rows = assignments.as_array().unwrap_or(&empty);
    let mut r: u32 = 1;
    for item in rows {
        let ty = jstr(item, "type");
        let keep = match kind {
            LeaderboardKind::KeyMappings => {
                KEY_TYPES.contains(&ty) && jf64(item, "time_saved") > 0.0
            }
            LeaderboardKind::Expansions => ty == "expansion",
        };
        if !keep {
            continue;
        }
        let (cf, intf, durf) = if r % 2 == 0 {
            (&s.cell_alt, &s.int_alt, &s.dur_alt)
        } else {
            (&s.cell, &s.int, &s.dur)
        };
        let label = jstr(item, "label");
        let trigger = jstr(item, "trigger");
        ws.write_number_with_format(r, 0, r as f64, intf).map_err(xe)?;
        ws.write_string_with_format(r, 1, if label.is_empty() { trigger } else { label }, cf)
            .map_err(xe)?;
        ws.write_string_with_format(r, 2, trigger, cf).map_err(xe)?;
        ws.write_string_with_format(r, 3, ty, cf).map_err(xe)?;
        ws.write_number_with_format(r, 4, jf64(item, "count"), intf)
            .map_err(xe)?;
        ws.write_number_with_format(r, 5, dur(jf64(item, "time_saved")), durf)
            .map_err(xe)?;
        ws.write_string_with_format(r, 6, jstr(item, "last_fired"), cf)
            .map_err(xe)?;
        r += 1;
    }
    if r > 1 {
        ws.autofilter(0, 0, r - 1, 6).map_err(xe)?;
    }
    Ok(())
}

// ── Top Apps ────────────────────────────────────────────────────────────────

fn sheet_top_apps(ws: &mut Worksheet, s: &Styles, top_apps: &Value) -> Result<(), String> {
    ws.set_name("Top Apps").map_err(xe)?;
    for (c, h) in ["Rank", "App", "Actions", "Time saved"].iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    ws.set_column_width(0, 6).map_err(xe)?;
    ws.set_column_width(1, 28).map_err(xe)?;
    ws.set_column_width(2, 10).map_err(xe)?;
    ws.set_column_width(3, 12).map_err(xe)?;
    ws.set_freeze_panes(1, 0).map_err(xe)?;

    let empty = Vec::new();
    let rows = top_apps.as_array().unwrap_or(&empty);
    for (i, item) in rows.iter().enumerate() {
        let r = 1 + i as u32;
        let (cf, intf, durf) = if r % 2 == 0 {
            (&s.cell_alt, &s.int_alt, &s.dur_alt)
        } else {
            (&s.cell, &s.int, &s.dur)
        };
        ws.write_number_with_format(r, 0, (i + 1) as f64, intf).map_err(xe)?;
        ws.write_string_with_format(r, 1, jstr(item, "app"), cf).map_err(xe)?;
        ws.write_number_with_format(r, 2, jf64(item, "count"), intf)
            .map_err(xe)?;
        ws.write_number_with_format(r, 3, dur(jf64(item, "time_saved")), durf)
            .map_err(xe)?;
    }
    Ok(())
}

// ── Expansion Efficiency ────────────────────────────────────────────────────

fn sheet_efficiency(ws: &mut Worksheet, s: &Styles, eff: &Value) -> Result<(), String> {
    ws.set_name("Expansion Efficiency").map_err(xe)?;
    let headers = ["Window", "Expansions fired", "Characters typed", "Characters expanded", "Efficiency ratio"];
    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    for c in 0..5u16 {
        ws.set_column_width(c, 20).map_err(xe)?;
    }
    let windows = [("This week", "week"), ("This month", "month"), ("All time", "all")];
    for (i, (label, key)) in windows.iter().enumerate() {
        let r = 1 + i as u32;
        let (cf, intf, numf) = if r % 2 == 0 {
            (&s.cell_alt, &s.int_alt, &s.num1_alt)
        } else {
            (&s.cell, &s.int, &s.num1)
        };
        let w = eff.get(key).cloned().unwrap_or(Value::Null);
        ws.write_string_with_format(r, 0, *label, cf).map_err(xe)?;
        ws.write_number_with_format(r, 1, jf64(&w, "total_expansions"), intf)
            .map_err(xe)?;
        ws.write_number_with_format(r, 2, jf64(&w, "chars_typed"), intf)
            .map_err(xe)?;
        ws.write_number_with_format(r, 3, jf64(&w, "chars_expanded"), intf)
            .map_err(xe)?;
        ws.write_number_with_format(r, 4, jf64(&w, "ratio"), numf)
            .map_err(xe)?;
    }
    Ok(())
}

// ── Action Log (raw) ────────────────────────────────────────────────────────

fn sheet_action_log(ws: &mut Worksheet, s: &Styles, conn: &Connection, win: &Window) -> Result<(), String> {
    ws.set_name("Action Log").map_err(xe)?;
    let headers = ["Timestamp (UTC)", "Type", "Trigger", "Label", "Characters", "Time saved (s)", "Target app"];
    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &s.header)
            .map_err(xe)?;
    }
    ws.set_column_width(0, 24).map_err(xe)?;
    ws.set_column_width(1, 14).map_err(xe)?;
    ws.set_column_width(2, 36).map_err(xe)?;
    ws.set_column_width(3, 28).map_err(xe)?;
    ws.set_column_width(4, 10).map_err(xe)?;
    ws.set_column_width(5, 12).map_err(xe)?;
    ws.set_column_width(6, 16).map_err(xe)?;
    ws.set_freeze_panes(1, 0).map_err(xe)?;

    // Same local-calendar-day windowing as every analytics query.
    let query = format!(
        "SELECT timestamp, action_type, trigger_key, label, char_count, time_saved, target_app \
         FROM action_log{} ORDER BY id ASC",
        win.where_clause()
    );
    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, String>(1).unwrap_or_default(),
                row.get::<_, String>(2).unwrap_or_default(),
                row.get::<_, String>(3).unwrap_or_default(),
                row.get::<_, i64>(4).unwrap_or(0),
                row.get::<_, f64>(5).unwrap_or(0.0),
                row.get::<_, String>(6).unwrap_or_default(),
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut r: u32 = 1;
    for row in rows.flatten() {
        // Plain formats on the big sheet — zebra fills across 100k+ rows bloat
        // the file for no reading benefit under an autofilter.
        ws.write_string(r, 0, &row.0).map_err(xe)?;
        ws.write_string(r, 1, &row.1).map_err(xe)?;
        ws.write_string(r, 2, &row.2).map_err(xe)?;
        ws.write_string(r, 3, &row.3).map_err(xe)?;
        ws.write_number(r, 4, row.4 as f64).map_err(xe)?;
        ws.write_number_with_format(r, 5, row.5, &s.num1).map_err(xe)?;
        ws.write_string(r, 6, &row.6).map_err(xe)?;
        r += 1;
    }
    if r > 1 {
        ws.autofilter(0, 0, r - 1, 6).map_err(xe)?;
    }
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB matching the full migrated action_log shape, seeded with
    /// rows across every action type and a spread of days/hours.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE action_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT NOT NULL,
                action_type TEXT NOT NULL,
                char_count  INTEGER DEFAULT 0,
                time_saved  REAL NOT NULL,
                trigger_key TEXT NOT NULL DEFAULT '',
                label       TEXT NOT NULL DEFAULT '',
                target_app  TEXT NOT NULL DEFAULT ''
            );",
        )
        .unwrap();

        let now = chrono::Utc::now();
        let samples: Vec<(i64, &str, i64, f64, &str, &str, &str)> = vec![
            (0, "expansion", 120, 36.0, "GLOBAL::EXPANSION::sig", "Signature", "chrome"),
            (1, "expansion", 45, 13.5, "GLOBAL::EXPANSION::addr", "Address", "outlook"),
            (2, "macro", 0, 42.5, "Default::Ctrl::K", "Morning setup", "explorer"),
            (3, "macro", 0, 12.0, "GLOBAL::QUICKRECORD::replay", "Quick Record Replay", "excel"),
            (4, "text", 0, 9.0, "Default::Ctrl::T", "Boilerplate", "word"),
            (5, "app", 0, 3.0, "Default::Ctrl::A", "Open Revit", "explorer"),
            (6, "url", 0, 3.0, "Default::Ctrl::U", "Open dashboard", "chrome"),
            (7, "autocorrect", 8, 2.0, "teh", "teh", "word"),
            (8, "search_template", 0, 3.0, "GLOBAL::QUICKACTION::g", "Google", "chrome"),
            (9, "ahk", 0, 3.0, "Default::Ctrl::H", "AHK helper", "notepad"),
        ];
        for (i, (day_off, ty, chars, saved, trig, label, app)) in samples.iter().enumerate() {
            let ts = (now - chrono::Duration::days(*day_off) + chrono::Duration::hours(i as i64 % 5))
                .to_rfc3339();
            conn.execute(
                "INSERT INTO action_log (timestamp, action_type, char_count, time_saved, trigger_key, label, target_app)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![ts, ty, chars, saved, trig, label, app],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn builds_workbook_with_all_sheets() {
        let conn = test_conn();
        let dir = std::env::temp_dir().join("keyfire-xlsx-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyfire-analytics-test.xlsx");
        let _ = std::fs::remove_file(&path);

        export_xlsx(&conn, &path, &Window::All).expect("export_xlsx failed");

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 4_000, "workbook suspiciously small: {} bytes", bytes.len());

        // Inspect the zip: workbook.xml must declare every sheet, and the
        // chart part must exist (Daily Activity embeds a combined chart).
        let reader = std::io::Cursor::new(&bytes);
        let mut zip = zip::ZipArchive::new(reader).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n.starts_with("xl/charts/chart")), "no chart part: {:?}", names);

        let mut workbook_xml = String::new();
        {
            use std::io::Read;
            zip.by_name("xl/workbook.xml")
                .unwrap()
                .read_to_string(&mut workbook_xml)
                .unwrap();
        }
        for sheet in [
            "Overview",
            "Daily Activity",
            "Breakdown",
            "Heatmap",
            "Key Mappings",
            "Text Expansions",
            "Top Apps",
            "Expansion Efficiency",
            "Action Log",
        ] {
            assert!(
                workbook_xml.contains(&format!("name=\"{}\"", sheet)),
                "sheet missing from workbook.xml: {}",
                sheet
            );
        }
    }

    #[test]
    fn empty_database_exports_cleanly() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE action_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT NOT NULL,
                action_type TEXT NOT NULL,
                char_count  INTEGER DEFAULT 0,
                time_saved  REAL NOT NULL,
                trigger_key TEXT NOT NULL DEFAULT '',
                label       TEXT NOT NULL DEFAULT '',
                target_app  TEXT NOT NULL DEFAULT ''
            );",
        )
        .unwrap();
        let dir = std::env::temp_dir().join("keyfire-xlsx-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyfire-analytics-empty.xlsx");
        let _ = std::fs::remove_file(&path);
        export_xlsx(&conn, &path, &Window::All).expect("empty export failed");
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);
    }

    #[test]
    fn scoped_export_filters_action_log() {
        let conn = test_conn();
        let dir = std::env::temp_dir().join("keyfire-xlsx-test");
        std::fs::create_dir_all(&dir).unwrap();

        // Every period value the UI can send must export cleanly.
        for days in [1u32, 7, 14, 30] {
            let path = dir.join(format!("keyfire-analytics-{}d.xlsx", days));
            let _ = std::fs::remove_file(&path);
            export_xlsx(&conn, &path, &Window::Days(days))
                .unwrap_or_else(|e| panic!("days={} failed: {}", days, e));
            assert!(std::fs::metadata(&path).unwrap().len() > 4_000);
        }

        // Today-scope drops the seeded rows from earlier days: the 7-day file
        // carries more shared strings than the 1-day file.
        let d1 = std::fs::metadata(dir.join("keyfire-analytics-1d.xlsx")).unwrap().len();
        let d7 = std::fs::metadata(dir.join("keyfire-analytics-7d.xlsx")).unwrap().len();
        assert!(d7 >= d1, "7-day export ({}) should not be smaller than today-only ({})", d7, d1);
    }

    #[test]
    fn custom_range_exports_cleanly() {
        let conn = test_conn();
        let dir = std::env::temp_dir().join("keyfire-xlsx-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyfire-analytics-range.xlsx");
        let _ = std::fs::remove_file(&path);

        // Bank-statement style: an explicit from/to window covering the seed data.
        let today = chrono::Local::now().date_naive();
        let from = (today - chrono::Duration::days(6)).format("%Y-%m-%d").to_string();
        let to = today.format("%Y-%m-%d").to_string();
        export_xlsx(&conn, &path, &Window::Range(from, to)).expect("range export failed");
        assert!(std::fs::metadata(&path).unwrap().len() > 4_000);

        // A reversed/eccentric range must still produce a valid (empty) workbook.
        let path2 = dir.join("keyfire-analytics-range-past.xlsx");
        let _ = std::fs::remove_file(&path2);
        export_xlsx(
            &conn,
            &path2,
            &Window::Range("2020-01-01".to_string(), "2020-01-31".to_string()),
        )
        .expect("past range export failed");
        assert!(std::fs::metadata(&path2).unwrap().len() > 1_000);
    }
}
