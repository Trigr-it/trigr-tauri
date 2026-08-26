import React, { useMemo } from 'react';
import { Disc, MousePointer2, Type, AppWindow, Layers, Search, Repeat2 } from 'lucide-react';
import './QuickTips.css';

const TIPS = [
  { Icon: Disc,          text: 'Press Record to capture any hotkey combo instantly' },
  { Icon: Repeat2,       text: 'Double-press (Pro) — assign a second action to any hotkey' },
  { Icon: MousePointer2, text: 'Switch to Mouse view to assign macros to mouse buttons' },
  { Icon: Type,          text: 'Text Expansions — type a trigger word + Space to expand' },
  { Icon: AppWindow,     text: 'App Profiles — create per-app hotkeys that auto-switch' },
  { Icon: Layers,        text: 'Macro Sequences — chain multiple actions into one hotkey' },
  { Icon: Search,        text: 'Quick Search — press {{QS}} to find any action instantly', needsQuickSearch: true },
];

const COUNT = 3;

export default function QuickTips({ onDismiss, searchOverlayHotkey = 'Ctrl+Space', searchOverlayEnabled = true }) {
  const shown = useMemo(() => {
    // Live hotkey, and drop the Quick Search tip entirely when it's turned off.
    const pool = TIPS
      .filter(t => !(t.needsQuickSearch && (!searchOverlayEnabled || !searchOverlayHotkey)))
      .map(t => ({ ...t, text: t.text.replace('{{QS}}', searchOverlayHotkey) }));
    const shuffled = [...pool].sort(() => Math.random() - 0.5);
    return shuffled.slice(0, COUNT);
  }, [searchOverlayHotkey, searchOverlayEnabled]);

  return (
    <div className="quick-tips">
      {/* Gold tip box — same visual language as the panel TIP boxes
          (radial / templates / expansions / clipboard). */}
      <div className="qt-box">
        <span className="qt-badge">TIPS</span>
        <ul className="qt-list">
          {shown.map((tip, i) => {
            const TipIcon = tip.Icon;
            return (
              <li key={i} className="qt-row">
                <span className="qt-icon" aria-hidden="true">
                  <TipIcon size={14} strokeWidth={1.75} />
                </span>
                <span className="qt-text">{tip.text}</span>
              </li>
            );
          })}
        </ul>
        <button
          className="qt-dismiss"
          onClick={onDismiss}
          type="button"
          title="Hide these tips (restore in Settings)"
          aria-label="Hide these tips"
        >&#10005;</button>
      </div>
    </div>
  );
}
