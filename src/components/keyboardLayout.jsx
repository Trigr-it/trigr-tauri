// Full keyboard layout definition
// Each row is an array of key objects
// width is in units (1 = standard key width ~42px)

export const KEYBOARD_ROWS = [
  // Row 0: Function keys
  [
    { id: 'Escape', label: 'Esc', width: 1 },
    { id: 'SPACER_F1', spacer: true, width: 0.5 },
    { id: 'F1', label: 'F1', width: 1 },
    { id: 'F2', label: 'F2', width: 1 },
    { id: 'F3', label: 'F3', width: 1 },
    { id: 'F4', label: 'F4', width: 1 },
    { id: 'SPACER_F5', spacer: true, width: 0.35 },
    { id: 'F5', label: 'F5', width: 1 },
    { id: 'F6', label: 'F6', width: 1 },
    { id: 'F7', label: 'F7', width: 1 },
    { id: 'F8', label: 'F8', width: 1 },
    { id: 'SPACER_F9', spacer: true, width: 0.35 },
    { id: 'F9', label: 'F9', width: 1 },
    { id: 'F10', label: 'F10', width: 1 },
    { id: 'F11', label: 'F11', width: 1 },
    { id: 'F12', label: 'F12', width: 1 },
  ],

  // Row 1: Numbers
  [
    { id: 'Backquote', label: '`', sublabel: '~', width: 1 },
    { id: 'Digit1', label: '1', sublabel: '!', width: 1 },
    { id: 'Digit2', label: '2', sublabel: '@', width: 1 },
    { id: 'Digit3', label: '3', sublabel: '#', width: 1 },
    { id: 'Digit4', label: '4', sublabel: '$', width: 1 },
    { id: 'Digit5', label: '5', sublabel: '%', width: 1 },
    { id: 'Digit6', label: '6', sublabel: '^', width: 1 },
    { id: 'Digit7', label: '7', sublabel: '&', width: 1 },
    { id: 'Digit8', label: '8', sublabel: '*', width: 1 },
    { id: 'Digit9', label: '9', sublabel: '(', width: 1 },
    { id: 'Digit0', label: '0', sublabel: ')', width: 1 },
    { id: 'Minus', label: '-', sublabel: '_', width: 1 },
    { id: 'Equal', label: '=', sublabel: '+', width: 1 },
    { id: 'Backspace', label: '⌫', width: 2 },
  ],

  // Row 2: QWERTY
  [
    { id: 'Tab', label: 'Tab', width: 1.5 },
    { id: 'KeyQ', label: 'Q', width: 1 },
    { id: 'KeyW', label: 'W', width: 1 },
    { id: 'KeyE', label: 'E', width: 1 },
    { id: 'KeyR', label: 'R', width: 1 },
    { id: 'KeyT', label: 'T', width: 1 },
    { id: 'KeyY', label: 'Y', width: 1 },
    { id: 'KeyU', label: 'U', width: 1 },
    { id: 'KeyI', label: 'I', width: 1 },
    { id: 'KeyO', label: 'O', width: 1 },
    { id: 'KeyP', label: 'P', width: 1 },
    { id: 'BracketLeft', label: '[', sublabel: '{', width: 1 },
    { id: 'BracketRight', label: ']', sublabel: '}', width: 1 },
    { id: 'Backslash', label: '\\', sublabel: '|', width: 1.5 },
  ],

  // Row 3: ASDF
  [
    { id: 'CapsLock', label: 'Caps Lock', width: 1.75 },
    { id: 'KeyA', label: 'A', width: 1 },
    { id: 'KeyS', label: 'S', width: 1 },
    { id: 'KeyD', label: 'D', width: 1 },
    { id: 'KeyF', label: 'F', width: 1 },
    { id: 'KeyG', label: 'G', width: 1 },
    { id: 'KeyH', label: 'H', width: 1 },
    { id: 'KeyJ', label: 'J', width: 1 },
    { id: 'KeyK', label: 'K', width: 1 },
    { id: 'KeyL', label: 'L', width: 1 },
    { id: 'Semicolon', label: ';', sublabel: ':', width: 1 },
    { id: 'Quote', label: "'", sublabel: '"', width: 1 },
    { id: 'Enter', label: 'Enter', width: 2.25 },
  ],

  // Row 4: ZXCV
  [
    { id: 'ShiftLeft', label: 'Shift', width: 2.25 },
    { id: 'KeyZ', label: 'Z', width: 1 },
    { id: 'KeyX', label: 'X', width: 1 },
    { id: 'KeyC', label: 'C', width: 1 },
    { id: 'KeyV', label: 'V', width: 1 },
    { id: 'KeyB', label: 'B', width: 1 },
    { id: 'KeyN', label: 'N', width: 1 },
    { id: 'KeyM', label: 'M', width: 1 },
    { id: 'Comma', label: ',', sublabel: '<', width: 1 },
    { id: 'Period', label: '.', sublabel: '>', width: 1 },
    { id: 'Slash', label: '/', sublabel: '?', width: 1 },
    { id: 'ShiftRight', label: 'Shift', width: 2.75 },
  ],

  // Row 5: Bottom row
  [
    { id: 'ControlLeft', label: 'Ctrl', width: 1.25 },
    { id: 'MetaLeft', label: '⊞', width: 1.25 },
    { id: 'AltLeft', label: 'Alt', width: 1.25 },
    { id: 'Space', label: '', width: 6.25 },
    { id: 'AltRight', label: 'Alt', width: 1.25 },
    { id: 'MetaRight', label: '⊞', width: 1.25 },
    { id: 'ContextMenu', label: '☰', width: 1.25 },
    { id: 'ControlRight', label: 'Ctrl', width: 1.25 },
  ],
];

// Keys that can't be reassigned (modifiers used in combos)
export const SYSTEM_KEYS = new Set([
  'ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight',
  'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight', 'CapsLock'
]);

export const KEY_UNIT   = 42; // px per keyboard unit
export const KEY_GAP    = 4;  // px gap between keys (also used as row gap)
export const KEY_HEIGHT = 42; // px height of each key

// ── Natural (unscaled) outer dimensions of .keyboard-outer ───────────────────
// Used by KeyboardCanvas to derive the CSS scale factor.
function _rowPixelWidth(row) {
  let w = 0;
  for (let i = 0; i < row.length; i++) {
    if (i > 0) w += KEY_GAP; // flex row-gap between items
    const k = row[i];
    // Spacers use no gap correction; real keys span (width-1) extra gaps
    w += k.spacer
      ? k.width * KEY_UNIT
      : k.width * KEY_UNIT + (k.width - 1) * KEY_GAP;
  }
  return w;
}
const _maxBodyWidth = Math.max(...KEYBOARD_ROWS.map(_rowPixelWidth));
// .keyboard-outer padding: 16px top, 18px right, 18px bottom, 18px left + 1px border each side
export const KEYBOARD_NATURAL_WIDTH  = _maxBodyWidth + 18 * 2 + 2;
export const KEYBOARD_NATURAL_HEIGHT =
  KEYBOARD_ROWS.length * KEY_HEIGHT +
  (KEYBOARD_ROWS.length - 1) * KEY_GAP +
  16 + 18 + 2; // top-padding + bottom-padding + top-border + bottom-border
