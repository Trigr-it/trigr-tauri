import { useEffect, useState } from 'react';

// Number input that lets the user fully clear the field and type from scratch.
// The committed value (via `onCommit`) is always clamped to [min, max], but
// the visible text can be anything (including empty) while the field has
// focus. Clamps + commits on blur or Enter. `defaultOnEmpty` (defaults to
// `min`) is the value we snap to when the user leaves the field empty.
//
// Use everywhere numeric inputs live — the naive inline
// `parseInt(e.target.value) || 0` pattern forces the field back to 0/min on
// every keystroke of an empty state, so users can't type from scratch.
//
// Pass `float: true` for decimal-accepting fields (hourly rate, image scale
// if fractional, etc.); default is integer parsing via parseInt.
export default function NumberField({
  value, min, max, defaultOnEmpty, onCommit,
  className, style, title, placeholder, inputRef, disabled, step, float = false,
}) {
  // Local text state so the field can display an empty / in-progress value
  // even when the committed value (in the JSON) is still the last valid one.
  const [text, setText] = useState(value == null || value === '' ? '' : String(value));

  // Sync when the value changes externally (mode switch, reset, etc.).
  useEffect(() => {
    setText(value == null || value === '' ? '' : String(value));
  }, [value]);

  const commit = () => {
    const parsed = float ? parseFloat(text) : parseInt(text, 10);
    const fallback = defaultOnEmpty != null ? defaultOnEmpty : min;
    const clamped = Number.isNaN(parsed)
      ? fallback
      : Math.max(min, Math.min(max, parsed));
    setText(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <input
      ref={inputRef}
      className={className || 'form-input'}
      type="number"
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      value={text}
      onChange={e => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={e => {
        e.stopPropagation();
        if (e.key === 'Enter') e.currentTarget.blur();
      }}
      style={style}
      title={title}
      placeholder={placeholder}
    />
  );
}
