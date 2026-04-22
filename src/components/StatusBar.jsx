import React from 'react';
import './StatusBar.css';
import { friendlyKeyName } from './keyboardLayout';

export default function StatusBar({ selectedKey, currentCombo, macrosEnabled, assignmentCount, notification, engineStatus, lastFired, appVersion, globalPauseToggleKey }) {
  const { uiohookAvailable, nutjsAvailable, isDemoMode } = engineStatus || {};

  function pauseHotkeyLabel(combo) {
    if (!combo) return null;
    return combo.split('+').map(p => friendlyKeyName(p)).join('+');
  }

  return (
    <div className="statusbar">
      <div className="statusbar-left">
        <span className={`status-indicator ${macrosEnabled ? 'active' : 'inactive'}`}>
          <span className="status-dot" />
          {macrosEnabled
            ? 'Macros Active'
            : globalPauseToggleKey
              ? `Paused — press ${pauseHotkeyLabel(globalPauseToggleKey)} to resume`
              : 'Macros Paused'}
        </span>
        <span className="status-sep">·</span>
        <span className="status-info">{assignmentCount} assigned</span>

        {currentCombo && (
          <>
            <span className="status-sep">·</span>
            <span className="status-info">Layer: <strong>{currentCombo}</strong></span>
          </>
        )}

        {selectedKey && (
          <>
            <span className="status-sep">·</span>
            <span className="status-info">
              Editing: <strong>{currentCombo ? `${currentCombo}+` : ''}{selectedKey}</strong>
            </span>
          </>
        )}

        {lastFired && (
          <>
            <span className="status-sep">·</span>
            <span className="status-fired">▶ {lastFired.label}</span>
          </>
        )}
      </div>

      <div className="statusbar-right">
        <span
          className={`engine-chip ${uiohookAvailable ? 'ok' : 'warn'}`}
          title={uiohookAvailable ? 'Hotkey listener active' : 'Run: npm install uiohook-napi'}
        >
          {uiohookAvailable ? '⬤' : '○'} Listener
        </span>
        <span
          className={`engine-chip ${nutjsAvailable ? 'ok' : 'warn'}`}
          title={nutjsAvailable ? 'Macro executor active' : 'Run: npm install @nut-tree-fork/nut-js'}
        >
          {nutjsAvailable ? '⬤' : '○'} Executor
        </span>
        <span className="status-sep">·</span>
        <span className="status-info">Trigr {appVersion ? `v${appVersion}` : 'v…'}</span>
        {isDemoMode && (
          <>
            <span className="status-sep">·</span>
            <span className="demo-mode-badge">DEMO MODE</span>
          </>
        )}
      </div>

      {notification && (
        <div className={`notification ${notification.type}`}>
          {notification.msg}
        </div>
      )}
    </div>
  );
}
