import React, { useState, useEffect } from 'react';
import './AnalyticsReport.css';

// ── Analytics PDF report page (?report=1) ────────────────────────────────────
// Rendered in a hidden window created by the export_analytics_pdf command.
// The backend waits for the analytics_report_ready invoke, then drives
// WebView2 PrintToPdf against this DOM. Layout is three fixed A4 pages.
//
// Two flavours:
//   - Scoped (days=N or from/to): PERIOD-NATIVE. Everything derives from the
//     chosen window — period overview, sub-period breakdown (day/week/month
//     rows by span), period efficiency and ROI. No rolling today/7/14/30
//     windows, no streaks: those make no sense on "June's report".
//   - All time: rolling summary + records + streaks, plus a month-by-month
//     table.
//
// Outside Tauri (plain browser / headless testing) the component falls back
// to SAMPLE data and never calls analyticsReportReady. Inside Tauri it always
// renders real data; a fetch failure renders zeros, never fake numbers.

const IN_TAURI = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;

// Export period, passed by export_analytics_pdf via the window URL:
// ?report=1&days=N (0 = all time, 1 = today, 7/14/30 = last N days) or
// ?report=1&from=YYYY-MM-DD&to=YYYY-MM-DD for a custom bank-statement range.
const isIsoDate = (s) => /^\d{4}-\d{2}-\d{2}$/.test(s || '');

const { PERIOD_DAYS, RANGE } = (() => {
  try {
    const params = new URLSearchParams(window.location.search);
    let from = params.get('from');
    let to = params.get('to');
    if (isIsoDate(from) && isIsoDate(to)) {
      if (from > to) [from, to] = [to, from];
      return { PERIOD_DAYS: 0, RANGE: { from, to } };
    }
    const d = parseInt(params.get('days') || '0', 10);
    return { PERIOD_DAYS: [0, 1, 7, 14, 30].includes(d) ? d : 0, RANGE: null };
  } catch {
    return { PERIOD_DAYS: 0, RANGE: null };
  }
})();

const SCOPED = !!RANGE || PERIOD_DAYS !== 0;
const PERIOD_LABELS = { 0: 'All time', 1: 'Today', 7: 'Last 7 days', 14: 'Last 14 days', 30: 'Last 30 days' };
const PERIOD_LABEL = RANGE ? `${RANGE.from} to ${RANGE.to}` : PERIOD_LABELS[PERIOD_DAYS];

// Range span in whole days (inclusive). Chart caps at 31 bars.
const RANGE_SPAN = RANGE
  ? Math.floor((new Date(RANGE.to + 'T00:00') - new Date(RANGE.from + 'T00:00')) / 86400000) + 1
  : 0;
// Chart windows: a 1-day window makes no chart, so "Today" widens to 7.
const CHART_DAYS = RANGE
  ? Math.min(31, Math.max(1, RANGE_SPAN))
  : PERIOD_DAYS === 0 ? 14 : Math.min(30, Math.max(7, PERIOD_DAYS));
const HEATMAP_DAYS = PERIOD_DAYS === 0 ? 30 : Math.max(1, PERIOD_DAYS);
// Chart bars anchor at the range end for custom ranges, today otherwise.
const CHART_END = RANGE ? new Date(RANGE.to + 'T00:00') : new Date();

// ── Formatting helpers ───────────────────────────────────────────────────────

function formatTimeLong(seconds) {
  if (!seconds || seconds < 60) return `${Math.round(seconds || 0)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${Math.round(seconds % 60)}s`;
}

function formatTimeShort(seconds) {
  if (!seconds || seconds < 60) return `${Math.round(seconds || 0)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

// Local-date key (YYYY-MM-DD). NOT toISOString() — that converts to UTC and
// shifts midnight-anchored dates back a day in any UTC+ timezone.
function localDateKey(d) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function fmtDay(dateStr) {
  return new Date(dateStr + 'T00:00').toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
}

function gbp(v) {
  return v.toLocaleString(undefined, { style: 'currency', currency: 'GBP', maximumFractionDigits: 0 });
}

const DOW_LABELS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const KEY_MAPPING_TYPES = new Set(['hotkey', 'text', 'app', 'url', 'folder', 'macro', 'search_template', 'ahk']);

// ── Period maths ─────────────────────────────────────────────────────────────

function startOfDay(d) {
  const c = new Date(d);
  c.setHours(0, 0, 0, 0);
  return c;
}

// The equal-length window immediately before the export period. Scoped
// reports compare against it, bank-statement style ("July vs June").
const PREV_RANGE = (() => {
  if (!SCOPED) return null;
  const today = startOfDay(new Date());
  let start;
  let end;
  if (RANGE) {
    start = new Date(RANGE.from + 'T00:00');
    end = new Date(RANGE.to + 'T00:00');
  } else {
    end = today;
    start = new Date(today);
    start.setDate(start.getDate() - (PERIOD_DAYS - 1));
  }
  const span = Math.floor((end - start) / 86400000) + 1;
  const pEnd = new Date(start);
  pEnd.setDate(pEnd.getDate() - 1);
  const pStart = new Date(pEnd);
  pStart.setDate(pStart.getDate() - (span - 1));
  return {
    from: localDateKey(pStart),
    to: localDateKey(pEnd),
    label: `${fmtDay(localDateKey(pStart))} to ${fmtDay(localDateKey(pEnd))}`,
  };
})();

/// First and last local day of the export window. All-time anchors at the
/// first recorded activity.
function periodBounds(dailyFull) {
  const today = startOfDay(new Date());
  if (RANGE) {
    return { start: new Date(RANGE.from + 'T00:00'), end: new Date(RANGE.to + 'T00:00') };
  }
  if (PERIOD_DAYS > 0) {
    const start = new Date(today);
    start.setDate(start.getDate() - (PERIOD_DAYS - 1));
    return { start, end: today };
  }
  const first = dailyFull.length ? new Date(dailyFull[0].date + 'T00:00') : today;
  return { start: first, end: today };
}

function computePeriodStats(dailyFull, start, end) {
  let totalActions = 0;
  let totalSaved = 0;
  let busiest = null;
  let activeDays = 0;
  dailyFull.forEach((d) => {
    totalActions += d.actions || 0;
    totalSaved += d.time_saved || 0;
    if ((d.actions || 0) > 0) {
      activeDays += 1;
      if (!busiest || d.actions > busiest.actions) busiest = d;
    }
  });
  const spanDays = Math.max(1, Math.floor((end - start) / 86400000) + 1);
  return {
    totalActions,
    totalSaved,
    busiest,
    activeDays,
    spanDays,
    avgActions: totalActions / spanDays,
    avgSaved: totalSaved / spanDays,
  };
}

/// Roll the period's days up into sub-period rows. Granularity escalates with
/// span: days (≤14), calendar weeks starting Monday (≤92), else months.
/// Month rows cap at the last 12 with the rest rolled into "Earlier".
function buildSubPeriods(dailyFull, start, end) {
  const spanDays = Math.floor((end - start) / 86400000) + 1;
  if (spanDays <= 2) return null; // a table of one row says nothing
  const gran = spanDays <= 14 ? 'day' : spanDays <= 92 ? 'week' : 'month';

  const map = {};
  dailyFull.forEach((d) => { map[d.date] = d; });

  const buckets = [];
  let cur = null;
  for (let d = new Date(start); d <= end; d.setDate(d.getDate() + 1)) {
    let key;
    if (gran === 'day') {
      key = localDateKey(d);
    } else if (gran === 'week') {
      const monday = new Date(d);
      monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
      key = localDateKey(monday);
    } else {
      key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
    }
    if (!cur || cur.key !== key) {
      cur = { key, from: new Date(d), to: new Date(d), actions: 0, saved: 0, best: 0, bestDate: null };
      buckets.push(cur);
    }
    cur.to = new Date(d);
    const row = map[localDateKey(d)];
    if (row) {
      cur.actions += row.actions || 0;
      cur.saved += row.time_saved || 0;
      if ((row.actions || 0) > cur.best) {
        cur.best = row.actions;
        cur.bestDate = new Date(d);
      }
    }
  }

  // Labels
  const short = (d) => d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  buckets.forEach((b) => {
    if (gran === 'day') {
      b.label = b.from.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
    } else if (gran === 'week') {
      b.label = `${short(b.from)} to ${short(b.to)}`;
    } else {
      b.label = b.from.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
    }
  });

  // Cap at 11 rows: keep the most recent 10, roll the rest into "Earlier".
  // The table shares page 2 with the chart and heatmap — longer tables clip.
  let rows = buckets;
  if (buckets.length > 11) {
    const recent = buckets.slice(-10);
    const earlier = buckets.slice(0, -10);
    const rolled = {
      label: `Earlier (${earlier.length} ${gran === 'month' ? 'months' : 'weeks'})`,
      actions: earlier.reduce((a, b) => a + b.actions, 0),
      saved: earlier.reduce((a, b) => a + b.saved, 0),
      best: Math.max(...earlier.map((b) => b.best)),
      bestDate: null,
    };
    rows = [rolled, ...recent];
  }

  const titles = { day: 'Day by day', week: 'Week by week', month: 'Month by month' };
  return { gran, rows, title: titles[gran] };
}

// ── Sample data (non-Tauri preview/testing only) ────────────────────────────

function sampleData() {
  const today = startOfDay(new Date());
  const start = RANGE ? new Date(RANGE.from + 'T00:00') : (() => { const d = new Date(today); d.setDate(d.getDate() - 34); return d; })();
  const end = RANGE ? new Date(RANGE.to + 'T00:00') : today;
  const daily = [];
  let i = 0;
  for (let d = new Date(start); d <= end; d.setDate(d.getDate() + 1)) {
    const weekday = d.getDay() !== 0 && d.getDay() !== 6;
    const actions = weekday ? 40 + ((i * 37) % 90) : 8 + ((i * 13) % 20);
    daily.push({ date: localDateKey(d), actions, time_saved: actions * (14 + (i % 9)) });
    i += 1;
  }
  const heatmap = [];
  for (let dow = 0; dow < 7; dow++) {
    for (let hour = 8; hour < 19; hour++) {
      const count = dow >= 1 && dow <= 5 ? ((dow * 7 + hour * 3) % 25) : (hour % 5);
      if (count > 0) heatmap.push({ dow, hour, count, time_saved: count * 12 });
    }
  }
  // Previous window at ~85% of the period's numbers so the deltas read up.
  const prevDaily = PREV_RANGE
    ? (() => {
        const rows = [];
        let j = 0;
        for (
          let d = new Date(PREV_RANGE.from + 'T00:00');
          d <= new Date(PREV_RANGE.to + 'T00:00');
          d.setDate(d.getDate() + 1)
        ) {
          const weekday = d.getDay() !== 0 && d.getDay() !== 6;
          const actions = Math.round((weekday ? 40 + ((j * 31) % 80) : 6 + ((j * 11) % 18)) * 0.85);
          rows.push({ date: localDateKey(d), actions, time_saved: actions * 15 });
          j += 1;
        }
        return rows;
      })()
    : [];
  const mk = (label, trigger, type, count, saved) => ({ label, trigger, type, count, time_saved: saved });
  const effBlock = (n) => ({ total_expansions: n, chars_typed: n * 5.4 | 0, chars_expanded: n * 126 | 0, ratio: 23.2 });
  return {
    stats: {
      total_actions: 4821, total_time_saved_seconds: 63120,
      actions_today: 64, time_saved_today_seconds: 1180,
      actions_last_7_days: 512, time_saved_last_7_days_seconds: 8640,
      actions_last_14_days: 1094, time_saved_last_14_days_seconds: 16920,
      actions_last_30_days: 2306, time_saved_last_30_days_seconds: 34980,
      best_day_time_saved_seconds: 3120, best_7_days_time_saved_seconds: 11460,
      expansions: 2914, hotkeys: 1240, macros: 493, search_templates: 88, autocorrects: 86,
    },
    typeBreakdown: {
      total: 4821, expansions: 2914, hotkeys: 1240, macros: 493, autocorrects: 86,
      time_saved: 63120, expansions_saved: 38200, hotkeys_saved: 9800, macros_saved: 14260, autocorrects_saved: 860,
    },
    streaks: { current: 9, longest: 23 },
    dailyFull: daily,
    chartDaily: daily,
    prevDaily,
    heatmap,
    assignments: [
      mk('Email signature', 'GLOBAL::EXPANSION::sig', 'expansion', 342, 10260),
      mk('Morning setup', 'Default::Ctrl+Alt::M', 'macro', 61, 7930),
      mk('Meeting notes header', 'GLOBAL::EXPANSION::mtg', 'expansion', 188, 4890),
      mk('Invoice reply', 'GLOBAL::EXPANSION::inv', 'expansion', 122, 4270),
      mk('Open project folder', 'Default::Ctrl::P', 'folder', 240, 720),
      mk('Quick Record Replay', 'GLOBAL::QUICKRECORD::replay', 'macro', 44, 2860),
      mk('Site report boilerplate', 'GLOBAL::EXPANSION::rep', 'expansion', 96, 3550),
      mk('Open Revit', 'Default::Ctrl+Shift::R', 'app', 92, 276),
      mk('Address block', 'GLOBAL::EXPANSION::addr', 'expansion', 84, 1930),
      mk('Export drawings', 'Default::Ctrl+Alt::E', 'macro', 38, 3210),
      mk('Daily standup note', 'GLOBAL::EXPANSION::stand', 'expansion', 72, 1660),
      mk('Search drawings', 'GLOBAL::QUICKACTION::dwg', 'search_template', 66, 198),
    ],
    topApps: [
      { app: 'chrome', count: 1315, time_saved: 17400 },
      { app: 'outlook', count: 942, time_saved: 14820 },
      { app: 'word', count: 588, time_saved: 9060 },
      { app: 'excel', count: 512, time_saved: 7380 },
      { app: 'revit', count: 449, time_saved: 6240 },
      { app: 'explorer', count: 361, time_saved: 2190 },
      { app: 'teams', count: 244, time_saved: 2020 },
      { app: 'notepad', count: 121, time_saved: 900 },
      { app: 'acad', count: 98, time_saved: 1410 },
      { app: 'slack', count: 84, time_saved: 700 },
    ],
    efficiency: SCOPED
      ? { period: effBlock(861) }
      : { week: effBlock(214), month: effBlock(861), all: effBlock(2914) },
    hourlyRate: 45,
    sample: true,
  };
}

async function fetchRealData() {
  const api = window.electronAPI;
  const from = RANGE?.from || null;
  const to = RANGE?.to || null;
  const daysArg = RANGE ? null : (PERIOD_DAYS || null);
  const [stats, typeBreakdown, streaks, dailyFull, chartExtra, prevDaily, heatmap, assignments, topApps, efficiency] =
    await Promise.all([
      api.getAnalytics(),
      api.getTypeBreakdown(daysArg, from, to),
      api.getStreaks(),
      // Full window of daily rows: drives the overview stats, the sub-period
      // table AND the chart (null days + no range = whole history).
      api.getDailyChart(daysArg, from, to),
      // "Today" widens the chart to a week — needs 7 days of data.
      PERIOD_DAYS === 1 ? api.getDailyChart(7) : Promise.resolve(null),
      // Preceding equal-length window for the comparison strip.
      PREV_RANGE ? api.getDailyChart(null, PREV_RANGE.from, PREV_RANGE.to) : Promise.resolve(null),
      api.getHourlyHeatmap(RANGE ? null : HEATMAP_DAYS, from, to),
      api.getAssignmentBreakdown(daysArg, from, to),
      api.getTopApps(daysArg, from, to),
      api.getExpansionEfficiency(daysArg, from, to),
    ]);
  let hourlyRate = 0;
  try { hourlyRate = parseFloat(localStorage.getItem('trigr.hourlyRate')) || 0; } catch { /* ignore */ }
  const dailyArr = Array.isArray(dailyFull) ? dailyFull : [];
  return {
    stats: stats || {},
    typeBreakdown: typeBreakdown || {},
    streaks: streaks || { current: 0, longest: 0 },
    dailyFull: dailyArr,
    chartDaily: Array.isArray(chartExtra) ? chartExtra : dailyArr,
    prevDaily: Array.isArray(prevDaily) ? prevDaily : [],
    heatmap: Array.isArray(heatmap) ? heatmap : [],
    assignments: Array.isArray(assignments) ? assignments : [],
    topApps: Array.isArray(topApps) ? topApps : [],
    efficiency: efficiency || {},
    hourlyRate,
    sample: false,
  };
}

// ── Sub-renders ──────────────────────────────────────────────────────────────

function SectionTitle({ children }) {
  return <div className="rpt-section-title">{children}</div>;
}

function ActivityChart({ daily, windowDays = 14, endDate = new Date() }) {
  // Fill the whole window so gaps render as zero-height bars. The window
  // anchors at endDate (range end for custom exports, today otherwise).
  const map = {};
  daily.forEach(d => { map[d.date] = d; });
  const days = [];
  for (let i = windowDays - 1; i >= 0; i--) {
    const d = new Date(endDate);
    d.setDate(d.getDate() - i);
    const key = localDateKey(d);
    days.push(map[key] || { date: key, actions: 0, time_saved: 0 });
  }
  const maxA = Math.max(1, ...days.map(d => d.actions));
  const maxT = Math.max(1, ...days.map(d => d.time_saved));

  const W = 700, H = 200, PAD = 6;
  const slot = W / days.length;
  const barW = slot * 0.28;
  // Thin the date labels when the window is wide (30 days would collide).
  const labelEvery = Math.ceil(days.length / 15);

  return (
    <div className="rpt-chart-wrap">
      <svg className="rpt-chart" viewBox={`0 0 ${W} ${H + 26}`} preserveAspectRatio="none">
        {[0.25, 0.5, 0.75, 1].map(f => (
          <line key={f} x1="0" x2={W} y1={H - H * f + PAD} y2={H - H * f + PAD} className="rpt-chart-grid" />
        ))}
        {days.map((d, i) => {
          const hA = Math.max(2, (d.actions / maxA) * (H - PAD));
          const hT = Math.max(2, (d.time_saved / maxT) * (H - PAD));
          const x = i * slot + slot / 2;
          return (
            <g key={d.date}>
              <rect x={x - barW - 1.5} y={H + PAD - hA} width={barW} height={hA} rx="1.5" className="rpt-bar-actions" />
              <rect x={x + 1.5} y={H + PAD - hT} width={barW} height={hT} rx="1.5" className="rpt-bar-saved" />
              {i % labelEvery === 0 && (
                <text x={x} y={H + PAD + 15} textAnchor="middle" className="rpt-chart-label">
                  {new Date(d.date + 'T00:00').toLocaleDateString(undefined, { day: 'numeric', month: 'numeric' })}
                </text>
              )}
            </g>
          );
        })}
      </svg>
      <div className="rpt-legend">
        <span className="rpt-legend-item"><span className="rpt-swatch rpt-swatch-actions" />Actions (peak {maxA})</span>
        <span className="rpt-legend-item"><span className="rpt-swatch rpt-swatch-saved" />Time saved (peak {formatTimeShort(maxT)})</span>
      </div>
    </div>
  );
}

function Heatmap({ heatmap }) {
  const grid = Array.from({ length: 7 }, () => Array(24).fill(0));
  let max = 1;
  heatmap.forEach(({ dow, hour, count }) => {
    if (dow < 7 && hour < 24) {
      grid[dow][hour] = count;
      if (count > max) max = count;
    }
  });
  return (
    <div className="rpt-heatmap">
      <div className="rpt-hm-corner" />
      {Array.from({ length: 24 }, (_, h) => (
        <div key={h} className="rpt-hm-hour">{h}</div>
      ))}
      {grid.map((row, dow) => (
        <React.Fragment key={dow}>
          <div className="rpt-hm-dow">{DOW_LABELS[dow]}</div>
          {row.map((count, h) => {
            const intensity = count > 0 ? Math.max(0.14, count / max) : 0;
            return (
              <div
                key={h}
                className="rpt-hm-cell"
                style={{ background: intensity > 0 ? `rgba(232, 160, 32, ${intensity})` : 'rgba(0,0,0,0.05)' }}
              />
            );
          })}
        </React.Fragment>
      ))}
    </div>
  );
}

function Leaderboard({ title, rows, showTrigger = true }) {
  return (
    <div className="rpt-board">
      <SectionTitle>{title}</SectionTitle>
      {rows.length === 0 ? (
        <div className="rpt-empty">No data recorded in this period</div>
      ) : (
        <table className="rpt-table">
          <thead>
            <tr>
              <th className="rpt-th-rank">#</th>
              <th>Name</th>
              <th className="rpt-th-num">Fires</th>
              <th className="rpt-th-num">Saved</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((item, i) => (
              <tr key={i}>
                <td className="rpt-td-rank">{i + 1}</td>
                <td className="rpt-td-name">
                  <span className="rpt-td-label">{item.label || item.trigger || item.app || '(unnamed)'}</span>
                  {showTrigger && item.trigger && <span className="rpt-td-trigger">{item.trigger}</span>}
                </td>
                <td className="rpt-td-num">{(item.count || 0).toLocaleString()}</td>
                <td className="rpt-td-num rpt-accent">{formatTimeShort(item.time_saved || 0)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

/// Sub-period rows (day/week/month). The share bar visualises each row's
/// slice of the period's total actions. With an hourly rate set, each row
/// also carries the monetary value of its time saved.
function SubPeriodTable({ sub, totalActions, hourlyRate = 0 }) {
  if (!sub || sub.rows.length === 0) return null;
  const maxActions = Math.max(1, ...sub.rows.map((r) => r.actions));
  const showValue = hourlyRate > 0;
  return (
    <>
      <SectionTitle>{sub.title}</SectionTitle>
      <table className="rpt-table rpt-subtable">
        <thead>
          <tr>
            <th>{sub.gran === 'day' ? 'Day' : sub.gran === 'week' ? 'Week' : 'Month'}</th>
            <th className="rpt-th-num">Actions</th>
            <th className="rpt-th-bar" />
            <th className="rpt-th-num">Time saved</th>
            {showValue && <th className="rpt-th-num">Value</th>}
            <th className="rpt-th-num">{sub.gran === 'day' ? 'Share' : 'Busiest day'}</th>
          </tr>
        </thead>
        <tbody>
          {sub.rows.map((r, i) => (
            <tr key={i}>
              <td className="rpt-td-label-cell">{r.label}</td>
              <td className="rpt-td-num">{r.actions.toLocaleString()}</td>
              <td className="rpt-td-bar">
                <div className="rpt-row-bar-wrap">
                  <div className="rpt-row-bar" style={{ width: `${Math.max(1, (r.actions / maxActions) * 100)}%` }} />
                </div>
              </td>
              <td className="rpt-td-num rpt-accent">{formatTimeShort(r.saved)}</td>
              {showValue && <td className="rpt-td-num">{gbp(r.saved / 3600 * hourlyRate)}</td>}
              <td className="rpt-td-num">
                {sub.gran === 'day'
                  ? `${totalActions > 0 ? Math.round((r.actions / totalActions) * 100) : 0}%`
                  : r.bestDate
                    ? `${fmtDay(localDateKey(r.bestDate))} (${r.best.toLocaleString()})`
                    : '-'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

/// One comparison tile: the period's value with a coloured delta line
/// against the previous window.
function DeltaTile({ label, cur, prev, fmt }) {
  const pct = prev > 0 ? Math.round(((cur - prev) / prev) * 100) : null;
  const dir = pct === null ? 'new' : pct > 2 ? 'up' : pct < -2 ? 'down' : 'flat';
  const arrow = dir === 'up' ? '▲' : dir === 'down' ? '▼' : '';
  const deltaText =
    pct === null
      ? (cur > 0 ? 'no previous activity' : 'no activity either period')
      : dir === 'flat'
        ? `about level (${fmt(prev)})`
        : `${arrow} ${Math.abs(pct)}% (was ${fmt(prev)})`;
  return (
    <div className="rpt-card">
      <div className="rpt-card-value">{fmt(cur)}</div>
      <div className="rpt-card-label">{label}</div>
      <div className={`rpt-delta rpt-delta-${dir}`}>{deltaText}</div>
    </div>
  );
}

/// Comparison strip vs the preceding equal-length window.
function ComparisonSection({ prevDaily, period }) {
  if (!PREV_RANGE) return null;
  let prevActions = 0;
  let prevSaved = 0;
  let prevActive = 0;
  prevDaily.forEach((d) => {
    prevActions += d.actions || 0;
    prevSaved += d.time_saved || 0;
    if ((d.actions || 0) > 0) prevActive += 1;
  });
  return (
    <>
      <SectionTitle>Compared with the Previous Period ({PREV_RANGE.label})</SectionTitle>
      <div className="rpt-cards-3">
        <DeltaTile label="actions" cur={period.totalActions} prev={prevActions} fmt={(v) => v.toLocaleString()} />
        <DeltaTile label="time saved" cur={period.totalSaved} prev={prevSaved} fmt={formatTimeShort} />
        <DeltaTile label="active days" cur={period.activeDays} prev={prevActive} fmt={(v) => String(v)} />
      </div>
    </>
  );
}

/// "Best of the period" strip — rendered on EVERY report flavour: most
/// valuable action, most fired action, top app, power hour, busiest weekday,
/// characters expanded (typing you skipped). Two rows of three.
function HighlightsSection({ title = 'Period Highlights', assignments, topApps, heatmap, efficiencyBlock, hourlyRate }) {
  const bySaved = [...(assignments || [])].sort((a, b) => (b.time_saved || 0) - (a.time_saved || 0));
  const byCount = [...(assignments || [])].sort((a, b) => (b.count || 0) - (a.count || 0));
  const valuable = bySaved[0];
  const fired = byCount[0];
  const topApp = (topApps || [])[0];

  const hourTotals = Array(24).fill(0);
  const dowTotals = Array(7).fill(0);
  (heatmap || []).forEach(({ dow, hour, count }) => {
    if (hour < 24) hourTotals[hour] += count;
    if (dow < 7) dowTotals[dow] += count;
  });
  const maxHour = hourTotals.reduce((best, v, h) => (v > hourTotals[best] ? h : best), 0);
  const powerHour = hourTotals[maxHour] > 0 ? maxHour : null;
  const maxDow = dowTotals.reduce((best, v, d) => (v > dowTotals[best] ? d : best), 0);
  const busiestDow = dowTotals[maxDow] > 0 ? maxDow : null;
  const DOW_FULL = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

  const tiles = [];
  if (valuable && (valuable.time_saved || 0) > 0) {
    tiles.push({
      value: formatTimeShort(valuable.time_saved),
      label: `most valuable: ${valuable.label || valuable.trigger}`,
      sub: hourlyRate > 0 ? `worth ${gbp(valuable.time_saved / 3600 * hourlyRate)}` : `${(valuable.count || 0).toLocaleString()} fires`,
      accent: true,
    });
  }
  if (fired && (fired.count || 0) > 0) {
    tiles.push({
      value: `${(fired.count || 0).toLocaleString()}x`,
      label: `most fired: ${fired.label || fired.trigger}`,
      sub: `saved ${formatTimeShort(fired.time_saved || 0)}`,
    });
  }
  if (topApp) {
    tiles.push({
      value: topApp.app,
      label: 'top app',
      sub: `${(topApp.count || 0).toLocaleString()} actions, ${formatTimeShort(topApp.time_saved || 0)} saved`,
    });
  }
  if (powerHour !== null) {
    tiles.push({
      value: `${String(powerHour).padStart(2, '0')}:00`,
      label: 'power hour',
      sub: `${hourTotals[maxHour].toLocaleString()} actions in this hour of the day`,
    });
  }
  if (busiestDow !== null) {
    tiles.push({
      value: DOW_FULL[maxDow],
      label: 'busiest day of the week',
      sub: `${dowTotals[maxDow].toLocaleString()} actions on ${DOW_FULL[maxDow]}s`,
    });
  }
  const charsExpanded = efficiencyBlock?.chars_expanded || 0;
  if (charsExpanded > 0) {
    tiles.push({
      value: charsExpanded.toLocaleString(),
      label: 'characters you did not type',
      sub: `expansions typed them for you (${Math.round(efficiencyBlock?.ratio || 0)}x leverage)`,
      accent: true,
    });
  }
  if (tiles.length === 0) return null;

  return (
    <>
      <SectionTitle>{title}</SectionTitle>
      <div className="rpt-cards-3">
        {tiles.map((t, i) => (
          <div key={i} className="rpt-card">
            <div className={`rpt-card-value${t.accent ? ' rpt-accent' : ''} rpt-hl-value`}>{t.value}</div>
            <div className="rpt-card-label">{t.label}</div>
            <div className="rpt-hl-sub">{t.sub}</div>
          </div>
        ))}
      </div>
    </>
  );
}

/// Value of Time Saved — gold-highlighted tiles, page 1 of every report.
/// With no hourly rate set, shows the time itself instead of hiding.
function ValueSection({ tiles, hourlyRate }) {
  return (
    <>
      <SectionTitle>{hourlyRate > 0 ? `Value of Time Saved (at £${hourlyRate}/hr)` : 'Time Saved'}</SectionTitle>
      <div className="rpt-cards-3">
        {tiles.map((t, i) => (
          <div key={i} className="rpt-card rpt-card-gold">
            <div className="rpt-card-value rpt-accent">{t.value}</div>
            <div className="rpt-card-label">{t.label}</div>
          </div>
        ))}
      </div>
    </>
  );
}

// ── Main component ───────────────────────────────────────────────────────────

export default function AnalyticsReport() {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);

  // PrintToPdf stamps document.title into the PDF's Title metadata — without
  // this the report inherits index.html's "Trigr" tab title.
  useEffect(() => {
    document.title = `Keyfire Analytics Report - ${PERIOD_LABEL}`;
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const d = IN_TAURI ? await fetchRealData() : sampleData();
        if (!cancelled) setData(d);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Signal the backend once fonts are loaded and the DOM has settled.
  // Deliberately timer-based, NOT requestAnimationFrame: this window is
  // hidden (Controller.IsVisible false) so rAF never fires — PrintToPdf does
  // its own layout pass anyway, the wait just covers font swap-in.
  useEffect(() => {
    if (!data && !error) return;
    (async () => {
      try { await document.fonts.ready; } catch { /* older engines */ }
      // Images too (the header logo) — PrintToPdf must not race their decode.
      try {
        await Promise.all(
          Array.from(document.images).map((img) =>
            img.complete ? null : new Promise((r) => { img.onload = r; img.onerror = r; })
          )
        );
      } catch { /* never block the print on an image */ }
      await new Promise(r => setTimeout(r, 300));
      if (IN_TAURI) window.electronAPI?.analyticsReportReady?.();
    })();
  }, [data, error]);

  if (error) {
    return <div className="rpt-root"><div className="rpt-page"><div className="rpt-error">Could not load analytics: {error}</div></div></div>;
  }
  if (!data) return null;

  const { stats, typeBreakdown, streaks, efficiency, hourlyRate } = data;
  const generated = new Date().toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' });

  const bounds = periodBounds(data.dailyFull);
  const period = computePeriodStats(data.dailyFull, bounds.start, bounds.end);
  const sub = buildSubPeriods(data.dailyFull, bounds.start, bounds.end);

  const tb = typeBreakdown || {};
  const tbTotal = tb.total || 0;
  const breakdownRows = [
    { cls: 'expansion', label: 'Text expansions', count: tb.expansions || 0, saved: tb.expansions_saved || 0 },
    { cls: 'hotkey', label: 'Hotkey actions', count: tb.hotkeys || 0, saved: tb.hotkeys_saved || 0 },
    { cls: 'macro', label: 'Macros', count: tb.macros || 0, saved: tb.macros_saved || 0 },
    ...((tb.autocorrects || 0) > 0
      ? [{ cls: 'autocorrect', label: 'Typos fixed', count: tb.autocorrects, saved: tb.autocorrects_saved || 0 }]
      : []),
  ];

  const keysBoard = (data.assignments || [])
    .filter(a => KEY_MAPPING_TYPES.has(a.type) && (a.time_saved || 0) > 0)
    .sort((a, b) => (b.count || 0) - (a.count || 0))
    .slice(0, 12);
  const expBoard = (data.assignments || [])
    .filter(a => a.type === 'expansion')
    .sort((a, b) => (b.count || 0) - (a.count || 0))
    .slice(0, 12);
  const appsBoard = (data.topApps || []).slice(0, 12);

  const roi = (secs) => gbp((secs || 0) / 3600 * hourlyRate);

  const header = (pageNo) => (
    <div className="rpt-header">
      <div className="rpt-brand">
        <img className="rpt-brand-logo" src="/app-icon-64.png" alt="" />
        <span className="rpt-brand-mark">KEYFIRE</span>
        <span className="rpt-brand-sub">Analytics Report</span>
      </div>
      <div className="rpt-header-right">
        <span className="rpt-period">{PERIOD_LABEL}</span>
        <span>{generated}</span>
        <span className="rpt-page-no">Page {pageNo} of 3</span>
      </div>
    </div>
  );

  const footer = (
    <div className="rpt-footer">
      <span>Generated by Keyfire. Your analytics never leave this device.</span>
      <span className="rpt-footer-url">keyfire.app</span>
    </div>
  );

  const breakdownSection = (
    <>
      <SectionTitle>Breakdown by type</SectionTitle>
      <div className="rpt-breakdown">
        {breakdownRows.map(r => {
          const pct = tbTotal > 0 ? Math.round((r.count / tbTotal) * 100) : 0;
          return (
            <div key={r.cls} className="rpt-bd-row">
              <span className={`rpt-bd-dot rpt-bd-${r.cls}`} />
              <span className="rpt-bd-label">{r.label}</span>
              <span className="rpt-bd-count">{r.count.toLocaleString()}</span>
              <div className="rpt-bd-bar-wrap"><div className={`rpt-bd-bar rpt-bd-${r.cls}`} style={{ width: `${pct}%` }} /></div>
              <span className="rpt-bd-pct">{pct}%</span>
              <span className="rpt-bd-saved rpt-accent">{formatTimeShort(r.saved)}</span>
            </div>
          );
        })}
      </div>
    </>
  );

  const effCards = (blocks) => (
    <div className={`rpt-cards-${blocks.length}`}>
      {blocks.map((c, i) => (
        <div key={i} className="rpt-card rpt-card-row">
          <div>
            <div className="rpt-card-value rpt-accent">{Math.round(c.d?.ratio || 0)}x</div>
            <div className="rpt-card-label">{c.l}</div>
          </div>
          <div className="rpt-eff-detail">
            <span>{(c.d?.total_expansions || 0).toLocaleString()} fired</span>
            <span>{(c.d?.chars_typed || 0).toLocaleString()} typed</span>
            <span>{(c.d?.chars_expanded || 0).toLocaleString()} expanded</span>
          </div>
        </div>
      ))}
    </div>
  );

  const chartSection = (
    <>
      <SectionTitle>
        {RANGE
          ? `Daily Activity (${PERIOD_LABEL}${RANGE_SPAN > 31 ? ', last 31 days shown' : ''})`
          : `Daily Activity (last ${CHART_DAYS} days)`}
      </SectionTitle>
      <ActivityChart daily={data.chartDaily} windowDays={CHART_DAYS} endDate={CHART_END} />
    </>
  );

  const heatmapSection = (
    <>
      <SectionTitle>
        Activity Heatmap ({RANGE ? PERIOD_LABEL : HEATMAP_DAYS === 1 ? 'today' : `last ${HEATMAP_DAYS} days`})
      </SectionTitle>
      <Heatmap heatmap={data.heatmap} />
      <div className="rpt-hm-note">Darker gold means more actions fired in that hour.</div>
    </>
  );

  // Page 3 for both flavours: expansion efficiency + the three leaderboards.
  const finalPage = (effBlocks) => (
    <div className="rpt-page">
      {header(3)}
      <div className="rpt-gold-rule" />
      <SectionTitle>Expansion Efficiency</SectionTitle>
      {effBlocks.some((b) => (b.d?.total_expansions || 0) > 0) ? (
        effCards(effBlocks)
      ) : (
        <div className="rpt-empty">No expansions fired in this period</div>
      )}
      <Leaderboard title="Top Key Mappings" rows={keysBoard} />
      <div className="rpt-two-col rpt-boards-2">
        <Leaderboard title="Top Text Expansions" rows={expBoard} />
        <Leaderboard title="Top Apps" rows={appsBoard} showTrigger={false} />
      </div>
      {footer}
    </div>
  );

  // ── Scoped (period-native) report ──────────────────────────────────────────
  if (SCOPED) {
    const eff = efficiency?.period;
    const valueTiles = hourlyRate > 0
      ? [
          { value: roi(period.totalSaved), label: 'whole period' },
          { value: roi(period.avgSaved * 7), label: 'per week (average)' },
          { value: roi(period.avgSaved), label: 'per day (average)' },
        ]
      : [
          { value: formatTimeShort(period.totalSaved), label: 'whole period' },
          { value: formatTimeShort(period.avgSaved * 7), label: 'per week (average)' },
          { value: formatTimeShort(period.avgSaved), label: 'per day (average)' },
        ];
    return (
      <div className="rpt-root">
        {data.sample && <div className="rpt-sample-banner">SAMPLE DATA (preview outside Keyfire)</div>}

        <div className="rpt-page">
          {header(1)}
          <div className="rpt-gold-rule" />

          <SectionTitle>Period Overview</SectionTitle>
          <div className="rpt-cards-4">
            <div className="rpt-card">
              <div className="rpt-card-value">{period.totalActions.toLocaleString()}</div>
              <div className="rpt-card-label">actions in period</div>
            </div>
            <div className="rpt-card">
              <div className="rpt-card-value rpt-accent">{formatTimeLong(period.totalSaved)}</div>
              <div className="rpt-card-label">time saved</div>
            </div>
            <div className="rpt-card">
              <div className="rpt-card-value">{period.busiest ? fmtDay(period.busiest.date) : '-'}</div>
              <div className="rpt-card-label">
                {period.busiest ? `busiest day (${period.busiest.actions.toLocaleString()} actions)` : 'busiest day'}
              </div>
            </div>
            <div className="rpt-card">
              <div className="rpt-card-value">{Math.round(period.avgActions).toLocaleString()}</div>
              <div className="rpt-card-label">
                actions per day, active {period.activeDays} of {period.spanDays} days
              </div>
            </div>
          </div>

          <ValueSection tiles={valueTiles} hourlyRate={hourlyRate} />
          <ComparisonSection prevDaily={data.prevDaily} period={period} />
          <HighlightsSection
            assignments={data.assignments}
            topApps={data.topApps}
            heatmap={data.heatmap}
            efficiencyBlock={eff}
            hourlyRate={hourlyRate}
          />
          {footer}
        </div>

        <div className="rpt-page">
          {header(2)}
          <div className="rpt-gold-rule" />
          {chartSection}
          {heatmapSection}
          {breakdownSection}
          <SubPeriodTable sub={sub} totalActions={period.totalActions} hourlyRate={hourlyRate} />
          {footer}
        </div>

        {finalPage([{ d: eff, l: 'this period' }])}
      </div>
    );
  }

  // ── All-time report ────────────────────────────────────────────────────────
  const allValueTiles = hourlyRate > 0
    ? [
        { value: roi(stats.time_saved_last_7_days_seconds), label: 'this week' },
        { value: roi(stats.time_saved_last_30_days_seconds), label: 'this month' },
        { value: roi(stats.total_time_saved_seconds), label: 'all time' },
      ]
    : [
        { value: formatTimeShort(stats.time_saved_last_7_days_seconds), label: 'this week' },
        { value: formatTimeShort(stats.time_saved_last_30_days_seconds), label: 'this month' },
        { value: formatTimeShort(stats.total_time_saved_seconds), label: 'all time' },
      ];
  return (
    <div className="rpt-root">
      {data.sample && <div className="rpt-sample-banner">SAMPLE DATA (preview outside Keyfire)</div>}

      <div className="rpt-page">
        {header(1)}
        <div className="rpt-gold-rule" />

        <SectionTitle>Activity Summary</SectionTitle>
        <div className="rpt-cards-4">
          {[
            { title: 'TODAY', actions: stats.actions_today, saved: stats.time_saved_today_seconds },
            { title: 'LAST 7 DAYS', actions: stats.actions_last_7_days, saved: stats.time_saved_last_7_days_seconds },
            { title: 'LAST 14 DAYS', actions: stats.actions_last_14_days, saved: stats.time_saved_last_14_days_seconds },
            { title: 'LAST 30 DAYS', actions: stats.actions_last_30_days, saved: stats.time_saved_last_30_days_seconds },
          ].map(p => (
            <div key={p.title} className="rpt-card">
              <div className="rpt-card-title">{p.title}</div>
              <div className="rpt-card-value">{(p.actions || 0).toLocaleString()}</div>
              <div className="rpt-card-label">actions</div>
              <div className="rpt-card-value rpt-accent">{formatTimeLong(p.saved)}</div>
              <div className="rpt-card-label">saved</div>
            </div>
          ))}
        </div>

        <ValueSection tiles={allValueTiles} hourlyRate={hourlyRate} />

        <SectionTitle>Records and Streaks</SectionTitle>
        <div className="rpt-cards-4">
          {[
            { v: (stats.total_actions || 0).toLocaleString(), l: 'all-time actions', accent: false },
            { v: formatTimeLong(stats.best_day_time_saved_seconds), l: 'best day', accent: true },
            { v: String(streaks.current || 0), l: 'current streak (days)', accent: false },
            { v: String(streaks.longest || 0), l: 'longest streak (days)', accent: true },
          ].map((c, i) => (
            <div key={i} className="rpt-card rpt-card-sm">
              <div className={`rpt-card-value${c.accent ? ' rpt-accent' : ''}`}>{c.v}</div>
              <div className="rpt-card-label">{c.l}</div>
            </div>
          ))}
        </div>

        <HighlightsSection
          title="All-Time Highlights"
          assignments={data.assignments}
          topApps={data.topApps}
          heatmap={data.heatmap}
          efficiencyBlock={efficiency.all}
          hourlyRate={hourlyRate}
        />
        {footer}
      </div>

      <div className="rpt-page">
        {header(2)}
        <div className="rpt-gold-rule" />
        {chartSection}
        {heatmapSection}
        {breakdownSection}
        <SubPeriodTable sub={sub} totalActions={period.totalActions} hourlyRate={hourlyRate} />
        {footer}
      </div>

      {finalPage([
        { d: efficiency.week, l: 'this week' },
        { d: efficiency.month, l: 'this month' },
        { d: efficiency.all, l: 'all time' },
      ])}
    </div>
  );
}
