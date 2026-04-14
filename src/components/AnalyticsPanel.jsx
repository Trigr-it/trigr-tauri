import React, { useState, useEffect, useCallback } from 'react';
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
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return `${m}m ${s}s`;
}

export default function AnalyticsPanel() {
  const [stats, setStats] = useState(null);

  const fetchStats = useCallback(async () => {
    const data = await window.electronAPI?.getAnalytics();
    if (data) setStats(data);
  }, []);

  useEffect(() => {
    fetchStats();
    const interval = setInterval(fetchStats, 30000);
    return () => clearInterval(interval);
  }, [fetchStats]);

  const [confirmReset, setConfirmReset] = useState(false);
  const confirmTimer = React.useRef(null);

  function handleResetClick() {
    if (confirmReset) {
      clearTimeout(confirmTimer.current);
      setConfirmReset(false);
      window.electronAPI?.resetAnalytics().then(() => fetchStats());
    } else {
      setConfirmReset(true);
      confirmTimer.current = setTimeout(() => setConfirmReset(false), 3000);
    }
  }

  useEffect(() => {
    return () => clearTimeout(confirmTimer.current);
  }, []);

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

        {/* ── Section 4: Reset ─────────────────────────────── */}
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
