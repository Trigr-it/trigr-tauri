import React, { useRef, useImperativeHandle, forwardRef } from 'react';
import './SearchBar.css';

// Shared search/filter input — same visual language as the Ctrl+Space quick search.
// Use anywhere a filter or search field appears so the user can spot it quickly.
export const SearchBar = forwardRef(function SearchBar(
  {
    value,
    onChange,
    placeholder = 'Search…',
    autoFocus = false,
    spellCheck = false,
    onKeyDown,
    onBlur,
    onFocus,
    className = '',
    inputClassName = '',
    icon = '⌕',
    // Set false to suppress the clear-X button (rare — e.g. inline comboboxes
    // where the caller manages clearing itself). Defaults on so every search
    // field gets the affordance automatically.
    clearable = true,
  },
  ref
) {
  const inputRef = useRef(null);
  useImperativeHandle(ref, () => inputRef.current, []);
  const hasValue = value != null && value !== '';
  const handleClear = () => {
    // Fire a synthetic onChange with empty value so every caller's existing
    // `onChange={e => setSearch(e.target.value)}` handler clears cleanly
    // without needing an extra onClear prop. Refocus so the user can keep
    // typing a new query immediately.
    if (onChange) onChange({ target: { value: '' } });
    inputRef.current?.focus();
  };
  return (
    <div className={`app-search-bar ${className}`.trim()}>
      <span className="app-search-bar-icon" aria-hidden="true">{icon}</span>
      <input
        ref={inputRef}
        className={`app-search-bar-input ${inputClassName}`.trim()}
        type="text"
        value={value}
        onChange={onChange}
        placeholder={placeholder}
        autoFocus={autoFocus}
        spellCheck={spellCheck}
        onKeyDown={onKeyDown}
        onBlur={onBlur}
        onFocus={onFocus}
      />
      {clearable && hasValue && (
        <button
          type="button"
          className="app-search-bar-clear"
          onClick={handleClear}
          onMouseDown={e => e.preventDefault()}
          title="Clear search"
          aria-label="Clear search"
          tabIndex={-1}
        >×</button>
      )}
    </div>
  );
});

export default SearchBar;
