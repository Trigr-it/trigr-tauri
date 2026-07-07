// Reserved Windows shortcuts that commonly cause real harm if shadowed by a
// Keyfire mapping. Combo strings use the same sorted-modifier format as the
// comboString() helper in KeyboardCanvas.jsx (Ctrl, Shift, Alt, Win order).
// Only single-press mappings are checked against this list — double-tap
// variants of reserved combos are safe because the first press still passes
// through to the OS.

export const RESERVED_SHORTCUTS = [
  // ── Text editing — universal ──────────────────────────────────────
  { combo: 'Ctrl', keyId: 'KeyC', osFunction: 'Copy' },
  { combo: 'Ctrl', keyId: 'KeyV', osFunction: 'Paste' },
  { combo: 'Ctrl', keyId: 'KeyX', osFunction: 'Cut' },
  { combo: 'Ctrl', keyId: 'KeyZ', osFunction: 'Undo' },
  { combo: 'Ctrl', keyId: 'KeyY', osFunction: 'Redo' },
  { combo: 'Ctrl', keyId: 'KeyA', osFunction: 'Select All' },
  { combo: 'Ctrl', keyId: 'KeyS', osFunction: 'Save' },

  // ── App navigation — near-universal ───────────────────────────────
  { combo: 'Ctrl', keyId: 'KeyF', osFunction: 'Find' },
  { combo: 'Ctrl', keyId: 'KeyP', osFunction: 'Print' },
  { combo: 'Ctrl', keyId: 'KeyN', osFunction: 'New' },
  { combo: 'Ctrl', keyId: 'KeyO', osFunction: 'Open' },
  { combo: 'Ctrl', keyId: 'KeyT', osFunction: 'New Tab' },
  { combo: 'Ctrl', keyId: 'KeyW', osFunction: 'Close Tab / Window' },
  { combo: 'Ctrl', keyId: 'Tab', osFunction: 'Cycle Tabs' },
  { combo: 'Ctrl+Shift', keyId: 'KeyT', osFunction: 'Reopen Closed Tab' },

  // ── OS-level ──────────────────────────────────────────────────────
  { combo: 'Win', keyId: 'KeyL', osFunction: 'Lock' },
  { combo: 'Win', keyId: 'KeyD', osFunction: 'Show Desktop' },
  { combo: 'Win', keyId: 'KeyE', osFunction: 'File Explorer' },
  { combo: 'Win', keyId: 'KeyR', osFunction: 'Run' },
  { combo: 'Win', keyId: 'KeyV', osFunction: 'Clipboard History' },
  { combo: 'Win', keyId: 'KeyI', osFunction: 'Settings' },
  { combo: 'Win', keyId: 'KeyX', osFunction: 'Quick Links Menu' },
  { combo: 'Win', keyId: 'Tab', osFunction: 'Task View' },
  { combo: 'Shift+Win', keyId: 'KeyS', osFunction: 'Snipping Tool' },
  { combo: 'Alt', keyId: 'F4', osFunction: 'Close Window' },
  { combo: 'Alt', keyId: 'Tab', osFunction: 'Switch Apps' },
  { combo: 'Ctrl+Shift', keyId: 'Escape', osFunction: 'Task Manager' },
];

// Friendly key labels for the modal copy. Falls back to keyId when not listed.
const KEY_DISPLAY_NAMES = {
  Tab: 'Tab',
  Escape: 'Esc',
  F4: 'F4',
};

function keyDisplay(keyId) {
  if (KEY_DISPLAY_NAMES[keyId]) return KEY_DISPLAY_NAMES[keyId];
  if (keyId.startsWith('Key')) return keyId.slice(3); // KeyC → C
  return keyId;
}

// Returns the matching reserved-shortcut entry (or null) for a combo + keyId.
export function findReservedShortcut(combo, keyId) {
  return (
    RESERVED_SHORTCUTS.find(r => r.combo === combo && r.keyId === keyId) || null
  );
}

// Human-readable shortcut string for display in the modal: "Ctrl+C", "Win+Shift+S"
// (⌘/⌥ on macOS — display-only, storage tokens unchanged).
import { displayModifier } from '../components/keyboardLayout';
export function formatComboDisplay(combo, keyId) {
  const parts = combo ? combo.split('+').map(displayModifier) : [];
  parts.push(keyDisplay(keyId));
  return parts.join('+');
}
