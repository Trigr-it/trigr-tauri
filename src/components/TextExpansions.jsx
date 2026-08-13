import React, { useState, useRef, useLayoutEffect, useEffect, useCallback, useMemo } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import {
  Bold as BoldIcon, Italic as ItalicIcon, Underline as UnderlineIcon,
  List as ListIcon,
  Palette as PaletteIcon, Heading as HeadingIcon,
  Highlighter as HighlighterIcon, Table as TableIcon,
  ArrowUpFromLine as InsertRowAboveIcon,
  ArrowDownFromLine as InsertRowBelowIcon,
  ArrowLeftFromLine as InsertColLeftIcon,
  ArrowRightFromLine as InsertColRightIcon,
  Trash2 as TrashIcon,
  CalendarClock as CalendarClockIcon,
  Clipboard as ClipboardIcon, TextCursor as TextCursorIcon,
  Variable as VariableIcon, Keyboard as KeyboardIcon,
  FormInput as FillInIcon, FunctionSquare as FormulaIcon, Calendar as CalendarPickIcon,
  Blocks as NestedExpansionIcon,
} from 'lucide-react';
import './TextExpansions.css';
import { SearchBar } from './SearchBar';
import { ClipboardExcludedAppsEditor } from './SettingsPanel';
import NumberField from './NumberField';
import { FireTargetPicker } from './MacroPanel';

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
    // Table structure → tab-separated cells, newline-terminated rows so a
    // plain-text paste round-trips as a Word/Excel-compatible table. The
    // caret-sentinel <br> inside empty cells must go FIRST or it becomes a
    // stray newline that splits every row apart in the plain-text output.
    .replace(/<br\s*\/?>(?=<\/t[dh]>)/gi, '')
    .replace(/<\/th>/gi, '\t')
    .replace(/<\/td>/gi, '\t')
    .replace(/<\/tr>/gi, '\n')
    .replace(/<\/table>/gi, '\n')
    .replace(/<t(?:able|body|head|foot|r|d|h)[^>]*>/gi, '')
    .replace(/<div[^>]*>/gi, '')              // 3. opening <div> → nothing
    .replace(/<p[^>]*>/gi, '');               //    opening <p> → nothing
  // Replace token chips with their raw token strings before stripping markup
  tmp.querySelectorAll('[data-token]').forEach(el => {
    el.replaceWith(document.createTextNode(el.dataset.token));
  });
  return (tmp.textContent || tmp.innerText || '')
    .replace(/\u200B/g, '')  // ZWSP cursor anchors after token chips — editor-internal, never inject
    .replace(/\u00A0/g, ' ')     // &nbsp; contenteditable inserts next to chips → plain space
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

// Walk a chunk of editor HTML and return every fillIn LABEL found. Matches both
// the legacy `{fillIn:Label}` shape and typed shapes like
// `{fillIn:Label:dropdown:opts}` — the reusable-chip surface stays meaningful
// across types (label is the only identifier the runtime values map cares about).
function extractFillInLabels(html) {
  if (!html) return [];
  const tmp = document.createElement('div');
  tmp.innerHTML = html;
  const labels = [];
  tmp.querySelectorAll('[data-token]').forEach(el => {
    const t = el.dataset.token || '';
    // Legacy: {fillIn:Label} — entire content is the label
    let m = t.match(/^\{fillIn:([^:}]+)\}$/);
    if (m) { labels.push(m[1]); return; }
    // Typed: {fillIn:Label:...} — label is the first segment before the second colon
    m = t.match(/^\{fillIn:([^:}]+):/);
    if (m) labels.push(m[1]);
  });
  return labels;
}

// Surface every named formula / set variable defined in the snippet so the
// formula reference panel can show them as chips. Two sources:
//   1. Plain-text {set name = …} — typed in directly via the Set variable
//      button. Lives in the editor's text content.
//   2. Chip data-tokens — named formulas built via the Expression popup's
//      Name field produce a {set NAME = …}{=NAME} token, which lives in the
//      chip's data-token attribute (NOT in textContent, since the chip's
//      visible text is just "ƒ name").
// Both paths get swept here.
function extractSetVarNames(html) {
  if (!html) return [];
  const tmp = document.createElement('div');
  tmp.innerHTML = html;
  const out = new Set();
  const re = /\{set\s+([A-Za-z_][A-Za-z0-9_]*)\s*=/g;
  // Pass 1: plain-text occurrences (textContent decodes &nbsp; etc.).
  const text = tmp.textContent || '';
  let m;
  while ((m = re.exec(text)) !== null) out.add(m[1]);
  // Pass 2: chip-embedded named formulas.
  tmp.querySelectorAll('[data-token]').forEach(el => {
    const t = el.dataset.token || '';
    re.lastIndex = 0;
    while ((m = re.exec(t)) !== null) out.add(m[1]);
  });
  // Pass 3: named if-blocks via {ifset NAME …}{endif} chips.
  const ifsetRe = /\{ifset\s+([A-Za-z_][A-Za-z0-9_]*)\s/g;
  tmp.querySelectorAll('[data-token]').forEach(el => {
    const t = el.dataset.token || '';
    ifsetRe.lastIndex = 0;
    let mm;
    while ((mm = ifsetRe.exec(t)) !== null) out.add(mm[1]);
  });
  return Array.from(out);
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
  { type: 'sep' },
  { type: 'header', label: 'Selection (Ctrl+C)' },
  { type: 'item', token: '{selection}',           label: 'Selected Text',           display: 'Selection'  },
  { type: 'item', token: '{selection:uppercase}', label: 'Selection (UPPERCASE)',   display: 'SEL ▲'     },
  { type: 'item', token: '{selection:lowercase}', label: 'Selection (lowercase)',   display: 'sel ▼'     },
  { type: 'item', token: '{selection:trim}',      label: 'Selection (trimmed)',     display: 'Sel ✂'     },
  { type: 'item', token: '{selection:urlencode}', label: 'Selection (URL encode)',  display: 'Sel %'     },
];

const DATETIME_ITEMS = [
  { type: 'header', label: 'Pick a fixed date' },
  { type: 'item', token: '__pick_date__',      label: 'Pick a date…',         display: <CalendarPickIcon size={14} strokeWidth={2} /> },
  { type: 'sep' },
  { type: 'header', label: 'Date' },
  { type: 'item', token: '{date}',             label: 'Date (your default)', display: 'Default'     },
  { type: 'item', token: '{date:DD/MM/YYYY}',  label: 'Date (DD/MM/YYYY)',   display: 'DD/MM/YYYY'  },
  { type: 'item', token: '{date:DD/MM/YY}',    label: 'Date (DD/MM/YY)',     display: 'DD/MM/YY'    },
  { type: 'item', token: '{date:MM/DD/YYYY}',  label: 'Date (MM/DD/YYYY)',   display: 'MM/DD/YYYY'  },
  { type: 'item', token: '{date:YYYY-MM-DD}',  label: 'Date (YYYY-MM-DD)',   display: 'YYYY-MM-DD'  },
  { type: 'item', token: '{date:D MMMM YYYY}', label: 'Date (1 May 2026)',   display: 'D MMMM YYYY' },
  { type: 'sep' },
  { type: 'header', label: 'Date Parts' },
  { type: 'item', token: '{dayofweek}', label: 'Day of Week',  display: 'Day'   },
  { type: 'item', token: '{month}',     label: 'Month Name',   display: 'Month' },
  { type: 'item', token: '{year}',      label: 'Year (YYYY)',  display: 'Year'  },
  { type: 'item', token: '{day}',       label: 'Day of Month', display: 'Day#'  },
  { type: 'sep' },
  { type: 'header', label: 'Time' },
  { type: 'item', token: '{time:HH:MM}',    label: 'Time (HH:MM)',       display: 'HH:MM'    },
  { type: 'item', token: '{time:HH:MM:SS}', label: 'Time (HH:MM:SS)',    display: 'HH:MM:SS' },
  { type: 'item', token: '{isodate}',       label: 'ISO 8601 Date+Time', display: 'ISO Date' },
  { type: 'sep' },
  { type: 'header', label: 'Date Math' },
  { type: 'item', token: '{date:+1d}', label: 'Tomorrow',   display: '+1 day'    },
  { type: 'item', token: '{date:-1d}', label: 'Yesterday',  display: '-1 day'    },
  { type: 'item', token: '{date:+7d}', label: 'Next Week',  display: '+7 days'   },
  { type: 'item', token: '{date:+1m}', label: 'Next Month', display: '+1 month'  },
];

const LIST_ITEMS = [
  { type: 'item', token: '__list_bullet__',   label: 'Bullet List',   display: '•' },
  { type: 'item', token: '__list_numbered__', label: 'Numbered List', display: '1.' },
];

const CURSOR_ITEMS = [
  { type: 'item', token: '{cursor}',    label: 'Cursor Position', display: '↕ Cursor', chipClass: 'cursor' },
];

const FORMULA_ITEMS = [
  { type: 'item', token: '__formula_expr__', label: 'Expression  {=…}',          display: 'ƒ',         chipClass: 'formula' },
  { type: 'item', token: '__formula_set__',  label: 'Set variable  {set …}',     display: '=',         chipClass: 'formula' },
  { type: 'item', token: '__formula_if__',   label: 'If / Else block',           display: '?',         chipClass: 'formula' },
];

const FILLIN_ITEMS = [
  { type: 'item', token: '__fillin__',  label: 'Text Field…',     display: <FillInIcon size={14} strokeWidth={2} />, chipClass: 'fillin' },
  { type: 'sep' },
  { type: 'header', label: 'Typed Fill-ins' },
  { type: 'item', token: '__fillin_multiline__', label: 'Multi-line Text…', display: '▭ Multi',    chipClass: 'fillin' },
  { type: 'item', token: '__fillin_dropdown__',  label: 'Dropdown…',        display: '▭ Dropdown', chipClass: 'fillin' },
  { type: 'item', token: '__fillin_checkbox__',  label: 'Yes/No Toggle…',   display: '▭ Toggle',   chipClass: 'fillin' },
  { type: 'item', token: '__fillin_number__',    label: 'Number…',          display: '▭ Number',   chipClass: 'fillin' },
  { type: 'item', token: '__fillin_date__',      label: 'Date Picker…',     display: '▭ Date',     chipClass: 'fillin' },
];

const INSERT_CATEGORIES = {
  clipboard: { items: CLIPBOARD_ITEMS, label: 'Clipboard',     chipClass: 'clipboard' },
  datetime:  { items: DATETIME_ITEMS,  label: 'Date & Time',   chipClass: 'date'   },
  lists:     { items: LIST_ITEMS,      label: 'Lists',         chipClass: null     },
  cursor:    { items: CURSOR_ITEMS,    label: 'Cursor',        chipClass: 'cursor' },
  fillin:    { items: FILLIN_ITEMS,    label: 'Fill-in Field', chipClass: 'fillin' },
  formula:   { items: FORMULA_ITEMS,   label: 'Formulas',      chipClass: 'formula' },
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

// ── Highlight swatches (for hiliteColor / backColor) ───────────────────────
// Content colours, deliberately not themed — they render the same on any
// paste target. First swatch (null) removes any existing highlight.
const HIGHLIGHT_COLOURS = [
  { hex: null,      label: 'None'    },
  { hex: '#FFF59D', label: 'Yellow'  },
  { hex: '#C8E6C9', label: 'Green'   },
  { hex: '#B3E5FC', label: 'Blue'    },
  { hex: '#F8BBD0', label: 'Pink'    },
  { hex: '#FFCC80', label: 'Orange'  },
  { hex: '#E1BEE7', label: 'Lavender' },
  { hex: '#EF9A9A', label: 'Red'     },
  { hex: '#B0BEC5', label: 'Grey'    },
  { hex: '#FFF176', label: 'Amber'   },
  { hex: '#80CBC4', label: 'Teal'    },
  { hex: '#F5F5F5', label: 'Paper'   },
];

// ── Table inline styles ─────────────────────────────────────────────────────
// Tables must carry their styling as inline style="" attributes, NOT CSS
// classes: the expansion fires by pasting CF_HTML into arbitrary target apps
// (Word, Gmail, eM Client...) and app-stylesheet classes never travel with
// the clipboard. Without inline borders the target renders an invisible,
// collapsed table — which reads as "all my text in one cell". Same rule as
// HTML email. Hardcoded hexes are fine here: this is document content, not
// app theme (same exemption as TEXT_COLOURS).
const TABLE_INLINE_STYLE = 'border-collapse:collapse;table-layout:fixed;';
// 1pt solid black mirrors what Word itself emits when copying a native table
// (`border:solid windowtext 1.0pt`) — the most reliably-parsed border form
// across Word / Outlook / Gmail / eM Client, and it reads as a native table
// in the target. The editor stylesheet overrides the COLOUR in-app (black is
// invisible on the dark theme) — see .rte-editor table td in the CSS.
const CELL_INLINE_STYLE = 'border:1pt solid #000000;padding:4px 8px;vertical-align:top;';
const CELL_DEFAULT_WIDTH = 96; // px — resizable by dragging the cell's right edge

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

function RichTextEditor({ initialHtml, onChange, globalVariables = {}, isPro = false, onShowUpgrade, reusableFillInLabels = [], setVarNames = [], expansions = [], excludeTrigger = null }) {
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
  const [insertCategory, setInsertCategory] = useState(null); // 'clipboard'|'date'|'time'|'datemath'|'cursor'|'variables'|'expansions'|'color'|'highlight'|'table'|'headings'|'lists'|'formula'|'fillin'
  // The nested-expansion picker was a plain sorted list; now a searchable
  // tag-grouped modal reusing FireTargetPicker from MacroPanel (Rory 2026-08-13).
  const [showExpansionPicker, setShowExpansionPicker] = useState(false);
  const [menuPos, setMenuPos] = useState(null);
  // Table-size picker hover state — {rows, cols} of the highlighted top-left
  // rectangle inside the 6×6 grid. null = nothing hovered yet.
  const [tablePickerHover, setTablePickerHover] = useState(null);
  // Whether the caret currently sits inside a <td> / <th> of a table living
  // in this editor. Drives the contextual table-editing toolbar row.
  const [caretInTable, setCaretInTable] = useState(false);
  const [fillInEntry, setFillInEntry] = useState(false);
  const [fillInLabel, setFillInLabel] = useState('');
  // Kind drives the token suffix at insert time. 'text' = legacy `{fillIn:Label}`.
  // Other kinds: multiline / dropdown / checkbox / number / date.
  const [fillInKind, setFillInKind] = useState('text');
  // Comma-separated options string for the 'dropdown' kind only. Ignored for others.
  const [fillInOptionsStr, setFillInOptionsStr] = useState('');
  // Inline entry for a formula expression. `formulaEntry === true` shows the
  // expression input row inside the formulas popup; the expression value is
  // tracked separately so chip edits can prefill it.
  const [formulaEntry, setFormulaEntry] = useState(false);
  const [formulaExpr, setFormulaExpr] = useState('');
  const [formulaName, setFormulaName] = useState('');
  const [formulaEditChip, setFormulaEditChip] = useState(null); // existing chip being edited
  const formulaInputRef = useRef(null);
  // If/Else block popup state.
  const [ifEntry, setIfEntry] = useState(false);
  const [ifCondition, setIfCondition] = useState('');
  const [ifThen, setIfThen] = useState('');
  const [ifElse, setIfElse] = useState('');
  const [ifHasElse, setIfHasElse] = useState(true);
  const [ifName, setIfName] = useState('');
  const [ifEditChip, setIfEditChip] = useState(null);

  // When the user drags the formula / if-else popup by its title bar, we
  // stash the free-form position here. While non-null AND the popup category
  // is 'formula' (only formula/if-else render the drag handle), this
  // overrides the toolbar-button anchoring and the auto-flip-upward logic.
  //
  // Persisted to localStorage so subsequent formula sessions open in the
  // same place — Rory's testers work through formulas in batches. Double-
  // clicking the drag handle resets. On load, any saved position that would
  // fall off the current viewport (window resized smaller since save) is
  // discarded.
  const [userDraggedPos, setUserDraggedPos] = useState(() => {
    try {
      const stored = localStorage.getItem('trigr.te.formulaPopupPos');
      if (!stored) return null;
      const parsed = JSON.parse(stored);
      if (!parsed || typeof parsed.top !== 'number' || typeof parsed.left !== 'number') return null;
      if (parsed.top < 0 || parsed.top > window.innerHeight - 40) return null;
      if (parsed.left < -400 || parsed.left > window.innerWidth - 40) return null;
      return parsed;
    } catch { return null; }
  });
  useEffect(() => {
    try {
      if (userDraggedPos) {
        localStorage.setItem('trigr.te.formulaPopupPos', JSON.stringify(userDraggedPos));
      } else {
        localStorage.removeItem('trigr.te.formulaPopupPos');
      }
    } catch {}
  }, [userDraggedPos]);
  const ifConditionRef = useRef(null);
  const ifThenRef = useRef(null);
  const ifElseRef = useRef(null);
  // Tracks which If/Else field last had focus — clicked chips insert into
  // this field. Defaults to Condition since that's where formula references
  // are most often used.
  const [ifActiveField, setIfActiveField] = useState('condition');
  const fillInInputRef = useRef(null);
  // Inline rename popover for fill-in chips clicked in the editor body.
  // For typed chips (kind === 'dropdown' etc.) the popover also exposes the
  // type-specific knobs — options for dropdown, currently. Default-value
  // editing is deferred to a later iteration; tokens with a default suffix
  // are preserved verbatim through the rename.
  const [fillInRename, setFillInRename] = useState(null); // { label, x, y, kind, optionsStr, defaultSuffix }
  const [fillInRenameValue, setFillInRenameValue] = useState('');
  const [fillInRenameOptions, setFillInRenameOptions] = useState('');
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

  function applyHighlight(hex) {
    editorRef.current?.focus();
    restoreSelection();
    // A null hex clears the highlight — hiliteColor 'transparent' works in
    // Chromium and produces `<span style="background-color: transparent">`
    // which the browser collapses when re-parsed.
    const value = hex || 'transparent';
    try { document.execCommand('hiliteColor', false, value); } catch {
      try { document.execCommand('backColor', false, value); } catch {}
    }
    notify();
    setShowInsert(false);
    setInsertCategory(null);
  }

  // Insert a fresh <table rows × cols> at the caret. Cells contain a <br> so
  // the browser gives each one a caret target (empty <td> is uneditable).
  // All styling is INLINE (see TABLE_INLINE_STYLE) so it survives the CF_HTML
  // paste into Word / Gmail / eM Client — classes don't travel with the
  // clipboard.
  function insertTable(rows, cols) {
    if (!rows || !cols || rows < 1 || cols < 1) return;
    editorRef.current?.focus();
    restoreSelection();

    const table = document.createElement('table');
    table.className = 'rte-inserted-table';
    table.setAttribute('style', TABLE_INLINE_STYLE);
    const tbody = document.createElement('tbody');
    for (let r = 0; r < rows; r++) {
      const tr = document.createElement('tr');
      for (let c = 0; c < cols; c++) tr.appendChild(buildEmptyCell());
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);

    const sel = window.getSelection();
    if (sel && sel.rangeCount > 0 && editorRef.current?.contains(sel.anchorNode)) {
      const range = sel.getRangeAt(0);
      range.deleteContents();
      range.insertNode(table);
      // Place the caret in the first cell so the user can start typing.
      const firstCell = table.querySelector('td');
      if (firstCell) {
        const newRange = document.createRange();
        newRange.selectNodeContents(firstCell);
        newRange.collapse(true);
        sel.removeAllRanges();
        sel.addRange(newRange);
      }
      // Give the table a trailing paragraph so the caret can escape below it.
      if (!table.nextSibling) {
        const trail = document.createElement('p');
        trail.appendChild(document.createElement('br'));
        table.parentNode.insertBefore(trail, table.nextSibling);
      }
    } else {
      editorRef.current.appendChild(table);
    }

    notify();
    setShowInsert(false);
    setInsertCategory(null);
    setTablePickerHover(null);
    savedRangeRef.current = null;
    // Land the caret inside the freshly-inserted table so the contextual
    // toolbar appears immediately without an extra click.
    setCaretInTable(true);
  }

  // Return the <td>/<th> the caret currently sits inside (or null). Shared
  // by the Tab-navigation, contextual-toolbar detection, and every table
  // edit operation below.
  function getCurrentTableCell() {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return null;
    const node = sel.anchorNode;
    if (!node) return null;
    const cell = (node.nodeType === 1 ? node : node.parentElement)?.closest?.('td, th');
    if (!cell || !editorRef.current?.contains(cell)) return null;
    return cell;
  }

  // Refresh the caret-in-table state. Called from every editor event that
  // could move the caret (keyup / click / focus) so the contextual toolbar
  // reliably tracks the caret across mouse, keyboard, and post-op landings.
  function refreshTableContext() {
    setCaretInTable(!!getCurrentTableCell());
  }

  // Place the caret at the start of a cell after a structural edit. Keeps
  // the editing flow moving without the user having to re-click.
  function focusCell(cell) {
    if (!cell) return;
    const range = document.createRange();
    range.selectNodeContents(cell);
    range.collapse(true);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    editorRef.current?.focus();
  }

  function buildEmptyCell(widthPx) {
    const td = document.createElement('td');
    td.setAttribute('style', `${CELL_INLINE_STYLE}width:${widthPx || CELL_DEFAULT_WIDTH}px;`);
    td.appendChild(document.createElement('br'));
    return td;
  }

  // Build a fresh empty row matching an existing row's column count AND
  // per-column widths, so inserted rows line up with resized columns.
  function buildRowLike(refRow) {
    const tr = document.createElement('tr');
    Array.from(refRow.children).forEach(refCell => {
      const w = parseInt(refCell.style?.width, 10);
      tr.appendChild(buildEmptyCell(Number.isFinite(w) && w > 0 ? w : undefined));
    });
    return tr;
  }

  // Move the caret to the neighbouring cell in the current <table>. Called
  // from the editor's onKeyDown when Tab is pressed inside a cell.
  // direction: +1 = next cell, -1 = previous cell. Tab from the final cell
  // appends a new row and lands the caret in its first cell.
  function moveToAdjacentCell(direction) {
    const cell = getCurrentTableCell();
    if (!cell) return false;

    const table = cell.closest('table');
    if (!table) return false;
    const cells = Array.from(table.querySelectorAll('td, th'));
    const idx = cells.indexOf(cell);
    if (idx === -1) return false;

    let target;
    if (direction > 0) {
      if (idx === cells.length - 1) {
        // Last cell — append a new row and land in its first cell.
        const tr = buildRowLike(cell.parentElement);
        (table.querySelector('tbody') || table).appendChild(tr);
        target = tr.querySelector('td');
      } else {
        target = cells[idx + 1];
      }
    } else {
      if (idx === 0) return true; // Shift+Tab in first cell — swallow, no-op.
      target = cells[idx - 1];
    }

    if (target) {
      focusCell(target);
      notify();
      refreshTableContext();
    }
    return true;
  }

  // ── Table structure edits — all guarded by "caret is in a cell of a
  //    table inside this editor". Any op that would leave the table with
  //    zero rows or zero columns removes the whole table instead.

  function tableInsertRow(direction) {
    const cell = getCurrentTableCell();
    if (!cell) return;
    const row = cell.parentElement;
    if (!row) return;
    const tr = buildRowLike(row);
    row.parentElement.insertBefore(tr, direction === 'above' ? row : row.nextSibling);
    focusCell(tr.children[0]);
    notify();
    refreshTableContext();
  }

  function tableInsertColumn(direction) {
    const cell = getCurrentTableCell();
    if (!cell) return;
    const row = cell.parentElement;
    if (!row) return;
    const colIdx = Array.from(row.children).indexOf(cell);
    if (colIdx === -1) return;
    const table = cell.closest('table');
    if (!table) return;
    let landingCell = null;
    table.querySelectorAll('tr').forEach(tr => {
      const target = tr.children[colIdx];
      const fresh = buildEmptyCell();
      if (direction === 'left') {
        tr.insertBefore(fresh, target || null);
      } else {
        tr.insertBefore(fresh, target ? target.nextSibling : null);
      }
      if (tr === row) landingCell = fresh;
    });
    focusCell(landingCell);
    notify();
    refreshTableContext();
  }

  function tableDeleteRow() {
    const cell = getCurrentTableCell();
    if (!cell) return;
    const row = cell.parentElement;
    const table = cell.closest('table');
    if (!row || !table) return;
    const totalRows = table.querySelectorAll('tr').length;
    if (totalRows <= 1) { tableDeleteEntire(); return; }
    // Land the caret in the neighbouring row so editing continues smoothly.
    const nextRow = row.nextElementSibling || row.previousElementSibling;
    row.remove();
    if (nextRow?.children?.[0]) focusCell(nextRow.children[0]);
    notify();
    refreshTableContext();
  }

  function tableDeleteColumn() {
    const cell = getCurrentTableCell();
    if (!cell) return;
    const row = cell.parentElement;
    const table = cell.closest('table');
    if (!row || !table) return;
    const colIdx = Array.from(row.children).indexOf(cell);
    if (colIdx === -1) return;
    const colCount = row.children.length;
    if (colCount <= 1) { tableDeleteEntire(); return; }
    let landingCell = null;
    table.querySelectorAll('tr').forEach(tr => {
      const victim = tr.children[colIdx];
      if (!victim) return;
      const neighbour = victim.nextElementSibling || victim.previousElementSibling;
      victim.remove();
      if (tr === row) landingCell = neighbour;
    });
    focusCell(landingCell);
    notify();
    refreshTableContext();
  }

  function tableDeleteEntire() {
    const cell = getCurrentTableCell();
    if (!cell) return;
    const table = cell.closest('table');
    if (!table) return;
    // Leave the caret where the table used to be — insert an empty paragraph
    // if the table was the only child so contenteditable still has something
    // to hold the caret.
    const parent = table.parentElement;
    const anchor = document.createElement('p');
    anchor.appendChild(document.createElement('br'));
    parent.insertBefore(anchor, table);
    table.remove();
    focusCell(anchor);
    notify();
    refreshTableContext();
  }

  // ── Column resize — drag a cell's right edge. Widths are written as inline
  //    styles on EVERY cell in the column so they (a) survive save/reload and
  //    (b) travel with the CF_HTML paste into the target app.

  const RESIZE_ZONE_PX = 6;
  const tableResizeRef = useRef(null); // { colCells, startX, startWidth } while dragging

  // Return the cell whose right edge the pointer is within RESIZE_ZONE_PX of.
  function cellResizeEdgeHit(e) {
    const cell = e.target.closest?.('td, th');
    if (!cell || !editorRef.current?.contains(cell)) return null;
    const rect = cell.getBoundingClientRect();
    return rect.right - e.clientX <= RESIZE_ZONE_PX ? cell : null;
  }

  function handleEditorMouseMove(e) {
    if (tableResizeRef.current) return; // mid-drag — document listeners own the pointer
    if (!editorRef.current) return;
    editorRef.current.style.cursor = cellResizeEdgeHit(e) ? 'col-resize' : '';
  }

  function handleEditorMouseDown(e) {
    const cell = cellResizeEdgeHit(e);
    if (!cell) return;
    e.preventDefault(); // keep the caret where it is — this press is a resize, not a click
    const row = cell.parentElement;
    const table = cell.closest('table');
    if (!row || !table) return;
    const colIdx = Array.from(row.children).indexOf(cell);
    const colCells = Array.from(table.querySelectorAll('tr'))
      .map(tr => tr.children[colIdx])
      .filter(Boolean);
    tableResizeRef.current = {
      colCells,
      startX: e.clientX,
      startWidth: cell.getBoundingClientRect().width,
    };
    const onMove = ev => {
      const st = tableResizeRef.current;
      if (!st) return;
      const w = Math.max(24, Math.round(st.startWidth + (ev.clientX - st.startX)));
      st.colCells.forEach(c => { c.style.width = `${w}px`; });
    };
    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      tableResizeRef.current = null;
      if (editorRef.current) editorRef.current.style.cursor = '';
      notify(); // persist the new widths into the saved HTML
    };
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
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

  // Map typed-fillin sentinels onto the canonical kind string consumed by
  // insertFillInToken. The 'text' kind is the legacy single-input path.
  const FILLIN_SENTINELS = {
    '__fillin__':            'text',
    '__fillin_multiline__':  'multiline',
    '__fillin_dropdown__':   'dropdown',
    '__fillin_checkbox__':   'checkbox',
    '__fillin_number__':     'number',
    '__fillin_date__':       'date',
  };

  function handleInsertItem(e, item) {
    e.preventDefault();
    if (FILLIN_SENTINELS[item.token]) {
      setFillInKind(FILLIN_SENTINELS[item.token]);
      setFillInOptionsStr('');
      setFillInEntry(true);
      setFillInLabel('');
      return;
    }
    if (item.token === '__formula_expr__') {
      if (!isPro) { onShowUpgrade?.('Formula expressions'); return; }
      // Always reset ALL formula state when opening the popup fresh from the
      // toolbar — otherwise leftover state from a previous edit-chip session
      // (formulaName, formulaEditChip) would either bleed into the new entry
      // or, worse, cause Insert to mutate the previously-edited chip instead
      // of inserting a new one.
      setFormulaEntry(true);
      setFormulaExpr('');
      setFormulaName('');
      setFormulaEditChip(null);
      setTimeout(() => formulaInputRef.current?.focus(), 0);
      return;
    }
    if (item.token === '__formula_if__') {
      if (!isPro) { onShowUpgrade?.('Conditional blocks'); return; }
      // Same reset story as above — clear ifName + ifEditChip so a fresh
      // toolbar insert doesn't reopen the last-edited chip's state.
      setIfEntry(true);
      setIfCondition('');
      setIfThen('');
      setIfElse('');
      setIfHasElse(true);
      setIfName('');
      setIfEditChip(null);
      setTimeout(() => ifConditionRef.current?.focus(), 0);
      return;
    }
    if (item.token === '__pick_date__') {
      // Shortcut to creating a date-typed fill-in. The user is firing the
      // expansion picks the date at fire time via the native calendar in
      // the fill-in window; the snippet stores a {fillIn:Label:date} token.
      // Same as Fill-in field → Date Picker… but located in the Date & Time
      // dropdown for discoverability.
      setFillInKind('date');
      setFillInOptionsStr('');
      setFillInEntry(true);
      setFillInLabel('');
      setTimeout(() => fillInInputRef.current?.focus(), 0);
      return;
    }
    if (item.token === '__formula_set__') {
      if (!isPro) { onShowUpgrade?.('Set variables'); return; }
      // Insert a template the user edits in place. The token is invisible at
      // fire time (zero-width substitution) — its only purpose is to let
      // later {=…} / {if …} tokens reference the named result.
      insertTextAtCursor('{set name = expression}');
      setShowInsert(false);
      setInsertCategory(null);
      return;
    }
    if (item.token === '__list_bullet__') {
      // Restore the saved selection before invoking execCommand, otherwise
      // contentEditable converts the wrong line. Same pattern as the legacy
      // direct-format buttons that lived in the toolbar before consolidation.
      restoreSelection();
      format('insertUnorderedList');
      setShowInsert(false);
      setInsertCategory(null);
      return;
    }
    if (item.token === '__list_numbered__') {
      restoreSelection();
      format('insertOrderedList');
      setShowInsert(false);
      setInsertCategory(null);
      return;
    }
    insertTokenHtml(item.token, item.display);
    setShowInsert(false);
    setInsertCategory(null);
  }

  // Build a formula chip from the expression. Display abbreviates long
  // expressions so the chip stays readable in the editor; the full text
  // round-trips through data-token. Named formulas show "ƒ name" since the
  // name is the user's mental anchor.
  function buildFormulaChipDisplay(expr, name) {
    const trimmed = (expr || '').trim();
    const cleanName = (name || '').trim();
    if (cleanName) return `ƒ ${cleanName}`;
    const abbreviated = trimmed.length > 24 ? trimmed.slice(0, 22) + '…' : trimmed;
    return `ƒ ${abbreviated}`;
  }

  // Named formulas store both a {set} definition and a {=name} render in a
  // single chip so the user sees the value AND has a reusable name. Anonymous
  // formulas keep the simple {=expr} shape. Both round-trip through this
  // pair of build / parse helpers.
  function buildFormulaToken(expr, name) {
    const cleanExpr = (expr || '').trim();
    const cleanName = (name || '').trim();
    if (cleanName && /^[A-Za-z_][A-Za-z0-9_]*$/.test(cleanName)) {
      return `{set ${cleanName} = ${cleanExpr}}{=${cleanName}}`;
    }
    return `{=${cleanExpr}}`;
  }

  function parseFormulaToken(token) {
    if (!token) return null;
    // Named: {set NAME = EXPR}{=NAME}
    const named = token.match(/^\{set\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([\s\S]*?)\}\{=\1\}$/);
    if (named) return { name: named[1], expr: named[2] };
    // Anonymous: {=EXPR}
    if (token.startsWith('{=') && token.endsWith('}')) {
      return { name: '', expr: token.slice(2, -1) };
    }
    return null;
  }

  // Insert a chip name (fill-in label, var name, reserved word) at the current
  // textarea cursor position so the user can build expressions by clicking
  // rather than retyping field names from memory.
  function insertIntoFormula(text) {
    const el = formulaInputRef.current;
    if (!el) {
      setFormulaExpr(prev => prev + text);
      return;
    }
    const start = el.selectionStart ?? formulaExpr.length;
    const end = el.selectionEnd ?? formulaExpr.length;
    const next = formulaExpr.slice(0, start) + text + formulaExpr.slice(end);
    setFormulaExpr(next);
    const caret = start + text.length;
    setTimeout(() => {
      el.focus();
      try { el.setSelectionRange(caret, caret); } catch (_) {}
    }, 0);
  }

  function commitFormulaEntry() {
    const expr = formulaExpr.trim();
    if (!expr) { setFormulaEntry(false); return; }
    const name = formulaName.trim();
    if (name && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      // Invalid identifier — refuse to commit. Inline error shown via title
      // attribute; user fixes and retries.
      return;
    }
    const token = buildFormulaToken(expr, name);
    const display = buildFormulaChipDisplay(expr, name);
    if (formulaEditChip) {
      // Edit existing chip in place rather than inserting a new one.
      formulaEditChip.setAttribute('data-token', token);
      formulaEditChip.textContent = display;
      notify();
    } else {
      insertTokenHtml(token, display);
    }
    setFormulaEntry(false);
    setFormulaExpr('');
    setFormulaName('');
    setFormulaEditChip(null);
    setShowInsert(false);
    setInsertCategory(null);
  }

  // Insert chip text into whichever If/Else field last held focus. Mirrors
  // the formula popup's insertIntoFormula behaviour so chip clicks feel
  // identical across both editors.
  function insertIntoIfField(text) {
    const target =
      ifActiveField === 'then' ? ifThenRef.current :
      ifActiveField === 'else' ? ifElseRef.current :
      ifConditionRef.current;
    const setter =
      ifActiveField === 'then' ? setIfThen :
      ifActiveField === 'else' ? setIfElse :
      setIfCondition;
    const current =
      ifActiveField === 'then' ? ifThen :
      ifActiveField === 'else' ? ifElse :
      ifCondition;
    if (!target) {
      setter(current + text);
      return;
    }
    const start = target.selectionStart ?? current.length;
    const end = target.selectionEnd ?? current.length;
    const next = current.slice(0, start) + text + current.slice(end);
    setter(next);
    const caret = start + text.length;
    setTimeout(() => {
      target.focus();
      try { target.setSelectionRange(caret, caret); } catch (_) {}
    }, 0);
  }

  function buildIfToken(condition, thenText, elseText, hasElse, name) {
    const cleanName = (name || '').trim();
    const named = cleanName && /^[A-Za-z_][A-Za-z0-9_]*$/.test(cleanName);
    const opener = named ? `{ifset ${cleanName} ${condition}}` : `{if ${condition}}`;
    return hasElse
      ? `${opener}${thenText}{else}${elseText}{endif}`
      : `${opener}${thenText}{endif}`;
  }

  function buildIfChipDisplay(condition, name) {
    const cleanName = (name || '').trim();
    if (cleanName) return cleanName;
    const trimmed = (condition || '').trim();
    return trimmed.length > 20 ? trimmed.slice(0, 18) + '…' : trimmed;
  }

  // Parse a stored if-block token back into editable fields. Handles both
  // anonymous {if cond}…{endif} and named {ifset NAME cond}…{endif} shapes.
  // V1 assumes a non-nested block — power users can still edit nested chains
  // by clicking the chip's data-token directly.
  function parseIfToken(token) {
    if (!token || !token.endsWith('{endif}')) return null;
    let name = '';
    let headerStart;
    if (token.startsWith('{ifset ')) {
      const nameAndCondStart = 7;
      const spaceIdx = token.indexOf(' ', nameAndCondStart);
      if (spaceIdx < 0) return null;
      name = token.slice(nameAndCondStart, spaceIdx);
      headerStart = spaceIdx + 1;
    } else if (token.startsWith('{if ')) {
      headerStart = 4;
    } else {
      return null;
    }
    const condEnd = token.indexOf('}', headerStart);
    if (condEnd < 0) return null;
    const condition = token.slice(headerStart, condEnd);
    const inner = token.slice(condEnd + 1, token.length - '{endif}'.length);
    const elseIdx = inner.indexOf('{else}');
    if (elseIdx >= 0) {
      return {
        name,
        condition,
        thenText: inner.slice(0, elseIdx),
        elseText: inner.slice(elseIdx + '{else}'.length),
        hasElse: true,
      };
    }
    return { name, condition, thenText: inner, elseText: '', hasElse: false };
  }

  function commitIfEntry() {
    const cond = ifCondition.trim();
    if (!cond) { setIfEntry(false); return; }
    const name = ifName.trim();
    if (name && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return;
    const token = buildIfToken(cond, ifThen || '', ifElse || '', ifHasElse, name);
    const display = buildIfChipDisplay(cond, name);
    if (ifEditChip) {
      const stillInDom = !!ifEditChip.parentNode && editorRef.current?.contains(ifEditChip);
      if (!stillInDom) {
        insertTokenHtml(token, display);
      } else {
        ifEditChip.setAttribute('data-token', token);
        ifEditChip.textContent = display;
        notify();
      }
    } else {
      insertTokenHtml(token, display);
    }
    setIfEntry(false);
    setIfCondition('');
    setIfThen('');
    setIfElse('');
    setIfHasElse(true);
    setIfName('');
    setIfEditChip(null);
    setShowInsert(false);
    setInsertCategory(null);
  }

  // Plain text insert at saved cursor position — used for the {if}{endif}
  // template. Mirrors the pattern used by insertTokenHtml but skips the chip
  // wrapping so the result is editable inline as raw text.
  function insertTextAtCursor(text) {
    const range = savedRangeRef.current;
    if (!range || !editorRef.current?.contains(range.startContainer)) {
      // Fall back: append at end of editor.
      editorRef.current?.focus();
      const sel = window.getSelection();
      sel?.selectAllChildren(editorRef.current);
      sel?.collapseToEnd();
      document.execCommand('insertText', false, text);
    } else {
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
      document.execCommand('insertText', false, text);
    }
    notify();
    savedRangeRef.current = null;
  }

  // Build the appropriate token from the entry state. Legacy bare `{fillIn:Label}`
  // produced when kind is 'text' so existing snippets and the reusable-chip path
  // remain byte-identical.
  function buildFillInToken(label, kind, optionsStr) {
    if (kind === 'text' || !kind) {
      return `{fillIn:${label}}`;
    }
    if (kind === 'dropdown') {
      const opts = optionsStr
        .split(',')
        .map(s => s.trim())
        .filter(Boolean)
        .join(',');
      // Bare `{fillIn:Label:dropdown}` with no options is still valid — backend
      // falls back to a text input. Users can add options later by editing.
      return opts
        ? `{fillIn:${label}:dropdown:${opts}}`
        : `{fillIn:${label}:dropdown}`;
    }
    return `{fillIn:${label}:${kind}}`;
  }

  // Chip display text. Plain `▭ Label` for the text kind (matches legacy),
  // `▭ Label · type` for typed kinds so the editor surfaces which type the
  // user picked without showing the full token.
  function buildFillInChipDisplay(label, kind) {
    if (kind === 'text' || !kind) return `▭ ${label}`;
    return `▭ ${label} · ${kind}`;
  }

  function insertFillInToken(label, kindOverride, optionsOverride) {
    const kind = kindOverride ?? fillInKind;
    const opts = optionsOverride ?? fillInOptionsStr;
    const token = buildFillInToken(label, kind, opts);
    const display = buildFillInChipDisplay(label, kind);
    insertTokenHtml(token, display);
    setFillInEntry(false);
    setFillInLabel('');
    setFillInKind('text');
    setFillInOptionsStr('');
    setShowInsert(false);
    setInsertCategory(null);
  }

  function commitFillInRename() {
    if (!fillInRename) return;
    const oldLabel = fillInRename.label;
    const newLabel = fillInRenameValue.trim() || oldLabel;
    const renameKind = fillInRename.kind || 'text';
    const renameDefaultSuffix = fillInRename.defaultSuffix || '';
    // For dropdown chips the popover also edits options. Normalise comma input
    // (trim, drop empty) so the resulting token is clean even if the user
    // typed trailing commas or extra spaces.
    const newOpts = renameKind === 'dropdown'
      ? fillInRenameOptions
          .split(',')
          .map(s => s.trim())
          .filter(Boolean)
          .join(',')
      : '';
    setFillInRename(null);
    setFillInRenameValue('');
    setFillInRenameOptions('');
    if (!editorRef.current) return;

    // Build the canonical new token for THE clicked chip's type. The token
    // applies to every chip referencing the same label in this editor so the
    // user's rename / options change propagates everywhere that field appears.
    const buildNewToken = () => {
      if (renameKind === 'text') return `{fillIn:${newLabel}}${''}`;
      if (renameKind === 'dropdown') {
        return newOpts
          ? `{fillIn:${newLabel}:dropdown:${newOpts}${renameDefaultSuffix}}`
          : `{fillIn:${newLabel}:dropdown${renameDefaultSuffix}}`;
      }
      return `{fillIn:${newLabel}:${renameKind}${renameDefaultSuffix}}`;
    };
    const newToken = buildNewToken();
    const newChipText = renameKind === 'text' ? `▭ ${newLabel}` : `▭ ${newLabel} · ${renameKind}`;

    const chips = editorRef.current.querySelectorAll('[data-token]');
    const oldBare = `{fillIn:${oldLabel}}`;
    const oldTypedPrefix = `{fillIn:${oldLabel}:`;
    chips.forEach(chip => {
      const tok = chip.dataset.token || '';
      if (tok === oldBare || tok.startsWith(oldTypedPrefix)) {
        chip.setAttribute('data-token', newToken);
        chip.textContent = newChipText;
      }
    });
    notify();
  }

  function cancelFillInRename() {
    setFillInRename(null);
    setFillInRenameValue('');
    setFillInRenameOptions('');
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
    // Reset every sub-editor state when opening a category menu from the
    // toolbar. Without this, a popup still in chip-edit mode (formulaEntry
    // or ifEntry truthy) hides the dropdown items behind its edit form and
    // the user thinks the toolbar is "stuck" on the previous chip.
    setFillInEntry(false);
    setFillInLabel('');
    setFormulaEntry(false);
    setFormulaExpr('');
    setFormulaName('');
    setFormulaEditChip(null);
    setIfEntry(false);
    setIfCondition('');
    setIfThen('');
    setIfElse('');
    setIfHasElse(true);
    setIfName('');
    setIfEditChip(null);
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
    setFillInKind('text');
    setFillInOptionsStr('');
    setFormulaEntry(false);
    setFormulaExpr('');
    setFormulaEditChip(null);
  }

  // Position the popup vertically so it never clips. Re-runs whenever a
  // sub-editor open/close state changes (formula, if-block, fill-in, etc.)
  // because those expand the popup's content significantly. We measure the
  // NATURAL height first (clear inline maxHeight + position), decide which
  // way to open, then apply maxHeight to fit. A ResizeObserver here would
  // feedback-loop — applying maxHeight changes the measured height, which
  // would re-trigger the observer.
  useLayoutEffect(() => {
    if (!(showInsert && menuPos && menuRef.current)) return;
    const popup = menuRef.current;
    const margin = 8;

    // User has dragged the popup by its title bar. Skip anchor-based flip
    // logic entirely; React's inline style prop supplies top/left. We only
    // cap maxHeight so the popup can't run past the viewport bottom.
    // Only applies while a sub-editor (formulaEntry / ifEntry) is active —
    // the drag handle only appears there. The category LIST view (clicking
    // the formula toolbar icon) still anchors to the toolbar button.
    if (userDraggedPos && (formulaEntry || ifEntry)) {
      const maxH = Math.max(120, window.innerHeight - userDraggedPos.top - margin);
      popup.style.maxHeight = `${maxH}px`;
      return;
    }

    // Clear previous inline placement so we measure the popup's intrinsic
    // height (the height it WANTS to be) before deciding which way to open.
    popup.style.maxHeight = '';
    popup.style.top = '';

    const rect = popup.getBoundingClientRect();
    const availableBelow = window.innerHeight - menuPos.top - margin;
    const availableAbove = Math.max(0, menuPos.btnTop - margin - 4);
    let top;
    let maxH;
    if (rect.height > availableBelow && availableAbove > availableBelow) {
      top = Math.max(margin, menuPos.btnTop - Math.min(rect.height, availableAbove) - 4);
      maxH = availableAbove;
    } else {
      top = menuPos.top;
      maxH = availableBelow;
    }
    popup.style.top = `${top}px`;
    popup.style.maxHeight = `${maxH}px`;

    if (!menuPos.anchorRight) {
      let left = menuPos.left;
      if (rect.right > window.innerWidth - margin) {
        left = menuPos.btnRight - rect.width;
      }
      popup.style.left = `${Math.max(margin, left)}px`;
    }
  }, [
    showInsert, menuPos, insertCategory,
    // Sub-editor visibility toggles — each adds significant height to the popup.
    fillInEntry, fillInKind, formulaEntry, ifEntry, ifHasElse,
    // Reference-panel chip count drives the "Named references" section height.
    reusableFillInLabels.length, setVarNames.length,
    // User-drag overrides anchor + auto-flip.
    userDraggedPos,
  ]);

  // Grab the popup by its title bar and reposition to follow the cursor.
  // Clamps so at least ~40px of the popup stays on-screen on every side.
  function handlePopupDragStart(e) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const popup = menuRef.current;
    if (!popup) return;
    const rect = popup.getBoundingClientRect();
    const grabX = e.clientX - rect.left;
    const grabY = e.clientY - rect.top;
    document.body.style.cursor = 'grabbing';
    document.body.style.userSelect = 'none';

    function onMove(ev) {
      const rawTop = ev.clientY - grabY;
      const rawLeft = ev.clientX - grabX;
      // Keep at least 40px on-screen so the popup is always grabbable.
      const minLeft = 40 - rect.width;
      const minTop = 8;
      const maxLeft = window.innerWidth - 40;
      const maxTop = window.innerHeight - 40;
      const top = Math.max(minTop, Math.min(maxTop, rawTop));
      const left = Math.max(minLeft, Math.min(maxLeft, rawLeft));
      setUserDraggedPos({ top: Math.round(top), left: Math.round(left) });
    }
    function onUp() {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    }
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  }

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
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'highlight' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'highlight')}
          title="Highlight colour"
        ><HighlighterIcon size={14} strokeWidth={2} /></button>
        <div className="rte-sep" />
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'headings' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'headings')}
          title="Heading style"
        ><HeadingIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'lists' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'lists')}
          title="Bullet or numbered list"
        ><ListIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'table' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => {
            setTablePickerHover(null);
            openCategoryMenu(e, 'table');
          }}
          title="Insert table"
        ><TableIcon size={14} strokeWidth={2} /></button>

        <div className="rte-sep" />

        {/* ── Category dropdowns ── */}
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'datetime' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'datetime')}
          title="Insert date, time, or date math"
        ><CalendarClockIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'clipboard' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'clipboard')}
          title="Insert clipboard contents"
        ><ClipboardIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'fillin' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'fillin')}
          title="Insert fill-in form field"
        ><FillInIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'formula' ? ' rte-btn-on' : ''}${!isPro ? ' rte-btn-pro-locked' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'formula')}
          title={isPro ? 'Insert formula or conditional block' : 'Formulas (Pro)'}
        ><FormulaIcon size={14} strokeWidth={2} /></button>
        <button
          type="button"
          className={`rte-btn${showInsert && insertCategory === 'cursor' ? ' rte-btn-on' : ''}`}
          onMouseDown={e => openCategoryMenu(e, 'cursor')}
          title="Insert cursor position marker"
        ><TextCursorIcon size={14} strokeWidth={2} /></button>
        {Object.keys(globalVariables).length > 0 && (
          <button
            type="button"
            className={`rte-btn${showInsert && insertCategory === 'variables' ? ' rte-btn-on' : ''}`}
            onMouseDown={e => openCategoryMenu(e, 'variables')}
            title="Insert global variable"
          ><VariableIcon size={14} strokeWidth={2} /></button>
        )}
        {expansions.filter(e => !excludeTrigger || e.trigger !== excludeTrigger).length > 0 && (
          <button
            type="button"
            className={`rte-btn${showExpansionPicker ? ' rte-btn-on' : ''}`}
            onMouseDown={e => {
              e.preventDefault();
              saveSelection();
              setShowInsert(false);
              setInsertCategory(null);
              setShowExpansionPicker(true);
            }}
            title="Insert another expansion"
          ><NestedExpansionIcon size={14} strokeWidth={2} /></button>
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

      {/* Contextual table-edit toolbar. Only visible when the caret sits in a
          <td>/<th> inside this editor. Buttons use onMouseDown w/ preventDefault
          so the caret stays in the target cell (avoids the click stealing focus
          and dropping the selection before the handler runs). */}
      {caretInTable && (
        <div className="rte-toolbar rte-table-toolbar">
          <button
            type="button"
            className="rte-btn"
            onMouseDown={e => { e.preventDefault(); tableInsertRow('above'); }}
            title="Insert row above"
          ><InsertRowAboveIcon size={14} strokeWidth={2} /></button>
          <button
            type="button"
            className="rte-btn"
            onMouseDown={e => { e.preventDefault(); tableInsertRow('below'); }}
            title="Insert row below"
          ><InsertRowBelowIcon size={14} strokeWidth={2} /></button>
          <button
            type="button"
            className="rte-btn"
            onMouseDown={e => { e.preventDefault(); tableInsertColumn('left'); }}
            title="Insert column left"
          ><InsertColLeftIcon size={14} strokeWidth={2} /></button>
          <button
            type="button"
            className="rte-btn"
            onMouseDown={e => { e.preventDefault(); tableInsertColumn('right'); }}
            title="Insert column right"
          ><InsertColRightIcon size={14} strokeWidth={2} /></button>
          <div className="rte-sep" />
          <button
            type="button"
            className="rte-btn rte-btn-danger"
            onMouseDown={e => { e.preventDefault(); tableDeleteRow(); }}
            title="Delete current row"
          >− Row</button>
          <button
            type="button"
            className="rte-btn rte-btn-danger"
            onMouseDown={e => { e.preventDefault(); tableDeleteColumn(); }}
            title="Delete current column"
          >− Col</button>
          <div className="rte-sep" />
          <button
            type="button"
            className="rte-btn rte-btn-danger"
            onMouseDown={e => { e.preventDefault(); tableDeleteEntire(); }}
            title="Delete entire table"
          ><TrashIcon size={14} strokeWidth={2} /></button>
        </div>
      )}

      <div
        ref={editorRef}
        contentEditable
        className="rte-editor"
        onInput={() => { notify(); refreshTableContext(); }}
        onBlur={saveSelection}
        onFocus={refreshTableContext}
        onKeyUp={refreshTableContext}
        onMouseMove={handleEditorMouseMove}
        onMouseDown={handleEditorMouseDown}
        onKeyDown={e => {
          // Tab-in-cell — override contenteditable's default (which either
          // does nothing or focuses the next form element).
          if (e.key === 'Tab') {
            const inCell = getCurrentTableCell();
            if (inCell) {
              e.preventDefault();
              moveToAdjacentCell(e.shiftKey ? -1 : 1);
            }
          }
        }}
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
          // If/Else block chips — click to reopen the structured popup with
          // condition/then/else parsed back from the stored token. Matches
          // both anonymous {if } and named {ifset } chip shapes.
          const ifChip = e.target.closest?.('[data-token^="{if "], [data-token^="{ifset "]');
          if (ifChip) {
            e.preventDefault();
            if (!isPro) { onShowUpgrade?.('Conditional blocks'); return; }
            const tok = ifChip.dataset.token || '';
            const parsed = parseIfToken(tok);
            if (!parsed) return;
            // Close any sibling sub-editor that might still be open so its
            // stale state doesn't bleed into this popup.
            setFormulaEntry(false);
            setFormulaEditChip(null);
            setFillInEntry(false);
            saveSelection();
            setInsertCategory('formula');
            const r = ifChip.getBoundingClientRect();
            setMenuPos({
              top: r.bottom + 4,
              left: r.left,
              btnTop: r.top,
              btnRight: r.right,
              anchorRight: false,
              rightOffset: window.innerWidth - r.right,
            });
            setShowInsert(true);
            setIfEntry(true);
            setIfCondition(parsed.condition);
            setIfThen(parsed.thenText);
            setIfElse(parsed.elseText);
            setIfHasElse(parsed.hasElse);
            setIfName(parsed.name || '');
            setIfEditChip(ifChip);
            setIfActiveField('condition');
            setTimeout(() => ifConditionRef.current?.focus(), 0);
            return;
          }
          // Formula chips — click to edit the expression (and optional name)
          // inline. Matches both anonymous `{=expr}` and named
          // `{set NAME = expr}{=NAME}` chip shapes.
          const formulaChip = e.target.closest?.('[data-token^="{="], [data-token^="{set "]');
          if (formulaChip) {
            const tok = formulaChip.dataset.token || '';
            const parsed = parseFormulaToken(tok);
            // Skip {set …} chips that AREN'T paired named formulas — those
            // should be handled by the if-block selector above or treated as
            // raw {set} text the user typed.
            if (!parsed) return;
            e.preventDefault();
            if (!isPro) { onShowUpgrade?.('Formula expressions'); return; }
            // Close any sibling sub-editor so its stale state doesn't bleed in.
            setIfEntry(false);
            setIfEditChip(null);
            setFillInEntry(false);
            saveSelection();
            setInsertCategory('formula');
            const r = formulaChip.getBoundingClientRect();
            setMenuPos({
              top: r.bottom + 4,
              left: r.left,
              btnTop: r.top,
              btnRight: r.right,
              anchorRight: false,
              rightOffset: window.innerWidth - r.right,
            });
            setShowInsert(true);
            setFormulaEntry(true);
            setFormulaExpr(parsed.expr);
            setFormulaName(parsed.name || '');
            setFormulaEditChip(formulaChip);
            setTimeout(() => formulaInputRef.current?.focus(), 0);
            return;
          }

          const fillinChip = e.target.closest?.('[data-token^="{fillIn:"]');
          if (fillinChip) {
            e.preventDefault();
            // Parse the full token shape so the popover knows whether this is a
            // legacy text chip (label only) or a typed chip (dropdown shows its
            // options for inline editing; other types just rename the label).
            // Token grammar: `{fillIn:<label>[:<kind>[:<opts>][:default=<v>]]}`
            const tok = fillinChip.dataset.token || '';
            const inner = tok.slice(8, -1); // strip "{fillIn:" and "}"
            // Lift the optional `:default=...` suffix first so it can contain colons
            let defaultSuffix = '';
            let head = inner;
            const defIdx = inner.lastIndexOf(':default=');
            if (defIdx !== -1) {
              defaultSuffix = inner.slice(defIdx); // includes leading ":default="
              head = inner.slice(0, defIdx);
            }
            const parts = head.split(':');
            const label = parts[0] || '';
            const kind = parts[1] || 'text';
            const optsRaw = parts.slice(2).join(':');
            if (label) {
              const rect = fillinChip.getBoundingClientRect();
              setFillInRename({ label, x: rect.left, y: rect.bottom + 4, kind, defaultSuffix });
              setFillInRenameValue(label);
              setFillInRenameOptions(kind === 'dropdown' ? optsRaw : '');
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
          style={userDraggedPos && (formulaEntry || ifEntry)
            ? { top: userDraggedPos.top, left: userDraggedPos.left, right: 'auto' }
            : (menuPos.anchorRight
                ? { top: menuPos.top, right: Math.max(8, menuPos.rightOffset) }
                : { top: menuPos.top, left: menuPos.left })
          }
        >
          {/* Fill-in label input — always mounted so ref is always valid,
              toggled visible/hidden via CSS to avoid React render-timing races.
              For typed kinds (multiline/dropdown/checkbox/number/date) the
              header shows the kind, and dropdown adds a second row for the
              comma-separated options. */}
          <div
            className="rte-fillin-row"
            style={{ display: fillInEntry ? 'flex' : 'none' }}
          >
            <span className="rte-fillin-prompt-label">
              {fillInKind === 'text' ? 'Field label:' : `${fillInKind} label:`}
            </span>
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

          {/* Formula expression editor — textarea-based so it visibly reads
              as "write code here" rather than "name a thing". A reference
              panel sits below with available data, functions, and
              click-to-insert examples. Enter commits; Shift+Enter newline.

              The optional Name field above the expression turns this into a
              named formula — backend stores it as {set NAME = expr}{=NAME}
              so the result both renders AND becomes referenceable in other
              formulas by its name. */}
          {formulaEntry && (() => {
            const nameTrimmed = formulaName.trim();
            const nameValid = !nameTrimmed || /^[A-Za-z_][A-Za-z0-9_]*$/.test(nameTrimmed);
            return (
            <>
              <div
                className="rte-popup-drag"
                onMouseDown={handlePopupDragStart}
                title="Drag to move. Double-click to reset."
                onDoubleClick={() => setUserDraggedPos(null)}
                role="presentation"
              >
                <span className="rte-popup-drag-grip" />
                <span className="rte-popup-drag-label">{formulaEditChip ? 'Edit formula' : 'New formula'}</span>
              </div>
              <div className="rte-fillin-row rte-formula-row">
                <span className="rte-fillin-prompt-label">Name <span style={{ color: 'var(--text-muted)', fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(optional)</span></span>
                <input
                  className="rte-fillin-input rte-formula-input"
                  type="text"
                  value={formulaName}
                  onChange={e => setFormulaName(e.target.value)}
                  placeholder='e.g. total — gives this result a name so other formulas can reference it'
                  spellCheck={false}
                  style={!nameValid ? { borderColor: '#ff6464' } : undefined}
                  title={!nameValid ? 'Use letters, digits, and underscores; cannot start with a digit.' : ''}
                  onKeyDown={e => {
                    e.stopPropagation();
                    if (e.key === 'Enter') { e.preventDefault(); formulaInputRef.current?.focus(); }
                    if (e.key === 'Escape') { setFormulaEntry(false); setFormulaExpr(''); setFormulaName(''); setFormulaEditChip(null); }
                  }}
                />
              </div>
              <div className="rte-fillin-row rte-formula-row">
                <span className="rte-fillin-prompt-label">{formulaEditChip ? 'Edit formula' : 'Formula'}</span>
                <textarea
                  ref={formulaInputRef}
                  className="rte-fillin-input rte-formula-input"
                  rows={2}
                  value={formulaExpr}
                  onChange={e => setFormulaExpr(e.target.value)}
                  placeholder='e.g. upper(name) or qty * 1.2 or if(n > 5, "many", "few")'
                  spellCheck={false}
                  onKeyDown={e => {
                    e.stopPropagation();
                    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); commitFormulaEntry(); }
                    if (e.key === 'Escape') { setFormulaEntry(false); setFormulaExpr(''); setFormulaName(''); setFormulaEditChip(null); }
                  }}
                />
                <button
                  type="button"
                  className="rte-fillin-ok"
                  onMouseDown={e => { e.preventDefault(); commitFormulaEntry(); }}
                  disabled={!nameValid}
                  title={!nameValid ? 'Fix the name first' : ''}
                >{formulaEditChip ? 'Save' : 'Insert'}</button>
              </div>

              <div className="rte-formula-help">
                <div className="rte-formula-banner">
                  <strong>Want to reuse this result in another formula?</strong> Type a name in the Name field above — the result will both render here AND become a referenceable chip in other formulas.
                </div>
                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Named references in this expansion</div>
                  <div className="rte-formula-chips">
                    {reusableFillInLabels.length === 0
                      && setVarNames.length === 0
                      && Object.keys(globalVariables).length === 0 ? (
                      <span className="rte-formula-chips-empty">
                        Nothing named yet. Add a fill-in field, or type a name in the field above when inserting a formula — it'll appear here as a chip for the next formula.
                      </span>
                    ) : (
                      <>
                        {reusableFillInLabels.map(label => (
                          <button
                            key={`fillin-${label}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--fillin"
                            title={`Insert "${label}" (fill-in field)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoFormula(label); }}
                          >{label}</button>
                        ))}
                        {setVarNames.map(name => (
                          <button
                            key={`set-${name}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--set"
                            title={`Insert "${name}" (set variable)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoFormula(name); }}
                          >{name}</button>
                        ))}
                        {Object.keys(globalVariables).map(name => (
                          <button
                            key={`var-${name}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--var"
                            title={`Insert "${name}" (global variable)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoFormula(name); }}
                          >{name}</button>
                        ))}
                      </>
                    )}
                  </div>
                </div>

                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Always available</div>
                  <div className="rte-formula-chips">
                    {['selection', 'clipboard', 'yes', 'no'].map(name => (
                      <button
                        key={`reserved-${name}`}
                        type="button"
                        className="rte-formula-chip rte-formula-chip--reserved"
                        title={`Insert "${name}"`}
                        onMouseDown={e => { e.preventDefault(); insertIntoFormula(name); }}
                      >{name}</button>
                    ))}
                  </div>
                </div>

                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Operators</div>
                  <div className="rte-formula-help-body">
                    <code>+ - * / %</code> math · <code>== != &lt; &gt;</code> compare · <code>&amp;&amp; || !</code> logic · <code>&amp;</code> join text
                  </div>
                </div>

                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Functions (click to insert)</div>
                  {[
                    {
                      label: 'Text',
                      fns: [
                        { name: 'upper',       template: 'upper()',                signature: 'upper(text)' },
                        { name: 'lower',       template: 'lower()',                signature: 'lower(text)' },
                        { name: 'trim',        template: 'trim()',                 signature: 'trim(text)' },
                        { name: 'len',         template: 'len()',                  signature: 'len(text)' },
                        { name: 'substring',   template: 'substring(, 0, 3)',      signature: 'substring(text, start, end)' },
                        { name: 'replace',     template: 'replace(, "", "")',      signature: 'replace(text, from, to)' },
                        { name: 'contains',    template: 'contains(, "")',         signature: 'contains(text, needle)' },
                        { name: 'startswith',  template: 'startswith(, "")',       signature: 'startswith(text, prefix)' },
                        { name: 'endswith',    template: 'endswith(, "")',         signature: 'endswith(text, suffix)' },
                        { name: 'urlencode',   template: 'urlencode()',            signature: 'urlencode(text)' },
                      ],
                    },
                    {
                      label: 'Math',
                      fns: [
                        { name: 'round', template: 'round()', signature: 'round(number)' },
                        { name: 'floor', template: 'floor()', signature: 'floor(number)' },
                        { name: 'ceil',  template: 'ceil()',  signature: 'ceil(number)'  },
                        { name: 'abs',   template: 'abs()',   signature: 'abs(number)'   },
                      ],
                    },
                    {
                      label: 'Date',
                      fns: [
                        { name: 'today',      template: 'today()',                            signature: 'today()' },
                        { name: 'dateadd',    template: 'dateadd(today(), 7)',                signature: 'dateadd(date, days)' },
                        { name: 'dateformat', template: 'dateformat(today(), "DD/MM/YYYY")',  signature: 'dateformat(date, pattern)' },
                        { name: 'datediff',   template: 'datediff(today(), )',                signature: 'datediff(later, earlier)' },
                      ],
                    },
                    {
                      label: 'Logic',
                      fns: [
                        { name: 'if', template: 'if(, , )', signature: 'if(cond, valueIfTrue, valueIfFalse)' },
                      ],
                    },
                    {
                      label: 'Random',
                      fns: [
                        { name: 'random', template: 'random("", "")', signature: 'random("a", "b", "c") — picks one at random per fire. Give the formula a Name above so the same pick can be reused.' },
                      ],
                    },
                  ].map(cat => (
                    <div key={`fn-cat-${cat.label}`} className="rte-formula-fn-row">
                      <span className="rte-formula-fn-cat">{cat.label}:</span>
                      <div className="rte-formula-fn-chips">
                        {cat.fns.map(fn => (
                          <button
                            key={`fn-${fn.name}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--fn"
                            title={fn.signature}
                            onMouseDown={e => { e.preventDefault(); insertIntoFormula(fn.template); }}
                          >{fn.name}</button>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>

                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Click to try</div>
                  <div className="rte-formula-examples">
                    {[
                      { label: 'Uppercase a fill-in',     expr: 'upper(name)' },
                      { label: 'Add 20% VAT',             expr: 'qty * price * 1.2' },
                      { label: 'URL-encode clipboard',    expr: 'urlencode(clipboard)' },
                      { label: 'Conditional value',       expr: 'if(formal, "Hi", "Hey")' },
                      { label: 'Random greeting',         expr: 'random("Hi", "Hello", "Hey")' },
                      { label: 'Joined string',           expr: '"Hello " & name & "!"' },
                      { label: '7 days after a picked date', expr: 'dateformat(dateadd(eventdate, 7), "DD/MM/YYYY")' },
                      { label: 'Days overdue from due date', expr: 'datediff(today(), duedate)' },
                    ].map(ex => (
                      <button
                        key={ex.expr}
                        type="button"
                        className="rte-formula-example"
                        onMouseDown={e => {
                          e.preventDefault();
                          setFormulaExpr(ex.expr);
                          setTimeout(() => formulaInputRef.current?.focus(), 0);
                        }}
                        title={ex.expr}
                      >
                        <span className="rte-formula-example-label">{ex.label}</span>
                        <code className="rte-formula-example-code">{ex.expr}</code>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </>
            );
          })()}

          {/* If / Else block popup — structured Condition + Then + Else fields
              so the user doesn't have to learn the {if cond}then{else}…{endif}
              syntax to insert one. The token still ships as plain editable
              text so power users can refine it inline after insertion. */}
          {ifEntry && (() => {
            const ifNameTrimmed = ifName.trim();
            const ifNameValid = !ifNameTrimmed || /^[A-Za-z_][A-Za-z0-9_]*$/.test(ifNameTrimmed);
            return (
            <>
              <div
                className="rte-popup-drag"
                onMouseDown={handlePopupDragStart}
                title="Drag to move. Double-click to reset."
                onDoubleClick={() => setUserDraggedPos(null)}
                role="presentation"
              >
                <span className="rte-popup-drag-grip" />
                <span className="rte-popup-drag-label">{ifEditChip ? 'Edit if/else' : 'New if/else'}</span>
              </div>
              <div className="rte-fillin-row rte-formula-row">
                <span className="rte-fillin-prompt-label">Name <span style={{ color: 'var(--text-muted)', fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(optional)</span></span>
                <input
                  className="rte-fillin-input rte-formula-input"
                  type="text"
                  value={ifName}
                  onChange={e => setIfName(e.target.value)}
                  placeholder='e.g. greeting — names this conditional so other formulas can reference its chosen branch'
                  spellCheck={false}
                  style={!ifNameValid ? { borderColor: '#ff6464' } : undefined}
                  title={!ifNameValid ? 'Letters, digits, and underscores only; no leading digit.' : ''}
                  onKeyDown={e => {
                    e.stopPropagation();
                    if (e.key === 'Enter') { e.preventDefault(); ifConditionRef.current?.focus(); }
                    if (e.key === 'Escape') { setIfEntry(false); setIfEditChip(null); setIfName(''); }
                  }}
                />
              </div>
              <div className="rte-fillin-row rte-formula-row">
                <span className="rte-fillin-prompt-label">If</span>
                <input
                  ref={ifConditionRef}
                  className="rte-fillin-input rte-formula-input"
                  value={ifCondition}
                  onChange={e => setIfCondition(e.target.value)}
                  onFocus={() => setIfActiveField('condition')}
                  placeholder='e.g. formal == "yes"  or  qty > 10'
                  spellCheck={false}
                  onKeyDown={e => {
                    e.stopPropagation();
                    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); commitIfEntry(); }
                    if (e.key === 'Escape') { setIfEntry(false); setIfEditChip(null); setIfName(''); }
                  }}
                />
              </div>
              <div className="rte-fillin-row rte-formula-row">
                <span className="rte-fillin-prompt-label">Then</span>
                <textarea
                  ref={ifThenRef}
                  className="rte-fillin-input rte-formula-input"
                  rows={2}
                  value={ifThen}
                  onChange={e => setIfThen(e.target.value)}
                  onFocus={() => setIfActiveField('then')}
                  placeholder='Text inserted when the condition is true'
                  onKeyDown={e => e.stopPropagation()}
                />
              </div>
              <div className="rte-fillin-row" style={{ paddingTop: 0, paddingBottom: 4 }}>
                <label className="rte-formula-else-toggle">
                  <input
                    type="checkbox"
                    checked={ifHasElse}
                    onChange={e => setIfHasElse(e.target.checked)}
                  />
                  <span>Include Else branch</span>
                </label>
              </div>
              {ifHasElse && (
                <div className="rte-fillin-row rte-formula-row">
                  <span className="rte-fillin-prompt-label">Else</span>
                  <textarea
                    ref={ifElseRef}
                    className="rte-fillin-input rte-formula-input"
                    rows={2}
                    value={ifElse}
                    onChange={e => setIfElse(e.target.value)}
                    onFocus={() => setIfActiveField('else')}
                    placeholder='Text inserted when the condition is false'
                    onKeyDown={e => e.stopPropagation()}
                  />
                </div>
              )}
              <div className="rte-fillin-row" style={{ justifyContent: 'flex-end', gap: 6 }}>
                <button
                  type="button"
                  className="rte-fillin-ok"
                  onMouseDown={e => { e.preventDefault(); commitIfEntry(); }}
                >Insert</button>
              </div>

              <div className="rte-formula-help">
                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">
                    From this expansion <span style={{ color: 'var(--text-muted)', fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(inserts into the {ifActiveField === 'then' ? 'Then' : ifActiveField === 'else' ? 'Else' : 'If'} field)</span>
                  </div>
                  <div className="rte-formula-chips">
                    {reusableFillInLabels.length === 0
                      && setVarNames.length === 0
                      && Object.keys(globalVariables).length === 0 ? (
                      <span className="rte-formula-chips-empty">
                        No fill-in fields, set variables, or global variables defined yet.
                      </span>
                    ) : (
                      <>
                        {reusableFillInLabels.map(label => (
                          <button
                            key={`fillin-${label}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--fillin"
                            title={`Insert "${label}" (fill-in field)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoIfField(label); }}
                          >{label}</button>
                        ))}
                        {setVarNames.map(name => (
                          <button
                            key={`set-${name}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--set"
                            title={`Insert "${name}" (set variable)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoIfField(name); }}
                          >{name}</button>
                        ))}
                        {Object.keys(globalVariables).map(name => (
                          <button
                            key={`var-${name}`}
                            type="button"
                            className="rte-formula-chip rte-formula-chip--var"
                            title={`Insert "${name}" (global variable)`}
                            onMouseDown={e => { e.preventDefault(); insertIntoIfField(name); }}
                          >{name}</button>
                        ))}
                      </>
                    )}
                  </div>
                </div>
                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Always available</div>
                  <div className="rte-formula-chips">
                    {['selection', 'clipboard', 'yes', 'no'].map(name => (
                      <button
                        key={`reserved-${name}`}
                        type="button"
                        className="rte-formula-chip rte-formula-chip--reserved"
                        title={`Insert "${name}"`}
                        onMouseDown={e => { e.preventDefault(); insertIntoIfField(name); }}
                      >{name}</button>
                    ))}
                  </div>
                </div>
                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Hints for the condition</div>
                  <div className="rte-formula-help-body">
                    Use comparisons (<code>==</code> <code>!=</code> <code>&lt;</code> <code>&gt;</code>) and logic (<code>&amp;&amp;</code> <code>||</code> <code>!</code>). Reference any fill-in field or set variable by name. A checkbox fill-in returns <code>yes</code> / <code>no</code>.
                  </div>
                </div>
                <div className="rte-formula-help-section">
                  <div className="rte-formula-help-label">Click to try</div>
                  <div className="rte-formula-examples">
                    {[
                      { label: 'Checkbox checked',     expr: 'formal' },
                      { label: 'Equals a value',       expr: 'tier == "Pro"' },
                      { label: 'Number threshold',     expr: 'qty > 10' },
                      { label: 'Non-empty selection',  expr: 'len(selection) > 0' },
                    ].map(ex => (
                      <button
                        key={ex.expr}
                        type="button"
                        className="rte-formula-example"
                        onMouseDown={e => {
                          e.preventDefault();
                          setIfCondition(ex.expr);
                          setTimeout(() => ifConditionRef.current?.focus(), 0);
                        }}
                        title={ex.expr}
                      >
                        <span className="rte-formula-example-label">{ex.label}</span>
                        <code className="rte-formula-example-code">{ex.expr}</code>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </>
            );
          })()}

          {/* Dropdown-only: comma-separated options. Empty options ship a bare
              `{fillIn:Label:dropdown}` token which the backend renders as a
              text input fallback — user can add options later. */}
          {fillInEntry && fillInKind === 'dropdown' && (
            <div className="rte-fillin-row">
              <span className="rte-fillin-prompt-label">Options:</span>
              <input
                className="rte-fillin-input"
                value={fillInOptionsStr}
                onChange={e => setFillInOptionsStr(e.target.value)}
                placeholder="comma-separated · e.g. Formal,Casual,Friendly"
                onKeyDown={e => {
                  if (e.key === 'Enter') handleInsertFillIn(e);
                  if (e.key === 'Escape') { setFillInEntry(false); setFillInLabel(''); setFillInOptionsStr(''); }
                }}
              />
            </div>
          )}

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

          {/* Menu items — hidden while a structured entry popup is active */}
          <div style={{ display: (fillInEntry || formulaEntry || ifEntry) ? 'none' : 'contents' }}>
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
            ) : insertCategory === 'highlight' ? (
              <>
                <div className="rte-menu-section-label">Highlight Colour</div>
                <div className="rte-colour-grid">
                  {HIGHLIGHT_COLOURS.map(c => (
                    <button
                      key={c.label}
                      type="button"
                      className={`rte-colour-swatch${c.hex === null ? ' rte-swatch-none' : ''}`}
                      style={c.hex ? { background: c.hex } : undefined}
                      title={c.label}
                      onMouseDown={e => { e.preventDefault(); applyHighlight(c.hex); }}
                    />
                  ))}
                </div>
              </>
            ) : insertCategory === 'table' ? (
              <>
                <div className="rte-menu-section-label">Insert Table</div>
                <div
                  className="rte-table-grid"
                  onMouseLeave={() => setTablePickerHover(null)}
                >
                  {Array.from({ length: 6 }, (_, r) =>
                    Array.from({ length: 6 }, (_, c) => {
                      const rr = r + 1, cc = c + 1;
                      const active = tablePickerHover
                        && rr <= tablePickerHover.rows
                        && cc <= tablePickerHover.cols;
                      return (
                        <button
                          key={`${r}-${c}`}
                          type="button"
                          className={`rte-table-cell${active ? ' rte-table-cell-active' : ''}`}
                          onMouseEnter={() => setTablePickerHover({ rows: rr, cols: cc })}
                          onMouseDown={e => { e.preventDefault(); insertTable(rr, cc); }}
                        />
                      );
                    })
                  )}
                </div>
                <div className="rte-table-readout">
                  {tablePickerHover
                    ? `${tablePickerHover.rows} × ${tablePickerHover.cols} table`
                    : 'Hover to size, click to insert'}
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
                        // {{var:key}} is the preferred namespaced form. Legacy
                        // bare {{key}} still resolves at fire time; existing
                        // expansions with the bare form keep working.
                        insertTokenHtml(`{{var:${key}}}`, keyToTitle(key));
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
            ) : insertCategory === 'expansions' ? (
              <>
                <div className="rte-menu-section-label">Nested Expansions</div>
                {(() => {
                  const others = expansions
                    .filter(e => !excludeTrigger || e.trigger !== excludeTrigger)
                    .sort((a, b) => a.trigger.localeCompare(b.trigger));
                  if (others.length === 0) {
                    return <div className="rte-menu-empty">No other expansions yet.</div>;
                  }
                  return others.map(exp => (
                    <button
                      key={exp.trigger}
                      type="button"
                      className="rte-menu-item"
                      onMouseDown={e => {
                        e.preventDefault();
                        insertTokenHtml(`{{expansion:${exp.trigger}}}`, exp.displayName || exp.trigger);
                        setShowInsert(false);
                        setInsertCategory(null);
                      }}
                      title={exp.text ? exp.text.slice(0, 80) : ''}
                    >
                      <span className="rte-menu-chip rte-chip-expansion">{exp.trigger}</span>
                      {exp.displayName || exp.trigger}
                    </button>
                  ));
                })()}
              </>
            ) : (
              <>
                <div className="rte-menu-section-label">{INSERT_CATEGORIES[insertCategory].label}</div>
                {INSERT_CATEGORIES[insertCategory].items.map((item, i) => {
                  if (item.type === 'sep') {
                    return <div key={`sep-${i}`} className="rte-menu-sep" />;
                  }
                  if (item.type === 'header') {
                    return <div key={`hdr-${i}`} className="rte-menu-section-label">{item.label}</div>;
                  }
                  return (
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
                  );
                })}
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
                  <NumberField
                    className="rte-key-repeat-input"
                    min={1}
                    max={99}
                    defaultOnEmpty={1}
                    value={keyPickerRepeat}
                    onCommit={v => setKeyPickerRepeat(v)}
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
          className={`rte-fillin-rename${fillInRename.kind === 'dropdown' ? ' rte-fillin-rename--dropdown' : ''}`}
          style={{ top: fillInRename.y, left: fillInRename.x }}
        >
          <span className="rte-fillin-rename-label">
            {fillInRename.kind === 'text' || !fillInRename.kind
              ? 'Rename field'
              : `Edit ${fillInRename.kind} field`}
          </span>
          <input
            ref={fillInRenameInputRef}
            autoFocus
            className="rte-fillin-input"
            value={fillInRenameValue}
            onChange={e => setFillInRenameValue(e.target.value)}
            placeholder="Field label"
            onKeyDown={e => {
              e.stopPropagation();
              if (e.key === 'Enter') commitFillInRename();
              if (e.key === 'Escape') cancelFillInRename();
            }}
          />
          {fillInRename.kind === 'dropdown' && (
            <input
              className="rte-fillin-input"
              value={fillInRenameOptions}
              onChange={e => setFillInRenameOptions(e.target.value)}
              placeholder="Options · comma-separated"
              onKeyDown={e => {
                e.stopPropagation();
                if (e.key === 'Enter') commitFillInRename();
                if (e.key === 'Escape') cancelFillInRename();
              }}
            />
          )}
          <button
            type="button"
            className="rte-fillin-ok"
            onMouseDown={e => { e.preventDefault(); commitFillInRename(); }}
          >Save</button>
        </div>,
        document.body
      )}
      {showExpansionPicker && (
        <FireTargetPicker
          mode="expansion"
          currentValue=""
          assignments={expansions
            .filter(exp => !excludeTrigger || exp.trigger !== excludeTrigger)
            .reduce((acc, exp) => {
              acc[`GLOBAL::EXPANSION::${exp.trigger}`] = {
                data: {
                  displayName: exp.displayName,
                  category: exp.category,
                  expansionType: exp.expansionType,
                  imagePath: exp.imagePath,
                  options: exp.options,
                  text: exp.text,
                },
              };
              return acc;
            }, {})}
          onSelect={(trigger) => {
            insertTokenHtml(`{{expansion:${trigger}}}`, `:${trigger}`);
            setShowExpansionPicker(false);
          }}
          onClose={() => setShowExpansionPicker(false)}
        />
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
  onDeleteMany,
  hiddenTips = [],
  onHideTip,
  categories = [],
  onAddCategory,
  onDeleteCategory,
  onReorderCategories,
  onUpdateCategoryColour,
  onRenameCategory,
  // Autocorrect props (Pro) — settings live in App.jsx state, entries are
  // flat GLOBAL::AUTOCORRECT:: assignments grouped by correct word here.
  autocorrectEnabled = false,
  autocorrectBuiltinTypos = false,
  autocorrectDoubleCaps = false,
  autocorrectDoubleCapsExceptions = [],
  autocorrectCapsLockFix = false,
  autocorrectSentenceCaps = false,
  autocorrectExtendedTypos = false,
  autocorrectDays = false,
  autocorrectSymbols = false,
  autocorrectEmojis = false,
  autocorrectExcludedApps = [],
  onUpdateAutocorrectSettings,
  autocorrections = [],
  onSaveAutocorrectGroup,
  onDeleteAutocorrectGroup,
  // Bundled-dictionary entries switched off individually (lowercase typo keys)
  autocorrectDisabledEntries = [],
  // Learn-from-undo: words undone twice awaiting a user decision
  acSuggestions = [],
  onAcSuggestionResolve,
  // Corrections CSV pack export/import
  onExportAutocorrections,
  onImportAutocorrections,
  acImportPrompt,
  onAcImportResolve,
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
  onImportExpansionsFrom,
  expansionImportPrompt,
  onExpansionImportResolve,
  // Suppress foreground auto-switch while the user is mid-edit
  onEditingChange,
}) {
  // ── Panel mode (expansions | autocorrect | globalvars) ──
  const [panelMode, setPanelMode] = useState('expansions');

  // ── Expansion form state ──
  const [editing, setEditing]             = useState(null);
  const [justSaved, setJustSaved]         = useState(false);
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
  const [variantRemoveConfirm, setVariantRemoveConfirm] = useState(null); // index of variant pending "collapse to single" confirm
  // When true, firing the trigger picks one variant at random (no picker popup).
  // Persisted on the primary expansion entry as data.randomVariant, plumbed
  // through to fire_variant_expansion in expansions.rs.
  const [randomVariant, setRandomVariant] = useState(false);
  const [voicePhrases, setVoicePhrases]   = useState([]);
  // Additional triggers that fire the same expansion. Stored on the primary as
  // data.aliases, and expanded into shadow GLOBAL::EXPANSION::<alias> entries
  // in App.jsx handleAddExpansion so the buffer matcher picks them up unchanged.
  const [aliases, setAliases]             = useState([]);
  const [aliasInput, setAliasInput]       = useState('');
  const [aliasError, setAliasError]       = useState('');
  // Inline create-category state — when user picks "+ Add Category" in the
  // editor dropdown, swap to an inline input that creates and selects in one go.
  const [creatingCatInEditor, setCreatingCatInEditor] = useState(false);
  const [editorNewCatName, setEditorNewCatName]       = useState('');

  // Edit-panel width: user-resizable via the splitter between list and editor.
  // `null` means "use the CSS default (clamp 320..480)". Persisted to
  // localStorage so the choice survives restart. Min clamp = 320px; max clamp
  // computed at drag time as (container width - 240px) so the list stays usable.
  const [editPanelWidth, setEditPanelWidth] = useState(() => {
    try {
      const stored = localStorage.getItem('trigr.te.editPanelWidth');
      const n = stored ? parseInt(stored, 10) : NaN;
      return Number.isFinite(n) && n >= 320 ? n : null;
    } catch { return null; }
  });
  const teBodyRef = useRef(null);
  useEffect(() => {
    if (editPanelWidth != null) {
      try { localStorage.setItem('trigr.te.editPanelWidth', String(editPanelWidth)); } catch {}
    } else {
      try { localStorage.removeItem('trigr.te.editPanelWidth'); } catch {}
    }
  }, [editPanelWidth]);

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
  const [importFromOpen, setImportFromOpen] = useState(false); // Import From ▾ menu
  const importFromRef = useRef(null);
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
  // Multi-select for bulk delete (Ctrl+click toggles, Shift+click ranges).
  // Set of trigger strings. Anchor = last Ctrl+clicked row, Windows-style:
  // Shift+click selects from the anchor to the clicked row in visible order.
  const [selectedTriggers, setSelectedTriggers] = useState(() => new Set());
  const [bulkDeleteConfirm, setBulkDeleteConfirm] = useState(false);
  const selectionAnchor = useRef(null);

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
  const [acEditing, setAcEditing]     = useState(null); // null | { isNew, originalWord?, originalTypos? }
  const [acWord, setAcWord]           = useState('');   // the CORRECT word
  const [acTypos, setAcTypos]         = useState([]);   // misspelling chips
  const [acTypoInput, setAcTypoInput] = useState('');   // chip input value
  const [acDcInput, setAcDcInput]     = useState('');   // double-caps exception input

  // Bundled dictionaries for the "Common typos" column — fetched from the
  // engine once, first time the tab opens (single source of truth in Rust).
  const [builtinEntries, setBuiltinEntries] = useState([]); // [[typo, correction, pack], ...]
  const [acDictFilter, setAcDictFilter] = useState('');
  // Autocorrect section rail: 'custom' | 'starter' | 'extended' | 'fixes' —
  // mirrors the expansions category sidebar pattern.
  const [acSection, setAcSection] = useState('custom');
  const [acCustomFilter, setAcCustomFilter] = useState('');
  // Where a Customise jump came from — Cancel returns there, Save clears it.
  const [acReturnSection, setAcReturnSection] = useState(null);
  // Snapshot of the correction form when it opened ("word|typo,typo") — the
  // form is dirty when current content differs or text sits uncommitted in
  // the chip input. A prefilled-but-untouched Customise form is NOT dirty.
  const [acFormBaseline, setAcFormBaseline] = useState('');
  // Rail section the user tried to open while the form had unsaved changes.
  // Non-null renders the discard prompt.
  const [acPendingNav, setAcPendingNav] = useState(null);
  useEffect(() => {
    if (panelMode !== 'autocorrect' || builtinEntries.length > 0) return;
    window.electronAPI?.getBuiltinAutocorrectEntries?.()
      .then(list => { if (Array.isArray(list)) setBuiltinEntries(list); })
      .catch(() => {});
  }, [panelMode, builtinEntries.length]);

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

  // Close the Import From menu on outside click
  useEffect(() => {
    if (!importFromOpen) return;
    function onDown(e) {
      if (!importFromRef.current?.contains(e.target)) setImportFromOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [importFromOpen]);

  // Prune the multi-select when expansions are removed elsewhere (delete,
  // overwrite import) so the selection never references ghosts.
  useEffect(() => {
    setSelectedTriggers(prev => {
      if (prev.size === 0) return prev;
      const live = new Set(expansions.map(e => e.trigger));
      const next = new Set([...prev].filter(t => live.has(t)));
      return next.size === prev.size ? prev : next;
    });
  }, [expansions]);

  // A selection spanning a filter change could delete rows the user can no
  // longer see — clear it whenever the visible pool changes shape.
  useEffect(() => {
    selectionAnchor.current = null;
    setSelectedTriggers(prev => (prev.size === 0 ? prev : new Set()));
  }, [activeCategory, typeFilter, searchQuery, panelMode]);

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
    setVariantRemoveConfirm(null);
    setRandomVariant(false);
    setVoicePhrases([]);
    setAliases([]);
    setAliasInput('');
    setAliasError('');
    setCreatingCatInEditor(false);
    setEditorNewCatName('');
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
    setVariantRemoveConfirm(null);
    setRandomVariant(exp.randomVariant === true);
    setVoicePhrases(Array.isArray(exp.voicePhrases) ? exp.voicePhrases : []);
    setAliases(Array.isArray(exp.aliases) ? exp.aliases : []);
    setAliasInput('');
    setAliasError('');
    setCreatingCatInEditor(false);
    setEditorNewCatName('');
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
    // Fold a still-uncommitted alias input into the list so a user typing an
    // alias and hitting Save (without pressing Enter/comma first) doesn't lose it.
    const trailingAlias = aliasInput.trim().toLowerCase().replace(/\s/g, '');
    const combinedAliases = trailingAlias && !aliases.includes(trailingAlias) && trailingAlias !== t
      ? [...aliases, trailingAlias]
      : aliases;
    // randomVariant only meaningful when the expansion has variants; strip the
    // flag otherwise so a saved variant-less expansion never carries stale state.
    const randomVariantOut = hasVariants ? randomVariant : false;
    onAdd(t, editorValue, originalTrigger, category, triggerMode, displayName.trim() || null, expansionType, imagePath, imageScale, cleanedVariants, voicePhrases, combinedAliases, randomVariantOut);
    // Keep the editor open after Save. Flip editing.isNew to false (and
    // re-anchor originalTrigger to the just-saved trigger) so the next Save
    // updates the same row instead of trying to create a fresh one.
    setEditing(prev => prev ? { ...prev, isNew: false, originalTrigger: t } : prev);
    setJustSaved(true);
  }

  function handleCancel() {
    setEditing(null);
    setJustSaved(false);
  }

  // Reset the "Saved ✓" badge after ~1.5s so the button returns to its
  // normal Save state. Cleared early if the user closes the editor.
  useEffect(() => {
    if (!justSaved) return;
    const id = setTimeout(() => setJustSaved(false), 1500);
    return () => clearTimeout(id);
  }, [justSaved]);

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

  function commitEditorNewCategory() {
    const name = editorNewCatName.trim();
    if (name) {
      const exists = normCategories.find(c => c.name.toLowerCase() === name.toLowerCase());
      if (!exists) onAddCategory(name, null);
      setCategory(exists ? exists.name : name);
    }
    setCreatingCatInEditor(false);
    setEditorNewCatName('');
  }

  function cancelEditorNewCategory() {
    setCreatingCatInEditor(false);
    setEditorNewCatName('');
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
      [], // aliases — never copied; a duplicate can't share trigger keys with its source
      original.randomVariant === true,
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
  // One UI row = one correct word with all its misspelling chips; storage
  // stays flat (one GLOBAL::AUTOCORRECT:: entry per misspelling).
  function openAcAdd() {
    setAcWord('');
    setAcTypos([]);
    setAcTypoInput('');
    setAcEditing({ isNew: true });
    setAcReturnSection(null);
    setAcFormBaseline('|');
  }

  function openAcEdit(group) {
    setAcWord(group.correction);
    setAcTypos([...group.typos]);
    setAcTypoInput('');
    setAcEditing({ isNew: false, originalWord: group.correction, originalTypos: [...group.typos] });
    setAcReturnSection(null);
    setAcFormBaseline(`${group.correction}|${[...group.typos].join(',')}`);
  }

  function acFormDirty() {
    if (!acEditing) return false;
    return `${acWord}|${acTypos.join(',')}` !== acFormBaseline || acTypoInput.trim() !== '';
  }

  // Rail navigation with an unsaved-changes gate: leaving the section closes
  // the open correction form; if it has uncommitted edits, prompt first.
  function acNavigate(section) {
    if (section === acSection) return;
    if (acEditing && acFormDirty()) {
      setAcPendingNav(section);
      return;
    }
    setAcEditing(null);
    setAcReturnSection(null);
    setAcSection(section);
    if (section !== 'custom') setAcDictFilter('');
  }

  function acConfirmDiscardNav() {
    const target = acPendingNav;
    setAcPendingNav(null);
    if (!target) return;
    setAcEditing(null);
    setAcReturnSection(null);
    setAcSection(target);
    if (target !== 'custom') setAcDictFilter('');
  }

  // Customise a bundled entry: pre-fill the custom form with the bundled
  // group and jump to Your Corrections. The saved custom entries shadow the
  // bundled ones per-typo (custom always wins in the engine). Remembers the
  // section the user came from so Cancel returns there; Save stays in Your
  // Corrections so the new entry is visible.
  function openAcCustomise(typos, correction) {
    setAcReturnSection(acSection);
    setAcSection('custom');
    setAcWord(correction);
    setAcTypos([...typos]);
    setAcTypoInput('');
    setAcEditing({ isNew: true });
    setAcFormBaseline(`${correction}|${[...typos].join(',')}`);
  }

  function acCommitTypoInput() {
    const t = acTypoInput.trim().toLowerCase().replace(/\s/g, '');
    setAcTypoInput('');
    if (!t) return;
    setAcTypos(prev => (prev.includes(t) ? prev : [...prev, t]));
  }

  function handleAcSave() {
    const word = acWord.trim();
    // Text still sitting in the chip input counts — covers "typed the typo
    // but didn't press Enter" before hitting Save.
    const pending = acTypoInput.trim().toLowerCase().replace(/\s/g, '');
    const typos = pending && !acTypos.includes(pending) ? [...acTypos, pending] : [...acTypos];
    if (!word || typos.length === 0) return;
    onSaveAutocorrectGroup?.(word, typos, acEditing?.originalTypos || []);
    setAcEditing(null);
    // Deliberately stay in Your Corrections after a save — the user sees
    // their new entry land. The return marker only serves Cancel.
    setAcReturnSection(null);
  }

  function handleAcCancel() {
    setAcEditing(null);
    // Cancelled a Customise jump — go back to the dictionary section the
    // user was browsing.
    if (acReturnSection) {
      setAcSection(acReturnSection);
      setAcReturnSection(null);
    }
  }

  function acCommitDcInput() {
    const w = acDcInput.trim().toLowerCase().replace(/\s/g, '');
    setAcDcInput('');
    if (!w || autocorrectDoubleCapsExceptions.includes(w)) return;
    onUpdateAutocorrectSettings?.({ exceptions: [...autocorrectDoubleCapsExceptions, w] });
  }

  const hasVariants = variantOptions.length > 0 && variantOptions.some(o => o.text?.trim());
  const canSave   = trigger.trim() && !triggerError && (
    expansionType === 'image' ? !!imagePath : (hasVariants || !!editorValue.text.trim())
  );
  const canAcSave = acWord.trim() && (acTypos.length > 0 || acTypoInput.trim());

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

  const setVarNames = useMemo(() => {
    const set = new Set();
    extractSetVarNames(editorValue.html).forEach(n => set.add(n));
    variantOptions.forEach(v => extractSetVarNames(v.html).forEach(n => set.add(n)));
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

  // Custom autocorrections grouped by correct word, both levels alphabetical
  const acGroups = Object.values(
    autocorrections.reduce((acc, { typo, correction }) => {
      const key = correction.toLowerCase();
      if (!acc[key]) acc[key] = { correction, typos: [] };
      acc[key].typos.push(typo);
      return acc;
    }, {})
  ).sort((a, b) => a.correction.localeCompare(b.correction));
  acGroups.forEach(g => g.typos.sort());

  // Toolbar search over the custom list — matches misspellings and the
  // correct word. The rail count stays the unfiltered total.
  const acCustomQuery = acCustomFilter.trim().toLowerCase();
  const acGroupsFiltered = acCustomQuery
    ? acGroups.filter(g =>
        g.correction.toLowerCase().includes(acCustomQuery) ||
        g.typos.some(t => t.includes(acCustomQuery)))
    : acGroups;

  // Bundled-dictionary entries switched off individually, for chip state.
  const acDisabledSet = new Set(autocorrectDisabledEntries);

  // Bundled dictionary for the ACTIVE rail section, grouped by correction,
  // filtered by the toolbar search, render-capped for the 4k extended pack.
  // One entry per bundled pack: rail label, enabled flag, and the settings
  // patch key its toggle flips.
  const AC_DICT_SECTIONS = {
    starter:  { enabled: autocorrectBuiltinTypos,  patchKey: 'builtinTypos' },
    extended: { enabled: autocorrectExtendedTypos, patchKey: 'extendedTypos' },
    days:     { enabled: autocorrectDays,          patchKey: 'days' },
    symbols:  { enabled: autocorrectSymbols,       patchKey: 'symbols' },
    emojis:   { enabled: autocorrectEmojis,        patchKey: 'emojis' },
  };
  const AC_DICT_RENDER_CAP = 150;
  const starterCount = builtinEntries.filter(e => e[2] === 'starter').length;
  const extendedCount = builtinEntries.filter(e => e[2] === 'extended').length;
  const daysCount = builtinEntries.filter(e => e[2] === 'days').length;
  const symbolsCount = builtinEntries.filter(e => e[2] === 'symbols').length;
  const emojisCount = builtinEntries.filter(e => e[2] === 'emojis').length;
  const dictPack = AC_DICT_SECTIONS[acSection] ? acSection : 'starter';
  const dictQuery = acDictFilter.trim().toLowerCase();
  const dictGroupMap = {};
  let dictGroupTotal = 0;
  for (const [typo, correction, pack] of builtinEntries) {
    if (pack !== dictPack) continue;
    if (dictQuery && !typo.includes(dictQuery) && !correction.toLowerCase().includes(dictQuery)) continue;
    const key = correction.toLowerCase();
    if (!dictGroupMap[key]) { dictGroupMap[key] = { correction, typos: [] }; dictGroupTotal += 1; }
    dictGroupMap[key].typos.push(typo);
  }
  const builtinGroups = Object.values(dictGroupMap)
    .sort((a, b) => a.correction.localeCompare(b.correction))
    .slice(0, AC_DICT_RENDER_CAP);
  builtinGroups.forEach(g => g.typos.sort());
  const dictHiddenGroups = Math.max(0, dictGroupTotal - builtinGroups.length);
  const dictPackEnabled = AC_DICT_SECTIONS[dictPack].enabled;
  const dictPackPatchKey = AC_DICT_SECTIONS[dictPack].patchKey;

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
            <span className="te-mode-tab-icon" aria-hidden="true">✦</span> Text Expansions
          </button>
          <button
            className={`te-mode-tab${panelMode === 'autocorrect' ? ' active' : ''}`}
            onClick={() => {
              if (!isPro) { onShowUpgrade?.('Autocorrect'); return; }
              setPanelMode('autocorrect');
            }}
            type="button"
          >
            <span className="te-mode-tab-icon" aria-hidden="true">✓</span> Autocorrect <span className="pro-badge">PRO</span>
          </button>
        </div>
        {/* How-to tip — same gold TIP treatment as the radial editor and
            templates panel. Replaces the old te-hint one-liner. */}
        {panelMode === 'expansions' && !hiddenTips.includes('expansions') && (
          <div className="te-tip">
            <span className="te-tip-badge">TIP</span>
            <span>
              Type your trigger characters then <kbd className="te-tip-kbd">Space</kbd> to fire the expansion, or select <span className="te-tip-instant">⚡ Instant</span> mode to fire immediately on the last character.
            </span>
            <button type="button" className="te-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('expansions')}>&#10005;</button>
          </div>
        )}
        {panelMode === 'autocorrect' && !hiddenTips.includes('autocorrect') && (
          <div className="te-tip">
            <span className="te-tip-badge">TIP</span>
            <span>
              Corrections fire the moment you finish a word with <kbd className="te-tip-kbd">Space</kbd>, <kbd className="te-tip-kbd">Enter</kbd>, <kbd className="te-tip-kbd">Tab</kbd> or punctuation. Wrong guess? Press <kbd className="te-tip-kbd">Backspace</kbd> straight away to get your typing back.
            </span>
            <button type="button" className="te-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('autocorrect')}>&#10005;</button>
          </div>
        )}
        <div className="te-header-right">
          {panelMode === 'expansions' && (
            <button className="te-add-btn" onClick={() => openAdd()} title="Add expansion" type="button">
              + New Expansion
            </button>
          )}
          {panelMode === 'autocorrect' && (
            <button
              className="te-add-btn"
              onClick={() => { setAcSection('custom'); openAcAdd(); }}
              title="Add custom correction"
              type="button"
            >
              + New Correction
            </button>
          )}
          {panelMode === 'globalvars' && (
            <button className="te-add-btn" onClick={() => openGdAdd()} title="Add variable" type="button">
              + New Variable
            </button>
          )}
          <button
            className={`te-gv-link${panelMode === 'globalvars' ? ' active' : ''}`}
            onClick={() => {
              setPanelMode('globalvars'); setGdEditing(null);
            }}
            type="button"
            title="Global Variables — reusable values inserted into expansions"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <rect x="1" y="1" width="10" height="10" rx="2"/>
              <path d="M4 4h1M7 4h1M4 6h4M4 8h3"/>
            </svg>
            Global Variables
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
              <div className="te-import-from-wrap" ref={importFromRef}>
                <button
                  className="te-cat-new-btn te-cat-pack-btn"
                  onClick={() => setImportFromOpen(o => !o)}
                  title="Migrate snippets from another tool"
                  type="button"
                >
                  ⇄ Import From…
                </button>
                {importFromOpen && (
                  <div className="te-import-from-menu">
                    <button
                      type="button"
                      onClick={() => { setImportFromOpen(false); onImportExpansionsFrom?.('espanso'); }}
                    >
                      Espanso <span className="te-import-from-ext">.yml</span>
                    </button>
                    <button
                      type="button"
                      onClick={() => { setImportFromOpen(false); onImportExpansionsFrom?.('ahk'); }}
                    >
                      AutoHotkey <span className="te-import-from-ext">.ahk</span>
                    </button>
                    <button
                      type="button"
                      onClick={() => { setImportFromOpen(false); onImportExpansionsFrom?.('textexpander'); }}
                    >
                      TextExpander <span className="te-import-from-ext">.csv</span>
                    </button>
                    <button
                      type="button"
                      onClick={() => { setImportFromOpen(false); onImportExpansionsFrom?.('textblaze'); }}
                    >
                      Text Blaze <span className="te-import-from-ext">.json</span>
                    </button>
                  </div>
                )}
              </div>
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
          <div className="te-body" ref={teBodyRef}>

            {/* Floating selection toast — overlays the list bottom so the
                rows never shift while the user is mid-selection. */}
            {selectedTriggers.size > 0 && (
              <div className="te-selection-toast">
                <span className="te-selection-count">
                  {selectedTriggers.size} selected
                </span>
                <button
                  className="te-selbar-btn"
                  type="button"
                  onClick={() => {
                    const visible = filteredListItems
                      .filter(it => it.type === 'item')
                      .map(it => it.exp.trigger);
                    setSelectedTriggers(new Set(visible));
                  }}
                  title="Select every expansion currently shown in the list"
                >
                  Select all in view ({itemCount})
                </button>
                <button
                  className="te-selbar-btn te-selbar-btn--danger"
                  type="button"
                  onClick={() => setBulkDeleteConfirm(true)}
                >
                  Delete selected
                </button>
                <button
                  className="te-selbar-btn"
                  type="button"
                  onClick={() => {
                    selectionAnchor.current = null;
                    setSelectedTriggers(new Set());
                  }}
                >
                  Clear
                </button>
              </div>
            )}

            {/* Scrollable list */}
            <div className={`te-list${selectedTriggers.size > 0 ? ' te-list--selecting' : ''}`}>

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
                <div className="te-col-header te-col-header--static" style={{ justifyContent: 'flex-end' }}>Tag</div>
                <div className="te-col-header-spacer" />
              </div>
              {itemCount === 0 ? (
                expansions.length === 0 ? (
                  <div className="te-empty-state">
                    <span className="te-empty-icon">✦</span>
                    <span className="te-empty-heading">No text expansions yet</span>
                    <span className="te-empty-sub">Click <strong>+ New Expansion</strong> to create your first expansion. Type a short trigger word and it expands to full text instantly anywhere on your computer.</span>
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
                  const isSelected = selectedTriggers.has(exp.trigger);
                  return (
                    <div
                      key={exp.trigger}
                      className={`te-item${isEditingThis ? ' te-item-editing' : ''}${isSelected ? ' te-item-selected' : ''}`}
                      onMouseDown={(e) => {
                        // Stop the browser's text-selection flash on modifier clicks.
                        if (e.ctrlKey || e.metaKey || e.shiftKey) e.preventDefault();
                      }}
                      onClick={(e) => {
                        // Shift+click selects the range from the anchor to this
                        // row (Windows semantics: replaces the selection; with
                        // Ctrl held it adds the range instead).
                        if (e.shiftKey && (selectionAnchor.current || selectedTriggers.size > 0)) {
                          e.preventDefault();
                          const visible = filteredListItems
                            .filter(it => it.type === 'item')
                            .map(it => it.exp.trigger);
                          const anchor = (selectionAnchor.current && visible.includes(selectionAnchor.current))
                            ? selectionAnchor.current
                            : visible.find(t => selectedTriggers.has(t));
                          if (anchor) {
                            const ai = visible.indexOf(anchor);
                            const ci = visible.indexOf(exp.trigger);
                            const [lo, hi] = ai <= ci ? [ai, ci] : [ci, ai];
                            const range = visible.slice(lo, hi + 1);
                            setSelectedTriggers(prev => {
                              const base = (e.ctrlKey || e.metaKey) ? new Set(prev) : new Set();
                              range.forEach(t => base.add(t));
                              return base;
                            });
                            return;
                          }
                        }
                        // Ctrl/Cmd+click (or a bare Shift+click with nothing
                        // selected yet) toggles this row and sets the anchor.
                        // Starting a selection while another row is open in
                        // the editor pulls that row in too — clicking an item
                        // first reads as "selecting it" (Windows semantics),
                        // and its editing highlight looks selected anyway.
                        if (e.ctrlKey || e.metaKey || e.shiftKey) {
                          e.preventDefault();
                          selectionAnchor.current = exp.trigger;
                          setSelectedTriggers(prev => {
                            const next = new Set(prev);
                            if (prev.size === 0 && editing && !editing.isNew
                                && editing.originalTrigger && editing.originalTrigger !== exp.trigger) {
                              next.add(editing.originalTrigger);
                            }
                            if (next.has(exp.trigger)) next.delete(exp.trigger);
                            else next.add(exp.trigger);
                            return next;
                          });
                          return;
                        }
                        if (selectedTriggers.size > 0) {
                          selectionAnchor.current = null;
                          setSelectedTriggers(new Set());
                        }
                        openEdit(exp);
                      }}
                      onContextMenu={e => handleItemContextMenu(e, exp.trigger)}
                    >
                      {/* Col 1 — Trigger */}
                      <div className="te-col-trigger">
                        <button
                          type="button"
                          className={`te-item-check${isSelected ? ' checked' : ''}`}
                          onClick={(e) => {
                            e.stopPropagation();
                            selectionAnchor.current = exp.trigger;
                            setSelectedTriggers(prev => {
                              const next = new Set(prev);
                              if (prev.size === 0 && editing && !editing.isNew
                                  && editing.originalTrigger && editing.originalTrigger !== exp.trigger) {
                                next.add(editing.originalTrigger);
                              }
                              if (next.has(exp.trigger)) next.delete(exp.trigger);
                              else next.add(exp.trigger);
                              return next;
                            });
                          }}
                          title={isSelected ? 'Remove from selection' : 'Select for bulk actions'}
                          aria-pressed={isSelected}
                        >{isSelected ? '✓' : ''}</button>
                        <kbd className="te-trigger-badge">{exp.trigger}</kbd>
                        {exp.triggerMode === 'immediate' && (
                          <span className="te-immediate-badge" title="Fires instantly (no Space needed)">⚡</span>
                        )}
                        {exp.expansionType === 'image' && (
                          <span className={`te-img-badge${!isPro ? ' locked' : ''}`} title={!isPro ? 'Image expansion (Pro only — does not fire on Free)' : 'Image expansion'}>IMG</span>
                        )}
                        {exp.options && exp.options.length > 0 && (
                          <span
                            className={`te-variant-badge${exp.randomVariant ? ' te-variant-badge--random' : ''}`}
                            title={
                              exp.randomVariant
                                ? `${exp.options.length} variants, one fires at random`
                                : `${exp.options.length} variants, picker shows on trigger`
                            }
                          >{exp.randomVariant ? '⇄' : '▾'}</span>
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
                      {/* Col 4 — Tag (also carries the Free-tier PRO chip for image expansions
                          so the trigger column isn't crowded — see fix 2026-08-13) */}
                      <div className="te-col-tag">
                        {exp.expansionType === 'image' && !isPro && (
                          <span className="pro-badge te-list-pro-badge" title="Pro feature">PRO</span>
                        )}
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

            {/* Splitter — drag horizontally to resize the edit panel.
                Double-click resets to the CSS default. Hidden in the stacked
                narrow-viewport layout via @media rule in the CSS. */}
            <div
              className="te-edit-splitter"
              onMouseDown={(e) => {
                e.preventDefault();
                const container = teBodyRef.current;
                const panel = container?.querySelector('.te-edit-panel');
                if (!container || !panel) return;
                const containerRect = container.getBoundingClientRect();
                const startX = e.clientX;
                const startWidth = editPanelWidth ?? panel.getBoundingClientRect().width;
                document.body.style.cursor = 'col-resize';
                document.body.style.userSelect = 'none';
                function onMove(ev) {
                  const dx = ev.clientX - startX;
                  const proposed = startWidth - dx;
                  const maxWidth = Math.max(320, containerRect.width - 240);
                  const clamped = Math.max(320, Math.min(maxWidth, proposed));
                  setEditPanelWidth(Math.round(clamped));
                }
                function onUp() {
                  document.body.style.cursor = '';
                  document.body.style.userSelect = '';
                  window.removeEventListener('mousemove', onMove);
                  window.removeEventListener('mouseup', onUp);
                }
                window.addEventListener('mousemove', onMove);
                window.addEventListener('mouseup', onUp);
              }}
              onDoubleClick={() => setEditPanelWidth(null)}
              title="Drag to resize. Double-click to reset."
              aria-label="Resize edit panel"
              role="separator"
            />

            {/* Right edit panel — always visible */}
            <div
              className="te-edit-panel"
              style={editPanelWidth != null ? { width: editPanelWidth } : undefined}
            >
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

                  {/* Fixed-height top fields: category + name + voice + trigger + mode */}
                  <div className="te-panel-fields">
                    <div className="te-panel-field">
                      <label className="form-label">CATEGORY</label>
                      {creatingCatInEditor ? (
                        <div className="te-cat-inline-create">
                          <input
                            autoFocus
                            className="form-input te-cat-inline-input"
                            placeholder="New category name…"
                            value={editorNewCatName}
                            onChange={e => setEditorNewCatName(e.target.value)}
                            onKeyDown={e => {
                              if (e.key === 'Enter') { e.preventDefault(); commitEditorNewCategory(); }
                              if (e.key === 'Escape') { e.preventDefault(); cancelEditorNewCategory(); }
                            }}
                            spellCheck={false}
                          />
                          <button
                            type="button"
                            className="te-cat-inline-confirm"
                            onClick={commitEditorNewCategory}
                            disabled={!editorNewCatName.trim()}
                          >Add</button>
                          <button
                            type="button"
                            className="te-cat-inline-cancel"
                            onClick={cancelEditorNewCategory}
                            aria-label="Cancel"
                          >✕</button>
                        </div>
                      ) : (
                        <select
                          className="te-cat-select"
                          value={category || ''}
                          onChange={e => {
                            if (e.target.value === '__create_new__') {
                              setEditorNewCatName('');
                              setCreatingCatInEditor(true);
                            } else {
                              setCategory(e.target.value || null);
                            }
                          }}
                        >
                          <option value="">Uncategorised</option>
                          {normCategories.map(cat => (
                            <option key={cat.name} value={cat.name}>{cat.name}</option>
                          ))}
                          <option disabled value="__divider__">──────────</option>
                          <option value="__create_new__">+ Add Category…</option>
                        </select>
                      )}
                    </div>
                    <div className="te-panel-field">
                      <label className="form-label">DISPLAY LABEL <span className="te-optional-label">(OPTIONAL)</span></label>
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
                    <div className="te-panel-field">
                      <label className="form-label">ALIASES</label>
                      <div className="te-alias-input-row">
                        {aliases.map((a, i) => (
                          <span key={`${a}-${i}`} className="te-alias-chip">
                            {a}
                            <button
                              type="button"
                              className="te-alias-chip-remove"
                              title="Remove alias"
                              onClick={() => setAliases(aliases.filter((_, idx) => idx !== i))}
                            >×</button>
                          </span>
                        ))}
                        <input
                          className={`form-input te-alias-input${aliasError ? ' te-input-error' : ''}`}
                          placeholder={aliases.length === 0 ? 'brb2, back-soon (Space or Enter to add)' : 'Add another…'}
                          value={aliasInput}
                          onChange={e => {
                            setAliasInput(e.target.value.replace(/\s/g, ''));
                            if (aliasError) setAliasError('');
                          }}
                          onKeyDown={e => {
                            if (e.key === 'Escape') { e.preventDefault(); handleCancel(); return; }
                            if (e.key === 'Enter' || e.key === ',' || e.key === ' ') {
                              e.preventDefault();
                              const candidate = aliasInput.trim().toLowerCase().replace(/\s/g, '');
                              if (!candidate) return;
                              const primaryNorm = trigger.trim().toLowerCase();
                              if (candidate === primaryNorm) {
                                setAliasError('An alias cannot equal the primary trigger.');
                                return;
                              }
                              if (aliases.includes(candidate)) {
                                setAliasError('Alias already added.');
                                return;
                              }
                              // Clash check covers three cases:
                              //   1. Alias equals another expansion's primary trigger
                              //   2. Alias equals another expansion's saved alias (Rory 2026-08-13)
                              //   3. Same match on the SAME expansion is allowed via edit continuity
                              //      (compared against originalTrigger, not the currently-typed trigger)
                              const editingOriginal = editing?.originalTrigger?.toLowerCase() || null;
                              const clash = expansions.find(exp => {
                                const primary = exp.trigger.toLowerCase();
                                if (primary === editingOriginal) return false; // don't clash against self
                                if (primary === candidate) return true;
                                const otherAliases = Array.isArray(exp.aliases) ? exp.aliases : [];
                                return otherAliases.some(a => (a || '').toLowerCase() === candidate);
                              });
                              if (clash) {
                                const clashLabel = clash.displayName || clash.trigger;
                                const isAliasOfClash = clash.trigger.toLowerCase() !== candidate;
                                setAliasError(
                                  isAliasOfClash
                                    ? `"${candidate}" is already an alias of "${clashLabel}".`
                                    : `"${candidate}" is already a trigger for "${clashLabel}".`
                                );
                                return;
                              }
                              setAliases([...aliases, candidate]);
                              setAliasInput('');
                            }
                          }}
                          spellCheck={false}
                        />
                      </div>
                      {aliasError
                        ? <span className="te-trigger-error">{aliasError}</span>
                        : <span className="form-hint" style={{ marginTop: 2 }}>Extra triggers that fire the same expansion. Space, Enter or comma to add. Click × to remove.</span>
                      }
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
                            setVarNames={setVarNames}
                            expansions={expansions}
                            excludeTrigger={editing.isNew ? (trigger.trim().toLowerCase() || null) : editing.originalTrigger}
                          />
                        </>
                      ) : (
                        <>
                          <div className="te-variant-header">
                            <label className="form-label">VARIANTS</label>
                            <label
                              className="te-variant-random-toggle"
                              title={isPro
                                ? (randomVariant
                                    ? 'Firing the trigger picks one variant at random'
                                    : 'Firing the trigger shows the variant picker so the user chooses')
                                : 'Random variant firing is a Pro feature'}
                            >
                              <input
                                type="checkbox"
                                checked={randomVariant}
                                disabled={!isPro}
                                onChange={e => {
                                  if (!isPro) { onShowUpgrade?.('Random variant firing'); return; }
                                  setRandomVariant(e.target.checked);
                                }}
                              />
                              <span>Fire a random variant</span>
                              {!isPro && <span className="pro-badge te-variant-tab-pro">PRO</span>}
                            </label>
                          </div>
                          <p className="te-variant-hint">
                            {randomVariant
                              ? 'When triggered, one of the variants below fires at random. No picker is shown.'
                              : 'When triggered, a popup lets the user pick which variant to insert.'}
                          </p>
                          <div className="te-variant-tabs" role="tablist">
                            {variantOptions.map((opt, i) => {
                              const isActive   = i === activeVariantIndex;
                              const isRenaming = i === renamingVariantIndex;
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
                                        if (variantOptions.length > 2) {
                                          // Plenty left — remove this variant directly.
                                          const next = variantOptions.filter((_, j) => j !== i);
                                          const newActive = Math.min(activeVariantIndex, next.length - 1);
                                          setVariantOptions(next);
                                          setActiveVariantIndex(Math.max(0, newActive));
                                        } else {
                                          // Only 2 left — removing one drops variants entirely.
                                          // Confirm first; the other option survives as the body.
                                          setVariantRemoveConfirm(i);
                                        }
                                      }}
                                      title={variantOptions.length > 2 ? 'Remove this variant' : 'Remove this variant (keeps the other as the body)'}
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
                              setVarNames={setVarNames}
                              expansions={expansions}
                              excludeTrigger={editing.isNew ? (trigger.trim().toLowerCase() || null) : editing.originalTrigger}
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
                          <NumberField
                            className="form-input te-image-scale-input"
                            min={10}
                            max={100}
                            defaultOnEmpty={10}
                            value={imageScale}
                            onCommit={v => setImageScale(v)}
                          />
                          <span className="te-image-scale-pct">%</span>
                        </div>
                      </div>
                    </div>
                  )}

                  </div>{/* /te-panel-scroll */}

                  <div className="te-panel-footer">
                    <div className="te-form-actions">
                      <button
                        className={`te-save-btn${justSaved ? ' te-save-btn--saved' : ''}`}
                        onClick={handleSave}
                        disabled={!canSave}
                        type="button"
                      >
                        {justSaved ? '✓ Saved' : 'Save'}
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
                  <p>Select an expansion to edit,<br/>or click <strong>+ New Expansion</strong> to create a new one</p>
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

          {/* Learn-from-undo: never applied silently, always the user's call */}
          {acSuggestions.length > 0 && (
            <div className="ac-suggest-banner">
              {acSuggestions.slice(0, 3).map(s => (
                <div key={s.key} className="ac-suggest-row">
                  <span className="ac-suggest-text">
                    You've undone <kbd className="te-trigger-badge ac-typo-badge">{s.key}</kbd>
                    <span className="te-item-arrow">→</span>
                    <strong>{s.replacement}</strong> {s.count === 2 ? 'twice' : `${s.count} times`}. Stop correcting it?
                  </span>
                  <button
                    className="te-cancel-btn"
                    onClick={() => onAcSuggestionResolve?.(s.key, 'keep')}
                    type="button"
                    title="Keep this correction and stop asking"
                  >
                    Keep
                  </button>
                  <button
                    className="te-save-btn"
                    onClick={() => onAcSuggestionResolve?.(s.key, 'stop')}
                    type="button"
                  >
                    Stop correcting
                  </button>
                </div>
              ))}
            </div>
          )}

          <div className="te-content">

          {/* ── Section rail — same pattern as the expansions category sidebar ── */}
          <div className="te-cat-sidebar">
            <div className="te-cat-sidebar-list">
              <button
                type="button"
                className={`te-cat-row${acSection === 'custom' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('custom')}
              >
                <span className="te-cat-row-name">Your Corrections</span>
                <span className="te-cat-count">{acGroups.length}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'starter' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('starter')}
              >
                <span className="te-cat-row-name">Common Typos</span>
                <span className="te-cat-count">{starterCount}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'extended' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('extended')}
              >
                <span className="te-cat-row-name">Extended Dictionary</span>
                <span className="te-cat-count">{extendedCount}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'days' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('days')}
              >
                <span className="te-cat-row-name">Days Of The Week</span>
                <span className="te-cat-count">{daysCount}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'symbols' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('symbols')}
              >
                <span className="te-cat-row-name">Symbols</span>
                <span className="te-cat-count">{symbolsCount}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'emojis' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('emojis')}
              >
                <span className="te-cat-row-name">Emoji</span>
                <span className="te-cat-count">{emojisCount}</span>
              </button>
              <button
                type="button"
                className={`te-cat-row${acSection === 'fixes' ? ' te-cat-row-active' : ''}`}
                onClick={() => acNavigate('fixes')}
              >
                <span className="te-cat-row-name">Autocorrect Settings</span>
              </button>
              <button
                className="te-cat-new-btn te-cat-pack-btn"
                onClick={() => onImportAutocorrections?.()}
                title="Import corrections from a CSV file (one typo,correction pair per line)"
                type="button"
              >
                ↓ Import CSV
              </button>
              <button
                className="te-cat-new-btn te-cat-pack-btn"
                onClick={() => onExportAutocorrections?.()}
                title="Export your corrections to a CSV file"
                type="button"
              >
                ↑ Export CSV
              </button>
            </div>
          </div>

          {/* ── Main area ── */}
          <div className="te-main">

          {/* Your corrections */}
          {acSection === 'custom' && (<>
          <div className="te-toolbar">
            <span className="te-toolbar-count">Grouped by correct word. Your entries always win over the bundled lists.</span>
            {acGroups.length > 0 && (
              <SearchBar
                className="te-search-bar"
                placeholder="Search…"
                value={acCustomFilter}
                onChange={e => setAcCustomFilter(e.target.value)}
                onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') { setAcCustomFilter(''); e.target.blur(); } }}
              />
            )}
            <span className="te-toolbar-count">{acGroups.length} {acGroups.length === 1 ? 'word' : 'words'}</span>
          </div>

          {/* Add / Edit form — misspelling chips on the left, correct word on the right */}
          {acEditing && (
            <div className="ac-form">
              <div className="ac-form-fields">
                <div className="ac-form-col ac-form-col-typos">
                  <label className="form-label">MISSPELLINGS</label>
                  <div className="ac-chiprow ac-chiprow-input">
                    {acTypos.map(t => (
                      <span key={t} className="ac-chip">
                        {t}
                        <button
                          className="ac-chip-x"
                          onClick={() => setAcTypos(prev => prev.filter(x => x !== t))}
                          type="button"
                          title={`Remove "${t}"`}
                          aria-label={`Remove misspelling ${t}`}
                        >&#10005;</button>
                      </span>
                    ))}
                    <input
                      className="ac-chip-input"
                      placeholder={acTypos.length === 0 ? 'teh, hte' : 'add another'}
                      value={acTypoInput}
                      onChange={e => setAcTypoInput(e.target.value.replace(/\s/g, ''))}
                      onKeyDown={e => {
                        e.stopPropagation();
                        if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); acCommitTypoInput(); }
                        if (e.key === 'Backspace' && !acTypoInput && acTypos.length > 0) {
                          setAcTypos(prev => prev.slice(0, -1));
                        }
                        if (e.key === 'Escape') handleAcCancel();
                      }}
                      onBlur={acCommitTypoInput}
                      autoFocus
                      spellCheck={false}
                    />
                  </div>
                </div>
                <div className="ac-form-arrow">→</div>
                <div className="ac-form-col">
                  <label className="form-label">CORRECT WORD</label>
                  <input
                    className="form-input ac-field-input"
                    placeholder="the"
                    value={acWord}
                    onChange={e => setAcWord(e.target.value)}
                    onKeyDown={e => { e.stopPropagation(); if (e.key === 'Enter') handleAcSave(); if (e.key === 'Escape') handleAcCancel(); }}
                    spellCheck={false}
                  />
                </div>
              </div>
              <div className="ac-form-footer">
                <span className="ac-form-hint">Press Enter or comma to add each misspelling</span>
                <button className="te-cancel-btn" onClick={handleAcCancel} type="button">Cancel</button>
                <button className="te-save-btn" onClick={handleAcSave} disabled={!canAcSave} type="button">
                  Save
                </button>
              </div>
            </div>
          )}

          {/* Grouped corrections list */}
          {acGroups.length === 0 && !acEditing ? (
            <div className="ac-empty-state">
              <div className="ac-empty-title">No corrections of your own yet</div>
              <div className="ac-empty-sub">Add a correct word with every misspelling you want fixed. Your entries always win over the bundled lists.</div>
              <button className="te-add-btn" onClick={openAcAdd} type="button">
                + Add your first correction
              </button>
            </div>
          ) : (
            <div className="ac-list ac-col-scroll">
              {acGroupsFiltered.length === 0 && acCustomQuery && (
                <div className="te-empty-row">No matches for "{acCustomFilter}"</div>
              )}
              {acGroupsFiltered.map(group => (
                <div key={group.correction} className="ac-item">
                  <div className="ac-group-typos">
                    {group.typos.map(t => (
                      <kbd key={t} className="te-trigger-badge ac-typo-badge">{t}</kbd>
                    ))}
                  </div>
                  <span className="te-item-arrow">→</span>
                  <span className="ac-correction">{group.correction}</span>
                  <div className="te-item-actions">
                    <button className="te-item-edit" onClick={() => openAcEdit(group)} type="button">Edit</button>
                    <button
                      className="te-item-delete"
                      onClick={() => onDeleteAutocorrectGroup?.(group.correction, group.typos)}
                      type="button"
                      title={`Delete "${group.correction}" and its ${group.typos.length} ${group.typos.length === 1 ? 'misspelling' : 'misspellings'}`}
                    >&#10005;</button>
                  </div>
                </div>
              ))}
            </div>
          )}
          </>)}

          {/* Bundled dictionaries: Common typos / Extended / Days / Symbols */}
          {AC_DICT_SECTIONS[acSection] && (<>
          <div className="te-toolbar">
            <div className="ac-pack-toggle">
              <button
                className={`ac-toggle${dictPackEnabled ? ' ac-toggle-on' : ''}`}
                onClick={() => onUpdateAutocorrectSettings?.({ [dictPackPatchKey]: !dictPackEnabled })}
                type="button"
                role="switch"
                aria-checked={dictPackEnabled}
                title={dictPackEnabled ? 'Switch this list off' : 'Switch this list on'}
              />
              <span className="ac-pack-toggle-label">
                {dictPackEnabled ? 'Fixing these as you type' : 'Switched off'}
              </span>
            </div>
            <SearchBar
              className="te-search-bar"
              placeholder="Search…"
              value={acDictFilter}
              onChange={e => setAcDictFilter(e.target.value)}
              onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') { setAcDictFilter(''); e.target.blur(); } }}
            />
          </div>
          <div className={`ac-list ac-col-scroll${dictPackEnabled ? '' : ' ac-list-dim'}`}>
            {builtinGroups.map(group => (
              <div key={group.correction} className="ac-item ac-item-readonly">
                <div className="ac-group-typos">
                  {group.typos.map(t => {
                    const off = acDisabledSet.has(t);
                    return (
                      <kbd key={t} className={`te-trigger-badge ac-typo-badge${off ? ' ac-typo-off' : ''}`}>
                        {t}
                        <button
                          className="ac-typo-toggle"
                          type="button"
                          title={off ? `Resume correcting "${t}"` : `Stop correcting "${t}"`}
                          aria-label={off ? `Resume correcting ${t}` : `Stop correcting ${t}`}
                          onClick={() => {
                            const next = off
                              ? autocorrectDisabledEntries.filter(w => w !== t)
                              : [...autocorrectDisabledEntries, t];
                            onUpdateAutocorrectSettings?.({ disabledEntries: next });
                          }}
                        >{off ? '↺' : '✕'}</button>
                      </kbd>
                    );
                  })}
                </div>
                <span className="te-item-arrow">→</span>
                <span className="ac-correction">{group.correction}</span>
                <div className="te-item-actions">
                  <button
                    className="te-item-edit"
                    onClick={() => openAcCustomise(group.typos, group.correction)}
                    type="button"
                    title="Copy this entry into Your Corrections and change what it corrects to. Your version wins."
                  >Customise</button>
                </div>
              </div>
            ))}
            {dictHiddenGroups > 0 && (
              <div className="te-empty-row">
                {dictHiddenGroups} more. Use the search box to find a word.
              </div>
            )}
            {dictGroupTotal === 0 && dictQuery && (
              <div className="te-empty-row">No matches for "{acDictFilter}"</div>
            )}
          </div>
          </>)}

          {/* Autocorrect settings */}
          {acSection === 'fixes' && (
            <div className="ac-col-scroll">
              {/* Master switch for the whole feature */}
              <div className="ac-builtin-row ac-master-row">
                <div className="ac-builtin-info">
                  <span className="ac-builtin-label">Autocorrect</span>
                  <span className="ac-builtin-sub">Fixes typos the instant you finish a word. Works in every app.</span>
                </div>
                <button
                  className={`ac-toggle${autocorrectEnabled ? ' ac-toggle-on' : ''}`}
                  onClick={() => onUpdateAutocorrectSettings?.({ enabled: !autocorrectEnabled })}
                  type="button"
                  role="switch"
                  aria-checked={autocorrectEnabled}
                  title={autocorrectEnabled ? 'Turn autocorrect off' : 'Turn autocorrect on'}
                />
              </div>
              <div className="ac-fixes-grid">
                <div className="ac-builtin-row">
                  <div className="ac-builtin-info">
                    <span className="ac-builtin-label">Fix double capitals</span>
                    <span className="ac-builtin-sub">HEllo becomes Hello. List words to leave alone below.</span>
                  </div>
                  <button
                    className={`ac-toggle${autocorrectDoubleCaps ? ' ac-toggle-on' : ''}`}
                    onClick={() => onUpdateAutocorrectSettings?.({ doubleCaps: !autocorrectDoubleCaps })}
                    type="button"
                    role="switch"
                    aria-checked={autocorrectDoubleCaps}
                    title={autocorrectDoubleCaps ? 'Disable double-capital fix' : 'Enable double-capital fix'}
                  />
                </div>
                <div className="ac-builtin-row">
                  <div className="ac-builtin-info">
                    <span className="ac-builtin-label">Fix accidental Caps Lock</span>
                    <span className="ac-builtin-sub">tHE becomes The and Caps Lock switches off</span>
                  </div>
                  <button
                    className={`ac-toggle${autocorrectCapsLockFix ? ' ac-toggle-on' : ''}`}
                    onClick={() => onUpdateAutocorrectSettings?.({ capsLockFix: !autocorrectCapsLockFix })}
                    type="button"
                    role="switch"
                    aria-checked={autocorrectCapsLockFix}
                    title={autocorrectCapsLockFix ? 'Disable Caps Lock fix' : 'Enable Caps Lock fix'}
                  />
                </div>
                <div className="ac-builtin-row">
                  <div className="ac-builtin-info">
                    <span className="ac-builtin-label">Capitalize sentences</span>
                    <span className="ac-builtin-sub">Lowercase words get capitalized after . ! ? or a new line</span>
                  </div>
                  <button
                    className={`ac-toggle${autocorrectSentenceCaps ? ' ac-toggle-on' : ''}`}
                    onClick={() => onUpdateAutocorrectSettings?.({ sentenceCaps: !autocorrectSentenceCaps })}
                    type="button"
                    role="switch"
                    aria-checked={autocorrectSentenceCaps}
                    title={autocorrectSentenceCaps ? 'Disable sentence capitalization' : 'Enable sentence capitalization'}
                  />
                </div>
              </div>
              {/* Excluded apps — autocorrect never fires while these are foreground */}
              <div className="ac-excluded-wrap">
                <ClipboardExcludedAppsEditor
                  apps={autocorrectExcludedApps}
                  onChange={apps => onUpdateAutocorrectSettings?.({ excludedApps: apps })}
                  label="Excluded apps"
                  sub="Autocorrect stays out of these apps entirely. Useful for code editors, terminals and games."
                />
              </div>
              {(autocorrectDoubleCaps || autocorrectCapsLockFix) && (
                <div className="ac-dc-exceptions">
                  <label className="form-label">DON'T CORRECT THESE WORDS</label>
                  <div className="ac-chiprow">
                    {autocorrectDoubleCapsExceptions.map(w => (
                      <span key={w} className="ac-chip">
                        {w}
                        <button
                          className="ac-chip-x"
                          onClick={() => onUpdateAutocorrectSettings?.({ exceptions: autocorrectDoubleCapsExceptions.filter(x => x !== w) })}
                          type="button"
                          title={`Remove "${w}"`}
                          aria-label={`Remove exception ${w}`}
                        >&#10005;</button>
                      </span>
                    ))}
                    <input
                      className="ac-chip-input"
                      placeholder="IDs"
                      value={acDcInput}
                      onChange={e => setAcDcInput(e.target.value.replace(/\s/g, ''))}
                      onKeyDown={e => {
                        e.stopPropagation();
                        if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); acCommitDcInput(); }
                        if (e.key === 'Backspace' && !acDcInput && autocorrectDoubleCapsExceptions.length > 0) {
                          onUpdateAutocorrectSettings?.({ exceptions: autocorrectDoubleCapsExceptions.slice(0, -1) });
                        }
                      }}
                      onBlur={acCommitDcInput}
                      spellCheck={false}
                    />
                  </div>
                </div>
              )}
            </div>
          )}

          </div>
          {/* end .te-main / .te-content */}
          </div>
        </div>
      )}

      {/* ════════════════════════════════════ GLOBAL VARIABLES VIEW ═══════════════════════════ */}
      {panelMode === 'globalvars' && (
        <div className="gd-view">
          <div className="gd-helper">
            Reusable values that resolve anywhere Keyfire outputs text.
            Insert as <kbd className="te-trigger-badge">{'{{var:key}}'}</kbd> via the <strong>Insert</strong> button in the expansion editor.
            <br />
            Works in text expansions, autocorrect, macro Type Text steps, and hotkey Text actions.
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
                      Will be inserted as <kbd className="te-trigger-badge gd-key-badge">{`{{var:${titleToKey(gdTitle.trim())}}}`}</kbd>
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
                  <span className="gd-item-value">{String(value)}</span>
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

      {/* Unsaved correction — user navigated away from an edited form */}
      {acPendingNav && (
        <div className="te-delete-overlay">
          <div className="te-delete-dialog">
            <div className="te-delete-title">Discard Changes</div>
            <p className="te-delete-body">
              You have an unsaved correction. Leave without saving it?
            </p>
            <div className="te-delete-actions">
              <button
                className="te-cancel-btn"
                onClick={() => setAcPendingNav(null)}
                type="button"
              >
                Keep Editing
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={acConfirmDiscardNav}
                type="button"
              >
                Discard
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Corrections CSV import collision dialog */}
      {acImportPrompt && (
        <div className="te-delete-overlay">
          <div className="te-delete-dialog te-import-dialog">
            <div className="te-delete-title">Import Corrections</div>
            <p className="te-delete-body">
              This file contains <strong>{acImportPrompt.totalCount}</strong>{' '}
              correction{acImportPrompt.totalCount !== 1 ? 's' : ''}.{' '}
              <strong>{acImportPrompt.collisions.length}</strong> misspelling
              {acImportPrompt.collisions.length !== 1 ? 's' : ''} already
              {acImportPrompt.collisions.length === 1 ? ' has' : ' have'} a different correction in your list:
            </p>
            <div className="te-import-collisions">
              {acImportPrompt.collisions.slice(0, 8).map(t => (
                <kbd key={t} className="te-trigger-badge ac-typo-badge">{t}</kbd>
              ))}
              {acImportPrompt.collisions.length > 8 && (
                <span className="te-import-collisions-more">
                  + {acImportPrompt.collisions.length - 8} more
                </span>
              )}
            </div>
            <div className="te-delete-actions te-import-actions">
              <button
                className="te-cancel-btn"
                onClick={() => onAcImportResolve?.('cancel')}
                type="button"
              >
                Cancel
              </button>
              <button
                className="te-cancel-btn"
                onClick={() => onAcImportResolve?.('skip')}
                type="button"
                title="Keep your existing corrections; only import new ones"
              >
                Skip Duplicates
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={() => onAcImportResolve?.('overwrite')}
                type="button"
                title="Replace your existing corrections with the ones in this file"
              >
                Overwrite All
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Bulk delete confirmation dialog (multi-select) */}
      {bulkDeleteConfirm && (
        <div className="te-delete-overlay">
          <div className="te-delete-dialog">
            <div className="te-delete-title">Delete {selectedTriggers.size} Expansion{selectedTriggers.size !== 1 ? 's' : ''}</div>
            <p className="te-delete-body">
              Delete the {selectedTriggers.size} selected expansion{selectedTriggers.size !== 1 ? 's' : ''}? This cannot be undone.
            </p>
            <div className="te-delete-actions">
              <button className="te-cancel-btn" onClick={() => setBulkDeleteConfirm(false)} type="button">
                Cancel
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={() => {
                  const triggers = Array.from(selectedTriggers);
                  onDeleteMany?.(triggers);
                  if (editing && !editing.isNew && selectedTriggers.has(editing.originalTrigger)) {
                    setEditing(null);
                  }
                  selectionAnchor.current = null;
                  setSelectedTriggers(new Set());
                  setBulkDeleteConfirm(false);
                }}
                type="button"
              >
                Delete
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

      {/* Collapse-to-single confirmation — fired by ✕ when only 2 variants remain */}
      {variantRemoveConfirm !== null && (() => {
        const survivorIdx   = variantRemoveConfirm === 0 ? 1 : 0;
        const survivor      = variantOptions[survivorIdx];
        const survivorLabel = survivor?.label || `Option ${survivorIdx + 1}`;
        return (
          <div className="te-delete-overlay">
            <div className="te-delete-dialog">
              <div className="te-delete-title">Remove all variants</div>
              <p className="te-delete-body">
                Variants need at least two options, so removing this one drops them entirely.{' '}
                <strong>{survivorLabel}</strong> is kept as the body and the picker will no longer appear on trigger.
              </p>
              <div className="te-delete-actions">
                <button className="te-cancel-btn" type="button" onClick={() => setVariantRemoveConfirm(null)}>
                  Cancel
                </button>
                <button
                  className="te-delete-confirm-btn"
                  type="button"
                  onClick={() => {
                    if (survivor) {
                      setEditorValue({ html: survivor.html || '', text: survivor.text || '' });
                    }
                    setVariantOptions([]);
                    setActiveVariantIndex(0);
                    setRenamingVariantIndex(null);
                    setVariantRemoveConfirm(null);
                  }}
                >
                  Remove all variants
                </button>
              </div>
            </div>
          </div>
        );
      })()}

    </div>
  );
}
