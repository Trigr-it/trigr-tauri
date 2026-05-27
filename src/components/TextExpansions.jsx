import React, { useState, useRef, useLayoutEffect, useEffect, useCallback, useMemo } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import {
  Bold as BoldIcon, Italic as ItalicIcon, Underline as UnderlineIcon,
  List as ListIcon, ListOrdered as ListOrderedIcon,
  Palette as PaletteIcon, Heading as HeadingIcon,
  Calendar as CalendarIcon, Clock as ClockIcon, CalendarClock as CalendarClockIcon,
  Clipboard as ClipboardIcon, TextCursor as TextCursorIcon,
  Variable as VariableIcon, Keyboard as KeyboardIcon,
  FormInput as FillInIcon,
} from 'lucide-react';
import './TextExpansions.css';
import { SearchBar } from './SearchBar';

// ── Helpers ────────────────────────────────────────────────────────────────

// Convert plain text (e.g. clipboard contents seeded from another part of the
// app) into HTML safe for innerHTML injection into the rich-text editor.
// Escapes &/</> and preserves line breaks as <br>.
function plainTextToHtml(text) {
  const escaped = (text || '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return escaped.replace(/\n/g, '<br>');
}

function htmlToPlainText(html) {
  const tmp = document.createElement('div');
  tmp.innerHTML = html
    .replace(/<br\s*\/?>/gi, '\n')           // 1. <br> → newline
    .replace(/<\/p>/gi, '\n')                 // 2. closing </p> → newline
    .replace(/<\/div>/gi, '\n')               //    closing </div> → newline
    .replace(/<\/li>/gi, '\n')                //    closing </li> → newline
    .replace(/<div[^>]*>/gi, '')              // 3. opening <div> → nothing
    .replace(/<p[^>]*>/gi, '');               //    opening <p> → nothing
  // Replace token chips with their raw token strings before stripping markup
  tmp.querySelectorAll('[data-token]').forEach(el => {
    el.replaceWith(document.createTextNode(el.dataset.token));
  });
  return (tmp.textContent || tmp.innerText || '')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

// Walk a chunk of editor HTML and return every {fillIn:LABEL} label found.
// Used to surface reusable fields across the single editor body + variants.
function extractFillInLabels(html) {
  if (!html) return [];
  const tmp = document.createElement('div');
  tmp.innerHTML = html;
  const labels = [];
  tmp.querySelectorAll('[data-token]').forEach(el => {
    const t = el.dataset.token || '';
    const m = t.match(/^\{fillIn:(.+)\}$/);
    if (m) labels.push(m[1]);
  });
  return labels;
}

// ── Insert token category menus ────────────────────────────────────────────
// Each toolbar icon opens its own focused dropdown. Items keep the same token
// strings and chip styles as before — the engine and storage are unchanged.

const CLIPBOARD_ITEMS = [
  { type: 'item', token: '{clipboard}',           label: 'Clipboard Contents',     display: 'Clipboard' },
  { type: 'item', token: '{clipboard:uppercase}', label: 'Clipboard (UPPERCASE)', display: 'CLIP ▲'    },
  { type: 'item', token: '{clipboard:lowercase}', label: 'Clipboard (lowercase)', display: 'clip ▼'    },
  { type: 'item', token: '{clipboard:trim}',      label: 'Clipboard (trimmed)',   display: 'Clip ✂'    },
  { type: 'item', token: '{clipboard:urlencode}', label: 'Clipboard (URL encode)', display: 'Clip %'    },
];

const DATE_ITEMS = [
  { type: 'item', token: '{date}',             label: 'Date (your default)', display: 'Default'     },
  { type: 'sep' },
  { type: 'item', token: '{date:DD/MM/YYYY}',  label: 'Date (DD/MM/YYYY)',  display: 'DD/MM/YYYY'  },
  { type: 'item', token: '{date:DD/MM/YY}',    label: 'Date (DD/MM/YY)',    display: 'DD/MM/YY'    },
  { type: 'item', token: '{date:MM/DD/YYYY}',  label: 'Date (MM/DD/YYYY)',  display: 'MM/DD/YYYY'  },
  { type: 'item', token: '{date:YYYY-MM-DD}',  label: 'Date (YYYY-MM-DD)',  display: 'YYYY-MM-DD'  },
  { type: 'item', token: '{date:D MMMM YYYY}', label: 'Date (1 May 2026)',  display: 'D MMMM YYYY' },
  { type: 'sep' },
  { type: 'item', token: '{dayofweek}', label: 'Day of Week',  display: 'Day'   },
  { type: 'item', token: '{month}',     label: 'Month Name',   display: 'Month' },
  { type: 'item', token: '{year}',      label: 'Year (YYYY)',  display: 'Year'  },
  { type: 'item', token: '{day}',       label: 'Day of Month', display: 'Day#'  },
];

const TIME_ITEMS = [
  { type: 'item', token: '{time:HH:MM}',    label: 'Time (HH:MM)',       display: 'HH:MM'    },
  { type: 'item', token: '{time:HH:MM:SS}', label: 'Time (HH:MM:SS)',    display: 'HH:MM:SS' },
  { type: 'item', token: '{isodate}',       label: 'ISO 8601 Date+Time', display: 'ISO Date' },
];

const DATE_MATH_ITEMS = [
  { type: 'item', token: '{date:+1d}', label: 'Tomorrow',   display: '+1 day'    },
  { type: 'item', token: '{date:-1d}', label: 'Yesterday',  display: '-1 day'    },
  { type: 'item', token: '{date:+7d}', label: 'Next Week',  display: '+7 days'   },
  { type: 'item', token: '{date:+1m}', label: 'Next Month', display: '+1 month'  },
];

const CURSOR_FILLIN_ITEMS = [
  { type: 'item', token: '{cursor}',    label: 'Cursor Position', display: '↕ Cursor', chipClass: 'cursor' },
  { type: 'item', token: '__fillin__',  label: 'Fill-in Field…',  display: <FillInIcon size={14} strokeWidth={2} />, chipClass: 'fillin' },
];

const INSERT_CATEGORIES = {
  clipboard: { items: CLIPBOARD_ITEMS,     label: 'Clipboard',         chipClass: 'clipboard' },
  date:      { items: DATE_ITEMS,          label: 'Date',              chipClass: 'date' },
  time:      { items: TIME_ITEMS,          label: 'Time',              chipClass: 'date' },
  datemath:  { items: DATE_MATH_ITEMS,     label: 'Date Math',         chipClass: 'date' },
  cursor:    { items: CURSOR_FILLIN_ITEMS, label: 'Cursor & Fill-in',  chipClass: null    },
};

// ── Text colour swatches (for foreColor) ───────────────────────────────────
const TEXT_COLOURS = [
  { hex: '#0F172A', label: 'Default' },
  { hex: '#475569', label: 'Slate'   },
  { hex: '#94A3B8', label: 'Grey'    },
  { hex: '#E84040', label: 'Red'     },
  { hex: '#E86020', label: 'Orange'  },
  { hex: '#E8A020', label: 'Amber'   },
  { hex: '#2ECC71', label: 'Green'   },
  { hex: '#1ABC9C', label: 'Teal'    },
  { hex: '#4080E8', label: 'Blue'    },
  { hex: '#5C6AE8', label: 'Indigo'  },
  { hex: '#9B59B6', label: 'Purple'  },
  { hex: '#E840A0', label: 'Pink'    },
];

// ── Heading levels ─────────────────────────────────────────────────────────
const HEADING_OPTIONS = [
  { block: 'h1', label: 'Heading 1', display: 'H1' },
  { block: 'h2', label: 'Heading 2', display: 'H2' },
  { block: 'h3', label: 'Heading 3', display: 'H3' },
  { block: 'p',  label: 'Paragraph', display: 'P'  },
];


// ── Key token helpers ──────────────────────────────────────────────────────

function parseKeyToken(token) {
  // "{key:Tab:1}" → { combo: "Tab", repeat: 1 }
  // "{key:Ctrl+F4:2}" → { combo: "Ctrl+F4", repeat: 2 }
  // "{key:Tab}" (legacy) → { combo: "Tab", repeat: 1 }
  const inner = token.slice(5, -1); // strip "{key:" and "}"
  const lastColon = inner.lastIndexOf(':');
  if (lastColon !== -1) {
    const n = parseInt(inner.slice(lastColon + 1), 10);
    if (!isNaN(n) && n > 0) {
      return { combo: inner.slice(0, lastColon), repeat: n };
    }
  }
  return { combo: inner, repeat: 1 };
}

// ── Global variable key helpers ─────────────────────────────────────────────

function titleToKey(title) {
  return title
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9\s]/g, '')
    .replace(/\s+/g, '.')
    .replace(/\.+/g, '.')
    .replace(/^\.+|\.+$/g, '');
}

function keyToTitle(key) {
  return key.split('.').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
}

const GD_SUGGESTIONS = [
  'My Full Name', 'My First Name', 'My Email Address',
  'My Phone Number', 'My Company', 'My Job Title', 'My Website',
];

// ── Category colour preset palette ─────────────────────────────────────────
const CATEGORY_COLOURS = [
  { hex: null,      label: 'None'   },
  { hex: '#E8A020', label: 'Amber'  },
  { hex: '#E84040', label: 'Red'    },
  { hex: '#2ECC71', label: 'Green'  },
  { hex: '#4080E8', label: 'Blue'   },
  { hex: '#9B59B6', label: 'Purple' },
  { hex: '#1ABC9C', label: 'Teal'   },
  { hex: '#E86020', label: 'Orange' },
  { hex: '#E840A0', label: 'Pink'   },
  { hex: '#5C6AE8', label: 'Indigo' },
  { hex: '#80C820', label: 'Lime'   },
  { hex: '#20B8E8', label: 'Cyan'   },
  { hex: '#E84060', label: 'Rose'   },
];

function ColourPicker({ value, onChange }) {
  return (
    <div className="cat-colour-picker">
      {CATEGORY_COLOURS.map((c, i) => (
        <button
          key={i}
          type="button"
          className={`cat-colour-swatch${value === c.hex ? ' selected' : ''}`}
          style={c.hex ? { '--swatch-color': c.hex } : {}}
          onMouseDown={e => e.preventDefault()}
          onClick={() => onChange(c.hex)}
          title={c.label}
        />
      ))}
    </div>
  );
}

// ── Rich text editor ───────────────────────────────────────────────────────

function RichTextEditor({ initialHtml, onChange, globalVariables = {}, isPro = false, onShowUpgrade, reusableFillInLabels = [] }) {
  const editorRef      = useRef(null);
  const menuRef        = useRef(null);
  const keyBtnRef      = useRef(null);
  const keyMenuRef     = useRef(null);
  const initialHtmlRef = useRef(initialHtml);
  // Saved selection range — captured before the dropdown opens so that focus
  // loss (e.g. when the fill-in label input steals focus) doesn't destroy the
  // insertion point.
  const savedRangeRef  = useRef(null);

  const [showInsert, setShowInsert] = useState(false);
  const [insertCategory, setInsertCategory] = useState(null); // 'clipboard'|'date'|'time'|'datemath'|'cursor'|'variables'
  const [menuPos, setMenuPos] = useState(null);
  const [fillInEntry, setFillInEntry] = useState(false);
  const [fillInLabel, setFillInLabel] = useState('');
  const fillInInputRef = useRef(null);
  // Inline rename popover for fill-in chips clicked in the editor body
  const [fillInRename, setFillInRename] = useState(null); // { label, x, y }
  const [fillInRenameValue, setFillInRenameValue] = useState('');
  const fillInRenameRef = useRef(null);
  const fillInRenameInputRef = useRef(null);
  const [showKeyPicker, setShowKeyPicker] = useState(false);
  const [keyPickerPos, setKeyPickerPos] = useState(null);
  const [keyPickerCapturing, setKeyPickerCapturing] = useState(false);
  const [keyPickerCaptured, setKeyPickerCaptured] = useState('');
  const [keyPickerRepeat, setKeyPickerRepeat] = useState(1);
  const keyPickerCapturingRef = useRef(false);
  const keyZoneRef = useRef(null);
  const [keyPickerEditTarget, setKeyPickerEditTarget] = useState(null);

  useLayoutEffect(() => {
    if (editorRef.current) {
      editorRef.current.innerHTML = initialHtmlRef.current || '';
    }
    // Use CSS styles for execCommand output (modern <span style>) instead of legacy <font>.
    // Target apps (Word/Outlook/Gmail) prefer inline-style HTML.
    try { document.execCommand('styleWithCSS', false, true); } catch {}
  }, []);

  // When fill-in entry mode activates, focus the label input.
  // The input is always mounted (CSS-hidden), so the ref is always valid —
  // no setTimeout needed, focus is immediate.
  useEffect(() => {
    if (fillInEntry) {
      fillInInputRef.current?.focus();
    }
  }, [fillInEntry]);

  // Keep ref in sync so the IPC handler can read it without a stale closure.
  useEffect(() => { keyPickerCapturingRef.current = keyPickerCapturing; }, [keyPickerCapturing]);

  // IPC: listen for captured key combo. Guard with ref so only the active picker
  // instance processes the event.
  useEffect(() => {
    if (!window.electronAPI?.onKeyCaptured) return;
    const handler = (combo) => {
      if (!keyPickerCapturingRef.current) return;
      setKeyPickerCaptured(combo);
      setKeyPickerCapturing(false);
    };
    window.electronAPI.onKeyCaptured(handler);
    return () => window.electronAPI.removeAllListeners?.('key-captured');
  }, []);

  // Close Insert dropdown on outside click or any scroll
  useEffect(() => {
    if (!showInsert) return;

    function close() {
      setShowInsert(false);
      setInsertCategory(null);
      setFillInEntry(false);
      setFillInLabel('');
    }
    function onMouseDown(e) {
      // Any toolbar icon button click is handled by the button itself — don't close from outside-click
      if (e.target.closest?.('.rte-toolbar')) return;
      if (!menuRef.current?.contains(e.target)) {
        close();
      }
    }

    function onScroll(e) {
      if (menuRef.current?.contains(e.target)) return;
      close();
    }

    document.addEventListener('mousedown', onMouseDown);
    window.addEventListener('scroll', onScroll, { capture: true });
    return () => {
      document.removeEventListener('mousedown', onMouseDown);
      window.removeEventListener('scroll', onScroll, { capture: true });
    };
  }, [showInsert]);

  // Close Press Key picker on outside click or any scroll
  useEffect(() => {
    if (!showKeyPicker) return;
    function close() {
      if (keyPickerCapturingRef.current) {
        window.electronAPI?.stopKeyCapture();
      }
      setShowKeyPicker(false);
      setKeyPickerCapturing(false);
      setKeyPickerCaptured('');
      setKeyPickerRepeat(1);
      setKeyPickerEditTarget(null);
    }
    function onMouseDown(e) {
      if (!keyBtnRef.current?.contains(e.target) && !keyMenuRef.current?.contains(e.target)) {
        close();
      }
    }
    function onScroll(e) {
      if (keyMenuRef.current?.contains(e.target)) return;
      close();
    }
    document.addEventListener('mousedown', onMouseDown);
    window.addEventListener('scroll', onScroll, { capture: true });
    return () => {
      document.removeEventListener('mousedown', onMouseDown);
      window.removeEventListener('scroll', onScroll, { capture: true });
    };
  }, [showKeyPicker]);

  // Close fill-in rename popover on outside click, Escape, or scroll
  useEffect(() => {
    if (!fillInRename) return;
    function onDown(e) {
      if (fillInRenameRef.current && !fillInRenameRef.current.contains(e.target)) {
        cancelFillInRename();
      }
    }
    function onScroll(e) {
      if (fillInRenameRef.current?.contains(e.target)) return;
      cancelFillInRename();
    }
    document.addEventListener('mousedown', onDown);
    window.addEventListener('scroll', onScroll, { capture: true });
    return () => {
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('scroll', onScroll, { capture: true });
    };
  }, [fillInRename]);

  // Flip the fill-in rename popover up / clamp left if its default position
  // (anchored at the chip's bottom-left) would clip the viewport.
  useLayoutEffect(() => {
    if (!fillInRename || !fillInRenameRef.current) return;
    const el = fillInRenameRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
  }, [fillInRename]);

  const notify = useCallback(() => {
    const html = editorRef.current.innerHTML;
    onChange({ html, text: htmlToPlainText(html) });
  }, [onChange]);

  function format(cmd, value) {
    editorRef.current?.focus();
    restoreSelection();
    document.execCommand(cmd, false, value ?? null);
    notify();
  }

  function applyTextColor(hex) {
    format('foreColor', hex);
    setShowInsert(false);
    setInsertCategory(null);
  }

  function applyHeading(blockTag) {
    // Use proper HTML5 block tags. Some browsers want angle brackets.
    format('formatBlock', blockTag.toUpperCase());
    setShowInsert(false);
    setInsertCategory(null);
  }

  function isActive(cmd) {
    try { return document.queryCommandState(cmd); } catch { return false; }
  }

  // Snapshot the current cursor/selection inside the editor so we can restore
  // it later even after focus has moved elsewhere (e.g. fill-in label input).
  function saveSelection() {
    const sel = window.getSelection();
    if (sel && sel.rangeCount > 0 && editorRef.current?.contains(sel.anchorNode)) {
      savedRangeRef.current = sel.getRangeAt(0).cloneRange();
    }
  }

  // Restore focus + cursor to the saved position before inserting content.
  function restoreSelection() {
    editorRef.current?.focus();
    const range = savedRangeRef.current;
    if (!range) return;
    const sel = window.getSelection();
    if (sel) {
      sel.removeAllRanges();
      sel.addRange(range);
    }
  }

  function insertTokenHtml(tokenStr, display) {
    console.log('insertTokenHtml called', { tokenStr, display, savedRange: savedRangeRef.current });
    restoreSelection();

    const sel = window.getSelection();
    console.log('after restoreSelection — rangeCount:', sel?.rangeCount, 'focused:', document.activeElement === editorRef.current);

    const span = document.createElement('span');
    span.className = 'rte-token';
    span.setAttribute('data-token', tokenStr);
    span.setAttribute('contenteditable', 'false');
    span.textContent = display;
    const zwsp = document.createTextNode('\u200B');

    if (sel && sel.rangeCount > 0) {
      console.log('inserting via Range API');
      const range = sel.getRangeAt(0);
      range.deleteContents();
      const frag = document.createDocumentFragment();
      frag.appendChild(span);
      frag.appendChild(zwsp);
      range.insertNode(frag);

      // Move cursor to just after the zero-width space
      const newRange = document.createRange();
      newRange.setStartAfter(zwsp);
      newRange.collapse(true);
      sel.removeAllRanges();
      sel.addRange(newRange);
    } else {
      // Fallback: no cursor position — append to end of editor
      console.warn('insertTokenHtml: no selection, appending to end of editor');
      editorRef.current.focus();
      editorRef.current.appendChild(span);
      editorRef.current.appendChild(zwsp);
    }

    notify();
    savedRangeRef.current = null;
  }

  function handleInsertItem(e, item) {
    e.preventDefault();
    if (item.token === '__fillin__') {
      setFillInEntry(true);
      setFillInLabel('');
      return;
    }
    insertTokenHtml(item.token, item.display);
    setShowInsert(false);
    setInsertCategory(null);
  }

  function insertFillInToken(label) {
    insertTokenHtml(`{fillIn:${label}}`, `▭ ${label}`);
    setFillInEntry(false);
    setFillInLabel('');
    setShowInsert(false);
    setInsertCategory(null);
  }

  function commitFillInRename() {
    if (!fillInRename) return;
    const oldLabel = fillInRename.label;
    const newLabel = fillInRenameValue.trim();
    setFillInRename(null);
    setFillInRenameValue('');
    if (!newLabel || newLabel === oldLabel) return;
    // Update every chip in this editor with the matching label so renaming a
    // field that appears multiple times stays consistent.
    const oldToken = `{fillIn:${oldLabel}}`;
    const newToken = `{fillIn:${newLabel}}`;
    if (editorRef.current) {
      const chips = editorRef.current.querySelectorAll('[data-token]');
      chips.forEach(chip => {
        if (chip.dataset.token === oldToken) {
          chip.setAttribute('data-token', newToken);
          chip.textContent = `▭ ${newLabel}`;
        }
      });
    }
    notify();
  }

  function cancelFillInRename() {
    setFillInRename(null);
    setFillInRenameValue('');
  }

  function handleInsertFillIn(e) {
    e.preventDefault();
    const label = fillInLabel.trim() || 'Field';
    insertFillInToken(label);
  }

  function openCategoryMenu(e, category) {
    e.preventDefault();
    saveSelection();
    if (showInsert && insertCategory === category) {
      setShowInsert(false);
      setInsertCategory(null);
      setFillInEntry(false);
      setFillInLabel('');
      return;
    }
    const r = e.currentTarget.getBoundingClientRect();
    // Right-anchor the popup when the button sits in the right half of the
    // viewport. As the popup widens (e.g. when the fill-in input row appears),
    // the LEFT edge grows leftward instead of the right edge overflowing.
    const anchorRight = r.right > window.innerWidth / 2;
    setMenuPos({
      top: r.bottom + 4,
      left: r.left,
      btnTop: r.top,
      btnRight: r.right,
      anchorRight,
      rightOffset: window.innerWidth - r.right,
    });
    setInsertCategory(category);
    setShowInsert(true);
    setShowKeyPicker(false);
    setFillInEntry(false);
    setFillInLabel('');
  }

  // Flip popup vertically when it would overflow the viewport. Horizontal
  // overflow is handled at click time by anchoring right vs left in menuPos
  // (see openCategoryMenu). The right-anchor case naturally grows leftward
  // when content widens (e.g. fill-in input row appearing) so no measure
  // correction is needed for it.
  useLayoutEffect(() => {
    if (!(showInsert && menuPos && menuRef.current)) return;
    const popup = menuRef.current;
    const rect = popup.getBoundingClientRect();
    const margin = 8;
    let top = menuPos.top;
    if (rect.bottom > window.innerHeight - margin) {
      top = menuPos.btnTop - rect.height - 4;
    }
    popup.style.top = `${Math.max(margin, top)}px`;

    if (!menuPos.anchorRight) {
      // Left-anchored: shift left if content widened past right edge.
      let left = menuPos.left;
      if (rect.right > window.innerWidth - margin) {
        left = menuPos.btnRight - rect.width;
      }
      popup.style.left = `${Math.max(margin, left)}px`;
    }
  }, [showInsert, insertCategory, menuPos, fillInEntry]);

  useLayoutEffect(() => {
    if (!(showKeyPicker && keyPickerPos && keyMenuRef.current)) return;
    const popup = keyMenuRef.current;
    const rect = popup.getBoundingClientRect();
    const margin = 8;
    let top = keyPickerPos.top;
    let left = keyPickerPos.left;
    if (rect.bottom > window.innerHeight - margin) {
      top = keyPickerPos.btnTop - rect.height - 4;
    }
    if (rect.right > window.innerWidth - margin) {
      left = keyPickerPos.btnRight - rect.width;
    }
    popup.style.top = `${Math.max(margin, top)}px`;
    popup.style.left = `${Math.max(margin, left)}px`;
  }, [showKeyPicker, keyPickerPos, keyPickerCapturing, keyPickerCaptured]);

  return (
    <div className="rte-wrap">
      <div className="rte-toolbar">
        <button
          type="button"
          className={`rte-btn${isActive('bold') ? ' rte-btn-on' : ''}`}
          onMouseDown={e => { e.preventDefault(); format('bold'); }}
          title="Bold"
        ><BoldIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${isActive('italic') ? ' rte-btn-on' : ''}`}
          onMouseDown={e => { e.preventDefault(); format('italic'); }}
          title="Italic"
        ><ItalicIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${isActive('underline') ? ' rte-btn-on' : ''}`}
          onMouseDown={e => { e.preventDefault(); format('underline'); }}
          title="Underline"
        ><UnderlineIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'color' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'color')}
          title="Text colour"
        ><PaletteIcon size={14} strokeWidth={2} /></button>
        <div className="rte-sep" />
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'headings' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'headings')}
          title="Heading style"
        ><HeadingIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className="rte-btn"
          onMouseDown={e => { e.preventDefault(); format('insertUnorderedList'); }}
          title="Bullet list"
        ><ListIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className="rte-btn"
          onMouseDown={e => { e.preventDefault(); format('insertOrderedList'); }}
          title="Numbered list"
        ><ListOrderedIcon size={14} strokeWidth={2} /></button>

        <div className="rte-sep" />

        {/* ── Category dropdowns ── */}
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'date' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'date')}
          title="Insert date"
        ><CalendarIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'time' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'time')}
          title="Insert time"
        ><ClockIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'datemath' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'datemath')}
          title="Date math (tomorrow, next week…)"
        ><CalendarClockIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'clipboard' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'clipboard')}
          title="Insert clipboard contents"
        ><ClipboardIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'cursor' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'cursor')}
          title="Cursor position & fill-in fields"
        ><TextCursorIcon size={14} strokeWidth={2} /></button>
        {Object.keys(globalVariables).length > 0 && (
          <button
            type="button"
            className={`rte-btn${showInsert && insertCategory === 'variables' ? ' rte-btn-on' : ''}`}
            onMouseDown={e => openCategoryMenu(e, 'variables')}
            title="Insert global variable"
          ><VariableIcon size={14} strokeWidth={2} /></button>
        )}

        <div className="rte-sep" />

        {/* ── Press Key dropdown ── */}
        <button
          ref={keyBtnRef}
          type="button"
          className={`rte-btn${showKeyPicker ? ' rte-btn-on' : ''}`}
          onMouseDown={e => {
            e.preventDefault();
            saveSelection();
            if (!showKeyPicker) {
              const r = e.currentTarget.getBoundingClientRect();
              setKeyPickerPos({ top: r.bottom + 4, left: r.left, btnTop: r.top, btnRight: r.right });
              setShowKeyPicker(true);
              setShowInsert(false);
              setInsertCategory(null);
              setKeyPickerCapturing(false);
              setKeyPickerCaptured('');
              setKeyPickerRepeat(1);
              setKeyPickerEditTarget(null);
            } else {
              if (keyPickerCapturingRef.current) {
                window.electronAPI?.stopKeyCapture();
              }
              setShowKeyPicker(false);
              setKeyPickerCapturing(false);
              setKeyPickerCaptured('');
              setKeyPickerRepeat(1);
              setKeyPickerEditTarget(null);
            }
          }}
          title="Insert a key press at cursor position"
        ><KeyboardIcon size={14} strokeWidth={2} /></button>
      </div>

      <div
        ref={editorRef}
        contentEditable
        className="rte-editor"
        onInput={notify}
        onBlur={saveSelection}
        suppressContentEditableWarning
        spellCheck={false}
        data-placeholder="Type replacement text…"
        onClick={e => {
          const keyChip = e.target.closest?.('[data-token^="{key:"]');
          if (keyChip) {
            e.preventDefault();
            const { combo, repeat } = parseKeyToken(keyChip.dataset.token);
            const rect = keyChip.getBoundingClientRect();
            setKeyPickerPos({ top: rect.bottom + 4, left: rect.left, btnTop: rect.top, btnRight: rect.right });
            setKeyPickerCaptured(combo);
            setKeyPickerRepeat(repeat);
            setKeyPickerCapturing(false);
            setKeyPickerEditTarget(keyChip);
            setShowKeyPicker(true);
            setShowInsert(false);
            return;
          }
          const fillinChip = e.target.closest?.('[data-token^="{fillIn:"]');
          if (fillinChip) {
            e.preventDefault();
            const match = fillinChip.dataset.token.match(/^\{fillIn:(.+)\}$/);
            if (match) {
              const label = match[1];
              const rect = fillinChip.getBoundingClientRect();
              setFillInRename({ label, x: rect.left, y: rect.bottom + 4 });
              setFillInRenameValue(label);
              setShowKeyPicker(false);
              setShowInsert(false);
            }
          }
        }}
      />

      {showInsert && insertCategory && menuPos && ReactDOM.createPortal(
        <div
          ref={menuRef}
          className="rte-insert-menu"
          style={menuPos.anchorRight
            ? { top: menuPos.top, right: Math.max(8, menuPos.rightOffset), maxHeight: window.innerHeight - menuPos.top - 16 }
            : { top: menuPos.top, left: menuPos.left, maxHeight: window.innerHeight - menuPos.top - 16 }
          }
        >
          {/* Fill-in label input — always mounted so ref is always valid,
              toggled visible/hidden via CSS to avoid React render-timing races */}
          <div
            className="rte-fillin-row"
            style={{ display: fillInEntry ? 'flex' : 'none' }}
          >
            <span className="rte-fillin-prompt-label">Field label:</span>
            <input
              ref={fillInInputRef}
              className="rte-fillin-input"
              value={fillInLabel}
              onChange={e => setFillInLabel(e.target.value)}
              placeholder="e.g. Recipient Name"
              onKeyDown={e => {
                if (e.key === 'Enter') handleInsertFillIn(e);
                if (e.key === 'Escape') { setFillInEntry(false); setFillInLabel(''); }
              }}
            />
            <button
              type="button"
              className="rte-fillin-ok"
              onMouseDown={handleInsertFillIn}
            >Insert</button>
          </div>

          {/* Reusable fill-in fields from this editor + sibling variants.
              Lets users insert the same field multiple times so a single
              expansion can prompt once and inject the answer at several
              cursor positions. */}
          {fillInEntry && reusableFillInLabels.length > 0 && (
            <div className="rte-fillin-reuse">
              <span className="rte-fillin-reuse-label">Reuse</span>
              <div className="rte-fillin-reuse-chips">
                {reusableFillInLabels.map(label => (
                  <button
                    key={label}
                    type="button"
                    className="rte-fillin-reuse-chip"
                    onMouseDown={e => {
                      e.preventDefault();
                      insertFillInToken(label);
                    }}
                    title={`Insert {fillIn:${label}} at cursor`}
                  >
                    ▭ {label}
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* Menu items — hidden while fill-in label input is active */}
          <div style={{ display: fillInEntry ? 'none' : 'contents' }}>
            {insertCategory === 'color' ? (
              <>
                <div className="rte-menu-section-label">Text Colour</div>
                <div className="rte-colour-grid">
                  {TEXT_COLOURS.map(c => (
                    <button
                      key={c.hex}
                      type="button"
                      className="rte-colour-swatch"
                      style={{ background: c.hex }}
                      title={c.label}
                      onMouseDown={e => { e.preventDefault(); applyTextColor(c.hex); }}
                    />
                  ))}
                </div>
              </>
            ) : insertCategory === 'headings' ? (
              <>
                <div className="rte-menu-section-label">Heading Style</div>
                {HEADING_OPTIONS.map(h => (
                  <button
                    key={h.block}
                    type="button"
                    className="rte-menu-item"
                    onMouseDown={e => { e.preventDefault(); applyHeading(h.block); }}
                  >
                    <span className={`rte-menu-chip rte-chip-heading rte-chip-${h.block}`}>{h.display}</span>
                    {h.label}
                  </button>
                ))}
              </>
            ) : insertCategory === 'variables' ? (
              <>
                <div className="rte-menu-section-label">Global Variables</div>
                {Object.entries(globalVariables)
                  .sort(([a], [b]) => a.localeCompare(b))
                  .map(([key]) => (
                    <button
                      key={key}
                      type="button"
                      className="rte-menu-item"
                      onMouseDown={e => {
                        e.preventDefault();
                        insertTokenHtml(`{{${key}}}`, keyToTitle(key));
                        setShowInsert(false);
                        setInsertCategory(null);
                      }}
                    >
                      <span className="rte-menu-chip rte-chip-globalvar">{keyToTitle(key)}</span>
                      {keyToTitle(key)}
                    </button>
                  ))
                }
              </>
            ) : (
              <>
                <div className="rte-menu-section-label">{INSERT_CATEGORIES[insertCategory].label}</div>
                {INSERT_CATEGORIES[insertCategory].items.map((item, i) =>
                  item.type === 'sep' ? (
                    <div key={`sep-${i}`} className="rte-menu-sep" />
                  ) : (
                    <button
                      key={item.token}
                      type="button"
                      className="rte-menu-item"
                      onMouseDown={e => handleInsertItem(e, item)}
                    >
                      <span className={`rte-menu-chip rte-chip-${item.chipClass || INSERT_CATEGORIES[insertCategory].chipClass}`}>
                        {item.display || '✎'}
                      </span>
                      {item.label}
                    </button>
                  )
                )}
              </>
            )}
          </div>
        </div>,
        document.body
      )}

      {showKeyPicker && keyPickerPos && ReactDOM.createPortal(
        <div
          ref={keyMenuRef}
          className="rte-insert-menu rte-key-capture-popup"
          style={{ top: keyPickerPos.top, left: keyPickerPos.left }}
        >
          <div className="rte-menu-section-label">{keyPickerEditTarget ? 'Edit Key Press' : 'Insert Key Press'}</div>
          <div className="rte-key-popup-body">
            {/* Capture zone — click to record, click again to cancel.
                tabIndex+focus() needed so the zone takes focus away from the
                contentEditable editor — the JS keydown intercept path in
                tauriAPI.js skips capture when isContentEditable is true. */}
            <div
              ref={keyZoneRef}
              tabIndex={0}
              className={`rte-key-zone${keyPickerCapturing ? ' rte-key-zone-active' : ''}`}
              onMouseDown={e => {
                e.preventDefault();
                if (keyPickerCapturing) {
                  window.electronAPI?.stopKeyCapture();
                  setKeyPickerCapturing(false);
                } else {
                  setKeyPickerCapturing(true);
                  setKeyPickerCaptured('');
                  keyZoneRef.current?.focus();
                  window.electronAPI?.startKeyCapture();
                }
              }}
            >
              {keyPickerCapturing ? (
                <span className="rte-key-zone-prompt">Press a key…</span>
              ) : keyPickerCaptured ? (
                <span className="rte-key-zone-value">
                  {keyPickerCaptured.split('+').map((k, i, arr) => (
                    <React.Fragment key={i}>
                      <kbd>{k}</kbd>
                      {i < arr.length - 1 && <span className="rte-key-zone-plus">+</span>}
                    </React.Fragment>
                  ))}
                </span>
              ) : (
                <span className="rte-key-zone-placeholder">Click to record a key…</span>
              )}
            </div>

            {keyPickerCaptured && !keyPickerCapturing && (
              <div className="rte-key-popup-footer">
                <div className="rte-key-repeat-row">
                  <span className="rte-key-repeat-x">×</span>
                  <input
                    type="number"
                    min="1"
                    max="99"
                    className="rte-key-repeat-input"
                    value={keyPickerRepeat}
                    onChange={e => {
                      const v = parseInt(e.target.value, 10);
                      if (!isNaN(v) && v >= 1) setKeyPickerRepeat(Math.min(v, 99));
                    }}
                    onBlur={e => {
                      const v = parseInt(e.target.value, 10);
                      setKeyPickerRepeat(isNaN(v) || v < 1 ? 1 : Math.min(v, 99));
                    }}
                    onMouseDown={e => e.stopPropagation()}
                    onClick={e => e.stopPropagation()}
                  />
                </div>
                <button
                  type="button"
                  className="rte-key-insert-btn"
                  onMouseDown={e => {
                    e.preventDefault();
                    const combo = keyPickerCaptured;
                    const repeat = keyPickerRepeat;
                    const token = `{key:${combo}:${repeat}}`;
                    const chipDisplay = repeat > 1 ? `${combo} ×${repeat}` : combo;
                    if (keyPickerEditTarget) {
                      keyPickerEditTarget.dataset.token = token;
                      keyPickerEditTarget.textContent = chipDisplay;
                      notify();
                    } else {
                      insertTokenHtml(token, chipDisplay);
                    }
                    setShowKeyPicker(false);
                    setKeyPickerCapturing(false);
                    setKeyPickerCaptured('');
                    setKeyPickerRepeat(1);
                    setKeyPickerEditTarget(null);
                  }}
                >{keyPickerEditTarget ? 'Update' : 'Insert'}</button>
              </div>
            )}
          </div>
        </div>,
        document.body
      )}

      {fillInRename && ReactDOM.createPortal(
        <div
          ref={fillInRenameRef}
          className="rte-fillin-rename"
          style={{ top: fillInRename.y, left: fillInRename.x }}
        >
          <span className="rte-fillin-rename-label">Rename field</span>
          <input
            ref={fillInRenameInputRef}
            autoFocus
            className="rte-fillin-input"
            value={fillInRenameValue}
            onChange={e => setFillInRenameValue(e.target.value)}
            onKeyDown={e => {
              e.stopPropagation();
              if (e.key === 'Enter') commitFillInRename();
              if (e.key === 'Escape') cancelFillInRename();
            }}
          />
          <button
            type="button"
            className="rte-fillin-ok"
            onMouseDown={e => { e.preventDefault(); commitFillInRename(); }}
          >Rename</button>
        </div>,
        document.body
      )}
    </div>
  );
}

// ── Left-click-only PointerSensor (right-click must pass through for context menu) ──

class LeftClickSensor extends PointerSensor {
  static activators = [
    {
      eventName: 'onPointerDown',
      handler: ({ nativeEvent }) => nativeEvent.button === 0,
    },
  ];
}

// ── Sortable category tab wrapper ──────────────────────────────────────────

function SortableCatTab({ id, children }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };
  return (
    <div ref={setNodeRef} style={style} {...attributes} {...listeners}>
      {children}
    </div>
  );
}

// ── Main component ─────────────────────────────────────────────────────────

export default function TextExpansions({
  expansions,
  onAdd,
  onDelete,
  categories = [],
  onAddCategory,
  onDeleteCategory,
  onReorderCategories,
  onUpdateCategoryColour,
  onRenameCategory,
  // Autocorrect props
  autocorrectEnabled,
  onToggleAutocorrect,
  autocorrections = [],
  onAddAutocorrect,
  onDeleteAutocorrect,
  // ── Global Variables
  globalVariables = {},
  onSaveGlobalVariables,
  // Pro gating
  isPro = false,
  onShowUpgrade,
  // One-shot prefill from clipboard "Create Expansion" button. Shape:
  // { text: string, requestedAt: number } — timestamp guarantees re-fire even
  // when the same text is sent twice in a row.
  prefill = null,
  onPrefillConsumed,
  // Expansion pack export/import
  onExportExpansions,
  onImportExpansions,
  expansionImportPrompt,
  onExpansionImportResolve,
  // Suppress foreground auto-switch while the user is mid-edit
  onEditingChange,
}) {
  // ── Panel mode (expansions | autocorrect | globalvars) ──
  const [panelMode, setPanelMode] = useState('expansions');

  // ── Expansion form state ──
  const [editing, setEditing]             = useState(null);
  const [trigger, setTrigger]             = useState('');
  const [displayName, setDisplayName]     = useState('');
  const [editorValue, setEditorValue]     = useState({ html: '', text: '' });
  const [category, setCategory]           = useState(null);
  const [triggerMode, setTriggerMode]     = useState('space'); // 'space' | 'immediate'
  const [expansionType, setExpansionType] = useState('text'); // 'text' | 'image'
  const [imagePath, setImagePath]         = useState('');
  const [imageScale, setImageScale]       = useState(100);
  const [imageExists, setImageExists]     = useState(true);
  const [imageDataUri, setImageDataUri]   = useState(null); // base64 data URI for preview
  const [variantOptions, setVariantOptions] = useState([]); // [{label, html, text}]
  const [activeVariantIndex, setActiveVariantIndex] = useState(0);
  const [renamingVariantIndex, setRenamingVariantIndex] = useState(null);
  const [variantRenameValue, setVariantRenameValue] = useState('');
  const [voicePhrases, setVoicePhrases]   = useState([]);

  // Push editing state to parent so foreground auto-switch is suppressed while
  // the user is mid-build. Non-null `editing` covers both Add and Edit flows.
  useEffect(() => {
    onEditingChange?.(editing !== null);
  }, [editing, onEditingChange]);

  // Load image preview via Rust when imagePath changes
  useEffect(() => {
    if (!imagePath) { setImageDataUri(null); return; }
    let cancelled = false;
    window.electronAPI?.readImageBase64(imagePath).then(uri => {
      if (cancelled) return;
      if (uri) { setImageDataUri(uri); setImageExists(true); }
      else     { setImageDataUri(null); setImageExists(false); }
    });
    return () => { cancelled = true; };
  }, [imagePath]);

  // ── Trigger duplicate error ──
  const [triggerError, setTriggerError] = useState('');

  // ── Category bar state ──
  const [activeCategory, setActiveCategory]     = useState('All');
  const [addingCategory, setAddingCategory]     = useState(false);
  const [newCategoryName, setNewCategoryName]   = useState('');
  const [newCategoryColour, setNewCategoryColour] = useState(null);
  // ── Category colour picker popover ──
  const [catColourPopover, setCatColourPopover] = useState(null); // { forCat, x, y }
  const catColourPopoverRef = useRef(null);
  // ── Category context menu ──
  const [catContextMenu, setCatContextMenu] = useState(null); // { catName, x, y }
  const [ctxDeleteConfirm, setCtxDeleteConfirm] = useState(false);
  const catContextMenuRef  = useRef(null);
  const catContextTabRef   = useRef(null); // DOM element of the right-clicked tab (for colour picker anchor)
  // ── Expansion row context menu ──
  const [itemContextMenu, setItemContextMenu] = useState(null); // { trigger, x, y }
  const itemContextMenuRef = useRef(null);
  // Pending edit signal: when a duplicate is created, we set this to the new
  // trigger and a useEffect picks it up once expansions re-renders, then opens
  // the edit panel on the new entry.
  const [pendingEditTrigger, setPendingEditTrigger] = useState(null);
  // ── Category inline rename ──
  const [renamingCat, setRenamingCat]   = useState(null);
  const [renameValue, setRenameValue]   = useState('');
  const [renameError, setRenameError]   = useState('');
  const renameInputRef                  = useRef(null);
  const renameCommitting                = useRef(false);
  const [deleteConfirm, setDeleteConfirm]       = useState(null); // trigger string awaiting confirmation

  // ── Category dnd-kit reorder ──
  const catDndSensors = useSensors(useSensor(LeftClickSensor, { activationConstraint: { distance: 8 } }));
  const [catDragId, setCatDragId] = useState(null);

  // ── Expansion type filter ──
  const [typeFilter, setTypeFilter] = useState('all'); // 'all' | 'text' | 'image'

  // ── Expansion search ──
  const [searchQuery, setSearchQuery] = useState('');

  // ── Expansion sort state (persisted to localStorage) ──
  const [sortKey, setSortKey] = useState(() =>
    localStorage.getItem('trigr.expansionSort') || 'default'
  );

  // ── Autocorrect form state ──
  const [acEditing, setAcEditing]       = useState(null); // null | { isNew, originalTypo? }
  const [acTypo, setAcTypo]             = useState('');
  const [acCorrection, setAcCorrection] = useState('');

  // ── Global Variables form state ──
  const [gdEditing, setGdEditing]   = useState(null); // null | { isNew, originalKey? }
  const [gdTitle,   setGdTitle]     = useState('');
  const [gdValue,   setGdValue]     = useState('');
  const [gdNameErr, setGdNameErr]   = useState('');

  // If the active category is deleted, fall back to All
  useEffect(() => {
    if (activeCategory !== 'All' && activeCategory !== '__uncategorised__' &&
        !categories.some(c => (typeof c === 'string' ? c : c?.name) === activeCategory)) {
      setActiveCategory('All');
    }
  }, [categories, activeCategory]);

  // Close colour picker popover on outside click
  useEffect(() => {
    if (!catColourPopover) return;
    function onDown(e) {
      if (!catColourPopoverRef.current?.contains(e.target)) setCatColourPopover(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [catColourPopover]);

  // Close category context menu on outside click or Escape
  useEffect(() => {
    if (!catContextMenu) return;
    function onDown(e) {
      if (catContextMenuRef.current && !catContextMenuRef.current.contains(e.target)) {
        setCatContextMenu(null);
      }
    }
    function onKey(e) { if (e.key === 'Escape') setCatContextMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [catContextMenu]);

  // Close expansion row context menu on outside click or Escape
  useEffect(() => {
    if (!itemContextMenu) return;
    function onDown(e) {
      if (itemContextMenuRef.current && !itemContextMenuRef.current.contains(e.target)) {
        setItemContextMenu(null);
      }
    }
    function onKey(e) { if (e.key === 'Escape') setItemContextMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [itemContextMenu]);

  // Flip the category colour popover up / clamp left if its default position
  // (anchored below the trigger tab) would clip the viewport.
  useLayoutEffect(() => {
    if (!catColourPopover || !catColourPopoverRef.current) return;
    const el = catColourPopoverRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
  }, [catColourPopover]);

  // Clamp both right-click context menus inside the viewport — raw clientX /
  // clientY overflow when right-clicking near the edge of the panel.
  useLayoutEffect(() => {
    if (!catContextMenu || !catContextMenuRef.current) return;
    const el = catContextMenuRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
  }, [catContextMenu]);

  useLayoutEffect(() => {
    if (!itemContextMenu || !itemContextMenuRef.current) return;
    const el = itemContextMenuRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
  }, [itemContextMenu]);

  // Auto-select all text when inline rename input appears
  useEffect(() => {
    if (renamingCat) renameInputRef.current?.select();
  }, [renamingCat]);

  // ── Expansion handlers ──
  function openAdd(prefillHtml = '', prefillText = '') {
    setTrigger('');
    setDisplayName('');
    setTriggerError('');
    setEditorValue({ html: prefillHtml, text: prefillText });
    setCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setTriggerMode('space');
    setExpansionType('text');
    setImagePath('');
    setImageScale(100);
    setImageExists(true);
    setVariantOptions([]);
    setActiveVariantIndex(0);
    setRenamingVariantIndex(null);
    setVoicePhrases([]);
    setEditing({ isNew: true });
  }

  // Consume a one-shot prefill from the clipboard panel and open the new-expansion
  // form with the body editor seeded with that text. The parent clears its
  // prefill state via onPrefillConsumed so subsequent tab switches don't re-fire.
  useEffect(() => {
    if (!prefill?.text) return;
    const html = plainTextToHtml(prefill.text);
    openAdd(html, prefill.text);
    onPrefillConsumed?.();
  }, [prefill]); // eslint-disable-line react-hooks/exhaustive-deps

  function openEdit(exp) {
    setTrigger(exp.trigger);
    setDisplayName(exp.displayName || '');
    setTriggerError('');
    setEditorValue({ html: exp.html || '', text: exp.text || '' });
    setCategory(exp.category || null);
    setTriggerMode(exp.triggerMode || 'space');
    const expType = exp.expansionType || 'text';
    setExpansionType(expType);
    setImagePath(exp.imagePath || '');
    setImageScale(exp.imageScale ?? 100);
    setImageExists(true);
    // Legacy variants stored as {label, text} only — synthesise html from text
    // so the new rich-text tab UI can edit them. New saves write all three.
    const migratedOptions = (exp.options || []).map(o => ({
      label: o.label || '',
      html: o.html || plainTextToHtml(o.text || ''),
      text: o.text || '',
    }));
    setVariantOptions(migratedOptions);
    setActiveVariantIndex(0);
    setRenamingVariantIndex(null);
    setVoicePhrases(Array.isArray(exp.voicePhrases) ? exp.voicePhrases : []);
    setEditing({ isNew: false, originalTrigger: exp.trigger });
  }

  function handleSave() {
    const t = trigger.trim().toLowerCase().replace(/\s/g, '');
    if (!t) return;
    const hasVariants = variantOptions.length > 0 && variantOptions.some(o => o.text?.trim());
    if (expansionType === 'image') {
      if (!imagePath) return;
    } else if (!hasVariants) {
      if (!editorValue.text.trim()) return;
    }
    const originalTrigger = editing.isNew ? null : editing.originalTrigger;
    const cleanedVariants = hasVariants ? variantOptions.filter(o => o.text?.trim()) : [];
    onAdd(t, editorValue, originalTrigger, category, triggerMode, displayName.trim() || null, expansionType, imagePath, imageScale, cleanedVariants, voicePhrases);
    setEditing(null);
  }

  function handleCancel() {
    setEditing(null);
  }

  function handleAddCategory(e) {
    e.preventDefault();
    const name = newCategoryName.trim();
    if (name) {
      onAddCategory(name, newCategoryColour);
      setNewCategoryName('');
      setNewCategoryColour(null);
    }
    setAddingCategory(false);
  }

  function openCatColourPopover(e, forCat) {
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    setCatColourPopover({ forCat, x: rect.left, y: rect.bottom + 4 });
  }

  function handleCatColourSelect(colour) {
    if (catColourPopover?.forCat === '__new__') {
      setNewCategoryColour(colour);
    } else if (catColourPopover?.forCat) {
      onUpdateCategoryColour?.(catColourPopover.forCat, colour);
    }
    setCatColourPopover(null);
  }

  // ── Category context menu handlers ──
  function handleCatContextMenu(e, catName) {
    e.preventDefault();
    catContextTabRef.current = e.currentTarget;
    setCtxDeleteConfirm(false);
    setCatContextMenu({ catName, x: e.clientX, y: e.clientY });
  }

  function ctxRename() {
    const name = catContextMenu.catName;
    setCatContextMenu(null);
    setRenamingCat(name);
    setRenameValue(name);
    setRenameError('');
  }

  function ctxChangeColour() {
    const { catName } = catContextMenu;
    // Read the tab's position fresh at click time — anchors picker below the tab
    const tabRect = catContextTabRef.current?.getBoundingClientRect();
    if (tabRect) {
      const PICKER_WIDTH = 212;
      const left = Math.min(tabRect.left, window.innerWidth - PICKER_WIDTH - 8);
      setCatColourPopover({ forCat: catName, x: left, y: tabRect.bottom + 4 });
    } else {
      // Fallback: open below the context menu if tab ref is unexpectedly gone
      setCatColourPopover({ forCat: catName, x: catContextMenu.x, y: catContextMenu.y + 4 });
    }
    setCatContextMenu(null);
  }

  function ctxDelete() {
    if (!ctxDeleteConfirm) {
      setCtxDeleteConfirm(true);
      return;
    }
    onDeleteCategory(catContextMenu.catName);
    setCatContextMenu(null);
    setCtxDeleteConfirm(false);
  }

  // ── Expansion row context menu handlers ──
  function handleItemContextMenu(e, trigger) {
    e.preventDefault();
    e.stopPropagation();
    setItemContextMenu({ trigger, x: e.clientX, y: e.clientY });
  }

  function ctxItemDuplicate() {
    if (!itemContextMenu) return;
    const original = expansions.find(e => e.trigger === itemContextMenu.trigger);
    setItemContextMenu(null);
    if (!original) return;

    // Find a unique trigger for the copy. "<trigger>-copy" first, then
    // "-copy-2", "-copy-3" if needed.
    let copyTrigger = `${original.trigger}-copy`;
    let counter = 2;
    while (expansions.some(e => e.trigger === copyTrigger)) {
      copyTrigger = `${original.trigger}-copy-${counter++}`;
    }

    const editorVal = { html: original.html || '', text: original.text || '' };
    onAdd(
      copyTrigger,
      editorVal,
      null, // null originalTrigger → new entry, not a rename
      original.category || null,
      original.triggerMode || 'space',
      original.displayName ? `${original.displayName} (copy)` : null,
      original.expansionType || 'text',
      original.imagePath || '',
      original.imageScale ?? 100,
      original.options || [],
      original.voicePhrases || [],
    );

    // Open the new copy in the edit panel once the expansions list re-renders.
    setPendingEditTrigger(copyTrigger);
  }

  function ctxItemDelete() {
    if (!itemContextMenu) return;
    const trigger = itemContextMenu.trigger;
    setItemContextMenu(null);
    setDeleteConfirm(trigger);
  }

  // When a duplicate is created, expansions re-renders with the new entry.
  // Open the edit panel on it, then clear the signal.
  useEffect(() => {
    if (!pendingEditTrigger) return;
    const exp = expansions.find(e => e.trigger === pendingEditTrigger);
    if (exp) {
      openEdit(exp);
      setPendingEditTrigger(null);
    }
  }, [pendingEditTrigger, expansions]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Inline rename handlers ──
  function commitCatRename() {
    const trimmed = renameValue.trim();
    if (!trimmed) { setRenameError('Name cannot be empty'); return; }
    if (trimmed !== renamingCat && normCategories.some(c => c.name === trimmed)) {
      setRenameError('Already exists'); return;
    }
    renameCommitting.current = true;
    if (trimmed !== renamingCat) {
      onRenameCategory?.(renamingCat, trimmed);
      if (activeCategory === renamingCat) setActiveCategory(trimmed);
    }
    setRenamingCat(null);
    setRenameValue('');
    setRenameError('');
  }

  function cancelCatRename() {
    if (renameCommitting.current) { renameCommitting.current = false; return; }
    setRenamingCat(null);
    setRenameValue('');
    setRenameError('');
  }

  // ── Category drag-and-drop handlers ──
  function handleCatDragEnd(event) {
    setCatDragId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIdx = normCategories.findIndex(c => c.name === active.id);
    const newIdx = normCategories.findIndex(c => c.name === over.id);
    if (oldIdx === -1 || newIdx === -1) return;
    onReorderCategories?.(arrayMove([...normCategories], oldIdx, newIdx));
  }

  // ── Autocorrect handlers ──
  function openAcAdd() {
    setAcTypo('');
    setAcCorrection('');
    setAcEditing({ isNew: true });
  }

  function openAcEdit(ac) {
    setAcTypo(ac.typo);
    setAcCorrection(ac.correction);
    setAcEditing({ isNew: false, originalTypo: ac.typo });
  }

  function handleAcSave() {
    const typo = acTypo.trim().toLowerCase().replace(/\s/g, '');
    const correction = acCorrection.trim();
    if (!typo || !correction) return;
    const originalTypo = acEditing.isNew ? null : acEditing.originalTypo;
    onAddAutocorrect?.(typo, correction, originalTypo);
    setAcEditing(null);
  }

  function handleAcCancel() {
    setAcEditing(null);
  }

  const hasVariants = variantOptions.length > 0 && variantOptions.some(o => o.text?.trim());
  const canSave   = trigger.trim() && !triggerError && (
    expansionType === 'image' ? !!imagePath : (hasVariants || !!editorValue.text.trim())
  );
  const canAcSave = acTypo.trim() && acCorrection.trim();

  // Normalise categories — guard against old string-array format surviving in
  // config or being introduced by a stale drag-and-drop state.
  const normCategories = categories
    .map(c => typeof c === 'string' ? { name: c, colour: null } : c)
    .filter(c => c && c.name);

  // Deduped fill-in field labels across the single-mode editor body and every
  // variant. Passed to all RichTextEditor instances so users can re-insert a
  // field they already created without re-typing the label. Same field can
  // appear at multiple cursor positions in one expansion — the engine prompts
  // once at fire time and injects the answer everywhere it appears.
  const reusableFillInLabels = useMemo(() => {
    const set = new Set();
    extractFillInLabels(editorValue.html).forEach(l => set.add(l));
    variantOptions.forEach(v => extractFillInLabels(v.html).forEach(l => set.add(l)));
    return Array.from(set);
  }, [editorValue.html, variantOptions]);

  // Apply type filter before category/sorting
  const typeFiltered = typeFilter === 'all'
    ? expansions
    : typeFilter === 'image'
      ? expansions.filter(e => e.expansionType === 'image')
      : expansions.filter(e => !e.expansionType || e.expansionType === 'text');

  const uncategorisedCount = typeFiltered.filter(e => !e.category).length;

  // Build flat list for the current expansion tab
  const listItems = (() => {
    function sortItems(arr) {
      const a = [...arr];
      switch (sortKey) {
        case 'trigger-desc': return a.sort((x, y) => y.trigger.localeCompare(x.trigger));
        case 'name-asc':     return a.sort((x, y) => (x.displayName || x.trigger).localeCompare(y.displayName || y.trigger));
        case 'name-desc':    return a.sort((x, y) => (y.displayName || y.trigger).localeCompare(x.displayName || x.trigger));
        default:             return a.sort((x, y) => x.trigger.localeCompare(y.trigger)); // 'default' = trigger A→Z
      }
    }

    if (activeCategory !== 'All') {
      const pool = activeCategory === '__uncategorised__'
        ? typeFiltered.filter(e => !e.category)
        : typeFiltered.filter(e => e.category === activeCategory);
      return sortItems(pool).map(exp => ({ type: 'item', exp }));
    }

    // All tab — grouped: uncategorised first, then named categories in user-defined order
    const result = [];
    const uncat = sortItems(typeFiltered.filter(e => !e.category));
    if (uncat.length > 0) {
      result.push({ type: 'header', label: 'Uncategorised', color: null, count: uncat.length });
      uncat.forEach(exp => result.push({ type: 'item', exp }));
    }
    for (const cat of normCategories) {
      const items = sortItems(typeFiltered.filter(e => e.category === cat.name));
      if (items.length === 0) continue;
      result.push({ type: 'header', label: cat.name, color: cat.colour || null, count: items.length });
      items.forEach(exp => result.push({ type: 'item', exp }));
    }
    return result;
  })();

  // Apply search filter to expansion list
  const filteredListItems = (() => {
    if (!searchQuery.trim()) return listItems;
    const q = searchQuery.trim().toLowerCase();
    return listItems.filter(item => {
      if (item.type === 'header') return false;
      const exp = item.exp;
      return (
        exp.trigger.toLowerCase().includes(q) ||
        (exp.displayName || '').toLowerCase().includes(q) ||
        (exp.text || '').toLowerCase().includes(q) ||
        (exp.category || '').toLowerCase().includes(q)
      );
    });
  })();

  // Sorted custom autocorrections
  const sortedAc = [...autocorrections].sort((a, b) => a.typo.localeCompare(b.typo));

  // ── Global Variables handlers ────────────────────────────────────────────

  function openGdAdd(preTitle = '') {
    setGdEditing({ isNew: true });
    setGdTitle(preTitle);
    setGdValue('');
    setGdNameErr('');
  }

  function openGdEdit(key) {
    setGdEditing({ isNew: false, originalKey: key });
    setGdTitle(keyToTitle(key));
    setGdValue(globalVariables[key] ?? '');
    setGdNameErr('');
  }

  function handleGdCancel() {
    setGdEditing(null);
    setGdNameErr('');
  }

  function validateGdTitle(title) {
    const key = titleToKey(title.trim());
    if (!title.trim()) return 'Display title is required';
    if (!key) return 'Title must contain at least one letter or digit';
    if (gdEditing?.isNew && key in globalVariables) return `Key "${key}" already exists — choose a different title`;
    if (!gdEditing?.isNew && key !== gdEditing?.originalKey && key in globalVariables) return `Key "${key}" already exists — choose a different title`;
    return '';
  }

  function handleGdSave() {
    const err = validateGdTitle(gdTitle);
    if (err) { setGdNameErr(err); return; }
    const key = titleToKey(gdTitle.trim());
    const next = { ...globalVariables };
    if (!gdEditing.isNew && gdEditing.originalKey && gdEditing.originalKey !== key) {
      delete next[gdEditing.originalKey];
    }
    next[key] = gdValue;
    onSaveGlobalVariables?.(next);
    setGdEditing(null);
    setGdNameErr('');
  }

  function handleGdDelete(key) {
    const next = { ...globalVariables };
    delete next[key];
    onSaveGlobalVariables?.(next);
  }

  const sortedGd = Object.entries(globalVariables).sort(([a], [b]) => a.localeCompare(b));
  const canGdSave = gdTitle.trim() !== '' && gdValue.trim() !== '' && !validateGdTitle(gdTitle);
  const gdSuggestionsToShow = GD_SUGGESTIONS.filter(title => !(titleToKey(title) in globalVariables));

  const itemCount = filteredListItems.filter(x => x.type === 'item').length;

  return (
    <div className="text-expansions">

      {/* ── Header ── */}
      <div className="te-header">
        <div className="te-mode-tabs">
          <button
            className={`te-mode-tab${panelMode === 'expansions' ? ' active' : ''}`}
            onClick={() => setPanelMode('expansions')}
            type="button"
          >
            ✦ Text Expansions
          </button>
          {/* Autocorrect tab hidden for Alpha
          <button
            className={`te-mode-tab${panelMode === 'autocorrect' ? ' active' : ''}`}
            onClick={() => setPanelMode('autocorrect')}
            type="button"
          >
            Autocorrect
          </button>
          */}
        </div>
        <div className="te-header-right">
          {panelMode !== 'globalvars' && (
            <span className="te-hint">
              {panelMode === 'expansions' ? 'type trigger + Space' : 'corrects on Space'}
            </span>
          )}
          {panelMode === 'expansions' && (
            <button className="te-add-btn" onClick={() => openAdd()} title="Add expansion" type="button">
              + Add
            </button>
          )}
          {panelMode === 'autocorrect' && (
            <button className="te-add-btn" onClick={openAcAdd} title="Add custom correction" type="button">
              + Add
            </button>
          )}
          {panelMode === 'globalvars' && (
            <button className="te-add-btn" onClick={() => openGdAdd()} title="Add variable" type="button">
              + Add Variable
            </button>
          )}
          <button
            className={`te-gv-link${panelMode === 'globalvars' ? ' active' : ''}`}
            onClick={() => {
              if (!isPro) { onShowUpgrade?.('Global variables'); return; }
              setPanelMode('globalvars'); setGdEditing(null);
            }}
            type="button"
            title="Global Variables — reusable values inserted into expansions (Pro)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <rect x="1" y="1" width="10" height="10" rx="2"/>
              <path d="M4 4h1M7 4h1M4 6h4M4 8h3"/>
            </svg>
            Global Variables <span className="pro-badge">PRO</span>
          </button>
        </div>
      </div>

      {/* ════════════════════════════════════ EXPANSIONS VIEW ══════════════════════════════════ */}
      {panelMode === 'expansions' && (
        <>
          {/* ── Content: category sidebar + main ── */}
          <div className="te-content">

          {/* ── Category sidebar ── */}
          <div className="te-cat-sidebar">
            <div className="te-cat-sidebar-list">
              <button
                className={`te-cat-row${activeCategory === 'All' ? ' te-cat-row-active' : ''}`}
                onClick={() => setActiveCategory('All')}
              >
                <span className="te-cat-row-name">All</span>
                <span className="te-cat-count">{typeFiltered.length}</span>
              </button>

              {expansions.length > 0 && uncategorisedCount > 0 && (
                <button
                  className={`te-cat-row te-cat-row-uncategorised${activeCategory === '__uncategorised__' ? ' te-cat-row-active' : ''}`}
                  onClick={() => setActiveCategory('__uncategorised__')}
                >
                  <span className="te-cat-row-name">Uncategorised</span>
                  <span className="te-cat-count">{uncategorisedCount}</span>
                </button>
              )}

              <DndContext sensors={catDndSensors} onDragStart={e => setCatDragId(e.active.id)} onDragEnd={handleCatDragEnd}>
                <SortableContext items={normCategories.map(c => c.name)} strategy={verticalListSortingStrategy}>
                  {normCategories.map(cat => {
                    const catColour   = cat.colour || null;
                    const count       = typeFiltered.filter(e => e.category === cat.name).length;
                    return (
                      <SortableCatTab key={cat.name} id={cat.name}>
                        <div className="te-cat-row-group" onContextMenu={e => handleCatContextMenu(e, cat.name)}>
                          {renamingCat === cat.name ? (
                            <div
                              className="te-cat-row te-cat-row-active te-cat-rename-wrap"
                              style={catColour ? { '--cat-color': catColour } : {}}
                            >
                              <input
                                ref={renameInputRef}
                                className="te-cat-rename-input"
                                value={renameValue}
                                onChange={e => { setRenameValue(e.target.value); setRenameError(''); }}
                                onKeyDown={e => {
                                  if (e.key === 'Enter')  { e.preventDefault(); commitCatRename(); }
                                  if (e.key === 'Escape') { e.preventDefault(); cancelCatRename(); }
                                  e.stopPropagation();
                                }}
                                onBlur={cancelCatRename}
                              />
                              {renameError && <span className="te-cat-rename-error">{renameError}</span>}
                            </div>
                          ) : (
                            <button
                              className={`te-cat-row${activeCategory === cat.name ? ' te-cat-row-active' : ''}`}
                              style={catColour ? { '--cat-color': catColour } : {}}
                              onClick={() => setActiveCategory(cat.name)}
                            >
                              <span
                                className="te-cat-dot te-cat-dot-pick"
                                style={catColour ? { background: catColour } : {}}
                                onClick={e => openCatColourPopover(e, cat.name)}
                                title="Change colour"
                              />
                              <span className="te-cat-row-name">{cat.name}</span>
                              <span className="te-cat-count">{count}</span>
                            </button>
                          )}
                        </div>
                      </SortableCatTab>
                    );
                  })}
                </SortableContext>
                <DragOverlay>
                  {catDragId ? (
                    <div className="te-cat-row-group te-cat-row-ghost">
                      <button className="te-cat-row te-cat-row-active">{catDragId}</button>
                    </div>
                  ) : null}
                </DragOverlay>
              </DndContext>

              {addingCategory ? (
                <form onSubmit={handleAddCategory} className="te-cat-add-form">
                  <span
                    className="te-cat-add-colour-dot"
                    style={newCategoryColour ? { background: newCategoryColour } : {}}
                    onMouseDown={e => e.preventDefault()}
                    onClick={e => openCatColourPopover(e, '__new__')}
                    title="Pick a colour (optional)"
                  />
                  <input
                    autoFocus
                    className="te-cat-add-input"
                    value={newCategoryName}
                    onChange={e => setNewCategoryName(e.target.value)}
                    placeholder="Category name…"
                    onBlur={handleAddCategory}
                    onKeyDown={e => e.key === 'Escape' && setAddingCategory(false)}
                  />
                </form>
              ) : (
                <button className="te-cat-new-btn" onClick={() => { setAddingCategory(true); setNewCategoryColour(null); }}>
                  + Add Category
                </button>
              )}
              <button
                className="te-cat-new-btn te-cat-pack-btn"
                onClick={() => onImportExpansions?.()}
                title="Import a category from a pack file (.json)"
                type="button"
              >
                ↓ Import Category
              </button>
              <button
                className="te-cat-new-btn te-cat-pack-btn"
                onClick={() => onExportExpansions?.('all')}
                title="Export all text expansions to a pack file"
                type="button"
              >
                ↑ Export All
              </button>
            </div>
          </div>

          {/* ── Main area (toolbar + list + edit panel) ── */}
          <div className="te-main">

          {/* ── Toolbar: type filter ── */}
          <div className="te-toolbar">
            <div className="te-type-filter">
              <button
                type="button"
                className={`te-type-filter-pill${typeFilter === 'all' ? ' active' : ''}`}
                onClick={() => setTypeFilter('all')}
              >All</button>
              <button
                type="button"
                className={`te-type-filter-pill${typeFilter === 'text' ? ' active' : ''}`}
                onClick={() => setTypeFilter('text')}
              >Text</button>
              <button
                type="button"
                className={`te-type-filter-pill${typeFilter === 'image' ? ' active' : ''}`}
                onClick={() => setTypeFilter('image')}
              >Image</button>
              <SearchBar
                className="te-search-bar"
                placeholder="Search…"
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
                onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') { setSearchQuery(''); e.target.blur(); } }}
              />
            </div>
            <span className="te-toolbar-count">{itemCount} expansion{itemCount !== 1 ? 's' : ''}</span>
          </div>

          {/* ── Body: list + edit panel side-by-side ── */}
          <div className="te-body">

            {/* Scrollable list */}
            <div className="te-list">

              {/* ── Column headers (sticky, clickable sort) ── */}
              <div className="te-col-headers">
                <button
                  type="button"
                  className={`te-col-header${sortKey === 'default' || sortKey === 'trigger-desc' ? ' active' : ''}`}
                  onClick={() => {
                    const next = sortKey === 'default' ? 'trigger-desc' : 'default';
                    setSortKey(next);
                    localStorage.setItem('trigr.expansionSort', next);
                  }}
                >
                  Trigger
                  <span className="te-sort-arrow">{sortKey === 'default' ? ' ▲' : sortKey === 'trigger-desc' ? ' ▼' : ''}</span>
                </button>
                <button
                  type="button"
                  className={`te-col-header${sortKey === 'name-asc' || sortKey === 'name-desc' ? ' active' : ''}`}
                  onClick={() => {
                    const next = sortKey === 'name-asc' ? 'name-desc' : 'name-asc';
                    setSortKey(next);
                    localStorage.setItem('trigr.expansionSort', next);
                  }}
                >
                  Name
                  <span className="te-sort-arrow">{sortKey === 'name-asc' ? ' ▲' : sortKey === 'name-desc' ? ' ▼' : ''}</span>
                </button>
                <div className="te-col-header te-col-header--static">Preview</div>
                <div className="te-col-header te-col-header--static">Tag</div>
                <div className="te-col-header-spacer" />
              </div>
              {itemCount === 0 ? (
                expansions.length === 0 ? (
                  <div className="te-empty-state">
                    <span className="te-empty-icon">✦</span>
                    <span className="te-empty-heading">No text expansions yet</span>
                    <span className="te-empty-sub">Click <strong>+ Add</strong> to create your first expansion. Type a short trigger word and it expands to full text instantly anywhere on your computer.</span>
                    <span className="te-empty-example">e.g. type <kbd className="te-empty-kbd">signoff</kbd> and press Space → <em>"Thanks for your message, speak soon!"</em></span>
                  </div>
                ) : typeFilter !== 'all' && typeFiltered.length === 0 ? (
                  <div className="te-empty-row">No {typeFilter} expansions</div>
                ) : (
                  <div className="te-empty-row">No expansions in this category yet</div>
                )
              ) : (
                filteredListItems.map((item, i) => {
                  if (item.type === 'header') {
                    return (
                      <div key={`h-${item.label}`} className="te-group-header">
                        {item.color && <span className="te-group-dot" style={{ background: item.color }} />}
                        <span className="te-group-name">{item.label.toUpperCase()}</span>
                        <span className="te-group-count">{item.count}</span>
                        <span className="te-group-rule" />
                      </div>
                    );
                  }
                  const { exp } = item;
                  const catObj = exp.category ? normCategories.find(c => c.name === exp.category) : null;
                  const color  = catObj?.colour || null;
                  const isEditingThis = editing && !editing.isNew && editing.originalTrigger === exp.trigger;
                  return (
                    <div
                      key={exp.trigger}
                      className={`te-item${isEditingThis ? ' te-item-editing' : ''}`}
                      onClick={() => openEdit(exp)}
                      onContextMenu={e => handleItemContextMenu(e, exp.trigger)}
                    >
                      {/* Col 1 — Trigger */}
                      <div className="te-col-trigger">
                        <kbd className="te-trigger-badge">{exp.trigger}</kbd>
                        {exp.triggerMode === 'immediate' && (
                          <span className="te-immediate-badge" title="Fires instantly (no Space needed)">⚡</span>
                        )}
                        {exp.expansionType === 'image' && (
                          <>
                            <span className={`te-img-badge${!isPro ? ' locked' : ''}`} title={!isPro ? 'Image expansion (Pro only — does not fire on Free)' : 'Image expansion'}>IMG</span>
                            {!isPro && <span className="pro-badge te-list-pro-badge" title="Pro feature">PRO</span>}
                          </>
                        )}
                        {exp.options && exp.options.length > 0 && (
                          <span className="te-variant-badge" title={`${exp.options.length} variants — picker shows on trigger`}>▾</span>
                        )}
                      </div>
                      {/* Col 2 — Name */}
                      <div className="te-col-name">{exp.displayName || exp.trigger}</div>
                      {/* Col 3 — Preview (text body, or variant chips when variants exist) */}
                      <div
                        className="te-col-preview"
                        title={
                          exp.expansionType === 'image' ? undefined
                            : exp.options && exp.options.length > 0 ? exp.options.map(o => o.label).join(' · ')
                            : (exp.text || undefined)
                        }
                      >
                        {exp.expansionType === 'image' ? (
                          exp.imagePath ? exp.imagePath.split(/[/\\]/).pop() : 'No image'
                        ) : exp.options && exp.options.length > 0 ? (
                          <div className="te-col-preview-variants">
                            {exp.options.map((opt, idx) => (
                              <span
                                key={idx}
                                className={`te-variant-chip${!isPro && idx > 0 ? ' locked' : ''}`}
                                title={(opt.text || '').replace(/\s+/g, ' ').trim() || (opt.label || `Option ${idx + 1}`)}
                              >
                                {opt.label || `Option ${idx + 1}`}
                              </span>
                            ))}
                          </div>
                        ) : (
                          (exp.text || '').replace(/\s+/g, ' ').trim()
                        )}
                      </div>
                      {/* Col 4 — Tag */}
                      <div className="te-col-tag">
                        {exp.category && (
                          <span
                            className="te-cat-badge"
                            style={color ? { '--cat-color': color } : {}}
                            title={exp.category}
                          >
                            {exp.category}
                          </span>
                        )}
                      </div>
                      {/* Col 5 — Actions */}
                      <div className="te-item-actions">
                        {exp.expansionType !== 'image' && (
                          <button
                            className="te-item-export"
                            onClick={e => { e.stopPropagation(); onExportExpansions?.('single', exp.trigger); }}
                            type="button"
                            title="Export expansion"
                          >↑</button>
                        )}
                        <button
                          className="te-item-delete"
                          onClick={e => { e.stopPropagation(); setDeleteConfirm(exp.trigger); }}
                          type="button"
                          title="Delete expansion"
                        >✕</button>
                      </div>
                    </div>
                  );
                })
              )}
            </div>

            {/* Right edit panel — always visible */}
            <div className="te-edit-panel">
              {editing ? (
                <>
                  <div className="te-panel-header">
                    <span className="te-panel-title">
                      {editing.isNew ? 'New Expansion' : 'Edit Expansion'}
                    </span>
                    <button className="te-panel-close" onClick={handleCancel} type="button">✕</button>
                  </div>

                  <div className="te-panel-scroll">

                  {/* Expansion type selector */}
                  <div className="te-type-selector">
                    <button
                      type="button"
                      className={`te-trigger-mode-btn${expansionType === 'text' ? ' active' : ''}`}
                      onClick={() => setExpansionType('text')}
                    >Text</button>
                    <button
                      type="button"
                      className={`te-trigger-mode-btn${expansionType === 'image' ? ' active' : ''}${!isPro ? ' locked' : ''}`}
                      onClick={() => {
                        if (!isPro) { onShowUpgrade?.('Image expansion'); return; }
                        setExpansionType('image');
                      }}
                      title={!isPro ? 'Upgrade to Pro for image expansions' : undefined}
                    >
                      Image
                      {!isPro && <span className="pro-badge">PRO</span>}
                    </button>
                  </div>

                  {/* Fixed-height top fields: name + trigger + mode + category */}
                  <div className="te-panel-fields">
                    <div className="te-panel-field">
                      <label className="form-label">NAME <span className="te-optional-label">(OPTIONAL)</span></label>
                      <input
                        className="form-input"
                        placeholder="e.g. Email sign-off, CAD polyline command…"
                        value={displayName}
                        onChange={e => setDisplayName(e.target.value)}
                        onKeyDown={e => { if (e.key === 'Escape') handleCancel(); }}
                        autoFocus
                        spellCheck={false}
                      />
                    </div>
                    <div className="te-panel-field">
                      <label className="form-label">VOICE COMMANDS <span className="pro-badge">PRO</span></label>
                      <div className="voice-phrase-list">
                        {voicePhrases.map((p, i) => (
                          <div className="voice-phrase-row" key={i}>
                            <input
                              className="form-input voice-phrase-input"
                              placeholder="e.g. my address"
                              value={p}
                              onChange={e => {
                                const next = [...voicePhrases];
                                next[i] = e.target.value;
                                setVoicePhrases(next);
                              }}
                              onKeyDown={e => { if (e.key === 'Escape') handleCancel(); }}
                              spellCheck={false}
                            />
                            <button
                              type="button"
                              className="voice-phrase-remove"
                              title="Remove phrase"
                              onClick={() => setVoicePhrases(voicePhrases.filter((_, idx) => idx !== i))}
                            >×</button>
                          </div>
                        ))}
                        <button
                          type="button"
                          className="voice-phrase-add"
                          onClick={() => setVoicePhrases([...voicePhrases, ''])}
                        >+ Add voice phrase</button>
                      </div>
                      <span className="form-hint" style={{ marginTop: 2 }}>All aliases fire the same expansion when spoken</span>
                    </div>
                    <div className="te-panel-field">
                      <label className="form-label">TRIGGER</label>
                      <input
                        className={`form-input te-trigger-input${triggerError ? ' te-input-error' : ''}`}
                        placeholder="brb"
                        value={trigger}
                        onChange={e => {
                          const val = e.target.value.replace(/\s/g, '');
                          setTrigger(val);
                          const normalized = val.trim().toLowerCase();
                          if (normalized) {
                            const clash = expansions.find(exp =>
                              exp.trigger.toLowerCase() === normalized &&
                              (editing?.isNew || exp.trigger.toLowerCase() !== editing?.originalTrigger?.toLowerCase())
                            );
                            if (clash) {
                              setTriggerError(`This trigger is already in use by "${clash.displayName || clash.trigger}". Delete or rename that expansion first.`);
                            } else {
                              setTriggerError('');
                            }
                          } else {
                            setTriggerError('');
                          }
                        }}
                        onKeyDown={e => { if (e.key === 'Escape') handleCancel(); }}
                        spellCheck={false}
                      />
                      {triggerError && <span className="te-trigger-error">{triggerError}</span>}
                    </div>
                    <div className="te-trigger-mode">
                      <button
                        type="button"
                        className={`te-trigger-mode-btn${triggerMode === 'space' ? ' active' : ''}`}
                        onClick={() => setTriggerMode('space')}
                        title="Fire after Space is pressed"
                      >+ Space</button>
                      <button
                        type="button"
                        className={`te-trigger-mode-btn${triggerMode === 'immediate' ? ' active' : ''}`}
                        onClick={() => setTriggerMode('immediate')}
                        title="Fire immediately when trigger is typed"
                      >⚡ Instant</button>
                    </div>
                    <div className="te-panel-field">
                      <label className="form-label">CATEGORY</label>
                      <select
                        className="te-cat-select"
                        value={category || ''}
                        onChange={e => setCategory(e.target.value || null)}
                      >
                        <option value="">Uncategorised</option>
                        {normCategories.map(cat => (
                          <option key={cat.name} value={cat.name}>{cat.name}</option>
                        ))}
                      </select>
                    </div>
                  </div>

                  {/* Content area — RTE for text, image picker for image */}
                  {expansionType === 'text' ? (
                    <div className="te-panel-rte">
                      {variantOptions.length === 0 ? (
                        <>
                          <div className="te-variant-header">
                            <label className="form-label">REPLACEMENT</label>
                            <button
                              type="button"
                              className={`te-variant-toggle-btn${!isPro ? ' locked' : ''}`}
                              onClick={() => {
                                if (!isPro) { onShowUpgrade?.('Expansion variants'); return; }
                                setVariantOptions([
                                  { label: 'Option 1', html: editorValue.html || '', text: editorValue.text || '' },
                                  { label: 'Option 2', html: '', text: '' },
                                ]);
                                setActiveVariantIndex(0);
                              }}
                              title={!isPro ? 'Upgrade to Pro to add variants' : undefined}
                            >
                              + Add Variants
                              {!isPro && <span className="pro-badge">PRO</span>}
                            </button>
                          </div>
                          <RichTextEditor
                            key={editing.isNew ? '__new__' : editing.originalTrigger}
                            initialHtml={editorValue.html}
                            onChange={setEditorValue}
                            globalVariables={globalVariables}
                            isPro={isPro}
                            onShowUpgrade={onShowUpgrade}
                            reusableFillInLabels={reusableFillInLabels}
                          />
                        </>
                      ) : (
                        <>
                          <div className="te-variant-header">
                            <label className="form-label">VARIANTS</label>
                          </div>
                          <p className="te-variant-hint">When triggered, a popup lets the user pick which variant to insert.</p>
                          <div className="te-variant-tabs" role="tablist">
                            {variantOptions.map((opt, i) => {
                              const isActive   = i === activeVariantIndex;
                              const isRenaming = i === renamingVariantIndex;
                              const canClose   = variantOptions.length > 2;
                              // Pro gate: Free users can edit Option 1 (which fires) but
                              // tabs 2+ are locked behind UpgradeModal. Data preserved on save.
                              const locked     = !isPro && i > 0;
                              return (
                                <div
                                  key={i}
                                  className={`te-variant-tab${isActive ? ' te-variant-tab--active' : ''}${locked ? ' te-variant-tab--locked' : ''}`}
                                  role="tab"
                                  aria-selected={isActive}
                                  onClick={() => {
                                    if (locked) { onShowUpgrade?.('Expansion variants'); return; }
                                    if (!isRenaming) setActiveVariantIndex(i);
                                  }}
                                  onDoubleClick={() => {
                                    if (locked) return;
                                    setRenamingVariantIndex(i);
                                    setVariantRenameValue(opt.label || '');
                                  }}
                                  title={locked ? 'Upgrade to Pro to use this variant' : (isActive ? 'Double-click to rename' : 'Click to switch, double-click to rename')}
                                >
                                  {isRenaming ? (
                                    <input
                                      autoFocus
                                      className="te-variant-tab-rename-input"
                                      value={variantRenameValue}
                                      onChange={e => setVariantRenameValue(e.target.value)}
                                      onClick={e => e.stopPropagation()}
                                      onBlur={() => {
                                        const trimmed = variantRenameValue.trim();
                                        if (trimmed) {
                                          const next = [...variantOptions];
                                          next[i] = { ...next[i], label: trimmed };
                                          setVariantOptions(next);
                                        }
                                        setRenamingVariantIndex(null);
                                      }}
                                      onKeyDown={e => {
                                        e.stopPropagation();
                                        if (e.key === 'Enter') { e.currentTarget.blur(); }
                                        if (e.key === 'Escape') {
                                          setRenamingVariantIndex(null);
                                        }
                                      }}
                                    />
                                  ) : (
                                    <span className="te-variant-tab-label">{opt.label || `Option ${i + 1}`}</span>
                                  )}
                                  {locked && <span className="pro-badge te-variant-tab-pro">PRO</span>}
                                  {!isRenaming && (
                                    <button
                                      type="button"
                                      className="te-variant-tab-close"
                                      onClick={e => {
                                        e.stopPropagation();
                                        if (!canClose) return;
                                        const next = variantOptions.filter((_, j) => j !== i);
                                        const newActive = Math.min(activeVariantIndex, next.length - 1);
                                        setVariantOptions(next);
                                        setActiveVariantIndex(Math.max(0, newActive));
                                      }}
                                      disabled={!canClose}
                                      title={canClose ? 'Remove this variant' : 'At least 2 variants required'}
                                      aria-label="Remove variant"
                                    >✕</button>
                                  )}
                                </div>
                              );
                            })}
                            <button
                              type="button"
                              className={`te-variant-tab-add${!isPro ? ' te-variant-tab-add--locked' : ''}`}
                              onClick={() => {
                                if (!isPro) { onShowUpgrade?.('Expansion variants'); return; }
                                const newIdx = variantOptions.length;
                                setVariantOptions([
                                  ...variantOptions,
                                  { label: `Option ${newIdx + 1}`, html: '', text: '' },
                                ]);
                                setActiveVariantIndex(newIdx);
                              }}
                              title={!isPro ? 'Upgrade to Pro to add more variants' : 'Add another variant'}
                            >+ Add{!isPro && <span className="pro-badge te-variant-tab-pro">PRO</span>}</button>
                          </div>

                          {variantOptions[activeVariantIndex] && (
                            <RichTextEditor
                              key={`__variant_${editing.isNew ? '__new__' : editing.originalTrigger}_${activeVariantIndex}__`}
                              initialHtml={variantOptions[activeVariantIndex].html || ''}
                              onChange={({ html, text }) => {
                                const next = [...variantOptions];
                                next[activeVariantIndex] = { ...next[activeVariantIndex], html, text };
                                setVariantOptions(next);
                              }}
                              globalVariables={globalVariables}
                              isPro={isPro}
                              onShowUpgrade={onShowUpgrade}
                              reusableFillInLabels={reusableFillInLabels}
                            />
                          )}

                          <button
                            type="button"
                            className="te-variant-remove-all-link"
                            onClick={() => {
                              const survivor = variantOptions[activeVariantIndex] || variantOptions[0];
                              if (survivor) {
                                setEditorValue({
                                  html: survivor.html || '',
                                  text: survivor.text || '',
                                });
                              }
                              setVariantOptions([]);
                              setActiveVariantIndex(0);
                              setRenamingVariantIndex(null);
                            }}
                            title="Drop all variants and keep only the active one"
                          >
                            Remove all variants
                          </button>
                        </>
                      )}
                    </div>
                  ) : (
                    <div className="te-panel-image">
                      {!isPro && (
                        <div className="te-image-pro-banner">
                          <span className="pro-badge">PRO</span>
                          <span>Image expansions don't fire on Free. Upgrade to use this expansion.</span>
                          <button
                            type="button"
                            className="te-image-pro-banner-cta"
                            onClick={() => onShowUpgrade?.('Image expansion')}
                          >Upgrade</button>
                        </div>
                      )}
                      <label className="form-label">IMAGE</label>
                      <button
                        type="button"
                        className={`te-image-pick-btn${!isPro ? ' locked' : ''}`}
                        onClick={async () => {
                          if (!isPro) { onShowUpgrade?.('Image expansion'); return; }
                          const path = await window.electronAPI?.browseForImage();
                          if (path) {
                            setImagePath(path);
                            setImageExists(true);
                          }
                        }}
                        title={!isPro ? 'Upgrade to Pro to change the image' : undefined}
                      >Choose Image…{!isPro && <span className="pro-badge">PRO</span>}</button>
                      {imagePath ? (
                        <div className="te-image-preview-wrap">
                          {imageDataUri ? (
                            <img
                              className="te-image-preview"
                              src={imageDataUri}
                              alt="Preview"
                            />
                          ) : !imageExists ? (
                            <span className="te-image-missing">File not found</span>
                          ) : null}
                          <span className="te-image-path" title={imagePath}>
                            {imagePath.split(/[/\\]/).pop()}
                          </span>
                        </div>
                      ) : (
                        <span className="te-image-none">No image selected</span>
                      )}
                      <div className="te-image-scale">
                        <label className="form-label">SCALE</label>
                        <div className="te-image-scale-row">
                          <input
                            type="number"
                            className="form-input te-image-scale-input"
                            min={10}
                            max={100}
                            value={imageScale}
                            onChange={e => {
                              const raw = e.target.value;
                              if (raw === '') { setImageScale(''); return; }
                              const v = parseInt(raw, 10);
                              if (!isNaN(v)) setImageScale(Math.min(100, v));
                            }}
                            onBlur={() => {
                              const v = parseInt(imageScale, 10);
                              setImageScale(isNaN(v) || v < 10 ? 10 : Math.min(100, v));
                            }}
                          />
                          <span className="te-image-scale-pct">%</span>
                        </div>
                      </div>
                    </div>
                  )}

                  </div>{/* /te-panel-scroll */}

                  <div className="te-panel-footer">
                    <div className="te-form-actions">
                      <button className="te-save-btn" onClick={handleSave} disabled={!canSave} type="button">
                        Save
                      </button>
                      <button className="te-cancel-btn" onClick={handleCancel} type="button">Cancel</button>
                    </div>
                    <span className="te-paste-note">{expansionType === 'image' ? 'Pastes as image via clipboard' : hasVariants ? 'Shows variant picker on trigger' : 'Pastes as plain text'}</span>
                    <div className="te-form-actions">
                      {!editing.isNew && (
                        <button
                          className="te-delete-confirm-btn"
                          onClick={() => setDeleteConfirm(editing.originalTrigger)}
                          type="button"
                          title="Delete this expansion"
                        >Delete</button>
                      )}
                    </div>
                  </div>
                </>
              ) : (
                <div className="te-panel-idle">
                  <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M12 20h9"/>
                    <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/>
                  </svg>
                  <p>Select an expansion to edit,<br/>or click <strong>+ Add</strong> to create a new one</p>
                </div>
              )}
            </div>
          </div>
          </div>
          </div>
        </>
      )}

      {/* ════════════════════════════════════ AUTOCORRECT VIEW ═════════════════════════════════ */}
      {panelMode === 'autocorrect' && (
        <div className="ac-view">

          {/* ── Built-in library toggle ── */}
          <div className="ac-builtin-row">
            <div className="ac-builtin-info">
              <span className="ac-builtin-label">Built-in corrections</span>
              <span className="ac-builtin-sub">50 common typos — teh→the, recieve→receive, definately→definitely…</span>
            </div>
            <button
              className={`ac-toggle${autocorrectEnabled ? ' ac-toggle-on' : ''}`}
              onClick={onToggleAutocorrect}
              type="button"
              role="switch"
              aria-checked={autocorrectEnabled}
              title={autocorrectEnabled ? 'Disable built-in corrections' : 'Enable built-in corrections'}
            />
          </div>

          {/* ── Custom corrections ── */}
          <div className="ac-section-header">
            <span>Custom Corrections</span>
            <span className="ac-section-count">{autocorrections.length}</span>
          </div>

          {/* Add / Edit form */}
          {acEditing && (
            <div className="ac-form">
              <div className="ac-form-fields">
                <div className="ac-form-col">
                  <label className="form-label">TYPO</label>
                  <input
                    className="form-input ac-field-input"
                    placeholder="recieve"
                    value={acTypo}
                    onChange={e => setAcTypo(e.target.value.replace(/\s/g, ''))}
                    onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') handleAcCancel(); }}
                    autoFocus
                    spellCheck={false}
                  />
                </div>
                <div className="ac-form-arrow">→</div>
                <div className="ac-form-col">
                  <label className="form-label">CORRECTION</label>
                  <input
                    className="form-input ac-field-input"
                    placeholder="receive"
                    value={acCorrection}
                    onChange={e => setAcCorrection(e.target.value)}
                    onKeyDown={e => { e.stopPropagation(); if (e.key === 'Enter') handleAcSave(); if (e.key === 'Escape') handleAcCancel(); }}
                    spellCheck={false}
                  />
                </div>
              </div>
              <div className="ac-form-footer">
                <button className="te-cancel-btn" onClick={handleAcCancel} type="button">Cancel</button>
                <button className="te-save-btn" onClick={handleAcSave} disabled={!canAcSave} type="button">
                  Save
                </button>
              </div>
            </div>
          )}

          {/* Custom corrections list */}
          {sortedAc.length === 0 && !acEditing ? (
            <div className="te-empty-row">
              No custom corrections yet — add your own typo→correction pairs above
            </div>
          ) : (
            <div className="ac-list">
              {sortedAc.map(ac => (
                <div key={ac.typo} className="ac-item">
                  <kbd className="te-trigger-badge ac-typo-badge">{ac.typo}</kbd>
                  <span className="te-item-arrow">→</span>
                  <span className="ac-correction">{ac.correction}</span>
                  <div className="te-item-actions">
                    <button className="te-item-edit" onClick={() => openAcEdit(ac)} type="button">Edit</button>
                    <button className="te-item-delete" onClick={() => onDeleteAutocorrect?.(ac.typo)} type="button">✕</button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* ════════════════════════════════════ GLOBAL VARIABLES VIEW ═══════════════════════════ */}
      {panelMode === 'globalvars' && (
        <div className="gd-view">
          <div className="gd-helper">
            Use <kbd className="te-trigger-badge">{'{{variable.key}}'}</kbd> in any expansion to insert the value automatically.
            Keys are auto-generated from the display title — use the <strong>Insert</strong> button in the expansion editor to pick variables.
          </div>

          {/* Add / Edit form */}
          {gdEditing && (
            <div className="gd-form">
              <div className="gd-form-fields">
                <div className="gd-form-col">
                  <label className="form-label">DISPLAY TITLE</label>
                  <input
                    className={`form-input${gdNameErr ? ' te-input-error' : ''}`}
                    placeholder="e.g. My Full Name"
                    value={gdTitle}
                    onChange={e => {
                      setGdTitle(e.target.value);
                      setGdNameErr('');
                    }}
                    onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') handleGdCancel(); }}
                    autoFocus
                    spellCheck={false}
                  />
                  {gdTitle.trim() && titleToKey(gdTitle.trim()) && (
                    <span className="gd-key-hint">
                      Will be inserted as <kbd className="te-trigger-badge gd-key-badge">{`{{${titleToKey(gdTitle.trim())}}}`}</kbd>
                    </span>
                  )}
                  {gdNameErr && <span className="te-trigger-error">{gdNameErr}</span>}
                </div>
                <div className="gd-form-col gd-form-col-value">
                  <label className="form-label">VALUE</label>
                  <input
                    className="form-input"
                    placeholder="e.g. Jane Smith"
                    value={gdValue}
                    onChange={e => setGdValue(e.target.value)}
                    onKeyDown={e => { e.stopPropagation(); if (e.key === 'Enter') handleGdSave(); if (e.key === 'Escape') handleGdCancel(); }}
                    spellCheck={false}
                  />
                </div>
              </div>
              <div className="ac-form-footer">
                <button className="te-cancel-btn" onClick={handleGdCancel} type="button">Cancel</button>
                <button className="te-save-btn" onClick={handleGdSave} disabled={!canGdSave} type="button">Save</button>
              </div>
            </div>
          )}

          {/* Filled-in variables */}
          {sortedGd.length > 0 && (
            <div className="gd-list">
              {sortedGd.map(([key, value]) => (
                <div key={key} className="gd-item">
                  <span className="gd-item-title">{keyToTitle(key)}</span>
                  <span className="gd-item-value">{value}</span>
                  <div className="te-item-actions">
                    <button className="te-item-edit" onClick={() => openGdEdit(key)} type="button">Edit</button>
                    <button className="te-item-delete" onClick={() => handleGdDelete(key)} type="button">✕</button>
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* Placeholder suggestions */}
          {gdSuggestionsToShow.length > 0 && (
            <div className="gd-suggestions">
              {sortedGd.length > 0 && (
                <div className="gd-suggestions-label">Suggested</div>
              )}
              {gdSuggestionsToShow.map(title => (
                <div
                  key={title}
                  className="gd-item gd-item-placeholder"
                  onClick={() => openGdAdd(title)}
                  title={`Click to add "${title}"`}
                >
                  <span className="gd-item-title gd-placeholder-title">{title}</span>
                  <span className="gd-placeholder-value">— not set</span>
                  <span className="gd-placeholder-cta">+ Add</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Category right-click context menu */}
      {catContextMenu && ReactDOM.createPortal(
        <div
          ref={catContextMenuRef}
          className="profile-ctx-menu"
          style={{ top: catContextMenu.y, left: catContextMenu.x }}
        >
          <button className="profile-ctx-item" onClick={ctxRename}>Rename</button>
          <button className="profile-ctx-item" onClick={ctxChangeColour}>Change Colour</button>
          <button
            className="profile-ctx-item"
            onClick={() => {
              const name = catContextMenu.catName;
              setCatContextMenu(null);
              onExportExpansions?.('category', name);
            }}
          >
            Export Category
          </button>
          <div className="profile-ctx-divider" />
          <button className="profile-ctx-item profile-ctx-delete" onClick={ctxDelete}>
            {ctxDeleteConfirm ? 'Confirm Delete?' : 'Delete'}
          </button>
        </div>,
        document.body
      )}

      {/* Expansion row right-click context menu */}
      {itemContextMenu && ReactDOM.createPortal(
        <div
          ref={itemContextMenuRef}
          className="profile-ctx-menu"
          style={{ top: itemContextMenu.y, left: itemContextMenu.x }}
        >
          <button className="profile-ctx-item" onClick={ctxItemDuplicate}>Duplicate</button>
          <div className="profile-ctx-divider" />
          <button className="profile-ctx-item profile-ctx-delete" onClick={ctxItemDelete}>Delete</button>
        </div>,
        document.body
      )}

      {/* Category colour picker popover */}
      {catColourPopover && ReactDOM.createPortal(
        <div
          ref={catColourPopoverRef}
          className="cat-colour-popover"
          style={{ left: catColourPopover.x, top: catColourPopover.y }}
        >
          <ColourPicker
            value={
              catColourPopover.forCat === '__new__'
                ? newCategoryColour
                : normCategories.find(c => c.name === catColourPopover.forCat)?.colour || null
            }
            onChange={handleCatColourSelect}
          />
        </div>,
        document.body
      )}

      {expansionImportPrompt && (
        <div className="te-delete-overlay">
          <div className="te-delete-dialog te-import-dialog">
            <div className="te-delete-title">Import Expansion Pack</div>
            <p className="te-delete-body">
              This pack contains <strong>{expansionImportPrompt.totalCount}</strong>{' '}
              expansion{expansionImportPrompt.totalCount !== 1 ? 's' : ''}.{' '}
              <strong>{expansionImportPrompt.collisions.length}</strong> trigger
              {expansionImportPrompt.collisions.length !== 1 ? 's' : ''} already
              exist{expansionImportPrompt.collisions.length === 1 ? 's' : ''} in your library:
            </p>
            <div className="te-import-collisions">
              {expansionImportPrompt.collisions.slice(0, 8).map(t => (
                <kbd key={t} className="te-trigger-badge">{t}</kbd>
              ))}
              {expansionImportPrompt.collisions.length > 8 && (
                <span className="te-import-collisions-more">
                  + {expansionImportPrompt.collisions.length - 8} more
                </span>
              )}
            </div>
            <div className="te-delete-actions te-import-actions">
              <button
                className="te-cancel-btn"
                onClick={() => onExpansionImportResolve?.('cancel')}
                type="button"
              >
                Cancel
              </button>
              <button
                className="te-cancel-btn"
                onClick={() => onExpansionImportResolve?.('skip')}
                type="button"
                title="Keep your existing expansions; only import new ones"
              >
                Skip Duplicates
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={() => onExpansionImportResolve?.('overwrite')}
                type="button"
                title="Replace your existing expansions with the ones in this pack"
              >
                Overwrite All
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Delete confirmation dialog */}
      {deleteConfirm && (
        <div className="te-delete-overlay">
          <div className="te-delete-dialog">
            <div className="te-delete-title">Delete Expansion</div>
            <p className="te-delete-body">
              Delete <kbd className="te-trigger-badge">{deleteConfirm}</kbd>? This cannot be undone.
            </p>
            <div className="te-delete-actions">
              <button className="te-cancel-btn" onClick={() => setDeleteConfirm(null)} type="button">
                Cancel
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={() => {
                  onDelete(deleteConfirm);
                  if (editing && !editing.isNew && editing.originalTrigger === deleteConfirm) {
                    setEditing(null);
                  }
                  setDeleteConfirm(null);
                }}
                type="button"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

    </div>
  );
}
