import React, { useMemo } from 'react';
import './QuickTips.css';

const TIPS = [
  { icon: '⏺', text: 'Press Record to capture any hotkey combo instantly' },
  { icon: '×2', text: 'Double-press support — assign a second action to any hotkey' },
  { icon: '🖱',  text: 'Switch to Mouse view to assign macros to mouse buttons' },
  { icon: '✦',  text: 'Text Expansions — type a trigger word + Space to expand' },
  { icon: '⬡',  text: 'App Profiles — create per-app hotkeys that auto-switch' },
  { icon: '◈',  text: 'Macro Sequences — chain multiple actions into one hotkey' },
  { icon: '🔍', text: 'Quick Search — press Ctrl+Space to find any macro instantly' },
];

const COUNT = 3;

export default function QuickTips({ onDismiss }) {
  const shown = useMemo(() => {
    const shuffled = [...TIPS].sort(() => Math.random() - 0.5);
    return shuffled.slice(0, COUNT);
  }, []);

  return (
    <div className="quick-tips">
      <ul className="qt-list">
        {shown.map((tip, i) => (
          <li key={i} className="qt-row">
            <span className="qt-icon">{tip.icon}</span>
            <span className="qt-text">{tip.text}</span>
          </li>
        ))}
      </ul>
      <button
        className="qt-dismiss"
        onClick={onDismiss}
        type="button"
      >
        hide tips
      </button>
    </div>
  );
}
