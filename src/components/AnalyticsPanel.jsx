import React, { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import './AnalyticsPanel.css';
import NumberField from './NumberField';

function formatTimeLong(seconds) {
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${Math.round(seconds % 60)}s`;
}

function formatTimeShort(seconds) {
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  const s = Math.round(seconds % 60);
  return `${m}m ${s}s`;
}

const DOW_LABELS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

// ── Free-tier teaser mock data ───────────────────────────────────────────────
// Shows behind a blur with an upgrade CTA overlaid — Free users see the shape
// of Detailed Analytics without the real numbers. Values are hand-picked to
// look plausible for a middle-of-the-road Pro user; they never render sharp so
// exact accuracy doesn't matter.
const TEASER_BAR_HEIGHTS = [62, 84, 71, 96, 58, 88, 74];
const TEASER_HEATMAP = Array.from({ length: 7 }, (_, dow) =>
  Array.from({ length: 24 }, (_, h) => {
    const workHours = dow >= 1 && dow <= 5 && h >= 9 && h <= 17;
    const noise = ((dow * 31 + h * 7) % 11) / 10;
    return workHours ? 0.45 + noise * 0.55 : noise * 0.32;
  })
);
const TEASER_LEADERBOARDS = {
  keys: [
    { label: 'Screenshot to clipboard', trigger: 'Ctrl+Shift+3', count: 412 },
    { label: 'Toggle mute', trigger: 'F13', count: 358 },
    { label: 'Paste sig block', trigger: 'Alt+S', count: 289 },
    { label: 'Open project folder', trigger: 'Ctrl+Alt+P', count: 246 },
    { label: 'Focus VS Code', trigger: 'Ctrl+Alt+V', count: 194 },
  ],
  exp: [
    { label: 'Email sign-off', trigger: ';sig', count: 512 },
    { label: 'Date', trigger: '/date', count: 388 },
    { label: 'Office address', trigger: ';addr', count: 267 },
    { label: 'Bank details', trigger: ';bank', count: 198 },
    { label: 'Meeting notes', trigger: '/mtg', count: 154 },
  ],
  apps: [
    { app: 'Google Chrome', count: 1240 },
    { app: 'VS Code', count: 890 },
    { app: 'Outlook', count: 612 },
    { app: 'Slack', count: 428 },
    { app: 'File Explorer', count: 315 },
  ],
};

export default function AnalyticsPanel({ isPro = false, onShowUpgrade }) {
  const [stats, setStats] = useState(null);
  const [dailyChart, setDailyChart] = useState([]);
  const [keysBreakdown, setKeysBreakdown] = useState([]);
  const [expBreakdown, setExpBreakdown] = useState([]);
  const [heatmap, setHeatmap] = useState([]);
  const [streaks, setStreaks] = useState({ current: 0, longest: 0 });
  const [keysSort, setKeysSort] = useState('count');         // 'count' | 'time'
  const [expSort, setExpSort] = useState('count');            // 'count' | 'time'
  const [keysRange, setKeysRange] = useState(0);             // 0=all, 7, 14, 30
  const [expRange, setExpRange] = useState(0);               // 0=all, 7, 14, 30
  const [typeRange, setTypeRange] = useState(0);             // 0=all, 7, 14, 30
  const [recordsRange, setRecordsRange] = useState(0);       // 0=all, 14, 30
  const [chartRange, setChartRange] = useState(7);            // 7 or 14
  const [typeBreakdown, setTypeBreakdown] = useState(null);  // { total, expansions, hotkeys, macros, time_saved }
  const [recordsBreakdown, setRecordsBreakdown] = useState(null);
  const [topApps, setTopApps] = useState([]);
  const [appsRange, setAppsRange] = useState(0);              // 0=all, 7, 14, 30
  const [appsSort, setAppsSort] = useState('count');           // 'count' | 'time'
  const [expEfficiency, setExpEfficiency] = useState(null);    // { total_expansions, chars_expanded, chars_typed, ratio }
  const [hourlyRate, setHourlyRate] = useState(() => {
    try { return parseFloat(localStorage.getItem('trigr.hourlyRate')) || 0; } catch { return 0; }
  });

  // ── Custom tooltip state ──
  const [tooltip, setTooltip] = useState(null); // { x, y, lines: [{ label, value, accent? }] }
  const tooltipTimer = useRef(null);

  function showTooltip(e, lines) {
    clearTimeout(tooltipTimer.current);
    const rect = e.currentTarget.getBoundingClientRect();
    const panelRect = e.currentTarget.closest('.analytics-panel')?.getBoundingClientRect();
    const yAbove = rect.top - (panelRect?.top || 0) - 4;
    const yBelow = rect.bottom - (panelRect?.top || 0) + 4;
    // Approximate tooltip height; flip to below the trigger when there's no room above.
    const estimatedHeight = Math.max(60, lines.length * 22 + 16);
    const flipBelow = yAbove < estimatedHeight;
    setTooltip({
      x: rect.left + rect.width / 2 - (panelRect?.left || 0),
      y: flipBelow ? yBelow : yAbove,
      below: flipBelow,
      lines,
    });
  }

  function hideTooltip() {
    tooltipTimer.current = setTimeout(() => setTooltip(null), 80);
  }

  const fetchStats = useCallback(async () => {
    const data = await window.electronAPI?.getAnalytics();
    if (data) setStats(data);
  }, []);

  const fetchChartData = useCallback(async () => {
    const [chart, hm] = await Promise.all([
      window.electronAPI?.getDailyChart(chartRange),
      window.electronAPI?.getHourlyHeatmap(chartRange),
    ]);
    if (chart) setDailyChart(chart);
    if (hm) setHeatmap(hm);
    if (isPro) {
      const st = await window.electronAPI?.getStreaks();
      if (st) setStreaks(st);
    }
  }, [isPro, chartRange]);

  // Fetch breakdown for each leaderboard independently
  const fetchBreakdown = useCallback(async () => {
    if (!isPro) return;
    // If both ranges match, one fetch covers both tables
    if (keysRange === expRange) {
      const bd = await window.electronAPI?.getAssignmentBreakdown(keysRange || null);
      if (bd) { setKeysBreakdown(bd); setExpBreakdown(bd); }
    } else {
      const [kbd, ebd] = await Promise.all([
        window.electronAPI?.getAssignmentBreakdown(keysRange || null),
        window.electronAPI?.getAssignmentBreakdown(expRange || null),
      ]);
      if (kbd) setKeysBreakdown(kbd);
      if (ebd) setExpBreakdown(ebd);
    }
  }, [isPro, keysRange, expRange]);

  const fetchTypeBreakdown = useCallback(async () => {
    const tb = await window.electronAPI?.getTypeBreakdown(typeRange || null);
    if (tb) setTypeBreakdown(tb);
  }, [typeRange]);

  const fetchRecordsBreakdown = useCallback(async () => {
    if (recordsRange === 0) { setRecordsBreakdown(null); return; }
    const rb = await window.electronAPI?.getTypeBreakdown(recordsRange);
    if (rb) setRecordsBreakdown(rb);
  }, [recordsRange]);

  const fetchTopApps = useCallback(async () => {
    if (!isPro) return;
    const data = await window.electronAPI?.getTopApps(appsRange || null);
    if (data) setTopApps(data);
  }, [isPro, appsRange]);

  const fetchExpEfficiency = useCallback(async () => {
    if (!isPro) return;
    const data = await window.electronAPI?.getExpansionEfficiency();
    if (data) setExpEfficiency(data);
  }, [isPro]);

  useEffect(() => {
    const refreshAll = () => {
      fetchStats();
      fetchChartData();
      fetchBreakdown();
      fetchTypeBreakdown();
      fetchRecordsBreakdown();
      fetchTopApps();
      fetchExpEfficiency();
    };
    refreshAll();
    // The 30 s tick used to re-run every widget query (~10 aggregates over the
    // whole log). Heavy users felt that as a periodic hitch, so the tick now
    // refreshes only the headline counters; the full set refreshes when the
    // window regains focus, which is when the numbers can actually have moved
    // by something the user did elsewhere.
    const interval = setInterval(fetchStats, 30000);
    let lastFull = Date.now();
    const onFocus = () => {
      if (Date.now() - lastFull < 15000) return;
      lastFull = Date.now();
      refreshAll();
    };
    window.addEventListener('focus', onFocus);
    return () => { clearInterval(interval); window.removeEventListener('focus', onFocus); };
  }, [fetchStats, fetchChartData, fetchBreakdown, fetchTypeBreakdown, fetchRecordsBreakdown, fetchTopApps, fetchExpEfficiency]);

  const [confirmReset, setConfirmReset] = useState(false);
  const confirmTimer = React.useRef(null);

  function handleResetClick() {
    if (confirmReset) {
      clearTimeout(confirmTimer.current);
      setConfirmReset(false);
      window.electronAPI?.resetAnalytics().then(() => { fetchStats(); fetchChartData(); fetchBreakdown(); fetchTypeBreakdown(); fetchRecordsBreakdown(); fetchTopApps(); fetchExpEfficiency(); });
    } else {
      setConfirmReset(true);
      confirmTimer.current = setTimeout(() => setConfirmReset(false), 3000);
    }
  }

  // ── Export (XLSX workbook / PDF report) ──
  const [exportMenuOpen, setExportMenuOpen] = useState(false);
  const [exportRange, setExportRange] = useState(0); // 0=all, 1=today, 7, 14, 30, 'custom'
  const [exportFrom, setExportFrom] = useState(() => {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-01`;
  });
  const [exportTo, setExportTo] = useState(() => {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  });
  const [exportBusy, setExportBusy] = useState(null); // null | 'xlsx' | 'pdf'
  const [exportToast, setExportToast] = useState(null); // { kind: 'ok'|'error', text }
  const exportMenuRef = useRef(null);
  const toastTimer = useRef(null);
  const pdfTimeout = useRef(null);

  const showToast = useCallback((kind, text) => {
    clearTimeout(toastTimer.current);
    setExportToast({ kind, text });
    toastTimer.current = setTimeout(() => setExportToast(null), 6000);
  }, []);

  useEffect(() => {
    if (!exportMenuOpen) return;
    const close = (e) => {
      if (exportMenuRef.current && !exportMenuRef.current.contains(e.target)) setExportMenuOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [exportMenuOpen]);

  // PDF completion arrives as an event: the backend renders a hidden report
  // window, prints it, then emits analytics-pdf-done.
  useEffect(() => {
    window.electronAPI?.onAnalyticsPdfDone?.((payload) => {
      clearTimeout(pdfTimeout.current);
      setExportBusy(null);
      if (payload?.ok) {
        showToast('ok', `Report saved to ${payload.path}`);
      } else {
        showToast('error', payload?.error || 'PDF export failed');
      }
    });
    return () => {
      clearTimeout(toastTimer.current);
      clearTimeout(pdfTimeout.current);
    };
  }, [showToast]);

  // (days, from, to) for the export commands: a custom range passes dates,
  // presets pass the day count. Returns null when custom dates are incomplete.
  function exportArgs() {
    if (exportRange === 'custom') {
      if (!exportFrom || !exportTo) return null;
      return [0, exportFrom, exportTo];
    }
    return [exportRange, null, null];
  }

  async function handleExportXlsx() {
    const args = exportArgs();
    if (!args) { showToast('error', 'Pick both dates for a custom range'); return; }
    setExportMenuOpen(false);
    setExportBusy('xlsx');
    try {
      const res = await window.electronAPI?.exportAnalyticsXlsx(...args);
      if (res?.ok) {
        showToast('ok', `Workbook saved to ${res.path}`);
      } else if (!res?.cancelled) {
        showToast('error', res?.error || 'Export failed');
      }
    } catch {
      showToast('error', 'Export failed');
    } finally {
      setExportBusy(null);
    }
  }

  async function handleExportPdf() {
    const args = exportArgs();
    if (!args) { showToast('error', 'Pick both dates for a custom range'); return; }
    setExportMenuOpen(false);
    setExportBusy('pdf');
    try {
      const res = await window.electronAPI?.exportAnalyticsPdf(...args);
      if (!res?.ok) {
        setExportBusy(null);
        if (!res?.cancelled) showToast('error', res?.error || 'Export failed');
        return;
      }
      // Stay busy until analytics-pdf-done; backstop in case the event is lost.
      clearTimeout(pdfTimeout.current);
      pdfTimeout.current = setTimeout(() => {
        setExportBusy((b) => {
          if (b === 'pdf') showToast('error', 'PDF export timed out');
          return b === 'pdf' ? null : b;
        });
      }, 45000);
    } catch {
      setExportBusy(null);
      showToast('error', 'Export failed');
    }
  }

  useEffect(() => {
    return () => clearTimeout(confirmTimer.current);
  }, []);

  // ── Chart helpers ──────────────────────────────────────────────────────────

  const chartMax = useMemo(() => {
    if (!dailyChart.length) return 1;
    return Math.max(1, ...dailyChart.map(d => d.actions));
  }, [dailyChart]);

  const chartTimeSavedMax = useMemo(() => {
    if (!dailyChart.length) return 1;
    return Math.max(1, ...dailyChart.map(d => d.time_saved));
  }, [dailyChart]);

  // Fill in missing days for the chart
  const chartDayCount = chartRange;
  const chartDays = useMemo(() => {
    const map = {};
    dailyChart.forEach(d => { map[d.date] = d; });
    const days = [];
    for (let i = chartDayCount - 1; i >= 0; i--) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      const key = d.toISOString().slice(0, 10);
      days.push(map[key] || { date: key, actions: 0, time_saved: 0 });
    }
    return days;
  }, [dailyChart, chartDayCount]);

  // ── Heatmap helpers ────────────────────────────────────────────────────────

  const heatmapGrid = useMemo(() => {
    const grid = Array.from({ length: 7 }, () => Array.from({ length: 24 }, () => ({ count: 0, time_saved: 0 })));
    let max = 1;
    heatmap.forEach(({ dow, hour, count, time_saved }) => {
      grid[dow][hour] = { count, time_saved: time_saved || 0 };
      if (count > max) max = count;
    });
    return { grid, max };
  }, [heatmap]);

  // ── Sorted breakdown ──────────────────────────────────────────────────────

  const KEY_MAPPING_TYPES = new Set(['hotkey', 'text', 'app', 'url', 'folder', 'macro', 'search_template']);

  const keysLeaderboard = useMemo(() => {
    const arr = keysBreakdown.filter(item => KEY_MAPPING_TYPES.has(item.type) && item.time_saved > 0);
    return keysSort === 'time'
      ? arr.sort((a, b) => b.time_saved - a.time_saved)
      : arr.sort((a, b) => b.count - a.count);
  }, [keysBreakdown, keysSort]);

  const expLeaderboard = useMemo(() => {
    const arr = expBreakdown.filter(item => item.type === 'expansion');
    return expSort === 'time'
      ? arr.sort((a, b) => b.time_saved - a.time_saved)
      : arr.sort((a, b) => b.count - a.count);
  }, [expBreakdown, expSort]);

  const sortedApps = useMemo(() => {
    const arr = [...topApps];
    return appsSort === 'time'
      ? arr.sort((a, b) => b.time_saved - a.time_saved)
      : arr.sort((a, b) => b.count - a.count);
  }, [topApps, appsSort]);

  // ── Shared breakdown renderer ──
  function renderBreakdown() {
    const tb = typeBreakdown || { total: 0, expansions: 0, hotkeys: 0, macros: 0, autocorrects: 0 };
    const tbTotal = tb.total || 0;
    const tbExp = tb.expansions || 0;
    const tbHot = tb.hotkeys || 0;
    const tbMac = tb.macros || 0;
    const tbAc = tb.autocorrects || 0;
    const pExp = tbTotal > 0 ? Math.round((tbExp / tbTotal) * 100) : 0;
    const pHot = tbTotal > 0 ? Math.round((tbHot / tbTotal) * 100) : 0;
    const pMac = tbTotal > 0 ? Math.round((tbMac / tbTotal) * 100) : 0;
    const pAc = tbTotal > 0 ? Math.round((tbAc / tbTotal) * 100) : 0;
    if (tbTotal === 0) {
      return <div className="analytics-empty"><div className="analytics-empty-title">No data yet</div>Fire a hotkey, expansion, or macro to start tracking.</div>;
    }
    return (
      <div className="analytics-breakdown">
        {[
          { cls: 'expansion', label: 'Expansions', count: tbExp, pct: pExp },
          { cls: 'hotkey',    label: 'Hotkeys',    count: tbHot, pct: pHot },
          { cls: 'macro',     label: 'Macros',     count: tbMac, pct: pMac },
          // Row hidden until the first fire — free-tier users never see it.
          ...(tbAc > 0 ? [{ cls: 'autocorrect', label: 'Typos fixed', count: tbAc, pct: pAc }] : []),
        ].map(r => (
          <div key={r.cls} className="analytics-breakdown-row">
            <span className={`analytics-breakdown-dot ${r.cls}`} />
            <span className="analytics-breakdown-label">{r.label}</span>
            <span className="analytics-breakdown-count">{r.count.toLocaleString()}</span>
            <div className="analytics-breakdown-bar-wrap">
              <div className={`analytics-breakdown-bar ${r.cls}`} style={{ width: `${r.pct}%` }} />
            </div>
            <span className="analytics-breakdown-pct">{r.pct}%</span>
          </div>
        ))}
      </div>
    );
  }

  if (!stats) return null;

  const total = stats.total_actions || 0;
  const expansions = stats.expansions || 0;
  const hotkeys = stats.hotkeys || 0;
  const macros = stats.macros || 0;

  const pctExp = total > 0 ? Math.round((expansions / total) * 100) : 0;
  const pctHot = total > 0 ? Math.round((hotkeys / total) * 100) : 0;
  const pctMac = total > 0 ? Math.round((macros / total) * 100) : 0;

  return (
    <div className="analytics-panel">
      <div className="analytics-header">
        <span className="analytics-title">Analytics</span>
        {isPro && (
          <div className="analytics-export-wrap" ref={exportMenuRef}>
            <button
              type="button"
              className="analytics-export-btn"
              disabled={!!exportBusy}
              onClick={() => setExportMenuOpen(o => !o)}
            >
              {exportBusy === 'xlsx' ? 'Exporting…' : exportBusy === 'pdf' ? 'Generating report…' : 'Export'}
              {!exportBusy && <span className="analytics-export-caret" />}
              {!isPro && <span className="pro-badge">PRO</span>}
            </button>
            {exportMenuOpen && (
              <div className="analytics-export-menu">
                <div className="analytics-export-range-label">Period</div>
                <div className="analytics-export-ranges">
                  {[
                    { v: 0, label: 'All time' },
                    { v: 1, label: 'Today' },
                    { v: 7, label: '7 days' },
                    { v: 14, label: '14 days' },
                    { v: 30, label: '30 days' },
                    { v: 'custom', label: 'Custom' },
                  ].map(r => (
                    <button
                      key={r.v}
                      type="button"
                      className={`analytics-export-range${exportRange === r.v ? ' active' : ''}`}
                      onClick={() => setExportRange(r.v)}
                    >
                      {r.label}
                    </button>
                  ))}
                </div>
                {exportRange === 'custom' && (
                  <div className="analytics-export-dates">
                    <label className="analytics-export-date-field">
                      <span>From</span>
                      <input
                        type="date"
                        value={exportFrom}
                        max={exportTo || undefined}
                        onChange={e => setExportFrom(e.target.value)}
                      />
                    </label>
                    <label className="analytics-export-date-field">
                      <span>To</span>
                      <input
                        type="date"
                        value={exportTo}
                        min={exportFrom || undefined}
                        onChange={e => setExportTo(e.target.value)}
                      />
                    </label>
                  </div>
                )}
                <div className="analytics-export-divider" />
                <button type="button" className="analytics-export-item" onClick={handleExportXlsx}>
                  <span className="analytics-export-item-title">Excel workbook</span>
                  <span className="analytics-export-item-sub">Every section as its own sheet (.xlsx)</span>
                </button>
                <button type="button" className="analytics-export-item" onClick={handleExportPdf}>
                  <span className="analytics-export-item-title">PDF report</span>
                  <span className="analytics-export-item-sub">Styled summary with charts (.pdf)</span>
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="analytics-body">
        {/* ── Section 1: Period summary cards ──────────────── */}
        <section className="analytics-section">
          <div className="analytics-cards-4">
            {[
              { title: 'TODAY',        actions: stats.actions_today || 0,          saved: stats.time_saved_today_seconds || 0 },
              { title: 'LAST 7 DAYS',  actions: stats.actions_last_7_days || 0,   saved: stats.time_saved_last_7_days_seconds || 0 },
              { title: 'LAST 14 DAYS', actions: stats.actions_last_14_days || 0,  saved: stats.time_saved_last_14_days_seconds || 0 },
              { title: 'LAST 30 DAYS', actions: stats.actions_last_30_days || 0,  saved: stats.time_saved_last_30_days_seconds || 0 },
            ].map(p => (
              <div key={p.title} className="analytics-card-compound">
                <div className="analytics-card-compound-title">{p.title}</div>
                <div className="analytics-card-compound-stat">
                  <span className="analytics-card-compound-value">{p.actions.toLocaleString()}</span>
                  <span className="analytics-card-compound-label">actions</span>
                </div>
                <div className="analytics-card-compound-stat">
                  <span className="analytics-card-compound-value accent">{formatTimeLong(p.saved)}</span>
                  <span className="analytics-card-compound-label">saved</span>
                </div>
              </div>
            ))}
          </div>
        </section>

        {/* ── Records + Breakdown layout (differs for free vs pro) ── */}
        {isPro ? (
          <>
            {/* Pro: Records full row with 4 cards */}
            <section className="analytics-section">
              <div className="analytics-section-title">
                RECORDS
                <select className="analytics-range-select" value={recordsRange} onChange={e => setRecordsRange(Number(e.target.value))}>
                  <option value={0}>All time</option>
                  <option value={14}>Last 14 days</option>
                  <option value={30}>Last 30 days</option>
                </select>
              </div>
              {(() => {
                const rb = recordsBreakdown;
                const recActions = rb ? (rb.total || 0) : total;
                const recSaved = rb ? (rb.time_saved || 0) : (stats.total_time_saved_seconds || 0);
                const rangeLabel = recordsRange === 0 ? 'all time' : `last ${recordsRange} days`;
                return (
                  <div className="analytics-cards-4">
                    <div className="analytics-card-sm">
                      <span className="analytics-card-sm-value">{recActions.toLocaleString()}</span>
                      <span className="analytics-card-sm-label">{rangeLabel} actions</span>
                    </div>
                    <div className="analytics-card-sm">
                      <span className="analytics-card-sm-value accent">{formatTimeLong(recSaved)}</span>
                      <span className="analytics-card-sm-label">{rangeLabel} saved</span>
                    </div>
                    <div className="analytics-card-sm">
                      <span className="analytics-card-sm-value accent">{formatTimeLong(stats.best_day_time_saved_seconds || 0)}</span>
                      <span className="analytics-card-sm-label">best day</span>
                    </div>
                    <div className="analytics-card-sm">
                      <span className="analytics-card-sm-value accent">{formatTimeLong(stats.best_7_days_time_saved_seconds || 0)}</span>
                      <span className="analytics-card-sm-label">best 7 days</span>
                    </div>
                  </div>
                );
              })()}
            </section>

            {/* Pro: Breakdown + Streaks + ROI + Efficiency four-col */}
            <div className="analytics-four-col">
              <section className="analytics-section">
                <div className="analytics-section-title">
                  BREAKDOWN
                  <select className="analytics-range-select" value={typeRange} onChange={e => setTypeRange(Number(e.target.value))}>
                    <option value={0}>All time</option>
                    <option value={7}>Last 7 days</option>
                    <option value={14}>Last 14 days</option>
                    <option value={30}>Last 30 days</option>
                  </select>
                </div>
                {renderBreakdown()}
              </section>

              <section className="analytics-section">
                <div className="analytics-section-title">STREAKS {!isPro && <span className="pro-badge">PRO</span>}</div>
                <div className="analytics-streaks">
                  <div className="analytics-streak-card">
                    <span className="analytics-streak-value">{streaks.current}</span>
                    <span className="analytics-streak-label">current streak (days)</span>
                  </div>
                  <div className="analytics-streak-card">
                    <span className="analytics-streak-value accent">{streaks.longest}</span>
                    <span className="analytics-streak-label">longest streak (days)</span>
                  </div>
                </div>
              </section>

              <section className="analytics-section">
                <div className="analytics-section-title">ROI CALCULATOR {!isPro && <span className="pro-badge">PRO</span>}</div>
                <div className="analytics-roi-body">
                  <div className="analytics-roi-input-row">
                    <label className="analytics-roi-label">Hourly rate</label>
                    <div className="analytics-roi-input-wrap">
                      <span className="analytics-roi-currency">£</span>
                      <NumberField
                        className="analytics-roi-input"
                        value={hourlyRate || 0}
                        placeholder="0"
                        min={0}
                        max={100000}
                        defaultOnEmpty={0}
                        step="0.01"
                        float
                        onCommit={val => {
                          setHourlyRate(val);
                          localStorage.setItem('trigr.hourlyRate', String(val));
                        }}
                      />
                      <span className="analytics-roi-per">/hr</span>
                    </div>
                  </div>
                  {hourlyRate > 0 && (
                    <div className="analytics-roi-result analytics-roi-result--3">
                      <div className="analytics-roi-stat">
                        <span className="analytics-roi-stat-value accent">{((stats.time_saved_last_7_days_seconds || 0) / 3600 * hourlyRate).toLocaleString(undefined, { style: 'currency', currency: 'GBP', maximumFractionDigits: 0 })}</span>
                        <span className="analytics-roi-stat-label">this week</span>
                      </div>
                      <div className="analytics-roi-stat">
                        <span className="analytics-roi-stat-value accent">{((stats.time_saved_last_30_days_seconds || 0) / 3600 * hourlyRate).toLocaleString(undefined, { style: 'currency', currency: 'GBP', maximumFractionDigits: 0 })}</span>
                        <span className="analytics-roi-stat-label">this month</span>
                      </div>
                      <div className="analytics-roi-stat">
                        <span className="analytics-roi-stat-value accent">{((stats.total_time_saved_seconds || 0) / 3600 * hourlyRate).toLocaleString(undefined, { style: 'currency', currency: 'GBP', maximumFractionDigits: 0 })}</span>
                        <span className="analytics-roi-stat-label">total</span>
                      </div>
                    </div>
                  )}
                </div>
              </section>

              <section className="analytics-section">
                <div className="analytics-section-title">EXPANSION EFFICIENCY {!isPro && <span className="pro-badge">PRO</span>}</div>
                {expEfficiency && expEfficiency.all?.total_expansions > 0 ? (
                  <div className="analytics-efficiency-body-cols">
                    {[
                      { label: 'This Week', data: expEfficiency.week },
                      { label: 'This Month', data: expEfficiency.month },
                      { label: 'All Time', data: expEfficiency.all },
                    ].map(col => (
                      <div key={col.label} className="analytics-efficiency-col-wrap">
                        <div className="analytics-efficiency-multiplier">
                          <span className="analytics-efficiency-ratio-value accent">{Math.round(col.data?.ratio || 0)}x</span>
                          <span className="analytics-efficiency-col-header">{col.label}</span>
                        </div>
                        <div className="analytics-efficiency-col">
                          <div className="analytics-efficiency-col-stats">
                            <div className="analytics-efficiency-stat">
                              <span className="analytics-efficiency-stat-value">{(col.data?.total_expansions || 0).toLocaleString()}</span>
                              <span className="analytics-efficiency-stat-label">fired</span>
                            </div>
                            <div className="analytics-efficiency-stat">
                              <span className="analytics-efficiency-stat-value">{(col.data?.chars_typed || 0).toLocaleString()}</span>
                              <span className="analytics-efficiency-stat-label">typed</span>
                            </div>
                            <div className="analytics-efficiency-stat">
                              <span className="analytics-efficiency-stat-value accent">{(col.data?.chars_expanded || 0).toLocaleString()}</span>
                              <span className="analytics-efficiency-stat-label">expanded</span>
                            </div>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="analytics-empty-tab">Use expansions to see your efficiency ratio</div>
                )}
              </section>
            </div>
          </>
        ) : (
          <>
            {/* Free: Records (2 cards) + Breakdown side by side */}
            <div className="analytics-two-col">
              <section className="analytics-section">
                <div className="analytics-section-title">RECORDS</div>
                <div className="analytics-cards-2">
                  <div className="analytics-card-sm">
                    <span className="analytics-card-sm-value">{total.toLocaleString()}</span>
                    <span className="analytics-card-sm-label">all time actions</span>
                  </div>
                  <div className="analytics-card-sm">
                    <span className="analytics-card-sm-value accent">{formatTimeLong(stats.total_time_saved_seconds || 0)}</span>
                    <span className="analytics-card-sm-label">all time saved</span>
                  </div>
                </div>
              </section>

              <section className="analytics-section">
                <div className="analytics-section-title">
                  BREAKDOWN
                  <select className="analytics-range-select" value={typeRange} onChange={e => setTypeRange(Number(e.target.value))}>
                    <option value={0}>All time</option>
                    <option value={7}>Last 7 days</option>
                    <option value={14}>Last 14 days</option>
                    <option value={30}>Last 30 days</option>
                  </select>
                </div>
                {renderBreakdown()}
              </section>
            </div>

            {/* Free: teaser view of Detailed Analytics — blurred mock of the
                Pro layout with an upgrade CTA card centered over the top.
                Values are hardcoded, sections are visual approximations of
                the real Pro layout. */}
            <section className="analytics-teaser">
              <div className="analytics-teaser-mock" aria-hidden="true">
                <div className="analytics-teaser-row analytics-teaser-cards">
                  <div className="analytics-teaser-panel">
                    <div className="analytics-teaser-panel-title">STREAKS</div>
                    <div className="analytics-teaser-stats">
                      <div className="analytics-teaser-stat">
                        <span className="analytics-teaser-stat-value">14</span>
                        <span className="analytics-teaser-stat-label">current (days)</span>
                      </div>
                      <div className="analytics-teaser-stat">
                        <span className="analytics-teaser-stat-value accent">47</span>
                        <span className="analytics-teaser-stat-label">longest (days)</span>
                      </div>
                    </div>
                  </div>
                  <div className="analytics-teaser-panel">
                    <div className="analytics-teaser-panel-title">ROI CALCULATOR</div>
                    <div className="analytics-teaser-input">£45.00 /hr</div>
                    <div className="analytics-teaser-stats analytics-teaser-stats-3">
                      <div className="analytics-teaser-stat">
                        <span className="analytics-teaser-stat-value accent">£68</span>
                        <span className="analytics-teaser-stat-label">this week</span>
                      </div>
                      <div className="analytics-teaser-stat">
                        <span className="analytics-teaser-stat-value accent">£262</span>
                        <span className="analytics-teaser-stat-label">this month</span>
                      </div>
                      <div className="analytics-teaser-stat">
                        <span className="analytics-teaser-stat-value accent">£850</span>
                        <span className="analytics-teaser-stat-label">total</span>
                      </div>
                    </div>
                  </div>
                  <div className="analytics-teaser-panel">
                    <div className="analytics-teaser-panel-title">EXPANSION EFFICIENCY</div>
                    <div className="analytics-teaser-eff-row">
                      {[
                        { label: 'Week',  mult: '6x' },
                        { label: 'Month', mult: '7x' },
                        { label: 'All',   mult: '8x' },
                      ].map(e => (
                        <div key={e.label} className="analytics-teaser-eff">
                          <span className="analytics-teaser-eff-mult accent">{e.mult}</span>
                          <span className="analytics-teaser-eff-label">{e.label}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="analytics-teaser-row analytics-teaser-charts">
                  <div className="analytics-teaser-panel">
                    <div className="analytics-teaser-panel-title">ACTIVITY</div>
                    <div className="analytics-teaser-bars">
                      {TEASER_BAR_HEIGHTS.map((h, i) => (
                        <div key={i} className="analytics-teaser-bar-col">
                          <div className="analytics-teaser-bar analytics-teaser-bar-actions" style={{ height: `${h}%` }} />
                          <div className="analytics-teaser-bar analytics-teaser-bar-saved" style={{ height: `${Math.max(10, h - 20)}%` }} />
                        </div>
                      ))}
                    </div>
                  </div>
                  <div className="analytics-teaser-panel">
                    <div className="analytics-teaser-panel-title">HEATMAP</div>
                    <div className="analytics-teaser-heatmap">
                      {TEASER_HEATMAP.flatMap((row, dow) =>
                        row.map((v, h) => (
                          <div
                            key={`${dow}-${h}`}
                            className="analytics-teaser-cell"
                            style={{ opacity: v > 0.05 ? 1 : 0.3, background: v > 0.05 ? `rgba(232, 160, 32, ${v})` : 'var(--bg-elevated)' }}
                          />
                        ))
                      )}
                    </div>
                  </div>
                </div>

                <div className="analytics-teaser-row analytics-teaser-boards">
                  {[
                    { title: 'KEY MAPPINGS',    items: TEASER_LEADERBOARDS.keys },
                    { title: 'TEXT EXPANSIONS', items: TEASER_LEADERBOARDS.exp },
                    { title: 'TOP APPS',        items: TEASER_LEADERBOARDS.apps },
                  ].map(board => (
                    <div key={board.title} className="analytics-teaser-panel">
                      <div className="analytics-teaser-panel-title">{board.title}</div>
                      <div className="analytics-teaser-board">
                        {board.items.map((it, i) => (
                          <div key={i} className="analytics-teaser-board-row">
                            <span className="analytics-teaser-board-rank">{i + 1}</span>
                            <span className="analytics-teaser-board-label">{it.label || it.app}</span>
                            <span className="analytics-teaser-board-count">{it.count}x</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              </div>

              <div className="analytics-teaser-cta">
                <span className="pro-badge">PRO</span>
                <div className="analytics-teaser-cta-title">Detailed Analytics</div>
                <div className="analytics-teaser-cta-desc">
                  Streaks, ROI calculator, expansion efficiency, activity chart, heatmap, and per-mapping leaderboards. Plus Excel and PDF export.
                </div>
                <button
                  type="button"
                  className="analytics-teaser-cta-btn"
                  onClick={() => onShowUpgrade?.('Detailed Analytics')}
                >Upgrade to Pro</button>
              </div>
            </section>
          </>
        )}

        {/* ── Chart + Heatmap (two-col, pro only) ── */}
        {isPro && (
          <div className="analytics-two-col">
            <section className="analytics-section">
              <div className="analytics-section-title">
                ACTIVITY {!isPro && <span className="pro-badge">PRO</span>}
                <select className="analytics-range-select" value={chartRange} onChange={e => setChartRange(Number(e.target.value))}>
                  <option value={7}>Last 7 days</option>
                  <option value={14}>Last 14 days</option>
                </select>
              </div>
              <div className="analytics-chart">
                {chartDays.map((day) => {
                  const dayLabel = new Date(day.date + 'T00:00').toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
                  return (
                    <div
                      key={day.date}
                      className="analytics-chart-bar-col"
                      onMouseEnter={e => showTooltip(e, [
                        { label: dayLabel },
                        { label: 'Actions', value: String(day.actions) },
                        { label: 'Saved', value: formatTimeShort(day.time_saved), accent: true },
                      ])}
                      onMouseLeave={hideTooltip}
                    >
                      <div className="analytics-chart-bar-wrap">
                        <div
                          className="analytics-chart-bar analytics-chart-bar--actions"
                          style={{ height: `${Math.max(2, (day.actions / chartMax) * 100)}%` }}
                        />
                        <div
                          className="analytics-chart-bar analytics-chart-bar--saved"
                          style={{ height: `${Math.max(2, (day.time_saved / chartTimeSavedMax) * 100)}%` }}
                        />
                      </div>
                      <span className="analytics-chart-label">
                        {new Date(day.date + 'T00:00').toLocaleDateString(undefined, { weekday: 'narrow' })}
                      </span>
                    </div>
                  );
                })}
              </div>
              <div className="analytics-chart-legend">
                <span className="analytics-chart-legend-item"><span className="analytics-chart-legend-swatch analytics-chart-legend-swatch--actions" />Actions</span>
                <span className="analytics-chart-legend-item"><span className="analytics-chart-legend-swatch analytics-chart-legend-swatch--saved" />Time Saved</span>
              </div>
            </section>

            <section className="analytics-section">
              <div className="analytics-section-title">
                HEATMAP {!isPro && <span className="pro-badge">PRO</span>}
                <select className="analytics-range-select" value={chartRange} onChange={e => setChartRange(Number(e.target.value))}>
                  <option value={7}>Last 7 days</option>
                  <option value={14}>Last 14 days</option>
                </select>
              </div>
              <div className="analytics-heatmap">
                <div className="analytics-heatmap-corner" />
                {Array.from({ length: 24 }, (_, h) => (
                  <div key={h} className="analytics-heatmap-hour-label">{h}</div>
                ))}
                {(() => {
                  // Reorder rows so today is first, going backwards in time
                  const todayDow = new Date().getDay(); // 0=Sun..6=Sat
                  const orderedDows = [];
                  for (let i = 0; i < 7; i++) {
                    orderedDows.push((todayDow - i + 7) % 7);
                  }
                  return orderedDows.map(dow => {
                    const label = DOW_LABELS[dow];
                    return (
                  <React.Fragment key={dow}>
                    <div className="analytics-heatmap-dow-label">{dow === todayDow ? 'Today' : label}</div>
                    {Array.from({ length: 24 }, (_, h) => {
                      const cell = heatmapGrid.grid[dow][h];
                      const intensity = cell.count > 0 ? Math.max(0.15, cell.count / heatmapGrid.max) : 0;
                      return (
                        <div
                          key={h}
                          className="analytics-heatmap-cell"
                          style={{ opacity: intensity > 0 ? 1 : 0.3, background: intensity > 0 ? `rgba(232, 160, 32, ${intensity})` : 'var(--bg-elevated)' }}
                          onMouseEnter={cell.count > 0 ? e => showTooltip(e, [
                            { label: `${label} ${h}:00–${h + 1}:00` },
                            { label: 'Actions', value: String(cell.count) },
                            ...(cell.time_saved > 0 ? [{ label: 'Saved', value: formatTimeShort(cell.time_saved), accent: true }] : []),
                          ]) : undefined}
                          onMouseLeave={cell.count > 0 ? hideTooltip : undefined}
                        />
                      );
                    })}
                  </React.Fragment>
                    );
                  });
                })()}
              </div>
            </section>
          </div>
        )}

        {/* ── Leaderboards: Key Mappings + Text Expansions + Top Apps ── */}
        {isPro && (
          <div className="analytics-three-col">
            <section className="analytics-section">
              <div className="analytics-section-title">
                KEY MAPPINGS {!isPro && <span className="pro-badge">PRO</span>}
                <select className="analytics-range-select" value={keysRange} onChange={e => setKeysRange(Number(e.target.value))}>
                  <option value={0}>All time</option>
                  <option value={1}>Today</option>
                  <option value={7}>Last 7 days</option>
                  <option value={14}>Last 14 days</option>
                  <option value={30}>Last 30 days</option>
                </select>
              </div>
              <div className="analytics-breakdown-tabs">
                {[{ id: 'count', label: 'Most Used' }, { id: 'time', label: 'Most Saved' }].map(t => (
                  <button key={t.id} type="button" className={`analytics-breakdown-tab${keysSort === t.id ? ' active' : ''}`} onClick={() => setKeysSort(t.id)}>{t.label}</button>
                ))}
              </div>
              <div className="analytics-assignment-list">
                {keysLeaderboard.length > 0 ? keysLeaderboard.map((item, i) => (
                  <div key={item.trigger || i} className="analytics-assignment-row">
                    <span className="analytics-assignment-rank">{i + 1}</span>
                    <div className="analytics-assignment-info">
                      <span className="analytics-assignment-label">{item.label || item.trigger || '(unnamed)'}</span>
                      <span className="analytics-assignment-trigger">{item.trigger}</span>
                    </div>
                    <span className="analytics-assignment-count">{item.count}x</span>
                    <span className="analytics-assignment-saved">{formatTimeShort(item.time_saved)}</span>
                  </div>
                )) : <div className="analytics-empty-tab">No key mapping data yet</div>}
              </div>
            </section>

            <section className="analytics-section">
              <div className="analytics-section-title">
                TEXT EXPANSIONS {!isPro && <span className="pro-badge">PRO</span>}
                <select className="analytics-range-select" value={expRange} onChange={e => setExpRange(Number(e.target.value))}>
                  <option value={0}>All time</option>
                  <option value={1}>Today</option>
                  <option value={7}>Last 7 days</option>
                  <option value={14}>Last 14 days</option>
                  <option value={30}>Last 30 days</option>
                </select>
              </div>
              <div className="analytics-breakdown-tabs">
                {[{ id: 'count', label: 'Most Used' }, { id: 'time', label: 'Most Saved' }].map(t => (
                  <button key={t.id} type="button" className={`analytics-breakdown-tab${expSort === t.id ? ' active' : ''}`} onClick={() => setExpSort(t.id)}>{t.label}</button>
                ))}
              </div>
              <div className="analytics-assignment-list">
                {expLeaderboard.length > 0 ? expLeaderboard.map((item, i) => (
                  <div key={item.trigger || i} className="analytics-assignment-row">
                    <span className="analytics-assignment-rank">{i + 1}</span>
                    <div className="analytics-assignment-info">
                      <span className="analytics-assignment-label">{item.label || item.trigger || '(unnamed)'}</span>
                      <span className="analytics-assignment-trigger">{item.trigger}</span>
                    </div>
                    <span className="analytics-assignment-count">{item.count}x</span>
                    <span className="analytics-assignment-saved">{formatTimeShort(item.time_saved)}</span>
                  </div>
                )) : <div className="analytics-empty-tab">No expansion data yet</div>}
              </div>
            </section>

            <section className="analytics-section">
              <div className="analytics-section-title">
                TOP APPS {!isPro && <span className="pro-badge">PRO</span>}
                <select className="analytics-range-select" value={appsRange} onChange={e => setAppsRange(Number(e.target.value))}>
                  <option value={0}>All time</option>
                  <option value={1}>Today</option>
                  <option value={7}>Last 7 days</option>
                  <option value={14}>Last 14 days</option>
                  <option value={30}>Last 30 days</option>
                </select>
              </div>
              <div className="analytics-breakdown-tabs">
                {[{ id: 'count', label: 'Most Used' }, { id: 'time', label: 'Most Saved' }].map(t => (
                  <button key={t.id} type="button" className={`analytics-breakdown-tab${appsSort === t.id ? ' active' : ''}`} onClick={() => setAppsSort(t.id)}>{t.label}</button>
                ))}
              </div>
              <div className="analytics-assignment-list">
                {sortedApps.length > 0 ? sortedApps.map((item, i) => (
                  <div key={item.app || i} className="analytics-assignment-row">
                    <span className="analytics-assignment-rank">{i + 1}</span>
                    <div className="analytics-assignment-info">
                      <span className="analytics-assignment-label">{item.app || '(unknown)'}</span>
                    </div>
                    <span className="analytics-assignment-count">{item.count}x</span>
                    <span className="analytics-assignment-saved">{formatTimeShort(item.time_saved)}</span>
                  </div>
                )) : <div className="analytics-empty-tab">App data will appear as you use Keyfire</div>}
              </div>
            </section>
          </div>
        )}


        {/* ── Section: Reset ──────────────────────────────── */}
        {total > 0 && (
          <div className="analytics-reset-row">
            <button
              type="button"
              className={`analytics-reset-btn${confirmReset ? ' analytics-reset-btn--confirm' : ''}`}
              onClick={handleResetClick}
            >
              {confirmReset ? 'Are you sure?' : 'Reset Statistics'}
            </button>
          </div>
        )}
      </div>

      {/* ── Export result toast ── */}
      {exportToast && (
        <div className={`analytics-toast analytics-toast--${exportToast.kind}`}>
          {exportToast.text}
        </div>
      )}

      {/* ── Custom tooltip ── */}
      {tooltip && (
        <div
          className={`analytics-tooltip${tooltip.below ? ' analytics-tooltip--below' : ''}`}
          style={{ left: tooltip.x, top: tooltip.y }}
        >
          {tooltip.lines.map((line, i) => (
            <div key={i} className={`analytics-tooltip-line${line.value == null ? ' analytics-tooltip-header' : ''}`}>
              {line.value != null ? (
                <>
                  <span className="analytics-tooltip-label">{line.label}</span>
                  <span className={`analytics-tooltip-value${line.accent ? ' accent' : ''}`}>{line.value}</span>
                </>
              ) : (
                <span className="analytics-tooltip-title">{line.label}</span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
