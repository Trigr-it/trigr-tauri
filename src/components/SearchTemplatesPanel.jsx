import React, { useState, useRef, useEffect, useMemo } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import './SearchTemplatesPanel.css';
import { MacroSequenceForm } from './MacroPanel';

// ── Colour palette (matches TextExpansions) ────────────────────────────────

const CATEGORY_COLOURS = [
  { hex: null,      label: 'None' },
  { hex: '#e84040', label: 'Red' },
  { hex: '#e87040', label: 'Orange' },
  { hex: '#e8a020', label: 'Gold' },
  { hex: '#50c878', label: 'Green' },
  { hex: '#40b0b0', label: 'Teal' },
  { hex: '#4a9eff', label: 'Blue' },
  { hex: '#6a7eff', label: 'Indigo' },
  { hex: '#9a6eff', label: 'Purple' },
  { hex: '#c864ff', label: 'Violet' },
  { hex: '#ff6eb4', label: 'Pink' },
  { hex: '#8a8799', label: 'Grey' },
  { hex: '#c0b090', label: 'Sand' },
];

function ColourPicker({ value, onChange }) {
  return (
    <div className="cat-colour-picker">
      {CATEGORY_COLOURS.map(c => (
        <button
          key={c.label}
          className={`cat-colour-swatch${value === c.hex ? ' selected' : ''}`}
          style={c.hex ? { '--swatch-color': c.hex, background: c.hex } : undefined}
          onClick={() => onChange(c.hex)}
          title={c.label}
          type="button"
        />
      ))}
    </div>
  );
}

// ── Left-click-only sensor (prevents right-click from starting drag) ───────

class LeftClickSensor extends PointerSensor {
  static activators = [
    {
      eventName: 'onPointerDown',
      handler: ({ nativeEvent }) => nativeEvent.button === 0,
    },
  ];
}

// ── Sortable category row wrapper ──────────────────────────────────────────

function SortableCatRow({ id, children }) {
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

// ── Bundled presets (with categories for grouped picker) ────────────────────

const PRESETS = [
  { label: 'Google',            trigger: 'g',      urlTemplate: 'https://www.google.com/search?q={query}',                                       category: 'Search' },
  { label: 'DuckDuckGo',        trigger: 'ddg',    urlTemplate: 'https://duckduckgo.com/?q={query}',                                              category: 'Search' },
  { label: 'ChatGPT',           trigger: 'gpt',    urlTemplate: 'https://chatgpt.com/?q={query}',                                                 category: 'AI' },
  { label: 'Perplexity',        trigger: 'pp',     urlTemplate: 'https://www.perplexity.ai/search?q={query}',                                     category: 'AI' },
  { label: 'GitHub',            trigger: 'gh',     urlTemplate: 'https://github.com/search?q={query}&type=repositories',                          category: 'Development' },
  { label: 'Stack Overflow',    trigger: 'so',     urlTemplate: 'https://stackoverflow.com/search?q={query}',                                     category: 'Development' },
  { label: 'MDN',               trigger: 'mdn',    urlTemplate: 'https://developer.mozilla.org/en-US/search?q={query}',                           category: 'Development' },
  { label: 'npm',               trigger: 'npm',    urlTemplate: 'https://www.npmjs.com/search?q={query}',                                         category: 'Development' },
  { label: 'Hacker News',       trigger: 'hn',     urlTemplate: 'https://hn.algolia.com/?q={query}',                                              category: 'Development' },
  { label: 'YouTube',           trigger: 'yt',     urlTemplate: 'https://www.youtube.com/results?search_query={query}',                           category: 'Media' },
  { label: 'Reddit',            trigger: 'r',      urlTemplate: 'https://www.reddit.com/search/?q={query}',                                       category: 'Media' },
  { label: 'Wikipedia',         trigger: 'wiki',   urlTemplate: 'https://en.wikipedia.org/w/index.php?search={query}',                            category: 'Media' },
  { label: 'Google Maps',       trigger: 'maps',   urlTemplate: 'https://www.google.com/maps/search/{query}',                                     category: 'Maps' },
  { label: 'Ordnance Survey',   trigger: 'os',     urlTemplate: 'https://osdatahub.os.uk/search?q={query}',                                       category: 'Maps' },
  { label: 'Companies House',   trigger: 'ch',     urlTemplate: 'https://find-and-update.company-information.service.gov.uk/search?q={query}',    category: 'UK Business' },
  { label: 'Planning Portal',   trigger: 'plan',   urlTemplate: 'https://www.planningportal.co.uk/planning/search?q={query}',                     category: 'UK Business' },
  { label: 'BSI Knowledge',     trigger: 'bsi',    urlTemplate: 'https://knowledge.bsigroup.com/search?q={query}',                                 category: 'UK Business' },
];

const PRESET_CATEGORIES = [...new Set(PRESETS.map(p => p.category))];

// ── Quick Action types (subset of MacroPanel ACTION_TYPES) ─────────────────

const QA_ACTION_TYPES = [
  { id: 'app',    icon: '⬡', label: 'Open App',          desc: 'Launch an application or file',            color: '#50c878' },
  { id: 'url',    icon: '⊕', label: 'Open URL',          desc: 'Open a website in your browser',           color: '#ffc832' },
  { id: 'folder', icon: '⬢', label: 'Open Folder',       desc: 'Open a folder in File Explorer',           color: '#40c8a0' },
  { id: 'macro',  icon: '◈', label: 'Macro Sequence',    desc: 'Run a sequence of actions one after another', color: '#ff783c' },
];

// ── Helpers ─────────────────────────────────────────────────────────────────

function buildPreviewUrl(urlTemplate, sampleQuery) {
  if (!urlTemplate || !sampleQuery) return '';
  const encoded = encodeURIComponent(sampleQuery).replace(/%20/g, '+');
  return urlTemplate.replace('{query}', encoded);
}

function truncateUrl(url, maxLen = 60) {
  if (!url || url.length <= maxLen) return url;
  return url.slice(0, maxLen) + '…';
}

// ── Main component ──────────────────────────────────────────────────────────

export default function SearchTemplatesPanel({
  searchTemplates = [],
  categories = [],
  isPro = false,
  onAdd,
  onUpdate,
  onDelete,
  onAddCategory,
  onRenameCategory,
  onDeleteCategory,
  onUpdateCategoryColour,
  onReorderCategories,
  quickActions = [],
  onAddQuickAction,
  onUpdateQuickAction,
  onDeleteQuickAction,
  qaCategories = [],
  onAddQaCategory,
  onRenameQaCategory,
  onDeleteQaCategory,
  onUpdateQaCategoryColour,
  onReorderQaCategories,
  globalInputMethod,
  onShowNotification,
}) {
  // Panel mode: 'quickactions' | 'templates'
  const [panelMode, setPanelMode]           = useState('quickactions');

  const [selectedId, setSelectedId]         = useState(null);
  const [showPresets, setShowPresets]        = useState(false);
  const [presetFilter, setPresetFilter]     = useState('');

  // Template form state
  const [formLabel, setFormLabel]           = useState('');
  const [formTrigger, setFormTrigger]       = useState('');
  const [formUrl, setFormUrl]               = useState('');
  const [formEncode, setFormEncode]         = useState(true);
  const [formSource, setFormSource]         = useState('custom');
  const [formCategory, setFormCategory]     = useState(null);
  const [triggerError, setTriggerError]     = useState('');
  const [isNew, setIsNew]                   = useState(false);

  // Quick action form state
  const [qaSelectedId, setQaSelectedId]     = useState(null);
  const [qaIsNew, setQaIsNew]              = useState(false);
  const [qaLabel, setQaLabel]              = useState('');
  const [qaType, setQaType]                = useState('url');
  const [qaFormValue, setQaFormValue]      = useState({});
  const [qaCategory, setQaCategory]        = useState(null);

  // Test state
  const [testQuery, setTestQuery]           = useState('');
  const [showHelp, setShowHelp]             = useState(false);
  const helpRef = useRef(null);

  // Category sidebar state
  const [activeCategory, setActiveCategory]     = useState('All');
  const [addingCategory, setAddingCategory]     = useState(false);
  const [newCategoryName, setNewCategoryName]   = useState('');
  const [newCategoryColour, setNewCategoryColour] = useState(null);
  // Colour picker popover
  const [catColourPopover, setCatColourPopover] = useState(null); // { forCat, x, y }
  const catColourPopoverRef = useRef(null);
  // Context menu
  const [catContextMenu, setCatContextMenu]     = useState(null); // { catName, x, y }
  const [ctxDeleteConfirm, setCtxDeleteConfirm] = useState(false);
  const catContextMenuRef  = useRef(null);
  const catContextTabRef   = useRef(null);
  // Inline rename
  const [renamingCat, setRenamingCat]   = useState(null);
  const [renameValue, setRenameValue]   = useState('');
  const [renameError, setRenameError]   = useState('');
  const renameInputRef                  = useRef(null);
  const renameCommitting                = useRef(false);
  // Drag reorder
  const catDndSensors = useSensors(useSensor(LeftClickSensor, { activationConstraint: { distance: 8 } }));
  const [catDragId, setCatDragId] = useState(null);

  // ── Active categories depend on mode (must be before effects/functions) ──
  const activeCats = panelMode === 'quickactions' ? qaCategories : categories;
  const activeCatHandlers = panelMode === 'quickactions'
    ? { onAdd: onAddQaCategory, onRename: onRenameQaCategory, onDelete: onDeleteQaCategory, onColour: onUpdateQaCategoryColour, onReorder: onReorderQaCategories }
    : { onAdd: onAddCategory, onRename: onRenameCategory, onDelete: onDeleteCategory, onColour: onUpdateCategoryColour, onReorder: onReorderCategories };

  // Close help popover on outside click
  useEffect(() => {
    if (!showHelp) return;
    function onDown(e) {
      if (helpRef.current && !helpRef.current.contains(e.target)) setShowHelp(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [showHelp]);

  // Close colour picker on outside click
  useEffect(() => {
    if (!catColourPopover) return;
    function onDown(e) {
      if (!catColourPopoverRef.current?.contains(e.target)) setCatColourPopover(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [catColourPopover]);

  // Close context menu on outside click or Escape
  useEffect(() => {
    if (!catContextMenu) return;
    function onDown(e) {
      if (catContextMenuRef.current && !catContextMenuRef.current.contains(e.target)) setCatContextMenu(null);
    }
    function onKey(e) { if (e.key === 'Escape') setCatContextMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey); };
  }, [catContextMenu]);

  // Auto-select rename input text
  useEffect(() => {
    if (renamingCat) renameInputRef.current?.select();
  }, [renamingCat]);

  // If active category deleted externally, fall back to All
  useEffect(() => {
    if (activeCategory !== 'All' && activeCategory !== '__uncategorised__' &&
        !activeCats.some(c => c.name === activeCategory)) {
      setActiveCategory('All');
    }
  }, [activeCats, activeCategory]);

  // ── Trigger validation ──────────────────────────────────────────────────

  function validateTrigger(value, excludeId) {
    if (!value) return 'Trigger is required';
    if (!/^[a-z0-9]+$/.test(value)) return 'Lowercase letters and numbers only';
    if (value.length > 10) return 'Max 10 characters';
    const exists = searchTemplates.some(
      t => t.trigger.toLowerCase() === value.toLowerCase() && t.id !== excludeId
    );
    if (exists) return 'Trigger already in use';
    return '';
  }

  // ── Template CRUD ───────────────────────────────────────────────────────

  function selectTemplate(template) {
    setSelectedId(template.id);
    setFormLabel(template.label);
    setFormTrigger(template.trigger);
    setFormUrl(template.url_template);
    setFormEncode(template.encode_query ?? true);
    setFormSource(template.source || 'custom');
    setFormCategory(template.category || null);
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(false);
  }

  function closePanel() {
    setSelectedId(null);
    setIsNew(false);
  }

  function openNewFromPreset(preset) {
    let trigger = preset.trigger;
    const taken = new Set(searchTemplates.map(t => t.trigger.toLowerCase()));
    if (taken.has(trigger)) {
      for (let i = 2; i <= 99; i++) {
        const candidate = `${preset.trigger}${i}`;
        if (!taken.has(candidate) && candidate.length <= 10) { trigger = candidate; break; }
      }
    }
    setSelectedId(null);
    setFormLabel(preset.label);
    setFormTrigger(trigger);
    setFormUrl(preset.urlTemplate);
    setFormEncode(true);
    setFormSource('preset');
    setFormCategory(null);
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  function openNewCustom() {
    setSelectedId(null);
    setFormLabel('');
    setFormTrigger('');
    setFormUrl('');
    setFormEncode(true);
    setFormSource('custom');
    setFormCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  function handleSave() {
    const excludeId = isNew ? null : selectedId;
    const err = validateTrigger(formTrigger, excludeId);
    if (err) { setTriggerError(err); return; }
    if (!formLabel.trim()) return;
    if (!formUrl.includes('{query}')) return;

    if (isNew) {
      const newId = crypto.randomUUID();
      onAdd?.({
        id: newId,
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
        category: formCategory || null,
      });
      setSelectedId(newId);
      setIsNew(false);
      onShowNotification?.('Template added', 'success');
    } else {
      onUpdate?.(selectedId, {
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
        category: formCategory || null,
      });
      onShowNotification?.('Template updated', 'success');
    }
  }

  function handleDelete(id) {
    onDelete?.(id);
    if (selectedId === id) closePanel();
    onShowNotification?.('Template deleted', 'success');
  }

  function handleTest() {
    if (!testQuery.trim() || !formUrl) return;
    const encoded = formEncode
      ? encodeURIComponent(testQuery.trim()).replace(/%20/g, '+')
      : testQuery.trim();
    const finalUrl = formUrl.replace('{query}', encoded);
    window.electronAPI?.openExternal(finalUrl);
  }

  function handleNewClick() {
    if (atCap) return;
    setPresetFilter('');
    setShowPresets(true);
  }

  // ── Quick Action CRUD ──────────────────────────────────────────────────

  function selectQuickAction(qa) {
    setQaSelectedId(qa.id);
    setQaLabel(qa.label || '');
    setQaType(qa.type || 'app');
    setQaFormValue(qa.data || {});
    setQaCategory(qa.data?.category || null);
    setQaIsNew(false);
  }

  function openNewQuickAction() {
    setQaSelectedId(null);
    setQaLabel('');
    setQaType('app');
    setQaFormValue({});
    setQaCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setQaIsNew(true);
  }

  function closeQaPanel() {
    setQaSelectedId(null);
    setQaIsNew(false);
  }

  function handleQaSave() {
    if (!qaLabel.trim()) return;
    const data = { ...qaFormValue, category: qaCategory || null };
    if (qaIsNew) {
      const newId = crypto.randomUUID();
      onAddQuickAction?.({ id: newId, type: qaType, label: qaLabel.trim(), data });
      setQaSelectedId(newId);
      setQaIsNew(false);
      onShowNotification?.('Quick action added', 'success');
    } else {
      onUpdateQuickAction?.(qaSelectedId, { type: qaType, label: qaLabel.trim(), data });
      onShowNotification?.('Quick action updated', 'success');
    }
  }

  function handleQaDelete(id) {
    onDeleteQuickAction?.(id);
    if (qaSelectedId === id) closeQaPanel();
    onShowNotification?.('Quick action deleted', 'success');
  }

  // ── Category CRUD (matches TextExpansions exactly) ────────────────────

  function handleAddCategory(e) {
    e?.preventDefault?.();
    const name = newCategoryName.trim();
    if (name && !activeCats.some(c => c.name === name)) {
      activeCatHandlers.onAdd?.(name, newCategoryColour);
    }
    setNewCategoryName('');
    setNewCategoryColour(null);
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
      activeCatHandlers.onColour?.(catColourPopover.forCat, colour);
    }
    setCatColourPopover(null);
  }

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
    const tabRect = catContextTabRef.current?.getBoundingClientRect();
    if (tabRect) {
      const PICKER_WIDTH = 212;
      const left = Math.min(tabRect.left, window.innerWidth - PICKER_WIDTH - 8);
      setCatColourPopover({ forCat: catName, x: left, y: tabRect.bottom + 4 });
    } else {
      setCatColourPopover({ forCat: catName, x: catContextMenu.x, y: catContextMenu.y + 4 });
    }
    setCatContextMenu(null);
  }

  function ctxDelete() {
    if (!ctxDeleteConfirm) {
      setCtxDeleteConfirm(true);
      return;
    }
    activeCatHandlers.onDelete?.(catContextMenu.catName);
    if (activeCategory === catContextMenu.catName) setActiveCategory('All');
    setCatContextMenu(null);
    setCtxDeleteConfirm(false);
  }

  function commitCatRename() {
    const trimmed = renameValue.trim();
    if (!trimmed) { setRenameError('Name cannot be empty'); return; }
    if (trimmed !== renamingCat && activeCats.some(c => c.name === trimmed)) {
      setRenameError('Already exists'); return;
    }
    renameCommitting.current = true;
    if (trimmed !== renamingCat) {
      activeCatHandlers.onRename?.(renamingCat, trimmed);
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

  function handleCatDragEnd(event) {
    setCatDragId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIdx = activeCats.findIndex(c => c.name === active.id);
    const newIdx = activeCats.findIndex(c => c.name === over.id);
    if (oldIdx === -1 || newIdx === -1) return;
    activeCatHandlers.onReorder?.(arrayMove([...activeCats], oldIdx, newIdx));
  }

  // ── Filtered + grouped template list ──────────────────────────────────

  const uncategorisedCount = searchTemplates.filter(t => !t.category).length;

  const filteredTemplates = useMemo(() => {
    if (activeCategory === 'All') return searchTemplates;
    if (activeCategory === '__uncategorised__') return searchTemplates.filter(t => !t.category);
    return searchTemplates.filter(t => t.category === activeCategory);
  }, [searchTemplates, activeCategory]);

  const groupedList = useMemo(() => {
    if (activeCategory !== 'All') {
      return filteredTemplates.map(t => ({ type: 'item', template: t }));
    }
    const result = [];
    const uncat = searchTemplates.filter(t => !t.category);
    if (uncat.length > 0) {
      result.push({ type: 'header', label: 'Uncategorised', count: uncat.length });
      uncat.forEach(t => result.push({ type: 'item', template: t }));
    }
    for (const cat of categories) {
      const items = searchTemplates.filter(t => t.category === cat.name);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat.name, count: items.length, colour: cat.colour });
        items.forEach(t => result.push({ type: 'item', template: t }));
      }
    }
    return result;
  }, [searchTemplates, categories, activeCategory, filteredTemplates]);

  // Grouped presets for preset picker
  const filteredPresets = presetFilter
    ? PRESETS.filter(p => p.label.toLowerCase().includes(presetFilter.toLowerCase()) || p.trigger.includes(presetFilter.toLowerCase()))
    : PRESETS;

  const groupedPresets = useMemo(() => {
    const result = [];
    const catOrder = presetFilter ? [...new Set(filteredPresets.map(p => p.category))] : PRESET_CATEGORIES;
    for (const cat of catOrder) {
      const items = filteredPresets.filter(p => p.category === cat);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat });
        items.forEach(p => result.push({ type: 'preset', preset: p }));
      }
    }
    return result;
  }, [filteredPresets, presetFilter]);

  // Quick action filtered/grouped list
  const qaUncategorisedCount = quickActions.filter(a => !a.data?.category).length;

  const qaFilteredList = useMemo(() => {
    if (activeCategory === 'All') return quickActions;
    if (activeCategory === '__uncategorised__') return quickActions.filter(a => !a.data?.category);
    return quickActions.filter(a => a.data?.category === activeCategory);
  }, [quickActions, activeCategory]);

  const qaGroupedList = useMemo(() => {
    if (activeCategory !== 'All') {
      return qaFilteredList.map(a => ({ type: 'item', action: a }));
    }
    const result = [];
    const uncat = quickActions.filter(a => !a.data?.category);
    if (uncat.length > 0) {
      result.push({ type: 'header', label: 'Uncategorised', count: uncat.length });
      uncat.forEach(a => result.push({ type: 'item', action: a }));
    }
    for (const cat of qaCategories) {
      const items = quickActions.filter(a => a.data?.category === cat.name);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat.name, count: items.length, colour: cat.colour });
        items.forEach(a => result.push({ type: 'item', action: a }));
      }
    }
    return result;
  }, [quickActions, qaCategories, activeCategory, qaFilteredList]);

  const atCap = !isPro && searchTemplates.length >= 5;
  const editOpen = selectedId !== null || isNew;
  const canSave = formLabel.trim() && formTrigger && !triggerError && formUrl.includes('{query}');
  const qaEditOpen = qaSelectedId !== null || qaIsNew;
  const qaCanSave = !!qaLabel.trim() && (
    (qaType === 'url' && qaFormValue.url?.trim()) ||
    (qaType === 'app' && qaFormValue.path?.trim()) ||
    (qaType === 'folder' && qaFormValue.path?.trim()) ||
    (qaType === 'macro' && qaFormValue.steps?.length > 0)
  );

  // ── Render ──────────────────────────────────────────────────────────────

  return (
    <div className="stp-panel">
      {/* Header */}
      <div className="stp-header">
        <div className="stp-mode-tabs">
          <button
            className={`stp-mode-tab${panelMode === 'quickactions' ? ' active' : ''}`}
            onClick={() => { setPanelMode('quickactions'); closePanel(); setActiveCategory('All'); }}
            type="button"
          >⚡ Quick Actions</button>
          <button
            className={`stp-mode-tab${panelMode === 'templates' ? ' active' : ''}`}
            onClick={() => { setPanelMode('templates'); closeQaPanel(); setActiveCategory('All'); }}
            type="button"
          >⌕ Search Templates</button>
        </div>
        <div className="stp-header-right">
          {panelMode === 'templates' ? (
            atCap ? (
              <span className="stp-cap-nudge" title="Upgrade to Pro for unlimited templates">5/5 — Upgrade for more</span>
            ) : (
              <button className="stp-add-btn" onClick={handleNewClick} type="button">+ New Template</button>
            )
          ) : (
            <button className="stp-add-btn" onClick={openNewQuickAction} type="button">+ New Action</button>
          )}
        </div>
      </div>

      {/* Preset picker overlay */}
      {showPresets && (
        <div className="stp-presets-overlay">
          <div className="stp-presets">
            <div className="stp-presets-header">
              <span className="stp-presets-title">Choose a preset or create custom</span>
              <button className="stp-back-btn" onClick={() => setShowPresets(false)} type="button">Cancel</button>
            </div>
            <input
              className="stp-preset-filter"
              type="text"
              placeholder="Filter presets…"
              value={presetFilter}
              onChange={e => setPresetFilter(e.target.value)}
              spellCheck={false}
              autoFocus
            />
            <div className="stp-preset-list">
              {groupedPresets.map((entry) => {
                if (entry.type === 'header') {
                  return <div key={`ph-${entry.label}`} className="stp-preset-group-header">{entry.label}</div>;
                }
                const p = entry.preset;
                return (
                  <button key={p.trigger} className="stp-preset-row" onClick={() => openNewFromPreset(p)} type="button">
                    <span className="stp-preset-label">{p.label}</span>
                    <span className="stp-preset-trigger">{p.trigger}</span>
                    <span className="stp-preset-url">{truncateUrl(p.urlTemplate, 50)}</span>
                  </button>
                );
              })}
            </div>
            <button className="stp-custom-btn" onClick={openNewCustom} type="button">
              + Create custom template
            </button>
          </div>
        </div>
      )}

      {/* Body: sidebar + list + edit panel */}
      <div className="stp-body">
        {/* Category sidebar — switches between template and quick action categories based on mode */}
        <div className="stp-cat-sidebar">
          <div className="stp-cat-sidebar-list">
            <button
              className={`stp-cat-row${activeCategory === 'All' ? ' stp-cat-row-active' : ''}`}
              onClick={() => setActiveCategory('All')}
              type="button"
            >
              <span className="stp-cat-row-name">All</span>
              <span className="stp-cat-count">{panelMode === 'quickactions' ? quickActions.length : searchTemplates.length}</span>
            </button>

            {((panelMode === 'quickactions' ? qaUncategorisedCount : uncategorisedCount) > 0) && (
              <button
                className={`stp-cat-row stp-cat-row-uncategorised${activeCategory === '__uncategorised__' ? ' stp-cat-row-active' : ''}`}
                onClick={() => setActiveCategory('__uncategorised__')}
                type="button"
              >
                <span className="stp-cat-row-name">Uncategorised</span>
                <span className="stp-cat-count">{panelMode === 'quickactions' ? qaUncategorisedCount : uncategorisedCount}</span>
              </button>
            )}

            <DndContext sensors={catDndSensors} onDragStart={e => setCatDragId(e.active.id)} onDragEnd={handleCatDragEnd}>
              <SortableContext items={activeCats.map(c => c.name)} strategy={verticalListSortingStrategy}>
                {activeCats.map(cat => {
                  const catColour = cat.colour || null;
                  const count = panelMode === 'quickactions'
                    ? quickActions.filter(a => a.data?.category === cat.name).length
                    : searchTemplates.filter(t => t.category === cat.name).length;
                  return (
                    <SortableCatRow key={cat.name} id={cat.name}>
                      <div className="stp-cat-row-group" onContextMenu={e => handleCatContextMenu(e, cat.name)}>
                        {renamingCat === cat.name ? (
                          <div
                            className="stp-cat-row stp-cat-row-active stp-cat-rename-wrap"
                            style={catColour ? { '--cat-color': catColour } : {}}
                          >
                            <input
                              ref={renameInputRef}
                              className="stp-cat-rename-input"
                              value={renameValue}
                              onChange={e => { setRenameValue(e.target.value); setRenameError(''); }}
                              onKeyDown={e => {
                                if (e.key === 'Enter')  { e.preventDefault(); commitCatRename(); }
                                if (e.key === 'Escape') { e.preventDefault(); cancelCatRename(); }
                                e.stopPropagation();
                              }}
                              onBlur={cancelCatRename}
                            />
                            {renameError && <span className="stp-cat-rename-error">{renameError}</span>}
                          </div>
                        ) : (
                          <button
                            className={`stp-cat-row${activeCategory === cat.name ? ' stp-cat-row-active' : ''}`}
                            style={catColour ? { '--cat-color': catColour } : {}}
                            onClick={() => setActiveCategory(cat.name)}
                            type="button"
                          >
                            <span
                              className="stp-cat-dot stp-cat-dot-pick"
                              style={catColour ? { background: catColour } : {}}
                              onClick={e => openCatColourPopover(e, cat.name)}
                              title="Change colour"
                            />
                            <span className="stp-cat-row-name">{cat.name}</span>
                            <span className="stp-cat-count">{count}</span>
                          </button>
                        )}
                      </div>
                    </SortableCatRow>
                  );
                })}
              </SortableContext>
              <DragOverlay>
                {catDragId ? (
                  <div className="stp-cat-row-group stp-cat-row-ghost">
                    <button className="stp-cat-row stp-cat-row-active" type="button">{catDragId}</button>
                  </div>
                ) : null}
              </DragOverlay>
            </DndContext>

            {addingCategory ? (
              <form onSubmit={handleAddCategory} className="stp-cat-add-form">
                <span
                  className="stp-cat-add-colour-dot"
                  style={newCategoryColour ? { background: newCategoryColour } : {}}
                  onMouseDown={e => e.preventDefault()}
                  onClick={e => openCatColourPopover(e, '__new__')}
                  title="Pick a colour (optional)"
                />
                <input
                  autoFocus
                  className="stp-cat-add-input"
                  value={newCategoryName}
                  onChange={e => setNewCategoryName(e.target.value)}
                  placeholder="Category name…"
                  onBlur={handleAddCategory}
                  onKeyDown={e => e.key === 'Escape' && setAddingCategory(false)}
                />
              </form>
            ) : (
              <button className="stp-cat-new-btn" onClick={() => { setAddingCategory(true); setNewCategoryColour(null); }} type="button">
                + Add Category
              </button>
            )}
          </div>
        </div>

        {/* ═══ TEMPLATES MODE ═══ */}
        {panelMode === 'templates' && (<>
        <div className="stp-list">
          {searchTemplates.length === 0 && !isNew ? (
            <div className="stp-empty-state">
              <div className="stp-empty-icon">⌕</div>
              <div className="stp-empty-heading">No search templates yet</div>
              <div className="stp-empty-sub">Add one to search Google, GitHub, or your own URLs from Quick Search.</div>
              <button className="stp-add-btn stp-empty-cta" onClick={handleNewClick} type="button">+ New Template</button>
            </div>
          ) : (
            groupedList.map((entry) => {
              if (entry.type === 'header') {
                return (
                  <div key={`gh-${entry.label}`} className="stp-group-header">
                    {entry.colour && <span className="stp-cat-dot" style={{ background: entry.colour }} />}
                    <span className="stp-group-name">{entry.label.toUpperCase()}</span>
                    <span className="stp-group-count">{entry.count}</span>
                    <span className="stp-group-rule" />
                  </div>
                );
              }
              const t = entry.template;
              return (
                <div key={t.id} className={`stp-item${selectedId === t.id ? ' active' : ''}`} onClick={() => selectTemplate(t)}>
                  <span className="stp-item-trigger">{t.trigger}</span>
                  <span className="stp-item-label">{t.label}</span>
                  <span className="stp-item-url">{truncateUrl(t.url_template, 40)}</span>
                  <div className="stp-item-actions">
                    <button className="stp-item-del" onClick={e => { e.stopPropagation(); handleDelete(t.id); }} title="Delete" type="button">✕</button>
                  </div>
                </div>
              );
            })
          )}
        </div>
        {editOpen ? (
          <div className="stp-edit-panel">
            <div className="stp-ep-header">
              <span className="stp-ep-title">{isNew ? 'New Template' : 'Edit Template'}</span>
              <button className="stp-ep-close" onClick={closePanel} type="button">✕</button>
            </div>
            <div className="stp-ep-fields">
              <div className="stp-field">
                <label className="stp-label">Label</label>
                <input className="stp-input" type="text" value={formLabel} onChange={e => setFormLabel(e.target.value)} placeholder="e.g. Google" spellCheck={false} />
              </div>
              <div className="stp-field">
                <label className="stp-label">Trigger</label>
                <input className={`stp-input stp-trigger-input${triggerError ? ' error' : ''}`} type="text" value={formTrigger}
                  onChange={e => { const v = e.target.value.toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 10); setFormTrigger(v); setTriggerError(validateTrigger(v, isNew ? null : selectedId)); }}
                  placeholder="e.g. g" spellCheck={false} maxLength={10} />
                {triggerError && <div className="stp-trigger-error">{triggerError}</div>}
                <div className="stp-field-hint">Type this in Quick Search + Space to activate</div>
              </div>
              <div className="stp-field">
                <label className="stp-label">Category</label>
                <select className="stp-input stp-cat-select" value={formCategory || ''} onChange={e => setFormCategory(e.target.value || null)}>
                  <option value="">Uncategorised</option>
                  {categories.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
                </select>
              </div>
              <div className="stp-field">
                <div className="stp-label-row">
                  <label className="stp-label">URL Template</label>
                  <button className="stp-help-btn" onClick={() => setShowHelp(v => !v)} type="button" title="How to find the right URL">?</button>
                </div>
                {showHelp && (
                  <div className="stp-help-popover" ref={helpRef}>
                    <p><strong>How to find the right URL pattern:</strong></p>
                    <p>1. Go to the website and search for a word (e.g. "test")</p>
                    <p>2. Copy the URL from your browser's address bar</p>
                    <p>3. Paste it here and replace "test" with <code>{'{query}'}</code></p>
                    <p className="stp-help-example">Example: https://google.com/search?q=test becomes https://google.com/search?q={'{query}'}</p>
                  </div>
                )}
                <input className="stp-input stp-url-input" type="text" value={formUrl} onChange={e => setFormUrl(e.target.value)} placeholder="https://example.com/search?q={query}" spellCheck={false} />
                {formUrl && !formUrl.includes('{query}') && <div className="stp-trigger-error">URL must contain {'{query}'} placeholder</div>}
                {formUrl && formUrl.includes('{query}') && <div className="stp-preview-line">Example: typing "tauri" would open {truncateUrl(buildPreviewUrl(formUrl, 'tauri'), 80)}</div>}
              </div>
              <div className="stp-field">
                <label className="stp-toggle-label">
                  <input type="checkbox" checked={formEncode} onChange={e => setFormEncode(e.target.checked)} />
                  URL-encode query
                </label>
              </div>
            </div>
            <div className="stp-ep-test">
              <input className="stp-input stp-test-input" type="text" value={testQuery} onChange={e => setTestQuery(e.target.value)} placeholder="Test query…" spellCheck={false} onKeyDown={e => { if (e.key === 'Enter') handleTest(); }} />
              <button className="stp-test-btn" onClick={handleTest} disabled={!testQuery.trim() || !formUrl.includes('{query}')} type="button">Test</button>
            </div>
            <div className="stp-ep-footer">
              <button className="stp-save-btn" onClick={handleSave} disabled={!canSave} type="button">{isNew ? 'Add Template' : 'Save Changes'}</button>
              {!isNew && <button className="stp-delete-btn" onClick={() => handleDelete(selectedId)} type="button">Delete</button>}
            </div>
          </div>
        ) : (
          <div className="stp-edit-panel stp-panel-idle">
            <span className="stp-idle-text">Select a template to edit, or add a new one</span>
          </div>
        )}
        </>)}

        {/* ═══ QUICK ACTIONS MODE ═══ */}
        {panelMode === 'quickactions' && (<>
        <div className="stp-list">
          {quickActions.length === 0 && !qaIsNew ? (
            <div className="stp-empty-state">
              <div className="stp-empty-icon">⚡</div>
              <div className="stp-empty-heading">No quick actions yet</div>
              <div className="stp-empty-sub">Add actions accessible via Quick Search without assigning a hotkey. Open folders, URLs, apps, or type text.</div>
              <button className="stp-add-btn stp-empty-cta" onClick={openNewQuickAction} type="button">+ New Action</button>
            </div>
          ) : (
            qaGroupedList.map((entry) => {
              if (entry.type === 'header') {
                return (
                  <div key={`qh-${entry.label}`} className="stp-group-header">
                    {entry.colour && <span className="stp-cat-dot" style={{ background: entry.colour }} />}
                    <span className="stp-group-name">{entry.label.toUpperCase()}</span>
                    <span className="stp-group-count">{entry.count}</span>
                    <span className="stp-group-rule" />
                  </div>
                );
              }
              const a = entry.action;
              const typeIcons = { url: '⊕', app: '⬡', folder: '⬢', text: '✦', hotkey: '⌨', macro: '◈' };
              const typeColors = { url: '#ffc832', app: '#50c878', folder: '#40c8a0', text: '#64b4ff', hotkey: '#c864ff', macro: '#ff783c' };
              const preview = a.type === 'macro'
                ? `Sequence (${a.data?.steps?.length || 0} step${(a.data?.steps?.length || 0) !== 1 ? 's' : ''})`
                : (a.data?.url || a.data?.path || a.data?.folderPath || a.data?.urlName || a.data?.appName || '');
              return (
                <div key={a.id} className={`stp-item${qaSelectedId === a.id ? ' active' : ''}`} onClick={() => selectQuickAction(a)}>
                  <span className="stp-item-type-icon" style={{ color: typeColors[a.type] || '#8a8799' }}>{typeIcons[a.type] || '◈'}</span>
                  <span className="stp-item-label">{a.label}</span>
                  <span className="stp-item-url">{truncateUrl(preview, 40)}</span>
                  <div className="stp-item-actions">
                    <button className="stp-item-del" onClick={e => { e.stopPropagation(); handleQaDelete(a.id); }} title="Delete" type="button">✕</button>
                  </div>
                </div>
              );
            })
          )}
        </div>
        {qaEditOpen ? (
          <div className="stp-edit-panel stp-qa-edit">
            <div className="stp-ep-header">
              <span className="stp-ep-title">{qaIsNew ? 'New Quick Action' : 'Edit Quick Action'}</span>
              <button className="stp-ep-close" onClick={closeQaPanel} type="button">✕</button>
            </div>
            <div className="stp-qa-body">
              {/* Action type selector — matches MacroPanel type-selector */}
              <div className="type-selector">
                {QA_ACTION_TYPES.map(t => (
                  <button
                    key={t.id}
                    className={`type-btn${qaType === t.id ? ' active' : ''}`}
                    style={{ '--type-color': t.color }}
                    onClick={() => { setQaType(t.id); setQaFormValue({}); }}
                    type="button"
                  >
                    <span className="type-btn-icon">{t.icon}</span>
                    <span className="type-btn-label">{t.label}</span>
                  </button>
                ))}
              </div>

              {/* Type description */}
              <div className="type-desc">
                {QA_ACTION_TYPES.find(t => t.id === qaType)?.desc}
              </div>

              {/* Dynamic form per type */}
              <div className="form-body">
                {qaType === 'url' && (
                  <div className="form-section">
                    <label className="form-label">URL to open</label>
                    <input
                      className="form-input"
                      placeholder="https://example.com"
                      value={qaFormValue.url || ''}
                      onChange={e => setQaFormValue(prev => ({ ...prev, url: e.target.value }))}
                    />
                  </div>
                )}
                {qaType === 'app' && (
                  <div className="form-section">
                    <label className="form-label">Application path</label>
                    <div className="file-input-row">
                      <input
                        className="form-input"
                        placeholder="C:\Program Files\App\app.exe"
                        value={qaFormValue.path || ''}
                        readOnly
                      />
                      <button className="browse-btn" type="button" onClick={async () => {
                        const path = await window.electronAPI?.browseForFile();
                        if (path) setQaFormValue(prev => ({ ...prev, path }));
                      }}>Browse</button>
                    </div>
                  </div>
                )}
                {qaType === 'folder' && (
                  <div className="form-section">
                    <label className="form-label">Folder path</label>
                    <div className="file-input-row">
                      <input
                        className="form-input"
                        placeholder="C:\Users\Me\Documents"
                        value={qaFormValue.path || ''}
                        readOnly
                      />
                      <button className="browse-btn" type="button" onClick={async () => {
                        const path = await window.electronAPI?.browseForFolder();
                        if (path) setQaFormValue(prev => ({ ...prev, path }));
                      }}>Browse</button>
                    </div>
                  </div>
                )}
                {qaType === 'macro' && (
                  <MacroSequenceForm value={qaFormValue} onChange={setQaFormValue} globalInputMethod={globalInputMethod} />
                )}

                {/* Display label */}
                <div className="form-section" style={{ marginTop: 4 }}>
                  <label className="form-label">Display label</label>
                  <input
                    className="form-input"
                    placeholder="Short label for Quick Search..."
                    value={qaLabel}
                    onChange={e => setQaLabel(e.target.value)}
                  />
                </div>

                {/* Category */}
                <div className="form-section" style={{ marginTop: 4 }}>
                  <label className="form-label">Category</label>
                  <select className="form-select" style={{ width: 'auto', minWidth: 140 }} value={qaCategory || ''} onChange={e => setQaCategory(e.target.value || null)}>
                    <option value="">Uncategorised</option>
                    {qaCategories.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
                  </select>
                </div>
              </div>
            </div>
            <div className="macro-panel-footer">
              {!qaIsNew && (
                <div className="footer-assignment-actions">
                  <button className="btn-clear" onClick={() => handleQaDelete(qaSelectedId)} type="button">Delete</button>
                </div>
              )}
              <button className="btn-save" onClick={handleQaSave} disabled={!qaCanSave} type="button">
                {qaIsNew ? 'Add Action' : 'Save Changes'}
              </button>
            </div>
          </div>
        ) : (
          <div className="stp-edit-panel stp-panel-idle">
            <span className="stp-idle-text">Select an action to edit, or add a new one</span>
          </div>
        )}
        </>)}
      </div>

      {/* Category right-click context menu (portal) */}
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

      {/* Category colour picker popover (portal) */}
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
                : activeCats.find(c => c.name === catColourPopover.forCat)?.colour || null
            }
            onChange={handleCatColourSelect}
          />
        </div>,
        document.body
      )}
    </div>
  );
}
