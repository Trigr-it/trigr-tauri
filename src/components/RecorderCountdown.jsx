// Macro-recorder overlay. Two phases:
//   'countdown' → big 3-2-1 numeral in a centred box (400×400). Gives the
//   user time to alt-tab to their target app before the fg watcher starts
//   capturing, so target_app auto-detection binds to the RIGHT window.
//   'recording' → compact pill at bottom-centre, live duration + Stop.
//
// ARCHITECTURE (hard-won 2026-08-10 — do not re-plumb): the countdown TIMING
// is owned by a Rust thread in show_recorder_bar. Rust morphs the window and
// calls recorder::start() exactly 3s after show, regardless of anything this
// component does. This component derives EVERYTHING from one permanent
// get_recording_status poll:
//   recording=true            → 'recording' (durationMs drives the timer)
//   countdownRemainingMs > 0  → 'countdown' (numeral = ceil(remaining/1s))
//   neither                   → 'idle' (renders nothing)
// No local timers, no visibility listeners. Rust's clock is the single source
// of truth. Three earlier designs all failed on Chromium's unreliable
// visibility events: a mount-time visibilityState check ghost-started
// recordings at app launch; event-only listening missed the first lazy-build
// show; a Rust pending-flag raced the synchronous visibilitychange inside
// win.show(). Deriving display from polled Rust state has none of these
// failure modes — worst case the numeral appears one poll tick (150ms) late.

import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './RecorderCountdown.css';

export default function RecorderCountdown() {
  const [phase, setPhase] = useState('idle');
  const [countdownN, setCountdownN] = useState(3);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [recordHotkey, setRecordHotkey] = useState('Ctrl+Alt+R');
  const prevPhaseRef = useRef('idle');

  // Single permanent poll — 150ms while visible; Chromium throttles hidden
  // windows to ~1s, which is fine (a hidden window renders nothing anyway).
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      if (cancelled) return;
      try {
        const status = await invoke('get_recording_status');
        if (cancelled) return;
        if (status?.recording) {
          setPhase('recording');
          setElapsedMs(status.durationMs || 0);
        } else if ((status?.countdownRemainingMs || 0) > 0) {
          setPhase('countdown');
          setCountdownN(Math.min(3, Math.max(1, Math.ceil(status.countdownRemainingMs / 1000))));
        } else {
          setPhase('idle');
          setElapsedMs(0);
        }
      } catch (_) { /* ignore transient errors */ }
    };
    const interval = setInterval(tick, 150);
    tick();
    return () => { cancelled = true; clearInterval(interval); };
  }, []);

  // Refresh the configured record hotkey whenever a flow begins (idle → any),
  // so the pill hint reflects the user's current Settings choice.
  useEffect(() => {
    const prev = prevPhaseRef.current;
    prevPhaseRef.current = phase;
    if (prev === 'idle' && phase !== 'idle') {
      invoke('get_temp_macro_status').then(s => {
        if (s?.recordHotkey) setRecordHotkey(s.recordHotkey);
      }).catch(() => {});
    }
  }, [phase]);

  // Esc aborts the countdown / an in-flight recording — the abort command
  // cancels the Rust countdown thread, discards any partial buffer, hides
  // the window and emits cancelled so the editor flow can unwind.
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

  if (phase === 'countdown') {
    return (
      <div className="rc-root rc-root--countdown">
        <div className="rc-countdown-box">
          <div className="rc-countdown-title">Recording starts in</div>
          <div className="rc-countdown-number" key={countdownN} aria-live="polite">
            {countdownN}
          </div>
          <div className="rc-countdown-hint">
            Switch to the app you want to record now
          </div>
          <div className="rc-countdown-esc">
            <kbd>Esc</kbd> to cancel
          </div>
        </div>
      </div>
    );
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
