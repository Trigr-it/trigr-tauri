import React, { useState, useEffect, useCallback, useMemo } from 'react';
import './AnalyticsPanel.css';

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

export default function AnalyticsPanel({ isPro = false }) {
  const [stats, setStats] = useState(null);
  const [dailyChart, setDailyChart] = useState([]);
  const [breakdown, setBreakdown] = useState([]);
  const [heatmap, setHeatmap] = useState([]);
  const [streaks, setStreaks] = useState({ current: 0, longest: 0 });
  const [breakdownSort, setBreakdownSort] = useState('count');

  const fetchStats = useCallback(async () => {
    const data = await window.electronAPI?.getAnalytics();
    if (data) setStats(data);
  }, []);

  const fetchProData = useCallback(async () => {
    if (!isPro) return;
    const [chart, bd, hm, st] = await Promise.all([
      window.electronAPI?.getDailyChart(14),
      window.electronAPI?.getAssignmentBreakdown(),
      window.electronAPI?.getHourlyHeatmap(),
      window.electronAPI?.getStreaks(),
    ]);
    if (chart) setDailyChart(chart);
    if (bd) setBreakdown(bd);
    if (hm) setHeatmap(hm);
    if (st) setStreaks(st);
  }, [isPro]);

  useEffect(() => {
    fetchStats();
    fetchProData();
    const interval = setInterval(() => { fetchStats(); fetchProData(); }, 30000);
    return () => clearInterval(interval);
  }, [fetchStats, fetchProData]);

  const [confirmReset, setConfirmReset] = useState(false);
  const confirmTimer = React.useRef(null);

  function handleResetClick() {
    if (confirmReset) {
      clearTimeout(confirmTimer.current);
      setConfirmReset(false);
      window.electronAPI?.resetAnalytics().then(() => { fetchStats(); fetchProData(); });
    } else {
      setConfirmReset(true);
      confirmTimer.current = setTimeout(() => setConfirmReset(false), 3000);
    }
  }

  async function handleExportCsv() {
    const csv = await window.electronAPI?.exportAnalyticsCsv();
    if (!csv) return;
    const blob = new Blob([csv], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `trigr-analytics-${new Date().toISOString().slice(0, 10)}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  }

  useEffect(() => {
    return () => clearTimeout(confirmTimer.current);
  }, []);

  // ── Chart helpers ──────────────────────────────────────────────────────────

  const chartMax = useMemo(() => {
    if (!dailyChart.length) return 1;
    return Math.max(1, ...dailyChart.map(d => d.actions));
  }, [dailyChart]);

  // Fill in missing days for the chart
  const chartDays = useMemo(() => {
    const map = {};
    dailyChart.forEach(d => { map[d.date] = d; });
    const days = [];
    for (let i = 13; i >= 0; i--) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      const key = d.toISOString().slice(0, 10);
      days.push(map[key] || { date: key, actions: 0, time_saved: 0 });
    }
    return days;
  }, [dailyChart]);

  // ── Heatmap helpers ────────────────────────────────────────────────────────

  const heatmapGrid = useMemo(() => {
    const grid = Array.from({ length: 7 }, () => Array(24).fill(0));
    let max = 1;
    heatmap.forEach(({ dow, hour, count }) => {
      grid[dow][hour] = count;
      if (count > max) max = count;
    });
    return { grid, max };
  }, [heatmap]);

  // ── Sorted breakdown ──────────────────────────────────────────────────────

  const sortedBreakdown = useMemo(() => {
    const arr = [...breakdown];
    if (breakdownSort === 'count') arr.sort((a, b) => b.count - a.count);
    else if (breakdownSort === 'time') arr.sort((a, b) => b.time_saved - a.time_saved);
    else if (breakdownSort === 'label') arr.sort((a, b) => (a.label || '').localeCompare(b.label || ''));
    return arr;
  }, [breakdown, breakdownSort]);

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
          <button type="button" className="analytics-export-btn" onClick={handleExportCsv}>
            Export CSV
          </button>
        )}
      </div>

      <div className="analytics-body">
        {/* ── Section 1: Today + Last 7 Days ──────────────── */}
        <section className="analytics-section">
          <div className="analytics-cards-highlight">
            <div className="analytics-card-compound">
              <div className="analytics-card-compound-title">TODAY</div>
              <div className="analytics-card-compound-stat">
                <span className="analytics-card-compound-value">{(stats.actions_today || 0).toLocaleString()}</span>
                <span className="analytics-card-compound-label">actions</span>
              </div>
              <div className="analytics-card-compound-stat">
                <span className="analytics-card-compound-value accent">{formatTimeShort(stats.time_saved_today_seconds || 0)}</span>
                <span className="analytics-card-compound-label">saved</span>
              </div>
            </div>
            <div className="analytics-card-compound">
              <div className="analytics-card-compound-title">LAST 7 DAYS</div>
              <div className="analytics-card-compound-stat">
                <span className="analytics-card-compound-value">{(stats.actions_last_7_days || 0).toLocaleString()}</span>
                <span className="analytics-card-compound-label">actions</span>
              </div>
              <div className="analytics-card-compound-stat">
                <span className="analytics-card-compound-value accent">{formatTimeLong(stats.time_saved_last_7_days_seconds || 0)}</span>
                <span className="analytics-card-compound-label">saved</span>
              </div>
            </div>
          </div>
        </section>

        {/* ── Section 2: All Time + Records ───────────────── */}
        <section className="analytics-section">
          <div className="analytics-section-title">RECORDS</div>
          <div className="analytics-cards-4">
            <div className="analytics-card-sm">
              <span className="analytics-card-sm-value">{total.toLocaleString()}</span>
              <span className="analytics-card-sm-label">all time actions</span>
            </div>
            <div className="analytics-card-sm">
              <span className="analytics-card-sm-value accent">{formatTimeLong(stats.total_time_saved_seconds || 0)}</span>
              <span className="analytics-card-sm-label">all time saved</span>
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
        </section>

        {/* ── Section 3: Breakdown ─────────────────────────── */}
        <section className="analytics-section">
          <div className="analytics-section-title">BREAKDOWN</div>
          {total === 0 ? (
            <div className="analytics-empty">
              <div className="analytics-empty-title">No data yet</div>
              Fire a hotkey, expansion, or macro to start tracking.
            </div>
          ) : (
            <div className="analytics-breakdown">
              <div className="analytics-breakdown-row">
                <span className="analytics-breakdown-dot expansion" />
                <span className="analytics-breakdown-label">Expansions</span>
                <span className="analytics-breakdown-count">{expansions.toLocaleString()}</span>
                <div className="analytics-breakdown-bar-wrap">
                  <div className="analytics-breakdown-bar expansion" style={{ width: `${pctExp}%` }} />
                </div>
                <span className="analytics-breakdown-pct">{pctExp}%</span>
              </div>
              <div className="analytics-breakdown-row">
                <span className="analytics-breakdown-dot hotkey" />
                <span className="analytics-breakdown-label">Hotkeys</span>
                <span className="analytics-breakdown-count">{hotkeys.toLocaleString()}</span>
                <div className="analytics-breakdown-bar-wrap">
                  <div className="analytics-breakdown-bar hotkey" style={{ width: `${pctHot}%` }} />
                </div>
                <span className="analytics-breakdown-pct">{pctHot}%</span>
              </div>
              <div className="analytics-breakdown-row">
                <span className="analytics-breakdown-dot macro" />
                <span className="analytics-breakdown-label">Macros</span>
                <span className="analytics-breakdown-count">{macros.toLocaleString()}</span>
                <div className="analytics-breakdown-bar-wrap">
                  <div className="analytics-breakdown-bar macro" style={{ width: `${pctMac}%` }} />
                </div>
                <span className="analytics-breakdown-pct">{pctMac}%</span>
              </div>
            </div>
          )}
        </section>

        {/* ── PRO SECTIONS ─────────────────────────────────── */}

        {!isPro ? (
          <section className="analytics-section analytics-pro-gate">
            <div className="analytics-pro-gate-content">
              <span className="pro-badge">PRO</span>
              <div className="analytics-pro-gate-title">Detailed Analytics</div>
              <div className="analytics-pro-gate-desc">
                Activity chart, per-assignment breakdown, productivity heatmap, streaks, and CSV export.
              </div>
            </div>
          </section>
        ) : (
          <>
            {/* ── Streaks ────────────────────────────────────── */}
            <section className="analytics-section">
              <div className="analytics-section-title">STREAKS</div>
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

            {/* ── 14-Day Activity Chart ──────────────────────── */}
            <section className="analytics-section">
              <div className="analytics-section-title">LAST 14 DAYS</div>
              <div className="analytics-chart">
                {chartDays.map((day, i) => (
                  <div key={day.date} className="analytics-chart-bar-col" title={`${day.date}\n${day.actions} actions\n${formatTimeShort(day.time_saved)} saved`}>
                    <div className="analytics-chart-bar-wrap">
                      <div
                        className="analytics-chart-bar"
                        style={{ height: `${Math.max(2, (day.actions / chartMax) * 100)}%` }}
                      />
                    </div>
                    <span className="analytics-chart-label">
                      {new Date(day.date + 'T00:00').toLocaleDateString(undefined, { weekday: 'narrow' })}
                    </span>
                  </div>
                ))}
              </div>
            </section>

            {/* ── Hourly Heatmap ──────────────────────────────── */}
            <section className="analytics-section">
              <div className="analytics-section-title">ACTIVITY HEATMAP (7 DAYS)</div>
              <div className="analytics-heatmap">
                <div className="analytics-heatmap-corner" />
                {Array.from({ length: 24 }, (_, h) => (
                  <div key={h} className="analytics-heatmap-hour-label">{h}</div>
                ))}
                {DOW_LABELS.map((label, dow) => (
                  <React.Fragment key={dow}>
                    <div className="analytics-heatmap-dow-label">{label}</div>
                    {Array.from({ length: 24 }, (_, h) => {
                      const count = heatmapGrid.grid[dow][h];
                      const intensity = count > 0 ? Math.max(0.15, count / heatmapGrid.max) : 0;
                      return (
                        <div
                          key={h}
                          className="analytics-heatmap-cell"
                          style={{ opacity: intensity > 0 ? 1 : 0.3, background: intensity > 0 ? `rgba(232, 160, 32, ${intensity})` : 'var(--bg-elevated)' }}
                          title={`${label} ${h}:00 — ${count} action${count !== 1 ? 's' : ''}`}
                        />
                      );
                    })}
                  </React.Fragment>
                ))}
              </div>
            </section>

            {/* ── Per-Assignment Breakdown ────────────────────── */}
            {sortedBreakdown.length > 0 && (
              <section className="analytics-section">
                <div className="analytics-section-title">
                  TOP ASSIGNMENTS
                  <div className="analytics-sort-pills">
                    {[{ id: 'count', label: 'Most used' }, { id: 'time', label: 'Most saved' }, { id: 'label', label: 'A–Z' }].map(s => (
                      <button
                        key={s.id}
                        type="button"
                        className={`analytics-sort-pill ${breakdownSort === s.id ? 'active' : ''}`}
                        onClick={() => setBreakdownSort(s.id)}
                      >{s.label}</button>
                    ))}
                  </div>
                </div>
                <div className="analytics-assignment-list">
                  {sortedBreakdown.map((item, i) => (
                    <div key={item.trigger || i} className="analytics-assignment-row">
                      <span className="analytics-assignment-rank">{i + 1}</span>
                      <div className="analytics-assignment-info">
                        <span className="analytics-assignment-label">{item.label || item.trigger || '(unnamed)'}</span>
                        <span className="analytics-assignment-trigger">{item.trigger}</span>
                      </div>
                      <span className={`analytics-assignment-type ${item.type}`}>{item.type}</span>
                      <span className="analytics-assignment-count">{item.count}x</span>
                      <span className="analytics-assignment-saved">{formatTimeShort(item.time_saved)}</span>
                    </div>
                  ))}
                </div>
              </section>
            )}
          </>
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
    </div>
  );
}
