import React, { useState, useEffect } from 'react';
import './StatusBar.css';
import { friendlyKeyName } from './keyboardLayout';

export default function StatusBar({ selectedKey, currentCombo, macrosEnabled, assignmentCount, engineStatus, lastFired, appVersion, globalPauseToggleKey }) {
  const { uiohookAvailable, nutjsAvailable, isDemoMode } = engineStatus || {};

  // Silent OCR backfill progress. `null` = idle, otherwise {processed, total}.
  // Self-contained inside StatusBar so App.jsx doesn't need to route these
  // events through props. Auto-clears on the done event.
  const [ocrBackfill, setOcrBackfill] = useState(null);
  const [thumbBackfill, setThumbBackfill] = useState(null);
  useEffect(() => {
    window.electronAPI?.onClipboardOcrBackfillProgress?.(({ processed, total }) => {
      setOcrBackfill({ processed, total });
    });
    window.electronAPI?.onClipboardOcrBackfillDone?.(() => {
      // Hold on "done" for a beat so the user sees completion.
      setOcrBackfill({ processed: -1, total: -1 });
      setTimeout(() => setOcrBackfill(null), 2500);
    });
    // v0.8.4 thumbnail backfill — same silent-progress pattern.
    window.electronAPI?.onClipboardThumbBackfillProgress?.(({ processed, total }) => {
      // Zero-total "done immediately" case: skip the row entirely.
      if (total <= 0) return;
      setThumbBackfill({ processed, total });
    });
    window.electronAPI?.onClipboardThumbBackfillDone?.(({ total }) => {
      if (total > 0) {
        setThumbBackfill({ processed: -1, total: -1 });
        setTimeout(() => setThumbBackfill(null), 2500);
      } else {
        setThumbBackfill(null);
      }
    });
    return () => {
      window.electronAPI?.removeAllListeners?.('clipboard-ocr-backfill-progress');
      window.electronAPI?.removeAllListeners?.('clipboard-ocr-backfill-done');
      window.electronAPI?.removeAllListeners?.('clipboard-thumb-backfill-progress');
      window.electronAPI?.removeAllListeners?.('clipboard-thumb-backfill-done');
    };
  }, []);

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

        {ocrBackfill && (
          <>
            <span className="status-sep">·</span>
            <span className="status-info" style={{ opacity: 0.85 }}>
              {ocrBackfill.total < 0
                ? '✓ Image text extraction complete'
                : `Extracting text from images… ${ocrBackfill.processed}/${ocrBackfill.total}`}
            </span>
          </>
        )}

        {thumbBackfill && (
          <>
            <span className="status-sep">·</span>
            <span className="status-info" style={{ opacity: 0.85 }}>
              {thumbBackfill.total < 0
                ? '✓ Clipboard thumbnails ready'
                : `Building clipboard thumbnails… ${thumbBackfill.processed}/${thumbBackfill.total}`}
            </span>
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
        <span className="status-info">Keyfire {appVersion ? `v${appVersion}` : 'v…'}</span>
        {isDemoMode && (
          <>
            <span className="status-sep">·</span>
            <span className="demo-mode-badge">DEMO MODE</span>
          </>
        )}
      </div>

    </div>
  );
}
