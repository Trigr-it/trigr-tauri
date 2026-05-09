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
  },
  ref
) {
  const inputRef = useRef(null);
  useImperativeHandle(ref, () => inputRef.current, []);
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
    </div>
  );
});

export default SearchBar;
