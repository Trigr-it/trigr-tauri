import React, { useState, useRef, useLayoutEffect, useEffect, useCallback } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import './TextExpansions.css';

// ── Helpers ────────────────────────────────────────────────────────────────

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

// ── Insert token menu definition ───────────────────────────────────────────

const INSERT_MENU = [
  { type: 'item', token: '{clipboard}',       label: 'Clipboard Contents',  display: 'Clipboard'  },
  { type: 'sep'  },
  { type: 'item', token: '{date:DD/MM/YYYY}', label: 'Date (DD/MM/YYYY)',   display: 'DD/MM/YYYY' },
  { type: 'item', token: '{date:DD/MM/YY}',   label: 'Date (DD/MM/YY)',     display: 'DD/MM/YY'   },
  { type: 'item', token: '{date:MM/DD/YYYY}', label: 'Date (MM/DD/YYYY)',   display: 'MM/DD/YYYY' },
  { type: 'item', token: '{date:YYYY-MM-DD}', label: 'Date (YYYY-MM-DD)',   display: 'YYYY-MM-DD' },
  { type: 'item', token: '{time:HH:MM}',      label: 'Time (HH:MM)',        display: 'HH:MM'      },
  { type: 'item', token: '{time:HH:MM:SS}',   label: 'Time (HH:MM:SS)',     display: 'HH:MM:SS'   },
  { type: 'item', token: '{dayofweek}',        label: 'Day of Week',         display: 'Day'        },
  { type: 'sep'  },
  { type: 'item', token: '{cursor}',           label: 'Cursor Position',     display: '↕ Cursor'   },
  { type: 'item', token: '__fillin__',          label: 'Fill-in Field…',      display: null         },
];

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

function RichTextEditor({ initialHtml, onChange, globalVariables = {} }) {
  const editorRef      = useRef(null);
  const btnRef         = useRef(null);
  const menuRef        = useRef(null);
  const initialHtmlRef = useRef(initialHtml);
  // Saved selection range — captured before the dropdown opens so that focus
  // loss (e.g. when the fill-in label input steals focus) doesn't destroy the
  // insertion point.
  const savedRangeRef  = useRef(null);

  const [showInsert, setShowInsert] = useState(false);
  const [menuPos, setMenuPos] = useState(null);
  const [fillInEntry, setFillInEntry] = useState(false);
  const [fillInLabel, setFillInLabel] = useState('');
  const fillInInputRef = useRef(null);

  useLayoutEffect(() => {
    if (editorRef.current) {
      editorRef.current.innerHTML = initialHtmlRef.current || '';
    }
  }, []);

  // When fill-in entry mode activates, focus the label input.
  // The input is always mounted (CSS-hidden), so the ref is always valid —
  // no setTimeout needed, focus is immediate.
  useEffect(() => {
    if (fillInEntry) {
      fillInInputRef.current?.focus();
    }
  }, [fillInEntry]);

  // Close dropdown on outside click or any scroll — only mounted while open
  useEffect(() => {
    if (!showInsert) return;

    function close() {
      setShowInsert(false);
      setFillInEntry(false);
      setFillInLabel('');
    }
    function onMouseDown(e) {
      if (!btnRef.current?.contains(e.target) && !menuRef.current?.contains(e.target)) {
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

  const notify = useCallback(() => {
    const html = editorRef.current.innerHTML;
    onChange({ html, text: htmlToPlainText(html) });
  }, [onChange]);

  function format(cmd) {
    editorRef.current?.focus();
    document.execCommand(cmd, false, null);
    notify();
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
    console.log('MENU ITEM CLICKED', item.token);
    if (item.token === '__fillin__') {
      console.log('FILL-IN ENTRY MODE');
      setFillInEntry(true);
      setFillInLabel('');
      return;
    }
    insertTokenHtml(item.token, item.display);
    setShowInsert(false);
  }

  function handleInsertFillIn(e) {
    e.preventDefault();
    const label = fillInLabel.trim() || 'Field';
    console.log('handleInsertFillIn', { label, savedRange: savedRangeRef.current });
    insertTokenHtml(`{fillIn:${label}}`, `✎ ${label}`);
    setFillInEntry(false);
    setFillInLabel('');
    setShowInsert(false);
  }

  return (
    <div className="rte-wrap">
      <div className="rte-toolbar">
        <button
          type="button"
          className={`rte-btn rte-bold${isActive('bold') ? ' rte-btn-on' : ''}`}
          onMouseDown={e => { e.preventDefault(); format('bold'); }}
          title="Bold"
        ><b>B</b></button>
        <button
          type="button"
          className={`rte-btn rte-italic${isActive('italic') ? ' rte-btn-on' : ''}`}
          onMouseDown={e => { e.preventDefault(); format('italic'); }}
          title="Italic"
        ><i>I</i></button>
        <div className="rte-sep" />
        <button
          type="button"
          className="rte-btn"
          onMouseDown={e => { e.preventDefault(); format('insertUnorderedList'); }}
          title="Bullet list"
        >
          <svg width="13" height="11" viewBox="0 0 13 11" fill="none">
            <circle cx="1.5" cy="2" r="1.5" fill="currentColor"/>
            <circle cx="1.5" cy="6" r="1.5" fill="currentColor"/>
            <circle cx="1.5" cy="10" r="1.5" fill="currentColor"/>
            <line x1="5" y1="2" x2="13" y2="2" stroke="currentColor" strokeWidth="1.5"/>
            <line x1="5" y1="6" x2="13" y2="6" stroke="currentColor" strokeWidth="1.5"/>
            <line x1="5" y1="10" x2="13" y2="10" stroke="currentColor" strokeWidth="1.5"/>
          </svg>
        </button>
        <button
          type="button"
          className="rte-btn"
          onMouseDown={e => { e.preventDefault(); format('insertOrderedList'); }}
          title="Numbered list"
        >
          <svg width="13" height="11" viewBox="0 0 13 11" fill="none">
            <text x="0" y="3.5" fontSize="4" fill="currentColor" fontWeight="700">1.</text>
            <text x="0" y="7.5" fontSize="4" fill="currentColor" fontWeight="700">2.</text>
            <text x="0" y="11" fontSize="4" fill="currentColor" fontWeight="700">3.</text>
            <line x1="5" y1="2" x2="13" y2="2" stroke="currentColor" strokeWidth="1.5"/>
            <line x1="5" y1="6" x2="13" y2="6" stroke="currentColor" strokeWidth="1.5"/>
            <line x1="5" y1="10" x2="13" y2="10" stroke="currentColor" strokeWidth="1.5"/>
          </svg>
        </button>

        <div className="rte-sep" />

        {/* ── Insert token dropdown ── */}
        <button
          ref={btnRef}
          type="button"
          className={`rte-btn rte-insert-btn${showInsert ? ' rte-btn-on' : ''}`}
          style={{ width: 'auto', minWidth: 'fit-content', padding: '0 8px', display: 'inline-flex', alignItems: 'center', gap: '4px' }}
          onMouseDown={e => {
            e.preventDefault();
            // Editor is still focused here (preventDefault stops focus moving to button).
            // Capture selection now — this is the most reliable moment.
            saveSelection();
            console.log('INSERT CLICKED', { showInsert, savedRange: !!savedRangeRef.current });
            if (!showInsert) {
              const r = e.currentTarget.getBoundingClientRect();
              console.log('menu pos:', { top: r.bottom + 4, left: r.left });
              setMenuPos({ top: r.bottom + 4, left: r.left });
              setShowInsert(true);
            } else {
              setShowInsert(false);
              setFillInEntry(false);
              setFillInLabel('');
            }
          }}
          title="Insert dynamic field"
        >
          Insert <span className="rte-caret">▾</span>
        </button>
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
      />

      {showInsert && menuPos && ReactDOM.createPortal(
        <div
          ref={menuRef}
          className="rte-insert-menu"
          style={{ top: menuPos.top, left: menuPos.left, maxHeight: window.innerHeight - menuPos.top - 16 }}
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

          {/* Menu items — hidden while fill-in label input is active */}
          <div style={{ display: fillInEntry ? 'none' : 'contents' }}>
            <div className="rte-menu-section-label">Dynamic Fields</div>
            {INSERT_MENU.map((item, i) =>
              item.type === 'sep' ? (
                <div key={`sep-${i}`} className="rte-menu-sep" />
              ) : (
                <button
                  key={item.token}
                  type="button"
                  className="rte-menu-item"
                  onMouseDown={e => {
                    console.log('MENU ITEM CLICKED', item.token, 'fillInEntry:', fillInEntry);
                    handleInsertItem(e, item);
                  }}
                >
                  <span className={`rte-menu-chip rte-chip-${
                    item.token === '{clipboard}'   ? 'clipboard' :
                    item.token.startsWith('{date') ? 'date' :
                    item.token.startsWith('{time') ? 'date' :
                    item.token === '{dayofweek}'   ? 'date' :
                    item.token === '{cursor}'      ? 'cursor' :
                    'fillin'
                  }`}>
                    {item.display || '✎'}
                  </span>
                  {item.label}
                </button>
              )
            )}
            {Object.keys(globalVariables).length > 0 && (
              <>
                <div className="rte-menu-sep" />
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
                      }}
                    >
                      <span className="rte-menu-chip rte-chip-globalvar">{keyToTitle(key)}</span>
                      {keyToTitle(key)}
                    </button>
                  ))
                }
              </>
            )}
          </div>
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
  const [variantOptions, setVariantOptions] = useState([]); // [{label, text}]

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

  // Auto-select all text when inline rename input appears
  useEffect(() => {
    if (renamingCat) renameInputRef.current?.select();
  }, [renamingCat]);

  // ── Expansion handlers ──
  function openAdd() {
    setTrigger('');
    setDisplayName('');
    setTriggerError('');
    setEditorValue({ html: '', text: '' });
    setCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setTriggerMode('space');
    setExpansionType('text');
    setImagePath('');
    setImageScale(100);
    setImageExists(true);
    setVariantOptions([]);
    setEditing({ isNew: true });
  }

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
    setVariantOptions(exp.options || []);
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
    onAdd(t, editorValue, originalTrigger, category, triggerMode, displayName.trim() || null, expansionType, imagePath, imageScale, cleanedVariants);
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

  const itemCount = listItems.filter(x => x.type === 'item').length;

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
            <button className="te-add-btn" onClick={openAdd} title="Add expansion" type="button">
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
            onClick={() => { setPanelMode('globalvars'); setGdEditing(null); }}
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
            </div>
          </div>

          {/* ── Main area (toolbar + list + edit panel) ── */}
          <div className="te-main">

          {/* ── Toolbar: type filter + sort ── */}
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
            </div>
            <select
              className="te-sort-select"
              value={sortKey}
              onChange={e => {
                setSortKey(e.target.value);
                localStorage.setItem('trigr.expansionSort', e.target.value);
              }}
              title="Sort expansions"
            >
              <option value="default">Trigger A→Z</option>
              <option value="trigger-desc">Trigger Z→A</option>
              <option value="name-asc">Name A→Z</option>
              <option value="name-desc">Name Z→A</option>
            </select>
          </div>

          {/* ── Body: list + edit panel side-by-side ── */}
          <div className="te-body">

            {/* Scrollable list */}
            <div className="te-list">
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
                listItems.map((item, i) => {
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
                    >
                      {/* Col 1 — Trigger */}
                      <div className="te-col-trigger">
                        <kbd className="te-trigger-badge">{exp.trigger}</kbd>
                        {exp.triggerMode === 'immediate' && (
                          <span className="te-immediate-badge" title="Fires instantly (no Space needed)">⚡</span>
                        )}
                        {exp.expansionType === 'image' && (
                          <span className="te-img-badge" title="Image expansion">IMG</span>
                        )}
                      </div>
                      {/* Col 2 — Name */}
                      <div className="te-col-name">{exp.displayName || exp.trigger}</div>
                      {/* Col 3 — Preview (plain text, truncated) */}
                      <div className="te-col-preview">
                        {exp.expansionType === 'image'
                          ? (exp.imagePath ? exp.imagePath.split(/[/\\]/).pop() : 'No image')
                          : (exp.text || '').replace(/\s+/g, ' ').trim()
                        }
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

                  {/* Expansion type selector */}
                  <div className="te-type-selector">
                    <button
                      type="button"
                      className={`te-trigger-mode-btn${expansionType === 'text' ? ' active' : ''}`}
                      onClick={() => setExpansionType('text')}
                    >Text</button>
                    <button
                      type="button"
                      className={`te-trigger-mode-btn${expansionType === 'image' ? ' active' : ''}`}
                      onClick={() => setExpansionType('image')}
                    >Image</button>
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
                            <button type="button" className="te-variant-toggle-btn" onClick={() => setVariantOptions([{ label: 'Option 1', text: editorValue.text || '' }, { label: 'Option 2', text: '' }])}>
                              + Add Variants
                            </button>
                          </div>
                          <RichTextEditor
                            key={editing.isNew ? '__new__' : editing.originalTrigger}
                            initialHtml={editorValue.html}
                            onChange={setEditorValue}
                            globalVariables={globalVariables}
                          />
                        </>
                      ) : (
                        <>
                          <div className="te-variant-header">
                            <label className="form-label">VARIANTS</label>
                            <button type="button" className="te-variant-toggle-btn te-variant-remove" onClick={() => {
                              if (variantOptions.length > 0 && variantOptions[0].text) {
                                setEditorValue({ html: variantOptions[0].text, text: variantOptions[0].text });
                              }
                              setVariantOptions([]);
                            }}>
                              Remove Variants
                            </button>
                          </div>
                          <p className="te-variant-hint">When triggered, a popup lets the user pick which variant to insert.</p>
                          <div className="te-variant-list">
                            {variantOptions.map((opt, i) => (
                              <div key={i} className="te-variant-item">
                                <div className="te-variant-item-header">
                                  <input
                                    className="form-input te-variant-label-input"
                                    value={opt.label}
                                    onChange={e => {
                                      const next = [...variantOptions];
                                      next[i] = { ...next[i], label: e.target.value };
                                      setVariantOptions(next);
                                    }}
                                    placeholder={`Option ${i + 1}`}
                                    onKeyDown={e => e.stopPropagation()}
                                  />
                                  <button type="button" className="te-variant-remove-btn" onClick={() => {
                                    const next = variantOptions.filter((_, j) => j !== i);
                                    setVariantOptions(next);
                                  }}>✕</button>
                                </div>
                                <textarea
                                  className="form-textarea te-variant-text"
                                  value={opt.text}
                                  onChange={e => {
                                    const next = [...variantOptions];
                                    next[i] = { ...next[i], text: e.target.value };
                                    setVariantOptions(next);
                                  }}
                                  placeholder="Replacement text for this variant…"
                                  rows={2}
                                  onKeyDown={e => e.stopPropagation()}
                                />
                              </div>
                            ))}
                          </div>
                          <button type="button" className="te-variant-add-btn" onClick={() => setVariantOptions([...variantOptions, { label: `Option ${variantOptions.length + 1}`, text: '' }])}>
                            + Add Variant
                          </button>
                        </>
                      )}
                    </div>
                  ) : (
                    <div className="te-panel-image">
                      <label className="form-label">IMAGE</label>
                      <button
                        type="button"
                        className="te-image-pick-btn"
                        onClick={async () => {
                          const path = await window.electronAPI?.browseForImage();
                          if (path) {
                            setImagePath(path);
                            setImageExists(true);
                          }
                        }}
                      >Choose Image…</button>
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

                  <div className="te-panel-footer">
                    <span className="te-paste-note">{expansionType === 'image' ? 'Pastes as image via clipboard' : hasVariants ? 'Shows variant picker on trigger' : 'Pastes as plain text'}</span>
                    <div className="te-form-actions">
                      <button className="te-cancel-btn" onClick={handleCancel} type="button">Cancel</button>
                      <button className="te-save-btn" onClick={handleSave} disabled={!canSave} type="button">
                        Save
                      </button>
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
                    placeholder="e.g. Rory Brady"
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
          <div className="profile-ctx-divider" />
          <button className="profile-ctx-item profile-ctx-delete" onClick={ctxDelete}>
            {ctxDeleteConfirm ? 'Confirm Delete?' : 'Delete'}
          </button>
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
