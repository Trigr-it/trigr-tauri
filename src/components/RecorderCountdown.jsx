// Macro-recorder bar. Single pill, fixed bottom-centre. Recording starts
// the instant the user clicks Record — no countdown. Label reads
// "Recording 0:00" ticking up; Stop button or Ctrl+Shift+R ends it.

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './RecorderCountdown.css';

export default function RecorderCountdown() {
  // 'idle' = render nothing (window hidden). 'recording' = live bar.
  // Initial state MUST be 'idle' so the pre-created hidden window doesn't
  // render anything on app launch.
  const [phase, setPhase] = useState('idle');
  const [elapsedMs, setElapsedMs] = useState(0);
  // Configured record hotkey — refreshed on every show so the hint reflects
  // the current user choice (default Ctrl+Alt+R; Settings → Quick Record can
  // change it). Same hotkey stops the recording in BOTH the editor and global
  // flows, so showing it here is accurate regardless of how recording started.
  const [recordHotkey, setRecordHotkey] = useState('Ctrl+Alt+R');

  // Visibility-driven phase. Chromium fires visibilitychange synchronously
  // the moment Rust calls win.show(), so the bar appears in lockstep with
  // recorder::start() (which runs immediately in the same Rust command).
  useEffect(() => {
    function onVisChange() {
      if (document.visibilityState === 'visible') {
        setElapsedMs(0);
        setPhase('recording');
        // Refresh the record hotkey string in case the user changed it
        // between the last recording and this one.
        invoke('get_temp_macro_status').then(s => {
          if (s?.recordHotkey) setRecordHotkey(s.recordHotkey);
        }).catch(() => {});
      } else {
        setElapsedMs(0);
        setPhase('idle');
      }
    }
    document.addEventListener('visibilitychange', onVisChange);
    if (document.visibilityState === 'visible') {
      setElapsedMs(0);
      setPhase('recording');
      invoke('get_temp_macro_status').then(s => {
        if (s?.recordHotkey) setRecordHotkey(s.recordHotkey);
      }).catch(() => {});
    }
    return () => document.removeEventListener('visibilitychange', onVisChange);
  }, []);

  // Live elapsed-time poll — only when recording. Sourced directly from
  // Rust's status_snapshot so it matches what's saved in the macro buffer.
  useEffect(() => {
    if (phase !== 'recording') return undefined;
    let cancelled = false;
    const tick = async () => {
      if (cancelled) return;
      try {
        const status = await invoke('get_recording_status');
        if (cancelled || !status?.recording) return;
        setElapsedMs(status.durationMs || 0);
      } catch (_) { /* ignore */ }
    };
    const interval = setInterval(tick, 250);
    tick();
    return () => { cancelled = true; clearInterval(interval); };
  }, [phase]);

  // Esc aborts an in-flight recording — same path as the Stop button via
  // the abort command (which hides the window + emits cancelled).
  useEffect(() => {
    const onKey = (e) => {
      if (e.key !== 'Escape') return;
      invoke('recorder_countdown_abort').catch(() => {});
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  function handleStop() {
    invoke('recorder_stop_from_pill').catch(() => {});
  }

  const fmtDuration = (ms) => {
    const total = Math.floor(Math.max(0, ms) / 1000);
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m}:${String(s).padStart(2, '0')}`;
  };

  if (phase === 'idle') {
    return null;
  }

  return (
    <div className="rc-root rc-root--bar">
      <div className="rc-pill">
        <span className="rc-pill-dot" />
        <span className="rc-pill-label">Recording {fmtDuration(elapsedMs)}</span>
        <button type="button" className="rc-pill-stop" onClick={handleStop}>
          Stop
        </button>
        <span className="rc-pill-hint">
          {recordHotkey.split('+').map((p, i, arr) => (
            <React.Fragment key={i}>
              <kbd>{p}</kbd>
              {i < arr.length - 1 && '+'}
            </React.Fragment>
          ))}
        </span>
      </div>
    </div>
  );
}
