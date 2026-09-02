import React, { useState, useCallback, useEffect, useRef, useMemo, lazy, Suspense } from 'react';
import './styles/global.css';
import './styles/app.css';
import { readVoicePhrases, writeVoicePhrases } from './voicePhrases';
import TitleBar from './components/TitleBar';
import Sidebar from './components/Sidebar';
import KeyboardCanvas, { comboString } from './components/KeyboardCanvas';
import MouseCanvas from './components/MouseCanvas';
import MacroPanel from './components/MacroPanel';
import StatusBar from './components/StatusBar';
import Toaster from './components/Toaster';
// Main-window inner-tab panels are code-split so an autostart-into-tray user
// who never opens the window (or only lives in Triggers) never loads them.
// Once opened they stay resident for the session — instant on later switches.
const TextExpansions = lazy(() => import('./components/TextExpansions'));
import WelcomeModal from './components/WelcomeModal';
import UpgradeModal from './components/UpgradeModal';
import ReservedShortcutModal from './components/ReservedShortcutModal';
import AppNotRunningModal from './components/AppNotRunningModal';
import { findReservedShortcut, formatComboDisplay } from './utils/reservedShortcuts';
import { useModalKeyboard } from './hooks/useModalKeyboard';
import OnboardingTour from './components/OnboardingTour';
import ProTrialModal from './components/ProTrialModal';
import TrialEndModal from './components/TrialEndModal';
import TemplatesCoachmark from './components/TemplatesCoachmark';
import QuickTips from './components/QuickTips';
const AnalyticsPanel = lazy(() => import('./components/AnalyticsPanel'));
const ClipboardPanel = lazy(() => import('./components/ClipboardPanel'));
const SearchTemplatesPanel = lazy(() => import('./components/SearchTemplatesPanel'));
const RadialEditorView = lazy(() => import('./components/RadialEditorView'));
import { DndContext, PointerSensor, useSensor, useSensors, DragOverlay, pointerWithin } from '@dnd-kit/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen as listenEvent, emit as emitEvent } from '@tauri-apps/api/event';
import { MAX_SLOTS } from './components/RadialWheel';
import { downscaleIconDataUrl, ICON_DOWNSCALE_THRESHOLD } from './components/iconUtils';
import { friendlyKeyName, setLiveKeyLegends, STATIC_BARE_ALLOWED } from './components/keyboardLayout';
import { parseEspansoYaml, parseAhkHotstrings, parseTextExpanderCsv, parseTextBlazeJson } from './importAdapters';

// Bump whenever the onboarding tour changes meaningfully. Existing users whose
// `onboarding_version_seen` is below this value will see the tour again on
// their next launch — used so v0.4.4 → v0.4.5 upgraders see the new Pro
// callouts, app-profile pitch, and trial offer at the end of the tour.
const ONBOARDING_VERSION = 3;

// ── Trial modal predicates (module scope, pure) ───────────────────────────
// The 14-day Pro trial is STARTED by Rust (`licence::init`) on first launch;
// the frontend only announces it and, later, shows the one-shot end modal.
// Announcement: trial live, never announced, no real key.
const trialAnnouncePending = (ls) => !!ls && !ls.key_entered && ls.trial_active && !ls.trial_offer_shown;
// End modal: trial was started here, has lapsed, no key, not yet shown.
// `trial_started_at` guards the key-consumed case (activation clears it).
const trialJustEnded = (ls) => !!ls && !ls.is_pro && !ls.key_entered && ls.trial_used
  && !ls.trial_active && !!ls.trial_started_at && !ls.trial_end_shown;

// ── Storage-key helpers (module scope, shared by the assignment handlers) ──
// A key can hold up to three independent trigger variants, each under its own
// storage key: base (single press), base::double, base::hold. Any operation
// that moves an assignment must carry every variant that exists.
const ASSIGNMENT_VARIANT_SUFFIXES = ['', '::double', '::hold'];
// Press-mode name (as MacroPanel knows it) → storage-key suffix.
const PRESS_MODE_SUFFIX = { single: '', double: '::double', hold: '::hold' };
const PRESS_MODE_LABEL = { single: 'single press', double: 'double press', hold: 'hold' };

// Unassigned library entries live at "{Profile}::UNASSIGNED::{uuid}" in the
// same assignments map. This is THE predicate — don't hand-roll the check.
const isLibraryKey = (k) => k.includes('::UNASSIGNED::');

// Move every press-mode variant living at fromBase to toBase (mutates map).
// Returns how many variants were carried.
function moveVariantsInMap(map, fromBase, toBase) {
  let carried = 0;
  for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
    if (map[fromBase + suffix]) {
      map[toBase + suffix] = map[fromBase + suffix];
      delete map[fromBase + suffix];
      carried++;
    }
  }
  return carried;
}

// Displace whatever occupies targetBase into a fresh Unassigned entry
// (mutates map). Returns the new library base key, or null if the target was
// empty. This carries the feature's core promise: binding or dropping onto an
// occupied trigger never destroys the displaced action.
function displaceToLibraryInMap(map, targetBase, profile) {
  if (!ASSIGNMENT_VARIANT_SUFFIXES.some(s => map[targetBase + s])) return null;
  const newBase = `${profile}::UNASSIGNED::${crypto.randomUUID()}`;
  moveVariantsInMap(map, targetBase, newBase);
  return newBase;
}

// keyMap of fromBase→toBase across all variant suffixes — feed to
// remapRadialStorageKeys so radial wedges follow storage-key rewrites.
function variantKeyMap(fromBase, toBase) {
  const m = {};
  for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) m[fromBase + suffix] = toBase + suffix;
  return m;
}

// Human-readable trigger label ("Ctrl+Alt+K", bare keys show just the key).
function triggerLabel(combo, keyId) {
  const keyLabel = friendlyKeyName(keyId);
  return combo === 'BARE' ? keyLabel : `${combo}+${keyLabel}`;
}

// Confirm dialog for drag-drops onto an occupied and/or reserved trigger.
// A real component (not inline render JSX) so it can run useModalKeyboard —
// ESC dismisses THIS modal instead of leaking to the document-level ESC
// handler, and focus is trapped like every other modal. Styling reuses the
// reserved-shortcut-* classes (ReservedShortcutModal.css loads via App's
// static ReservedShortcutModal import).
function DropConfirmModal({ drop, onConfirm, onCancel }) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onCancel);
  const comboLabel = triggerLabel(drop.targetCombo, drop.targetKeyId);
  const confirmLabel = drop.conflictLabel ? (drop.mode === 'bind' ? 'Replace' : 'Swap') : 'Bind Anyway';
  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" onClick={onCancel}>
      <div className="modal-panel drop-confirm-modal" ref={panelRef} onClick={e => e.stopPropagation()}>
        <h1 className="reserved-shortcut-title">
          {drop.conflictLabel
            ? `${comboLabel} already fires "${drop.conflictLabel}"`
            : `${comboLabel} is a reserved Windows shortcut`}
        </h1>
        {drop.conflictLabel && (
          <p className="reserved-shortcut-body">
            {drop.mode === 'bind'
              ? 'Replace it? Its current action will move to Unassigned, not be deleted.'
              : 'Swap the two actions? The action on the target key moves to the key you dragged from.'}
          </p>
        )}
        {drop.reservedOsFunction && (
          <p className="reserved-shortcut-body">
            {comboLabel} is the Windows {drop.reservedOsFunction} shortcut. Mapping it will shadow
            that shortcut while this profile is active.
          </p>
        )}
        <div className="reserved-shortcut-actions">
          <button className="reserved-shortcut-cancel-btn" type="button" onClick={onCancel}>Cancel</button>
          <button className="reserved-shortcut-continue-btn" type="button" onClick={onConfirm}>{confirmLabel}</button>
        </div>
      </div>
    </div>
  );
}

// Extra radial layouts from config: [{ id, name, itemsByProfile }] (Pro,
// per-device wheels). Drops malformed entries so one bad sync can't take the
// radial editor down. 'default' is reserved for the legacy map.
function sanitizeRadialLayouts(raw) {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter(l => l && typeof l.id === 'string' && l.id && l.id !== 'default')
    .map(l => ({
      id: l.id,
      name: typeof l.name === 'string' && l.name.trim() ? l.name : 'Layout',
      itemsByProfile: (l.itemsByProfile && typeof l.itemsByProfile === 'object' && !Array.isArray(l.itemsByProfile)) ? l.itemsByProfile : {},
    }));
}

function App() {
  const [assignments, setAssignments]       = useState({});
  const [selectedKey, setSelectedKey]       = useState(null);
  const [activeProfile, setActiveProfile]   = useState('Default');
  const [profiles, setProfiles]             = useState(['Default']);
  const [profileSettings, setProfileSettings] = useState({}); // { profileName: { linkedApp: '...' } }
  const [macrosEnabled, setMacrosEnabled]   = useState(true);
  const [toasts, setToasts]                 = useState([]);
  const [activeModifiers, setActiveModifiers] = useState([]);  // e.g. ['Ctrl', 'Alt']
  // Duplicate draft: when the user right-clicks → Duplicate from the sidebar,
  // the cloned action lives in draftAssignment (plus draftDoubleAssignment)
  // state until they pick a destination key. See handleDuplicateFromContext.
  const [draftAssignment, setDraftAssignment] = useState(null);
  const [draftDoubleAssignment, setDraftDoubleAssignment] = useState(null);
  // Bumped by the sidebar's right-click → Duplicate to open MacroPanel's
  // duplicate-capture overlay for the freshly-selected source item.
  const [duplicateOverlaySignal, setDuplicateOverlaySignal] = useState(0);
  // Unassigned library selection — mutually exclusive with selectedKey.
  // Unassigned entries live in the same assignments map under
  // "{Profile}::UNASSIGNED::{uuid}" (+ ::double / ::hold variant suffixes).
  // The UNASSIGNED combo segment can never be constructed by the Rust
  // matcher's build_modifier_combo(), so these entries are unreachable by
  // hotkeys by construction — no engine changes needed.
  const [selectedLibraryId, setSelectedLibraryId] = useState(null);
  // Bumped by the sidebar's "Bind to key…" context item to open MacroPanel's
  // bind-capture overlay for the freshly-selected unassigned entry.
  const [bindOverlaySignal, setBindOverlaySignal] = useState(0);
  const [sidebarComboFilter, setSidebarComboFilter] = useState(null); // null = show all, string = filter by combo
  // Reserved Windows shortcut hazard modal — deferred-save pattern. Shape:
  // { keyId, macro, comboDisplay, osFunction, profileName } or null.
  const [reservedShortcutPending, setReservedShortcutPending] = useState(null);
  // { exe, hint } when a distilled Record Macro tried to fire but its bound
  // target app wasn't running. Backend emits `record-macro-app-missing`;
  // useEffect below wires the listener once at mount.
  const [appMissingModal, setAppMissingModal] = useState(null);
  const [engineStatus, setEngineStatus]     = useState({ uiohookAvailable: false, nutjsAvailable: false });
  const [lastFired, setLastFired]           = useState(null);
  // theme: 'auto' | 'light' | 'dark'. 'auto' follows the OS via prefers-color-scheme.
  // resolvedTheme is the actually-applied theme ('light' or 'dark') used for the
  // data-theme attribute and any UI that needs to know what's currently shown.
  const [theme, setTheme]                   = useState('auto');
  const [resolvedTheme, setResolvedTheme]   = useState('dark');
  const [expansionCategories, setExpansionCategories] = useState([]);
  const [globalVariables, setGlobalVariables]         = useState({});   // { 'my.name': 'Jane Smith', … }
  const [activeView, setActiveView]                 = useState('keyboard'); // 'keyboard' | 'mouse'
  const [activeArea, setActiveArea]                 = useState('mapping');  // 'mapping' | 'expansions' | 'analytics'
  // One-shot prefill seed for the text-expansion new-form, set when the user
  // clicks "Create Expansion" on a clipboard history item. TextExpansions
  // consumes it on mount and clears it via onPrefillConsumed.
  const [pendingExpansionPrefill, setPendingExpansionPrefill] = useState(null);
  // Sub-panel editing flags lifted from TextExpansions / SearchTemplatesPanel.
  // Combined with selectedKey / draftAssignment (mapping + radial via MacroPanel)
  // to suppress foreground auto-switching while the user is mid-edit.
  const [expansionEditing, setExpansionEditing]     = useState(false);
  const [quickActionEditing, setQuickActionEditing] = useState(false);
  const [isRecording, setIsRecording]               = useState(false);
  const [recordCapture, setRecordCapture]           = useState(null);
  const [tipsHidden, setTipsHidden]                 = useState(false);
  // Per-feature TIP box dismissals (radial/templates/expansions/clipboard).
  // Array of tip keys; reset via Settings "Show feature tips again".
  const [hiddenTips, setHiddenTips]                 = useState([]);
  const [firstLaunchDate, setFirstLaunchDate]       = useState(null);
  const [backupRestoredFrom, setBackupRestoredFrom] = useState(null); // non-null = show banner
  const [activeGlobalProfile, setActiveGlobalProfile] = useState('Default');
  const [autocorrectEnabled, setAutocorrectEnabled] = useState(false);
  const [autocorrectBuiltinTypos, setAutocorrectBuiltinTypos] = useState(false);
  const [autocorrectDoubleCaps, setAutocorrectDoubleCaps] = useState(false);
  const [autocorrectDoubleCapsExceptions, setAutocorrectDoubleCapsExceptions] = useState([]);
  const [autocorrectCapsLockFix, setAutocorrectCapsLockFix] = useState(false);
  const [autocorrectSentenceCaps, setAutocorrectSentenceCaps] = useState(false);
  const [autocorrectExtendedTypos, setAutocorrectExtendedTypos] = useState(false);
  const [autocorrectDays, setAutocorrectDays] = useState(false);
  const [autocorrectSymbols, setAutocorrectSymbols] = useState(false);
  const [autocorrectEmojis, setAutocorrectEmojis] = useState(false);
  const [autocorrectExcludedApps, setAutocorrectExcludedApps] = useState([]);
  // Apps where text expansions never fire (separate from autocorrect exclusions).
  const [expansionExcludedApps, setExpansionExcludedApps] = useState([]);
  // Individual bundled-dictionary entries the user switched off (lowercase typo keys).
  const [autocorrectDisabledEntries, setAutocorrectDisabledEntries] = useState([]);
  // Backspace-undo tracking: { [originalLower]: { count, replacement, source } }.
  // At 2 undos of the same word (and not muted) the Autocorrect tab offers
  // "stop correcting this" — never applied silently.
  const [autocorrectUndoCounts, setAutocorrectUndoCounts] = useState({});
  const [autocorrectUndoMuted, setAutocorrectUndoMuted] = useState([]);
  const [acImportPrompt, setAcImportPrompt] = useState(null);
  const [showWelcome, setShowWelcome]               = useState(false);
  const [showOnboarding, setShowOnboarding]         = useState(false);
  const [macrosEnabledOnStartup, setMacrosEnabledOnStartup] = useState(true);
  // On-screen keyboard shape. 'auto' = guess from the Windows input language
  // (Rust get_keyboard_layout_hint), upgraded to ISO for good the first time
  // the ISO-only key beside left Shift is pressed. Both persisted in config.
  const [physicalKeyboardLayout, setPhysicalKeyboardLayout] = useState('auto'); // 'auto' | 'ansi' | 'iso'
  const [isoKeyDetected, setIsoKeyDetected] = useState(false);
  const [keyboardLayoutHint, setKeyboardLayoutHint] = useState('ansi');
  // Live legends by canvas slot from the Windows input layout (Phase B).
  const [keyboardLegends, setKeyboardLegends] = useState(null);
  const resolvedPhysicalLayout = physicalKeyboardLayout === 'auto'
    ? (isoKeyDetected ? 'iso' : keyboardLayoutHint)
    : physicalKeyboardLayout;
  // Clipboard privacy controls. Defaults are permissive so existing installs
  // behave unchanged; users opt in via Settings.
  const [clipboardCaptureEnabled, setClipboardCaptureEnabled] = useState(true);
  const [clipboardExcludedApps, setClipboardExcludedApps]     = useState([]);
  // Anonymous-aggregate telemetry. Default ON during beta; persisted via
  // trigr-local-settings.json (machine-local, not in shared config). The Rust
  // side reads on every 6h tick so the flag takes effect immediately.
  const [telemetryEnabled, setTelemetryEnabled] = useState(true);
  const [globalInputMethod,  setGlobalInputMethod]  = useState('direct');
  const [macroSpeed,         setMacroSpeed]         = useState('safe');
  const [defaultDateFormat,  setDefaultDateFormat]  = useState('DD/MM/YYYY');
  const [keystrokeDelay,     setKeystrokeDelay]     = useState(10);
  const [macroTriggerDelay,  setMacroTriggerDelay]  = useState(10);
  const [searchOverlayHotkey,       setSearchOverlayHotkey]       = useState('Ctrl+Space');
  // Master on/off for Quick Search (mirrors clipboardCaptureEnabled). Off =
  // hotkey unregistered in the engine so the combo passes through to the app.
  const [searchOverlayEnabled,      setSearchOverlayEnabled]      = useState(true);
  const [voiceEnabled,              setVoiceEnabled]              = useState(false);
  const [voiceHotkey,               setVoiceHotkey]               = useState('');
  const [voiceMicId,               setVoiceMicId]               = useState('');
  const [overlayShowAll,             setOverlayShowAll]             = useState(true);
  const [overlayCloseAfterFiring,    setOverlayCloseAfterFiring]    = useState(true);
  const [overlayIncludeAutocorrect,  setOverlayIncludeAutocorrect]  = useState(false);
  const [clipboardPreviewWidth,      setClipboardPreviewWidth]      = useState(480);
  const [clipboardColumnMode,        setClipboardColumnMode]        = useState('auto');
  const [doubleTapWindow,            setDoubleTapWindow]            = useState(300);
  const [holdThresholdMs,            setHoldThresholdMs]            = useState(350);
  const [fireOnPress,                setFireOnPress]                = useState(false);
  const [updateInfo,     setUpdateInfo]     = useState(null);   // { version, percent, ready, dismissed }
  const [appVersion,     setAppVersion]     = useState('');
  const [globalPauseToggleKey, setGlobalPauseToggleKey] = useState(null);
  // Clipboard quick-paste overlay hotkey. Defaults to Ctrl+Shift+V; user
  // remappable in Settings to avoid clashes with other tools they use.
  const [clipboardPasteHotkey, setClipboardPasteHotkey] = useState('Ctrl+Shift+V');
  const [importPrompt, setImportPrompt]                 = useState(null); // { name, assignments }
  // Expansion pack import collision prompt. Shape:
  // { expansions: [{trigger, data}], categories: [{name, colour}], collisions: [trigger,...], totalCount }
  const [expansionImportPrompt, setExpansionImportPrompt] = useState(null);
  // Quick action pack import collision prompt. Shape:
  // { actions: [{id, type, label, data}], categories: [{name, colour}], collisions: [{id, label},...], totalCount }
  const [quickActionImportPrompt, setQuickActionImportPrompt] = useState(null);
  const [licenceStatus, setLicenceStatus]               = useState({ is_pro: false, key_entered: false, status: 'no_key', product_name: '', expires_at: null, email: null, key_id: null, trial_active: false, trial_days_remaining: 0, trial_used: false, trial_offer_shown: false, trial_started_at: null, trial_end_shown: false });
  // Live mirror for the focus-revalidation handler (registered once) so it can
  // detect the Pro → Free transition when a trial ends.
  const licenceStatusRef = useRef(licenceStatus);
  useEffect(() => { licenceStatusRef.current = licenceStatus; }, [licenceStatus]);
  // Shared-config grace period state, populated from Rust via getGracePeriodState.
  // Shape: { pro_expired_at, shared_active, days_remaining, migration_deferred }.
  // When pro_expired_at is non-null AND shared_active is true, the banner shows.
  const [gracePeriodState, setGracePeriodState]         = useState(null);
  // Transient banner shown for one session after auto-migration completes.
  const [postMigrationNotice, setPostMigrationNotice]   = useState(false);
  const [upgradePrompt, setUpgradePrompt]               = useState(null); // feature name string, or null
  const [showProTrialModal, setShowProTrialModal]       = useState(false);
  const [showTrialEndModal, setShowTrialEndModal]       = useState(false);
  // `get_trial_usage` result for the end modal ({ triggers, autocorrect });
  // null while the query is in flight.
  const [trialUsage, setTrialUsage]                     = useState(null);
  const trialEndOpenRef = useRef(false); // focus revalidation fires often; open once
  const openTrialEnd = useCallback((ls) => {
    if (trialEndOpenRef.current) return;
    trialEndOpenRef.current = true;
    setTrialUsage(null);
    setShowTrialEndModal(true);
    const empty = { triggers: [], autocorrect: 0 };
    const since = ls?.trial_started_at;
    if (since && window.electronAPI?.getTrialUsage) {
      window.electronAPI.getTrialUsage(since)
        .then((u) => setTrialUsage(u && Array.isArray(u.triggers) ? u : empty))
        .catch(() => setTrialUsage(empty));
    } else {
      setTrialUsage(empty);
    }
  }, []);
  const closeTrialEnd = useCallback(() => {
    setShowTrialEndModal(false);
    trialEndOpenRef.current = false;
    window.electronAPI?.markTrialEndShown?.().then((s) => { if (s) setLicenceStatus(s); });
  }, []);
  // Templates coachmark — drops down from the Templates pill once after the
  // onboarding tour + trial offer have settled. Anchored via templatesPillRef
  // (passed into TitleBar). openTemplatesSignal is a nonce: incrementing it
  // tells TitleBar to open its Templates dropdown.
  const [showTemplatesNudge, setShowTemplatesNudge]     = useState(false);
  const [templatesNudgeSeen, setTemplatesNudgeSeen]     = useState(true); // default true → don't fire until config loads and tells us otherwise
  const [templatesPillRect, setTemplatesPillRect]       = useState(null);
  const [openTemplatesSignal, setOpenTemplatesSignal]   = useState(0);
  const [licenceChecked, setLicenceChecked]             = useState(false); // true once checkLicenceRevalidation resolves; gates the nudge so it can't race the migration trial popup
  const templatesPillRef = useRef(null);
  const onboardingCompleteRef = useRef(false); // set when config load finishes; used by the nudge fire-effect
  const [listViewActive, setListViewActive]             = useState(() => {
    try { return localStorage.getItem('trigr_list_view') === 'true'; } catch { return false; }
  });
  const [searchTemplates, setSearchTemplates]           = useState([]);
  const [searchTemplateCategories, setSearchTemplateCategories] = useState([]);
  const [quickActionCategories, setQuickActionCategories] = useState([]);
  const [radialItemsMap, setRadialItemsMap]                 = useState({}); // { profileName: items[] }
  // Extra radial layouts (Pro, per-device). The Default layout stays in
  // radialItemsMap / radialMenuItemsByProfile so older builds sharing the
  // config keep working; these sync too, only the device's choice is local.
  const [radialLayouts, setRadialLayouts]                   = useState([]); // [{ id, name, itemsByProfile }]
  const [editingRadialLayoutId, setEditingRadialLayoutId]   = useState('default');
  const [deviceRadialLayoutId, setDeviceRadialLayoutId]     = useState('default');
  const [radialMenuHotkey, setRadialMenuHotkey]           = useState(null);
  const [radialHoldToSelect, setRadialHoldToSelect]       = useState(false);
  const [selectedRadialSegment, setSelectedRadialSegment] = useState(null); // index or null
  const [selectedRadialChild, setSelectedRadialChild] = useState(null);   // { folderId, childIndex } or null
  const [expandedRadialFolder, setExpandedRadialFolder] = useState(null); // folder item id or null


  // Current modifier combo string e.g. "Ctrl+Alt"
  const currentCombo = comboString(activeModifiers);
  const isPro = licenceStatus.is_pro;

  // Show the upgrade modal for a named Pro feature.
  const showUpgrade = useCallback((featureName) => setUpgradePrompt(featureName), []);

  // ── Per-profile radial menu items (of the layout being edited) ─────────
  // Free tier always edits the Default layout; the switcher is a Pro teaser.
  const effectiveEditingLayoutId = isPro ? editingRadialLayoutId : 'default';
  const editingRadialLayout = effectiveEditingLayoutId === 'default'
    ? null
    : (radialLayouts.find(l => l.id === effectiveEditingLayoutId) || null);
  const editingRadialMap = editingRadialLayout ? (editingRadialLayout.itemsByProfile || {}) : radialItemsMap;
  const radialMenuItems = editingRadialMap[activeProfile] || [];

  // Replace the whole per-profile map of the layout being edited and persist
  // it under the right config key (Default → radialMenuItemsByProfile, extra
  // layout → its slot in radialLayouts).
  const persistEditingRadialMap = useCallback((newMap) => {
    if (editingRadialLayout) {
      const nextLayouts = radialLayouts.map(l => l.id === editingRadialLayout.id ? { ...l, itemsByProfile: newMap } : l);
      setRadialLayouts(nextLayouts);
      window.electronAPI?.saveConfig({ radialLayouts: nextLayouts });
    } else {
      setRadialItemsMap(newMap);
      window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap });
    }
  }, [editingRadialLayout, radialLayouts]);

  // Assignment objects handed to the radial segment / folder-child editors.
  // MacroPanel's reset effect keys on the object's identity, and these used
  // to be rebuilt inline (`{ ...base, label }`) on every App render — so a
  // toast dismissing or any hotkey firing wiped an in-progress wedge edit.
  const radialSegmentAssignment = useMemo(() => {
    if (selectedRadialSegment == null) return null;
    const item = selectedRadialSegment < radialMenuItems.length ? radialMenuItems[selectedRadialSegment] : null;
    if (!item?.storageKey) return null;
    const base = assignments[item.storageKey];
    if (!base) return null;
    // Wheel label is a per-segment display override. Merge it in so reopening
    // the panel shows what's on the wheel, not the library action's name.
    return item.label ? { ...base, label: item.label } : base;
  }, [selectedRadialSegment, radialMenuItems, assignments]);
  const radialChildAssignment = useMemo(() => {
    if (!selectedRadialChild) return null;
    const folder = radialMenuItems.find(i => i && i.id === selectedRadialChild.folderId);
    const child = folder?.children?.[selectedRadialChild.childIndex];
    if (!child?.storageKey) return null;
    const base = assignments[child.storageKey];
    if (!base) return null;
    return child.label ? { ...base, label: child.label } : base;
  }, [selectedRadialChild, radialMenuItems, assignments]);

  // Ref tracks activeProfile so the wrapper below has a stable identity.
  // Without this, every handler that captures setRadialMenuItems would need
  // it in its dependency array, and a stale closure on profile switch causes
  // items to be written to the wrong profile — the root cause of drag-drop
  // failing on app-specific profiles.
  const activeProfileRef = useRef(activeProfile);
  activeProfileRef.current = activeProfile;
  const editingRadialLayoutIdRef = useRef('default');
  editingRadialLayoutIdRef.current = effectiveEditingLayoutId;

  // Drop-in wrapper: updates the per-profile map. The legacy flat
  // radialMenuItems key is no longer written — Rust resolves items from
  // radialMenuItemsByProfile[activeProfile] directly (flat is read only for
  // pre-per-profile configs). Writing it duplicated up to 1MB of icon data
  // per save, and the old sync-on-profile-switch effect here caused a full
  // config write to the (possibly shared/synced) file on every alt-tab —
  // the write-storm behind the shared-config clobber hazard.
  // Stable identity (empty deps) — reads activeProfile from ref at call time.
  const setRadialMenuItems = useCallback((updater) => {
    const profile = activeProfileRef.current;
    const layoutId = editingRadialLayoutIdRef.current;
    if (layoutId !== 'default') {
      // Extra layout (Pro): same per-profile shape, one level down.
      setRadialLayouts(layouts => {
        const idx = layouts.findIndex(l => l.id === layoutId);
        if (idx < 0) return layouts;
        const byProf = layouts[idx].itemsByProfile || {};
        const prev = byProf[profile] || [];
        const next = typeof updater === 'function' ? updater(prev) : updater;
        if (next === prev) return layouts;
        const nextLayouts = layouts.slice();
        nextLayouts[idx] = { ...layouts[idx], itemsByProfile: { ...byProf, [profile]: next } };
        window.electronAPI?.saveConfig({ radialLayouts: nextLayouts });
        return nextLayouts;
      });
      return;
    }
    setRadialItemsMap(map => {
      const prev = map[profile] || [];
      const next = typeof updater === 'function' ? updater(prev) : updater;
      if (next === prev) return map;
      const newMap = { ...map, [profile]: next };
      window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap });
      return newMap;
    });
  }, []);

  // One-time config hygiene after load: downscale custom icons stored as
  // full-resolution images (the pre-2026-06 picker stored raw files — a
  // single photo could be ~1MB of base64), and blank the legacy flat
  // radialMenuItems field once the per-profile map exists (Rust never reads
  // it then, but a stale copy duplicated up to 1MB of icon data on disk).
  // Saves at most once; no-op when the config is already clean.
  const cleanupRadialConfig = useCallback((map, config) => {
    const oversized = [];
    for (const prof of Object.keys(map)) {
      (map[prof] || []).forEach((item, idx) => {
        if (!item) return;
        if (typeof item.icon === 'string' && item.icon.startsWith('custom:') && item.icon.length > ICON_DOWNSCALE_THRESHOLD) {
          oversized.push({ prof, idx, child: -1, dataUrl: item.icon.slice('custom:'.length) });
        }
        (item.children || []).forEach((c, ci) => {
          if (c && typeof c.icon === 'string' && c.icon.startsWith('custom:') && c.icon.length > ICON_DOWNSCALE_THRESHOLD) {
            oversized.push({ prof, idx, child: ci, dataUrl: c.icon.slice('custom:'.length) });
          }
        });
      });
    }
    const staleFlat = !!config.radialMenuItemsByProfile
      && Array.isArray(config.radialMenuItems) && config.radialMenuItems.length > 0;
    if (oversized.length === 0 && !staleFlat) return;

    const finish = (newMap) => {
      window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap, radialMenuItems: [] });
      if (oversized.length > 0) setRadialItemsMap(newMap);
    };

    if (oversized.length === 0) {
      finish(map);
      return;
    }

    let remaining = oversized.length;
    const results = new Map();
    oversized.forEach((entry, i) => {
      downscaleIconDataUrl(entry.dataUrl, (scaled) => {
        results.set(i, scaled);
        remaining -= 1;
        if (remaining === 0) {
          const newMap = { ...map };
          oversized.forEach((e, j) => {
            const items = [...(newMap[e.prof] || [])];
            const item = { ...items[e.idx] };
            const scaledUrl = 'custom:' + results.get(j);
            if (e.child < 0) {
              item.icon = scaledUrl;
            } else {
              const children = [...(item.children || [])];
              children[e.child] = { ...children[e.child], icon: scaledUrl };
              item.children = children;
            }
            items[e.idx] = item;
            newMap[e.prof] = items;
          });
          finish(newMap);
        }
      });
    });
  }, []);

  // ── Apply a full config object to React state + engine ─────
  // Single source of truth for "a whole config just arrived from disk":
  // shared-config sync reload, Import Config, and Restore Backup. Import /
  // restore used to re-hydrate ~10 fields and leave variables, templates,
  // Quick Action categories, radial, global + autocorrect settings stale in
  // state — the next full-object save then wrote the stale values back over
  // the freshly imported file. Everything here is a stable setter or an
  // engine invoke, so the callback has no deps.
  const applyLoadedConfig = useCallback((config, opts = {}) => {
    if (!config) return;
    const raw = config.assignments || {};
    setAssignments(raw);
    setProfiles(config.profiles?.length ? config.profiles : ['Default']);
    const globalProfile = config.activeGlobalProfile || 'Default';
    // Sync reload follows the global profile (another machine may be mid-app-
    // switch); import/restore honour whatever editing profile the file saved.
    const editingProfile = (opts.useSavedActiveProfile && config.activeProfile) ? config.activeProfile : globalProfile;
    setActiveProfile(editingProfile);
    setActiveGlobalProfile(globalProfile);
    setProfileSettings(config.profileSettings || {});
    const savedTheme = config.theme || 'auto';
    setTheme(savedTheme);
    const resolvedInitial = savedTheme === 'auto'
      ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
      : savedTheme;
    setResolvedTheme(resolvedInitial);
    document.documentElement.setAttribute('data-theme', resolvedInitial);
    const rawCats = config.expansionCategories || [];
    setExpansionCategories(rawCats.map(c => typeof c === 'string' ? { name: c, colour: null } : c));
    setGlobalVariables(config.globalVariables || {});
    {
      const cfgAcEnabled = config.autocorrectEnabled ?? false;
      const cfgAcBuiltin = config.autocorrectBuiltinTypos ?? false;
      const cfgAcDoubleCaps = config.autocorrectDoubleCaps ?? false;
      const cfgAcExceptions = Array.isArray(config.autocorrectDoubleCapsExceptions) ? config.autocorrectDoubleCapsExceptions : [];
      const cfgAcCapsLockFix = config.autocorrectCapsLockFix ?? false;
      const cfgAcSentenceCaps = config.autocorrectSentenceCaps ?? false;
      const cfgAcExtended = config.autocorrectExtendedTypos ?? false;
      const cfgAcExcluded = Array.isArray(config.autocorrectExcludedApps) ? config.autocorrectExcludedApps : [];
      const cfgAcDisabled = Array.isArray(config.autocorrectDisabledEntries) ? config.autocorrectDisabledEntries : [];
      const cfgAcDays = config.autocorrectDays ?? false;
      const cfgAcSymbols = config.autocorrectSymbols ?? false;
      const cfgAcEmojis = config.autocorrectEmojis ?? false;
      const cfgExpExcluded = Array.isArray(config.expansionExcludedApps) ? config.expansionExcludedApps : [];
      setAutocorrectEnabled(cfgAcEnabled);
      setAutocorrectBuiltinTypos(cfgAcBuiltin);
      setAutocorrectDoubleCaps(cfgAcDoubleCaps);
      setAutocorrectDoubleCapsExceptions(cfgAcExceptions);
      setAutocorrectCapsLockFix(cfgAcCapsLockFix);
      setAutocorrectSentenceCaps(cfgAcSentenceCaps);
      setAutocorrectExtendedTypos(cfgAcExtended);
      setAutocorrectDays(cfgAcDays);
      setAutocorrectSymbols(cfgAcSymbols);
      setAutocorrectEmojis(cfgAcEmojis);
      setAutocorrectExcludedApps(cfgAcExcluded);
      setAutocorrectDisabledEntries(cfgAcDisabled);
      setAutocorrectUndoCounts(config.autocorrectUndoCounts && typeof config.autocorrectUndoCounts === 'object' ? config.autocorrectUndoCounts : {});
      setAutocorrectUndoMuted(Array.isArray(config.autocorrectUndoMuted) ? config.autocorrectUndoMuted : []);
      setExpansionExcludedApps(cfgExpExcluded);
      window.electronAPI?.updateAutocorrectSettings({
        enabled: cfgAcEnabled,
        builtinTypos: cfgAcBuiltin,
        extendedTypos: cfgAcExtended,
        days: cfgAcDays,
        symbols: cfgAcSymbols,
        emojis: cfgAcEmojis,
        doubleCaps: cfgAcDoubleCaps,
        doubleCapsExceptions: cfgAcExceptions,
        capsLockFix: cfgAcCapsLockFix,
        sentenceCaps: cfgAcSentenceCaps,
        excludedApps: cfgAcExcluded,
        disabledEntries: cfgAcDisabled,
      });
      window.electronAPI?.updateExpansionExcludedApps(cfgExpExcluded);
    }
    setMacrosEnabledOnStartup(config.macrosEnabledOnStartup ?? true);
    setPhysicalKeyboardLayout(['ansi', 'iso'].includes(config.physicalKeyboardLayout) ? config.physicalKeyboardLayout : 'auto');
    setIsoKeyDetected(!!config.isoKeyDetected);
    const cfgClipboardCapture = config.clipboardCaptureEnabled ?? true;
    const cfgClipboardExcluded = Array.isArray(config.clipboardExcludedApps) ? config.clipboardExcludedApps : [];
    setClipboardCaptureEnabled(cfgClipboardCapture);
    setClipboardExcludedApps(cfgClipboardExcluded);
    window.electronAPI?.setClipboardCaptureEnabled(cfgClipboardCapture);
    window.electronAPI?.setClipboardExcludedApps(cfgClipboardExcluded);
    setGlobalInputMethod(config.globalInputMethod   || 'direct');
    setMacroSpeed(       config.macroSpeed          || 'safe');
    setKeystrokeDelay(   config.keystrokeDelay      ?? 10);
    setMacroTriggerDelay(config.macroTriggerDelay   ?? 10);
    setDoubleTapWindow(  config.doubleTapWindow     ?? 300);
    setHoldThresholdMs(  config.holdThresholdMs     ?? 350);
    setFireOnPress(      config.fireOnPress         ?? false);
    setDefaultDateFormat(config.defaultDateFormat   || 'DD/MM/YYYY');
    setSearchOverlayHotkey(     config.searchOverlayHotkey      || 'Ctrl+Space');
    setVoiceEnabled(            config.voiceEnabled             ?? false);
    setVoiceHotkey(             config.voiceHotkey              || '');
    setVoiceMicId(              config.voiceMicId               || '');
    // Default global pause to Ctrl+Alt+Q on fresh installs (field missing
    // from config). Existing users who explicitly cleared it have null
    // stored and keep no hotkey.
    setGlobalPauseToggleKey(
      config.globalPauseToggleKey === undefined ? 'Ctrl+Alt+Q' : config.globalPauseToggleKey
    );
    // Clipboard paste hotkey — defaults to Ctrl+Shift+V on fresh installs,
    // null means user explicitly cleared and has no hotkey for it.
    {
      const cfgClipPaste = config.clipboardPasteHotkey === undefined
        ? 'Ctrl+Shift+V'
        : config.clipboardPasteHotkey;
      setClipboardPasteHotkey(cfgClipPaste || '');
      if (cfgClipPaste) {
        window.electronAPI?.setClipboardPasteHotkey(cfgClipPaste);
      } else {
        window.electronAPI?.clearClipboardPasteHotkey();
      }
    }
    setOverlayShowAll(          config.overlayShowAll            ?? true);
    setOverlayCloseAfterFiring( config.overlayCloseAfterFiring   ?? true);
    setOverlayIncludeAutocorrect(config.overlayIncludeAutocorrect ?? false);
    setClipboardPreviewWidth(   Math.max(320, Math.min(1200, config.clipboardPreviewWidth ?? 480)));
    setClipboardColumnMode(     config.clipboardColumnMode === 'one' || config.clipboardColumnMode === 'two' ? config.clipboardColumnMode : 'auto');
    setSearchTemplates(config.searchTemplates || []);
    setSearchTemplateCategories(config.searchTemplateCategories || []);
    setQuickActionCategories(config.quickActionCategories || []);
    {
      let map = config.radialMenuItemsByProfile || {};
      // Migration: old flat array → per-profile map under active profile
      if (!config.radialMenuItemsByProfile && Array.isArray(config.radialMenuItems) && config.radialMenuItems.length > 0) {
        map = { [globalProfile]: config.radialMenuItems };
      }
      setRadialItemsMap(map);
      setRadialLayouts(sanitizeRadialLayouts(config.radialLayouts));
    }
    {
      // Same default-on-fresh rule as the startup load path, and re-register
      // with the engine (this sync path previously never re-registered the
      // radial hotkey, unlike the clipboard-paste hotkey above).
      const effectiveRadialHotkey = config.radialMenuHotkey === undefined
        ? 'Ctrl+Shift+Space'
        : config.radialMenuHotkey;
      setRadialMenuHotkey(effectiveRadialHotkey || null);
      if (effectiveRadialHotkey) {
        window.electronAPI?.setRadialMenuHotkey(effectiveRadialHotkey);
      } else {
        window.electronAPI?.clearRadialMenuHotkey();
      }
      const holdToSelect = config.radialHoldToSelect ?? false;
      setRadialHoldToSelect(holdToSelect);
      window.electronAPI?.setRadialHoldToSelect(holdToSelect);
    }
    // Re-sync engine with updated config
    window.electronAPI?.updateAssignments(raw, editingProfile);
    window.electronAPI?.updateProfileSettings(config.profileSettings || {});
    window.electronAPI?.setActiveGlobalProfile(globalProfile);
    window.electronAPI?.updateGlobalVariables(config.globalVariables || {});
    window.electronAPI?.updateGlobalSettings({
      globalInputMethod: config.globalInputMethod  || 'direct',
      macroSpeed:        config.macroSpeed         || 'safe',
      keystrokeDelay:    config.keystrokeDelay     ?? 10,
      macroTriggerDelay: config.macroTriggerDelay  ?? 10,
      doubleTapWindow:   config.doubleTapWindow    ?? 300,
      holdThresholdMs:   config.holdThresholdMs    ?? 350,
      fireOnPress:       config.fireOnPress        ?? false,
      defaultDateFormat: config.defaultDateFormat  || 'DD/MM/YYYY',
    });

    // Hotkeys the sync path previously read into state but never re-registered
    // (pause / voice / Quick Search) — a change from another machine, or an
    // imported config, only took effect after a restart. Mirrors the startup
    // load path.
    {
      const effectivePauseKey = config.globalPauseToggleKey === undefined
        ? 'Ctrl+Alt+Q'
        : config.globalPauseToggleKey;
      if (effectivePauseKey) {
        window.electronAPI?.setPauseHotkey(effectivePauseKey);
      } else {
        window.electronAPI?.clearPauseHotkey?.();
      }
      if ((config.voiceEnabled ?? false) && config.voiceHotkey) {
        window.electronAPI?.setVoiceHotkey(config.voiceHotkey);
      } else {
        window.electronAPI?.clearVoiceHotkey();
      }
      const cfgSearchHotkey  = config.searchOverlayHotkey || 'Ctrl+Space';
      const cfgSearchEnabled = config.searchOverlayEnabled ?? true;
      setSearchOverlayEnabled(cfgSearchEnabled);
      window.electronAPI?.updateSearchSettings({
        searchOverlayEnabled: cfgSearchEnabled,
        searchOverlayHotkey:  cfgSearchHotkey,
      });
    }
  }, []);

  // ── Load config on mount ──────────────────────────────────
  useEffect(() => {
    const init = async () => {
      if (!window.electronAPI) return;
      window.electronAPI.getAppVersion().then(v => { if (v) setAppVersion(v); });
      const config = await window.electronAPI.loadConfig();
      // Hoisted so the migration trial-popup check below can use it. False
      // by default — a fresh install (no config or all flags off) leaves it
      // false and the migration popup is suppressed; the onboarding tour
      // handles the trial offer on Finish instead.
      let onboardingComplete = false;
      if (config) {
        // Migrate any pre-global expansion keys (Profile::EXPANSION::trigger →
        // GLOBAL::EXPANSION::trigger).  Done once on load; re-saved immediately.
        const raw = config.assignments || {};
        const migrated = { ...raw };
        let needsSave = false;
        for (const key of Object.keys(raw)) {
          const m = key.match(/^(?!GLOBAL::)([^:]+)::EXPANSION::(.+)$/);
          if (m) {
            const globalKey = `GLOBAL::EXPANSION::${m[2]}`;
            if (!migrated[globalKey]) migrated[globalKey] = raw[key];
            delete migrated[key];
            needsSave = true;
          }
        }
        setAssignments(migrated);
        setProfiles(config.profiles?.length ? config.profiles : ['Default']);
        // Always start on the global (Default) profile — do not restore last-used profile
        const globalProfile = config.activeGlobalProfile || 'Default';
        setActiveProfile(globalProfile);
        setActiveGlobalProfile(globalProfile);
        setProfileSettings(config.profileSettings || {});
        const savedTheme = config.theme || 'auto';
        setTheme(savedTheme);
        const resolvedInitial = savedTheme === 'auto'
          ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
          : savedTheme;
        setResolvedTheme(resolvedInitial);
        document.documentElement.setAttribute('data-theme', resolvedInitial);
        // Migrate old string[] format to object[] format — treat missing colour as null
        const rawCats = config.expansionCategories || [];
        setExpansionCategories(rawCats.map(c => typeof c === 'string' ? { name: c, colour: null } : c));
        setGlobalVariables(config.globalVariables || {});
        const savedAcEnabled = config.autocorrectEnabled ?? false;
        const savedAcBuiltin = config.autocorrectBuiltinTypos ?? false;
        const savedAcDoubleCaps = config.autocorrectDoubleCaps ?? false;
        const savedAcExceptions = Array.isArray(config.autocorrectDoubleCapsExceptions) ? config.autocorrectDoubleCapsExceptions : [];
        const savedAcCapsLockFix = config.autocorrectCapsLockFix ?? false;
        const savedAcSentenceCaps = config.autocorrectSentenceCaps ?? false;
        const savedAcExtended = config.autocorrectExtendedTypos ?? false;
        const savedAcExcluded = Array.isArray(config.autocorrectExcludedApps) ? config.autocorrectExcludedApps : [];
        const savedAcDisabled = Array.isArray(config.autocorrectDisabledEntries) ? config.autocorrectDisabledEntries : [];
        const savedAcDays = config.autocorrectDays ?? false;
        const savedAcSymbols = config.autocorrectSymbols ?? false;
        const savedAcEmojis = config.autocorrectEmojis ?? false;
        const savedExpExcluded = Array.isArray(config.expansionExcludedApps) ? config.expansionExcludedApps : [];
        setAutocorrectEnabled(savedAcEnabled);
        setAutocorrectBuiltinTypos(savedAcBuiltin);
        setAutocorrectDoubleCaps(savedAcDoubleCaps);
        setAutocorrectDoubleCapsExceptions(savedAcExceptions);
        setAutocorrectCapsLockFix(savedAcCapsLockFix);
        setAutocorrectSentenceCaps(savedAcSentenceCaps);
        setAutocorrectExtendedTypos(savedAcExtended);
        setAutocorrectDays(savedAcDays);
        setAutocorrectSymbols(savedAcSymbols);
        setAutocorrectEmojis(savedAcEmojis);
        setAutocorrectExcludedApps(savedAcExcluded);
        setAutocorrectDisabledEntries(savedAcDisabled);
        setAutocorrectUndoCounts(config.autocorrectUndoCounts && typeof config.autocorrectUndoCounts === 'object' ? config.autocorrectUndoCounts : {});
        setAutocorrectUndoMuted(Array.isArray(config.autocorrectUndoMuted) ? config.autocorrectUndoMuted : []);
        setExpansionExcludedApps(savedExpExcluded);
        window.electronAPI?.updateAutocorrectSettings({
          enabled: savedAcEnabled,
          builtinTypos: savedAcBuiltin,
          extendedTypos: savedAcExtended,
          days: savedAcDays,
          symbols: savedAcSymbols,
          emojis: savedAcEmojis,
          doubleCaps: savedAcDoubleCaps,
          doubleCapsExceptions: savedAcExceptions,
          capsLockFix: savedAcCapsLockFix,
          sentenceCaps: savedAcSentenceCaps,
          excludedApps: savedAcExcluded,
          disabledEntries: savedAcDisabled,
        });
        window.electronAPI?.updateExpansionExcludedApps(savedExpExcluded);
        const savedMacrosOnStartup = config.macrosEnabledOnStartup ?? true;
        setMacrosEnabledOnStartup(savedMacrosOnStartup);
        setPhysicalKeyboardLayout(['ansi', 'iso'].includes(config.physicalKeyboardLayout) ? config.physicalKeyboardLayout : 'auto');
        setIsoKeyDetected(!!config.isoKeyDetected);
        // Clipboard privacy controls — defaults preserve existing behaviour.
        const savedClipboardCapture = config.clipboardCaptureEnabled ?? true;
        const savedClipboardExcluded = Array.isArray(config.clipboardExcludedApps) ? config.clipboardExcludedApps : [];
        setClipboardCaptureEnabled(savedClipboardCapture);
        setClipboardExcludedApps(savedClipboardExcluded);
        window.electronAPI?.setClipboardCaptureEnabled(savedClipboardCapture);
        window.electronAPI?.setClipboardExcludedApps(savedClipboardExcluded);
        setGlobalInputMethod(config.globalInputMethod   || 'direct');
        setMacroSpeed(       config.macroSpeed          || 'safe');
        setKeystrokeDelay(   config.keystrokeDelay      ?? 10);
        setMacroTriggerDelay(config.macroTriggerDelay   ?? 10);
        setDoubleTapWindow(  config.doubleTapWindow     ?? 300);
        setHoldThresholdMs(  config.holdThresholdMs     ?? 350);
        setFireOnPress(      config.fireOnPress         ?? false);
        setDefaultDateFormat(config.defaultDateFormat   || 'DD/MM/YYYY');
        // Always start on the Mapping view — do not restore last-used view/area
        {
          // Quick Search hotkey + master toggle. These were previously never
          // persisted (the Settings handler skipped saveConfig) so the engine
          // always came up on its Ctrl+Space default; now the saved combo and
          // enabled flag are registered on every boot like pause/voice.
          const cfgSearchHotkey  = config.searchOverlayHotkey || 'Ctrl+Space';
          const cfgSearchEnabled = config.searchOverlayEnabled ?? true;
          setSearchOverlayHotkey(cfgSearchHotkey);
          setSearchOverlayEnabled(cfgSearchEnabled);
          window.electronAPI?.updateSearchSettings({
            searchOverlayEnabled: cfgSearchEnabled,
            searchOverlayHotkey:  cfgSearchHotkey,
          });
        }
        setVoiceEnabled(            config.voiceEnabled             ?? false);
        setVoiceHotkey(             config.voiceHotkey              || '');
        setVoiceMicId(              config.voiceMicId               || '');
        // Default global pause to Ctrl+Alt+Q on fresh installs (field missing
        // from config). Existing users who explicitly cleared it have null
        // stored and keep no hotkey.
        setGlobalPauseToggleKey(
          config.globalPauseToggleKey === undefined ? 'Ctrl+Alt+Q' : config.globalPauseToggleKey
        );
        // Clipboard paste hotkey — defaults to Ctrl+Shift+V on fresh installs,
        // null means user explicitly cleared and has no hotkey for it.
        {
          const cfgClipPaste = config.clipboardPasteHotkey === undefined
            ? 'Ctrl+Shift+V'
            : config.clipboardPasteHotkey;
          setClipboardPasteHotkey(cfgClipPaste || '');
          if (cfgClipPaste) {
            window.electronAPI?.setClipboardPasteHotkey(cfgClipPaste);
          } else {
            window.electronAPI?.clearClipboardPasteHotkey();
          }
        }
        setOverlayShowAll(          config.overlayShowAll            ?? true);
        setOverlayCloseAfterFiring( config.overlayCloseAfterFiring   ?? true);
        setOverlayIncludeAutocorrect(config.overlayIncludeAutocorrect ?? false);
        setClipboardPreviewWidth(   Math.max(320, Math.min(1200, config.clipboardPreviewWidth ?? 480)));
        setClipboardColumnMode(     config.clipboardColumnMode === 'one' || config.clipboardColumnMode === 'two' ? config.clipboardColumnMode : 'auto');
        setSearchTemplates(config.searchTemplates || []);
        setSearchTemplateCategories(config.searchTemplateCategories || []);
        setQuickActionCategories(config.quickActionCategories || []);
        {
          let map = config.radialMenuItemsByProfile || {};
          // Migration: old flat array → per-profile map under active profile
          if (!config.radialMenuItemsByProfile && Array.isArray(config.radialMenuItems) && config.radialMenuItems.length > 0) {
            map = { [globalProfile]: config.radialMenuItems };
          }
          // Migration: truncate radial arrays to MAX_SLOTS (handles 12 → 8 reduction)
          let migrated = false;
          for (const prof of Object.keys(map)) {
            if (Array.isArray(map[prof]) && map[prof].length > MAX_SLOTS) {
              map[prof] = map[prof].slice(0, MAX_SLOTS);
              migrated = true;
            }
          }
          if (migrated) {
            window.electronAPI?.saveConfig({ radialMenuItemsByProfile: map });
          }
          setRadialItemsMap(map);
          {
            const layouts = sanitizeRadialLayouts(config.radialLayouts);
            setRadialLayouts(layouts);
            // Which layout THIS machine fires is machine-local. Open the
            // editor on it so what you see is what the hotkey shows here.
            window.electronAPI?.getRadialLayoutId?.().then(id => {
              const found = typeof id === 'string' && layouts.some(l => l.id === id) ? id : 'default';
              setDeviceRadialLayoutId(found);
              setEditingRadialLayoutId(found);
            }).catch(() => {});
          }
          // One-time cleanup: downscale oversized custom icons (raw photos
          // could be ~1MB each in base64) and blank the legacy flat field
          // once the per-profile map exists (it's never read again, but a
          // stale copy could hold an extra ~1MB of duplicated icon data).
          cleanupRadialConfig(map, config);
        }
        // Radial menu hotkey — Ctrl+Shift+Space on fresh installs (field
        // missing from config). Users who explicitly cleared it have null
        // stored and keep no hotkey. Mirrors the globalPauseToggleKey
        // default-on-fresh pattern.
        const effectiveRadialHotkey = config.radialMenuHotkey === undefined
          ? 'Ctrl+Shift+Space'
          : config.radialMenuHotkey;
        setRadialMenuHotkey(effectiveRadialHotkey || null);
        // Sync new settings to engine on load
        window.electronAPI?.updateGlobalSettings({
          globalInputMethod: config.globalInputMethod  || 'direct',
          macroSpeed:        config.macroSpeed         || 'safe',
          keystrokeDelay:    config.keystrokeDelay     ?? 10,
          macroTriggerDelay: config.macroTriggerDelay  ?? 10,
          doubleTapWindow:   config.doubleTapWindow    ?? 300,
          holdThresholdMs:   config.holdThresholdMs    ?? 350,
          fireOnPress:       config.fireOnPress        ?? false,
          defaultDateFormat: config.defaultDateFormat  || 'DD/MM/YYYY',
        });
        // CRITICAL: updateAssignments MUST be called after config loads on startup.
        // Parameter name must match Rust command signature exactly (was 'incoming',
        // now 'config' — mismatch caused assignments=0 on all hotkeys).
        // The frontend sync was missing initially and caused silent hotkey failure.
        const loadProfile = config.activeGlobalProfile || 'Default';
        window.electronAPI?.updateAssignments(migrated, loadProfile);
        window.electronAPI?.updateProfileSettings(config.profileSettings || {});
        window.electronAPI?.setActiveGlobalProfile(loadProfile);
        // Sync global variables to expansion engine
        window.electronAPI?.updateGlobalVariables(config.globalVariables || {});
        // Register pause hotkey with Rust backend. Mirror the default-on-fresh
        // logic above so the Ctrl+Alt+Q default is actually live, not just
        // shown in the UI.
        {
          const effectivePauseKey = config.globalPauseToggleKey === undefined
            ? 'Ctrl+Alt+Q'
            : config.globalPauseToggleKey;
          if (effectivePauseKey) {
            window.electronAPI?.setPauseHotkey(effectivePauseKey);
          }
        }
        // Sync voice hotkey with Rust backend. When voice is disabled (or no
        // hotkey is set), explicitly clear the engine so it can't reach for a
        // stale default and silently swallow keystrokes the user can't unmap.
        if ((config.voiceEnabled ?? false) && config.voiceHotkey) {
          window.electronAPI?.setVoiceHotkey(config.voiceHotkey);
        } else {
          window.electronAPI?.clearVoiceHotkey();
        }
        // Register radial menu hotkey with Rust backend (default-on-fresh
        // mirrors the UI state above so Ctrl+Shift+Space is actually live).
        if (effectiveRadialHotkey) {
          window.electronAPI?.setRadialMenuHotkey(effectiveRadialHotkey);
        }
        // Radial hold-to-select mode (opt-in, default off).
        {
          const holdToSelect = config.radialHoldToSelect ?? false;
          setRadialHoldToSelect(holdToSelect);
          window.electronAPI?.setRadialHoldToSelect(holdToSelect);
        }
        // One-time conflict notice for pre-existing collisions (e.g., voice +
        // radial both bound to Ctrl+Alt+W from before validation was added).
        // The validation now blocks new collisions; this only fires while a
        // legacy duplicate is still in config, and disappears once the user
        // reassigns one of the slots. Voice wins in the LL hook firing order.
        const activeVoice = (config.voiceEnabled ?? false) && config.voiceHotkey;
        if (activeVoice && effectiveRadialHotkey && config.voiceHotkey === effectiveRadialHotkey) {
          // Delay slightly so the notification doesn't render before the main
          // window UI is fully mounted (otherwise the toast slot may not exist
          // when setNotification fires).
          setTimeout(() => {
            showNotification(
              `Voice and Radial menu both use ${config.voiceHotkey}. Voice wins — please reassign the Radial menu hotkey.`,
              'info'
            );
          }, 1200);
        }
        // If the main process auto-restored from a backup, surface that to the user
        if (config._restoredFrom) setBackupRestoredFrom(config._restoredFrom);

        // Tips — load hidden flag; record first launch date if not yet stored
        setTipsHidden(config.tipsHidden ?? false);
        setHiddenTips(Array.isArray(config.hiddenTips) ? config.hiddenTips : []);
        const isFirstLaunch = !config.firstLaunchDate;
        const fld = config.firstLaunchDate || new Date().toISOString();
        setFirstLaunchDate(fld);
        if (isFirstLaunch) needsSave = true;

        // Start with Windows: ON by default for new installs. Existing users
        // (firstLaunchDate already set) get the bootstrap flag silently without
        // touching the registry — preserves whatever state they've explicitly
        // chosen. Bootstrap runs at most once; once the flag is true, the toggle
        // in Settings is the only thing that changes the registry.
        if (!config.startupBootstrapped) {
          if (isFirstLaunch) {
            window.electronAPI?.setStartupEnabled(true);
          }
          needsSave = true;
        }

        // Maximise on first ever launch so onboarding has the full canvas to
        // work with. Existing users keep their window habits unchanged.
        if (isFirstLaunch) {
          getCurrentWindow().maximize().catch(() => {});
        }

        // Onboarding migration: existing users who already saw the welcome
        // should not see the new onboarding tour after updating.
        onboardingComplete = config.onboarding_complete;
        if (onboardingComplete === undefined && config.hasSeenWelcome) {
          onboardingComplete = true;
          needsSave = true;
        }
        // Re-show onboarding when the tour has been revised since the user
        // last saw it. v0.4.4 users land here with version_seen=undefined → 0
        // → triggers a one-time re-tour to surface the new Pro flow.
        const versionSeen = config.onboarding_version_seen ?? 0;
        if (onboardingComplete && versionSeen < ONBOARDING_VERSION) {
          onboardingComplete = false;
        }

        if (!onboardingComplete) {
          // New user — show WelcomeModal first (visual feature intro).
          // Clicking "Get Started" inside the modal then kicks off the
          // OnboardingTour. Clicking "Skip the tour" bypasses both and
          // marks onboarding complete.
          if (!config.hasSeenWelcome) {
            setShowWelcome(true);
          } else {
            // Existing user mid-replay (already saw welcome) — straight to tour
            setShowOnboarding(true);
          }
        } else if (!config.hasSeenWelcome) {
          // Edge case: onboarding complete but welcome not set
          setShowWelcome(true);
        }

        // Templates coachmark seed. Treat existing v0.4.5 users (where the
        // flag is undefined) as not-yet-seen so they get the nudge once on
        // their first launch of v0.4.6.
        setTemplatesNudgeSeen(config.templates_nudge_seen === true);
        onboardingCompleteRef.current = !!onboardingComplete;

        needsSave = needsSave || !config.hasSeenWelcome;
        if (needsSave) {
          window.electronAPI.saveConfig({
            ...config,
            assignments: migrated,
            hasSeenWelcome: true,
            firstLaunchDate: fld,
            onboarding_complete: onboardingComplete ?? false,
            startupBootstrapped: true,
          });
        }
      }
      const status = await window.electronAPI.getEngineStatus();
      setEngineStatus(status);

      // Check licence status (revalidates if >24h since last check). Also
      // fires the one-time trial-offer popup for existing v0.4.4 installs
      // that already finished onboarding before the trial mechanism existed.
      window.electronAPI.checkLicenceRevalidation?.().then(ls => {
        if (!ls) {
          setLicenceChecked(true);
          return;
        }
        setLicenceStatus(ls);

        // Refresh shared-config grace state — the Rust side ran
        // check_and_migrate_if_due during revalidation, which may have started
        // or cleared the grace timestamp, or completed the auto-migration.
        window.electronAPI.getGracePeriodState?.().then(g => setGracePeriodState(g));

        // Trial modals for installs whose onboarding tour is already done:
        // the announcement (Rust started the trial at this launch, e.g. an
        // existing install predating the trial, or a Welcome-skip user on a
        // later launch) or the one-shot end-of-trial summary. Fresh installs
        // get the announcement from handleOnboardingComplete instead.
        if (onboardingComplete) {
          if (trialAnnouncePending(ls)) {
            setShowProTrialModal(true);
          } else if (trialJustEnded(ls)) {
            openTrialEnd(ls);
          }
        }
        // Set this AFTER potentially queuing a trial modal so the templates
        // coachmark effect can't fire ahead of it.
        setLicenceChecked(true);
      }).catch(() => setLicenceChecked(true));

      // Listen for auto-migration events from Rust. Fires when the watcher
      // grace period elapses and the shared file is copied to local. Shows a
      // one-shot post-migration banner the user can dismiss.
      window.electronAPI.onSharedConfigMigrated?.(() => {
        setPostMigrationNotice(true);
        window.electronAPI.getGracePeriodState?.().then(g => setGracePeriodState(g));
        showNotification('Shared config moved to local storage. Re-enable Pro any time to resume sync.');
      });

      // Clipboard encryption error surfacing (v0.5 Phase 5). Two paths:
      // startup key-unreadable is polled (a Rust emit during setup would race
      // this listener registration); runtime decrypt failures arrive as an
      // event because the frontend is necessarily mounted by the time any
      // row is fetched and decrypted.
      window.electronAPI.getClipboardEncryptionStatus?.().then(s => {
        if (s?.key_unreadable) {
          showNotification('Clipboard encryption key could not be loaded. Open Settings > Privacy & Security and use Reset clipboard storage.', 'error');
        }
      });
      window.electronAPI.onClipboardEncryptionError?.(() => {
        showNotification('Some clipboard items could not be decrypted. If this keeps happening, use Reset clipboard storage in Settings > Privacy & Security.', 'error');
      });


      window.electronAPI.onEngineStatus((status) => {
        setEngineStatus(status);
        setMacrosEnabled(status.macrosEnabled);
        if (status.globalPauseToggleKey !== undefined) setGlobalPauseToggleKey(status.globalPauseToggleKey);
      });
      window.electronAPI.onMacroFired((data) => {
        setLastFired(data);
        setTimeout(() => setLastFired(null), 1500);
      });
      // Loop start — show a toast so the user knows the loop is active and
      // how to stop it. End event isn't toasted (the user already sees the
      // visible effect stop) but is wired for any future indicator UI.
      window.electronAPI.onLoopFireStarted?.((data) => {
        const labelPart = data?.label ? `"${data.label}"` : 'Macro';
        const countPart = data?.mode === 'forever'
          ? 'looping until stopped'
          : `looping × ${data?.count ?? '?'}`;
        showNotification(`${labelPart} ${countPart} — re-press trigger or Esc to stop`, 'info');
      });
      window.electronAPI.onLoopFireEnded?.(() => {});
      // Engine auto-switched profile (foreground app matched a linked profile)
      window.electronAPI.onProfileSwitched(({ profile }) => {
        setActiveProfile(profile);
        setSelectedKey(null);
        // The wheel is per-profile; a stale segment index would point into
        // the new profile's layout.
        setSelectedRadialSegment(null);
        setSelectedRadialChild(null);
      });
      // Distilled Record Macro fired but its bound target app isn't running.
      // Show the modal telling the user to launch it manually — no auto-launch
      // (PC startup times vary too much for a reliable wait-for-window flow).
      window.electronAPI.onRecordMacroAppMissing?.(({ exe, hint }) => {
        setAppMissingModal({ exe, hint });
      });
      // Window hidden to tray (X / tray toggle) — clear the open editor so the
      // window reopens to a blank slate. The editing-active effect then re-pushes
      // setEditingActive(false), and Rust already dropped the lock on hide, so the
      // foreground watcher resumes auto-switching. Minimise / navigate-away never
      // fire this, so the test-in-another-app flow keeps its lock.
      window.electronAPI.onResetEditingOnHide?.(() => {
        // Closing to the tray mid-edit used to wipe an unsaved action draft
        // with no prompt. Keep the editor as-is when MacroPanel reports
        // unsaved changes; the reset still runs for a clean editor.
        if (window.__kf_editor_dirty) return;
        setSelectedKey(null);
        setActiveModifiers([]);
        setSidebarComboFilter(null);
        setDraftAssignment(null);
        setDraftDoubleAssignment(null);
      });
      // Backspace-undo of an autocorrect fire — count repeats per word.
      // Muted/disabled filtering happens at suggestion-derivation time (render)
      // so this closure never needs fresh state beyond the counts map itself.
      window.electronAPI.onAutocorrectUndone?.((data) => {
        if (!data?.original) return;
        const key = String(data.original).toLowerCase();
        setAutocorrectUndoCounts(prev => {
          const cur = prev[key]?.count || 0;
          const next = {
            ...prev,
            [key]: { count: cur + 1, replacement: data.replacement || '', source: data.source || 'builtin' },
          };
          // Persist inline: a strict-mode double-call just re-saves the same
          // value (updater is pure over prev, so the count can't double-bump).
          window.electronAPI?.saveConfig({ autocorrectUndoCounts: next });
          return next;
        });
      });
      window.electronAPI.onOverlayFired?.((data) => {
        showNotification(`⚡ ${data.label || 'Macro fired'}`);
      });
      // Per-step toasts from Rust action arms (Change Audio Output device-missing,
      // future arms that need to surface a message). Payload: {level, message}.
      window.electronAPI.onSystemActionToast?.((data) => {
        if (!data?.message) return;
        showNotification(data.message, data.level === 'error' ? 'error' : (data.level || 'info'));
      });
      window.electronAPI.onHotkeyRecorded?.((data) => {
        setIsRecording(false);
        if (!data) {
          // Escape — cancelled. Discard any pending duplicate draft.
          setDraftAssignment(null);
          setDraftDoubleAssignment(null);
          return;
        }
        const { modifiers, keyId } = data;
        // No modifiers → treat as BARE key layer
        const mods = modifiers.length === 0 ? ['BARE'] : modifiers;
        setActiveModifiers(mods);
        setSelectedKey(keyId);
        setSelectedLibraryId(null);
        if (keyId.startsWith('MOUSE_')) setActiveView('mouse');
        else setActiveView('keyboard');
        setRecordCapture(modifiers.length === 0
          ? friendlyKeyName(keyId)
          : `${modifiers.join('+')}+${friendlyKeyName(keyId)}`);
        setTimeout(() => setRecordCapture(null), 2000);
      });

      // Shared config — listen for sync reload events from file watcher
      window.electronAPI.onConfigReloadedFromSync?.((config) => {
        if (!config) return;
        applyLoadedConfig(config);
        showNotification('Config updated from sync', 'info');
      });

      // Phase 2 cross-device merge: another machine's edits to top-level
      // sections survived this save because we'd never seen those changes
      // locally. Surface a toast so the user knows their save picked up
      // remote work.
      window.electronAPI.onSyncConflictResolved?.((payload) => {
        const sections = Array.isArray(payload?.sections) ? payload.sections : [];
        if (sections.length === 0) return;
        const SECTION_LABELS = {
          radialMenuItemsByProfile: 'radial menu',
          radialMenuItems: 'radial menu',
          radialLayouts: 'radial layouts',
          radialMenuHotkey: 'radial menu hotkey',
          assignments: 'triggers',
          profiles: 'profiles',
          profileSettings: 'profile settings',
          activeProfile: 'active profile',
          activeGlobalProfile: 'active profile',
          expansions: 'text expansions',
          expansionCategories: 'expansion categories',
          globalVariables: 'global variables',
          searchTemplates: 'search templates',
          searchTemplateCategories: 'search template categories',
          quickActionCategories: 'quick action categories',
          searchOverlayHotkey: 'search hotkey',
          clipboardPasteHotkey: 'clipboard hotkey',
          globalPauseToggleKey: 'pause hotkey',
          voiceHotkey: 'voice hotkey',
          theme: 'theme',
          autocorrectEnabled: 'autocorrect',
          macrosEnabledOnStartup: 'macros on startup',
          clipboardCaptureEnabled: 'clipboard capture',
          clipboardExcludedApps: 'clipboard exclusions',
        };
        const labels = Array.from(new Set(sections.map(s => SECTION_LABELS[s] || s)));
        let joined;
        if (labels.length === 1) joined = labels[0];
        else if (labels.length === 2) joined = `${labels[0]} and ${labels[1]}`;
        else joined = `${labels.slice(0, -1).join(', ')} and ${labels[labels.length - 1]}`;
        showNotification(`Merged ${joined} from another device.`, 'info');
      });
    };
    init();
    return () => {
      window.electronAPI?.removeAllListeners('macro-fired');
      window.electronAPI?.removeAllListeners('engine-status');
      window.electronAPI?.removeAllListeners('profile-switched');
      window.electronAPI?.removeAllListeners('reset-editing-on-hide');
      window.electronAPI?.removeAllListeners('system-action-toast');
      window.electronAPI?.removeAllListeners('overlay-fired');
      window.electronAPI?.removeAllListeners('hotkey-recorded');
      window.electronAPI?.removeAllListeners('loop-fire-started');
      window.electronAPI?.removeAllListeners('loop-fire-ended');
    };
  }, []);

  // ── Grace state refresh whenever Pro status changes ──
  // Covers manual Activate/Deactivate from Settings (those don't go through
  // the revalidation path). The Rust commands already call
  // check_and_migrate_if_due themselves; this effect just keeps the React
  // view in sync.
  useEffect(() => {
    window.electronAPI?.getGracePeriodState?.().then(g => setGracePeriodState(g));
  }, [licenceStatus.is_pro]);

  // One-shot OCR backfill after the first Pro launch that includes auto-OCR.
  // Guarded by localStorage `trigr_ocr_backfilled_v1` so it only ever runs
  // once per install. Runs after the licence check has resolved so we're
  // reading a settled isPro. Backend also gates on Pro + setting so an
  // accidental re-run costs nothing.
  useEffect(() => {
    if (!licenceChecked || !isPro) return;
    try {
      if (localStorage.getItem('trigr_ocr_backfilled_v1')) return;
    } catch { /* localStorage may be blocked in rare embed contexts */ }
    window.electronAPI?.getClipboardSettings?.().then(cs => {
      if (!cs?.auto_ocr) return; // user disabled it before we got here
      window.electronAPI?.backfillClipboardOcr?.();
      try { localStorage.setItem('trigr_ocr_backfilled_v1', String(Date.now())); } catch {}
    }).catch(() => {});
  }, [licenceChecked, isPro]);

  // v0.8.4 one-shot thumbnail backfill for legacy image rows. Not gated on
  // licence (thumbs benefit Free and Pro users alike). Fire-and-forget —
  // backend handles the actual work on its own thread and emits progress
  // events the StatusBar listens for.
  useEffect(() => {
    try {
      if (localStorage.getItem('trigr_thumb_backfilled_v1')) return;
    } catch { return; }
    window.electronAPI?.backfillClipboardThumbnails?.();
    try { localStorage.setItem('trigr_thumb_backfilled_v1', String(Date.now())); } catch {}
  }, []);

  // ── Licence re-validation on window focus ──
  useEffect(() => {
    const handleFocus = () => {
      window.electronAPI?.checkLicenceRevalidation?.().then(ls => {
        if (!ls) return;
        // Trial ran out while the app was open or between sessions: show the
        // one-shot end-of-trial summary instead of letting double-tap / hold
        // bindings just stop. `openTrialEnd` de-dupes repeated focus events.
        if (trialJustEnded(ls)) openTrialEnd(ls);
        licenceStatusRef.current = ls;
        setLicenceStatus(ls);
        // Grace period state may have changed (timer ticked over while
        // Keyfire was unfocused, or migration just ran).
        window.electronAPI?.getGracePeriodState?.().then(g => setGracePeriodState(g));
      });
    };
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, []);

  // Load the telemetry opt-in flag from trigr-local-settings.json on mount.
  // Failure silently keeps the permissive default (true) — the worst case is
  // the UI shows ON briefly and then snaps OFF on first save; not a bug.
  useEffect(() => {
    window.electronAPI?.getTelemetryEnabled?.()
      .then(v => { if (typeof v === 'boolean') setTelemetryEnabled(v); })
      .catch(() => {});
  }, []);

  // ── Featurebase Feedback Widget init (main window only) ──
  // The SDK <script> bootstrap lives in index.html and creates a queueing stub
  // on window.Featurebase. We call initialize_feedback_widget once here so the
  // widget UI is ready to mount on demand from SettingsPanel.
  //
  // App.jsx is only mounted in the main window (main.jsx routes overlay /
  // fillin / radialmenu / clipboardoverlay windows to dedicated components and
  // never loads App). The URL-param guard below is defence-in-depth in case
  // that routing ever changes.
  //
  // The effect depends on `theme` so we can lock in the latest theme value at
  // first run (config load updates theme asynchronously). A ref ensures we
  // only init once even though theme may change again later — per spec the
  // widget keeps its boot-time theme.
  const featurebaseInitedRef = useRef(false);
  useEffect(() => {
    if (featurebaseInitedRef.current) return;
    const params = new URLSearchParams(window.location.search);
    if (params.get('overlay') === '1'
        || params.get('fillin') === '1'
        || params.get('radialmenu') === '1'
        || params.get('clipboardoverlay') === '1') {
      return;
    }
    if (typeof window.Featurebase !== 'function') return;
    featurebaseInitedRef.current = true;
    try {
      // Signature: Featurebase('initialize_feedback_widget', options, callback)
      // The callback receives action payloads from the widget — currently used
      // as a console-only debug hook so silent submission failures are
      // traceable from DevTools. No UI toast on success per spec.
      // Note: `placement` is intentionally omitted. Per Featurebase docs,
      // setting `placement` is what makes the SDK render its own edge-tab
      // trigger. We provide a Keyfire-branded trigger in the titlebar
      // (TitleBar.jsx) and in Settings (SettingsPanel.jsx) instead, so the
      // auto-tab is suppressed by omission.
      window.Featurebase('initialize_feedback_widget', {
        organization: 'keyfire',
        theme: theme,
        defaultBoard: 'feature-requests',
        locale: 'en',
      }, (err, action) => {
        if (err) {
          console.warn('[Featurebase] error', err);
          return;
        }
        if (action && action.action === 'feedbackSubmitted') {
          console.log('[Featurebase] feedback submitted', action);
        }
      });
    } catch (e) {
      console.warn('[Featurebase] init failed', e);
    }
  }, [theme]);

  // ── Featurebase Changelog Widget init (main window only) ──
  // Surfaces published Keyfire Updates inside the app. A "What's New" button
  // in the titlebar carries the data-featurebase-changelog attribute, which
  // auto-binds open once init succeeds. Unread count is rendered into
  // <span id="fb-update-badge"> by the SDK; we also log it for visibility.
  //
  // `theme` must be 'dark' or 'light' (strict) — the SDK rejects 'auto', so
  // we pass resolvedTheme (the computed value) not the raw user preference.
  const featurebaseChangelogInitedRef = useRef(false);
  useEffect(() => {
    if (featurebaseChangelogInitedRef.current) return;
    const params = new URLSearchParams(window.location.search);
    if (params.get('overlay') === '1'
        || params.get('fillin') === '1'
        || params.get('radialmenu') === '1'
        || params.get('clipboardoverlay') === '1') {
      return;
    }
    if (typeof window.Featurebase !== 'function') return;
    featurebaseChangelogInitedRef.current = true;
    try {
      window.Featurebase('init_changelog_widget', {
        organization: 'keyfire',
        theme: resolvedTheme,
        popup: { enabled: true, autoOpenForNewUpdates: false },
        changelogCard: { enabled: true },
      }, (err, data) => {
        if (err) {
          console.warn('[Featurebase] changelog error', err);
          return;
        }
        if (data && data.action === 'unreadChangelogsCountChanged') {
          console.log('[Featurebase] unread changelogs', data.unreadCount);
        }
      });
    } catch (e) {
      console.warn('[Featurebase] changelog init failed', e);
    }
  }, [resolvedTheme]);

  // ── UPDATER — DO NOT MODIFY WITHOUT EXPLICIT INSTRUCTION ──
  // Permissions required: updater:allow-check, updater:default (default.json)
  // process:allow-restart required for relaunch after install
  // Removing any of these permissions will cause silent failure
  // Test any changes with cargo tauri dev before releasing
  // Both x64 and ARM64 builds required in release.yml matrix
  // Runs once on mount + every 6h thereafter, so long-running instances
  // (lid-closers who never restart Keyfire) still receive update prompts.
  // isChecking guard prevents overlap if a prompt is open when the next tick fires.
  useEffect(() => {
    let isChecking = false;
    async function checkForUpdates() {
      if (isChecking) return;
      isChecking = true;
      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        const { relaunch } = await import('@tauri-apps/plugin-process');
        const { confirm } = await import('@tauri-apps/plugin-dialog');
        const update = await check();
        if (update?.available) {
          // "No" used to be forgotten: the same "Install now?" box re-asked on
          // every launch and every 6 hours. Remember the declined version.
          let skipped = null;
          try { skipped = localStorage.getItem('trigr_update_skipped_version'); } catch {}
          if (skipped && skipped === update.version) return;
          // The native confirm is owned by the main window; while Keyfire is in
          // the tray it either stays invisible or disables the hidden owner.
          // Defer the prompt until the window is next shown; the toast below
          // still tells the user an update exists.
          if (document.hidden) {
            const onVisible = () => {
              if (document.hidden) return;
              document.removeEventListener('visibilitychange', onVisible);
              isChecking = false;
              checkForUpdates();
            };
            document.addEventListener('visibilitychange', onVisible);
          }
          // Native Windows toast so lid-closers / hidden-window users get
          // pinged even when the main window is in the tray. The confirm()
          // dialog below stays invisible until the window is shown, so the
          // toast is the signal that actually reaches set-and-forget users.
          // Failure here must not block the install flow, hence the inner catch.
          try {
            const { isPermissionGranted, requestPermission, sendNotification } =
              await import('@tauri-apps/plugin-notification');
            let granted = await isPermissionGranted();
            if (!granted) granted = (await requestPermission()) === 'granted';
            if (granted) {
              sendNotification({
                title: 'Keyfire update available',
                body: `Version ${update.version} is ready. Open Keyfire to install.`,
              });
            }
          } catch (notifyErr) {
            console.error('Update notification failed:', notifyErr);
          }
          if (document.hidden) return; // prompt deferred above
          const confirmed = await confirm(
            `Keyfire ${update.version} is available. Install now?`,
            { title: 'Update Available', kind: 'info' }
          );
          if (!confirmed) {
            try { localStorage.setItem('trigr_update_skipped_version', update.version); } catch {}
          }
          if (confirmed) {
            // The updater exits the process right after launching the
            // installer (RunEvent::Exit never runs), so release any held /
            // repeating synthetic input first.
            await window.electronAPI?.releaseInputForExit?.();
            await update.downloadAndInstall();
            await relaunch();
          }
        }
      } catch (e) {
        console.error('Update check failed:', e);
      } finally {
        isChecking = false;
      }
    }
    checkForUpdates();
    const SIX_HOURS_MS = 6 * 60 * 60 * 1000;
    const interval = setInterval(checkForUpdates, SIX_HOURS_MS);
    return () => clearInterval(interval);
  }, []);

  // ── Notify main process when a text input has focus ───────
  // uiohook is a system-level hook and cannot be blocked by DOM stopPropagation.
  // We tell main.js directly so it can skip macro interception while the user
  // is typing inside the app's own input fields.
  useEffect(() => {
    function isEditable(el) {
      if (!el) return false;
      const tag = el.tagName;
      return tag === 'INPUT' || tag === 'TEXTAREA' || el.contentEditable === 'true';
    }
    function onFocusIn(e) {
      if (isEditable(e.target)) window.electronAPI?.notifyInputFocus(true);
    }
    function onFocusOut(e) {
      if (isEditable(e.target) && !isEditable(e.relatedTarget)) {
        window.electronAPI?.notifyInputFocus(false);
      }
    }
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
    };
  }, []);

  // ── Escape clears modifier selection (only when no key is selected) ──
  useEffect(() => {
    function onKeyDown(e) {
      if (e.key !== 'Escape') return;
      if (window.__trigr_capturing || window.__trigr_recording) return; // let capture handle it
      if (selectedKey) return;           // action panel is open — do nothing
      if (activeModifiers.length === 0) return; // nothing to clear
      e.preventDefault();
      setActiveModifiers([]);
      setSidebarComboFilter(null);
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [selectedKey, activeModifiers]);

  // ── Sync to engine whenever assignments/profile changes ───
  const syncEngine = useCallback((newAssignments, profile) => {
    window.electronAPI?.updateAssignments(newAssignments, profile);
  }, []);

  // Push aggregated editing state to Rust. The foreground watcher uses this to
  // suppress auto-switching while any action editor is open — mapping panel
  // (selectedKey/draftAssignment, also covers radial segments via MacroPanel),
  // expansion form, and quick action form. When all are closed, Keyfire behaves
  // the same whether the main window is visible (side-monitor parking) or
  // hidden — auto-switch runs normally.
  useEffect(() => {
    // Radial segment / folder-child editors live outside `selectedKey` (the
    // panel gets a literal), so they never armed the lock: alt-tabbing to a
    // linked app mid-edit switched the profile under the open editor and Save
    // wrote into the other profile's wheel.
    const active = !!selectedKey || !!selectedLibraryId || !!draftAssignment || expansionEditing || quickActionEditing
      || selectedRadialSegment != null || selectedRadialChild != null;
    window.electronAPI?.setEditingActive(active);
  }, [selectedKey, selectedLibraryId, draftAssignment, expansionEditing, quickActionEditing, selectedRadialSegment, selectedRadialChild]);

  const saveConfig = useCallback((newAssignments, newProfiles, newProfile) => {
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories, clipboardCaptureEnabled, clipboardExcludedApps });
    syncEngine(newAssignments, newProfile);
  }, [syncEngine, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories, clipboardCaptureEnabled, clipboardExcludedApps]);

  const handleSaveGlobalVariables = useCallback((newVars) => {
    setGlobalVariables(newVars);
    window.electronAPI?.updateGlobalVariables(newVars);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables: newVars, searchTemplates, searchTemplateCategories, quickActionCategories });
  }, [assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, searchTemplates, searchTemplateCategories, quickActionCategories]);

  // ── Toasts ────────────────────────────────────────────────
  // Queue with max 3 visible (oldest dropped on overflow). Each toast has
  // its own dismiss timer (3.5s for info/success, 5s for warning/error so
  // users have time to read failure messages).
  const TOAST_MAX = 3;
  const dismissToast = useCallback((id) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);
  const showNotification = useCallback((msg, type = 'success') => {
    const id = Date.now() + Math.random();
    const duration = (type === 'warning' || type === 'error') ? 5000 : 3500;
    setToasts(prev => {
      const next = [...prev, { id, msg, type }];
      // Drop oldest when over cap
      return next.length > TOAST_MAX ? next.slice(next.length - TOAST_MAX) : next;
    });
    setTimeout(() => {
      setToasts(prev => prev.filter(t => t.id !== id));
    }, duration);
  }, []);

  // ── Search Template CRUD ──────────────────────────────────
  const handleAddSearchTemplate = useCallback((template) => {
    const next = [...searchTemplates, template];
    setSearchTemplates(next);
    window.electronAPI?.saveConfig({ searchTemplates: next });
  }, [searchTemplates]);

  const handleUpdateSearchTemplate = useCallback((id, updates) => {
    const next = searchTemplates.map(t => t.id === id ? { ...t, ...updates } : t);
    setSearchTemplates(next);
    window.electronAPI?.saveConfig({ searchTemplates: next });
  }, [searchTemplates]);

  const handleDeleteSearchTemplate = useCallback((id) => {
    const next = searchTemplates.filter(t => t.id !== id);
    setSearchTemplates(next);
    window.electronAPI?.saveConfig({ searchTemplates: next });
  }, [searchTemplates]);

  // ── Search Template Category CRUD (v0.8.5: sub-folder aware) ────────────
  const handleAddSearchTemplateCategory = useCallback((name, colour = null) => {
    const next = [...searchTemplateCategories, { name, colour: colour || null }];
    setSearchTemplateCategories(next);
    window.electronAPI?.saveConfig({ searchTemplateCategories: next });
  }, [searchTemplateCategories]);

  // Rename cascades: renaming a parent rewrites every child's prefix AND every
  // template.category that matches the parent OR any child path (prefix match).
  const handleRenameSearchTemplateCategory = useCallback((oldName, newName) => {
    const oldSlash = oldName + '/';
    const nextCats = searchTemplateCategories.map(c => {
      if (c.name === oldName) return { ...c, name: newName };
      if (c.name.startsWith(oldSlash)) return { ...c, name: newName + '/' + c.name.slice(oldSlash.length) };
      return c;
    });
    const nextTemplates = searchTemplates.map(t => {
      if (t.category === oldName) return { ...t, category: newName };
      if (typeof t.category === 'string' && t.category.startsWith(oldSlash)) {
        return { ...t, category: newName + '/' + t.category.slice(oldSlash.length) };
      }
      return t;
    });
    setSearchTemplateCategories(nextCats);
    setSearchTemplates(nextTemplates);
    window.electronAPI?.saveConfig({ searchTemplateCategories: nextCats, searchTemplates: nextTemplates });
  }, [searchTemplateCategories, searchTemplates]);

  // mode: 'single' (default; parent-with-children leaves callers to decide),
  // 'tree' (delete parent + all children), 'promote' (delete parent only;
  // children become top-level, auto-suffix on collision).
  const handleDeleteSearchTemplateCategory = useCallback((name, mode = 'single') => {
    const slash = name + '/';
    const children = searchTemplateCategories.filter(c => c.name.startsWith(slash));

    let nextCats;
    let nextTemplates = searchTemplates;

    if (children.length === 0 || mode === 'tree') {
      const doomed = new Set([name, ...children.map(c => c.name)]);
      nextCats = searchTemplateCategories.filter(c => !doomed.has(c.name));
      nextTemplates = searchTemplates.map(t =>
        (typeof t.category === 'string' && doomed.has(t.category))
          ? { ...t, category: null }
          : t);
    } else {
      // Promote children to top-level with collision auto-suffix.
      const existingTop = new Set(searchTemplateCategories.filter(c => !c.name.includes('/') && c.name !== name).map(c => c.name));
      const promoteMap = new Map();
      nextCats = searchTemplateCategories
        .filter(c => c.name !== name)
        .map(c => {
          if (!c.name.startsWith(slash)) return c;
          const childBase = c.name.slice(slash.length);
          let promoted = childBase;
          if (existingTop.has(promoted)) promoted = `${childBase} (from ${name})`;
          let n = 2;
          while (existingTop.has(promoted)) promoted = `${childBase} (from ${name}) ${n++}`;
          existingTop.add(promoted);
          promoteMap.set(c.name, promoted);
          return { ...c, name: promoted };
        });
      nextTemplates = searchTemplates.map(t => {
        if (t.category === name) return { ...t, category: null };
        if (typeof t.category === 'string' && promoteMap.has(t.category)) {
          return { ...t, category: promoteMap.get(t.category) };
        }
        return t;
      });
    }
    setSearchTemplateCategories(nextCats);
    setSearchTemplates(nextTemplates);
    window.electronAPI?.saveConfig({ searchTemplateCategories: nextCats, searchTemplates: nextTemplates });
  }, [searchTemplateCategories, searchTemplates]);

  const handleUpdateSearchTemplateCategoryColour = useCallback((name, colour) => {
    const next = searchTemplateCategories.map(c => c.name === name ? { ...c, colour } : c);
    setSearchTemplateCategories(next);
    window.electronAPI?.saveConfig({ searchTemplateCategories: next });
  }, [searchTemplateCategories]);

  const handleReorderSearchTemplateCategories = useCallback((newOrder) => {
    setSearchTemplateCategories(newOrder);
    window.electronAPI?.saveConfig({ searchTemplateCategories: newOrder });
  }, []);

  // Bulk-aware move: idOrIds is a single template id or an array (multi-select
  // drag). newCategory may be null (uncategorised) or a path.
  const handleMoveSearchTemplateToCategory = useCallback((idOrIds, newCategory) => {
    const ids = Array.isArray(idOrIds) ? idOrIds : [idOrIds];
    if (ids.length === 0) return;
    const next = newCategory || null;
    const idSet = new Set(ids);
    let changed = false;
    const nextTemplates = searchTemplates.map(t => {
      if (!idSet.has(t.id)) return t;
      const current = t.category ?? null;
      if (current === next) return t;
      changed = true;
      return { ...t, category: next };
    });
    if (!changed) return;
    setSearchTemplates(nextTemplates);
    window.electronAPI?.saveConfig({ searchTemplates: nextTemplates });
  }, [searchTemplates]);

  // Move a category in the tree: 'top' promotes a child, or a parent name
  // demotes a childless top-level under it. Depth cap = 1.
  const handleMoveSearchTemplateCategoryTo = useCallback((name, destination) => {
    const isChild = name.includes('/');
    const baseName = isChild ? name.slice(name.lastIndexOf('/') + 1) : name;

    let newPath;
    if (destination === 'top') {
      if (!isChild) return;
      newPath = baseName;
      if (searchTemplateCategories.some(c => c.name === newPath)) {
        const parentName = name.slice(0, name.lastIndexOf('/'));
        newPath = `${baseName} (from ${parentName})`;
        let n = 2;
        while (searchTemplateCategories.some(c => c.name === newPath)) newPath = `${baseName} (from ${parentName}) ${n++}`;
      }
    } else {
      if (destination.includes('/')) return;
      if (!isChild) {
        const hasChildren = searchTemplateCategories.some(c => c.name.startsWith(name + '/'));
        if (hasChildren) return;
      }
      if (!searchTemplateCategories.some(c => c.name === destination)) return;
      newPath = `${destination}/${baseName}`;
      if (name === newPath) return;
      if (searchTemplateCategories.some(c => c.name === newPath)) return;
    }

    const nextCats = searchTemplateCategories.map(c => c.name === name ? { ...c, name: newPath } : c);
    const nextTemplates = searchTemplates.map(t => t.category === name ? { ...t, category: newPath } : t);
    setSearchTemplateCategories(nextCats);
    setSearchTemplates(nextTemplates);
    window.electronAPI?.saveConfig({ searchTemplateCategories: nextCats, searchTemplates: nextTemplates });
  }, [searchTemplateCategories, searchTemplates]);

  // ── Quick Action CRUD (stored in assignments as GLOBAL::QUICKACTION::uuid) ──
  // Fetch and persist an app icon on a Quick Action assignment. Same priority
  // as the radial fetcher (iconSource → path → appId). Writes data.appIcon back
  // through setAssignments + saveConfig so tiles/overlay render immediately.
  const fetchAndSetQuickActionAppIcon = useCallback(async (qaId, assignmentOverride) => {
    const key = `GLOBAL::QUICKACTION::${qaId}`;
    const assignment = assignmentOverride || assignments[key];
    if (!assignment || assignment.type !== 'app') return;
    const target = assignment.data?.iconSource || assignment.data?.path || assignment.data?.appId;
    if (!target) return;
    try {
      const dataUrl = await window.electronAPI?.getAppIcon(target);
      if (!dataUrl) return;
      setAssignments(prev => {
        const cur = prev[key];
        if (!cur || cur.type !== 'app') return prev;
        if (cur.data?.appIcon === dataUrl) return prev;
        const next = { ...prev, [key]: { ...cur, data: { ...cur.data, appIcon: dataUrl } } };
        // Save ONLY the assignments. This runs after an await, so the helper
        // `saveConfig` captured before it carried stale copies of templates /
        // categories and wrote them back over edits made while the icon was
        // resolving. `next` itself is fresh (functional update).
        window.electronAPI?.saveConfig({ assignments: next });
        window.electronAPI?.updateAssignments(next, activeProfile);
        return next;
      });
    } catch (e) {}
  }, [assignments, activeProfile]);

  const handleAddQuickAction = useCallback((action) => {
    const key = `GLOBAL::QUICKACTION::${action.id}`;
    const newAssignments = { ...assignments, [key]: { type: action.type, label: action.label, data: action.data } };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    if (action.type === 'app' && (action.data?.iconSource || action.data?.path || action.data?.appId)) {
      fetchAndSetQuickActionAppIcon(action.id, { type: action.type, label: action.label, data: action.data });
    }
  }, [assignments, profiles, activeProfile, saveConfig, fetchAndSetQuickActionAppIcon]);

  const handleUpdateQuickAction = useCallback((id, updates) => {
    const key = `GLOBAL::QUICKACTION::${id}`;
    const existing = assignments[key];
    if (!existing) return;
    const merged = { ...existing, ...updates };
    const newAssignments = { ...assignments, [key]: merged };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    // If the app target changed (or was newly set), refetch. When data reference
    // changes we assume the target changed — drop any stale appIcon so the fetch
    // has room to write the new one; if fetch fails, we're left with no icon
    // (better than a wrong one).
    if (merged.type === 'app' && (merged.data?.iconSource || merged.data?.path || merged.data?.appId)) {
      const targetChanged = updates.data && (
        updates.data.iconSource !== existing.data?.iconSource ||
        updates.data.path !== existing.data?.path ||
        updates.data.appId !== existing.data?.appId
      );
      if (targetChanged || !merged.data?.appIcon) {
        fetchAndSetQuickActionAppIcon(id, targetChanged ? { ...merged, data: { ...merged.data, appIcon: undefined } } : merged);
      }
    }
  }, [assignments, profiles, activeProfile, saveConfig, fetchAndSetQuickActionAppIcon]);

  const handleDeleteQuickAction = useCallback((id) => {
    const key = `GLOBAL::QUICKACTION::${id}`;
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, profiles, activeProfile, saveConfig]);

  // ── Quick Action Category CRUD (v0.8.5: sub-folder aware) ───────────────
  const handleAddQaCategory = useCallback((name, colour = null) => {
    const next = [...quickActionCategories, { name, colour: colour || null }];
    setQuickActionCategories(next);
    window.electronAPI?.saveConfig({ quickActionCategories: next });
  }, [quickActionCategories]);

  // Rename cascades: parent → children (prefix rewrite) AND every QA
  // assignment whose data.category matches parent OR any child path.
  const handleRenameQaCategory = useCallback((oldName, newName) => {
    const oldSlash = oldName + '/';
    const nextCats = quickActionCategories.map(c => {
      if (c.name === oldName) return { ...c, name: newName };
      if (c.name.startsWith(oldSlash)) return { ...c, name: newName + '/' + c.name.slice(oldSlash.length) };
      return c;
    });
    setQuickActionCategories(nextCats);
    const newAssignments = { ...assignments };
    let changed = false;
    for (const [k, v] of Object.entries(newAssignments)) {
      if (!k.startsWith('GLOBAL::QUICKACTION::')) continue;
      const cat = v.data?.category;
      if (cat === oldName) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName } };
        changed = true;
      } else if (typeof cat === 'string' && cat.startsWith(oldSlash)) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName + '/' + cat.slice(oldSlash.length) } };
        changed = true;
      }
    }
    // One save carrying both keys. Two concurrent saves (helper + partial)
    // each re-read disk and merged independently, so the last writer dropped
    // the other's key.
    if (changed) {
      setAssignments(newAssignments);
      window.electronAPI?.saveConfig({ assignments: newAssignments, quickActionCategories: nextCats });
      syncEngine(newAssignments, activeProfile);
    } else {
      window.electronAPI?.saveConfig({ quickActionCategories: nextCats });
    }
  }, [quickActionCategories, assignments, activeProfile, syncEngine]);

  // mode: 'single' | 'tree' | 'promote'. Mirrors handleDeleteCategory.
  const handleDeleteQaCategory = useCallback((name, mode = 'single') => {
    const slash = name + '/';
    const children = quickActionCategories.filter(c => c.name.startsWith(slash));

    let nextCats;
    const newAssignments = { ...assignments };
    let changed = false;

    if (children.length === 0 || mode === 'tree') {
      const doomed = new Set([name, ...children.map(c => c.name)]);
      nextCats = quickActionCategories.filter(c => !doomed.has(c.name));
      for (const [k, v] of Object.entries(newAssignments)) {
        if (!k.startsWith('GLOBAL::QUICKACTION::')) continue;
        const cat = v.data?.category;
        if (typeof cat === 'string' && doomed.has(cat)) {
          newAssignments[k] = { ...v, data: { ...v.data, category: null } };
          changed = true;
        }
      }
    } else {
      // Promote children to top-level with collision auto-suffix.
      const existingTop = new Set(quickActionCategories.filter(c => !c.name.includes('/') && c.name !== name).map(c => c.name));
      const promoteMap = new Map();
      nextCats = quickActionCategories
        .filter(c => c.name !== name)
        .map(c => {
          if (!c.name.startsWith(slash)) return c;
          const childBase = c.name.slice(slash.length);
          let promoted = childBase;
          if (existingTop.has(promoted)) promoted = `${childBase} (from ${name})`;
          let n = 2;
          while (existingTop.has(promoted)) promoted = `${childBase} (from ${name}) ${n++}`;
          existingTop.add(promoted);
          promoteMap.set(c.name, promoted);
          return { ...c, name: promoted };
        });
      for (const [k, v] of Object.entries(newAssignments)) {
        if (!k.startsWith('GLOBAL::QUICKACTION::')) continue;
        const cat = v.data?.category;
        if (cat === name) {
          newAssignments[k] = { ...v, data: { ...v.data, category: null } };
          changed = true;
        } else if (typeof cat === 'string' && promoteMap.has(cat)) {
          newAssignments[k] = { ...v, data: { ...v.data, category: promoteMap.get(cat) } };
          changed = true;
        }
      }
    }
    setQuickActionCategories(nextCats);
    // One save carrying both keys. Two concurrent saves (helper + partial)
    // each re-read disk and merged independently, so the last writer dropped
    // the other's key.
    if (changed) {
      setAssignments(newAssignments);
      window.electronAPI?.saveConfig({ assignments: newAssignments, quickActionCategories: nextCats });
      syncEngine(newAssignments, activeProfile);
    } else {
      window.electronAPI?.saveConfig({ quickActionCategories: nextCats });
    }
  }, [quickActionCategories, assignments, activeProfile, syncEngine]);

  const handleUpdateQaCategoryColour = useCallback((name, colour) => {
    const next = quickActionCategories.map(c => c.name === name ? { ...c, colour } : c);
    setQuickActionCategories(next);
    window.electronAPI?.saveConfig({ quickActionCategories: next });
  }, [quickActionCategories]);

  const handleReorderQaCategories = useCallback((newOrder) => {
    setQuickActionCategories(newOrder);
    window.electronAPI?.saveConfig({ quickActionCategories: newOrder });
  }, []);

  // Bulk-aware move: idOrIds is a single QA id or array; newCategory null or path.
  const handleMoveQuickActionToCategory = useCallback((idOrIds, newCategory) => {
    const ids = Array.isArray(idOrIds) ? idOrIds : [idOrIds];
    if (ids.length === 0) return;
    const next = newCategory || null;
    const newAssignments = { ...assignments };
    let changed = false;
    for (const id of ids) {
      const key = `GLOBAL::QUICKACTION::${id}`;
      const existing = newAssignments[key];
      if (!existing) continue;
      const current = existing.data?.category ?? null;
      if (current === next) continue;
      newAssignments[key] = { ...existing, data: { ...existing.data, category: next } };
      changed = true;
    }
    if (!changed) return;
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, profiles, activeProfile, saveConfig]);

  // Move a QA category in the tree: 'top' or parent name. Depth cap = 1.
  const handleMoveQaCategoryTo = useCallback((name, destination) => {
    const isChild = name.includes('/');
    const baseName = isChild ? name.slice(name.lastIndexOf('/') + 1) : name;

    let newPath;
    if (destination === 'top') {
      if (!isChild) return;
      newPath = baseName;
      if (quickActionCategories.some(c => c.name === newPath)) {
        const parentName = name.slice(0, name.lastIndexOf('/'));
        newPath = `${baseName} (from ${parentName})`;
        let n = 2;
        while (quickActionCategories.some(c => c.name === newPath)) newPath = `${baseName} (from ${parentName}) ${n++}`;
      }
    } else {
      if (destination.includes('/')) return;
      if (!isChild) {
        const hasChildren = quickActionCategories.some(c => c.name.startsWith(name + '/'));
        if (hasChildren) return;
      }
      if (!quickActionCategories.some(c => c.name === destination)) return;
      newPath = `${destination}/${baseName}`;
      if (name === newPath) return;
      if (quickActionCategories.some(c => c.name === newPath)) return;
    }

    const nextCats = quickActionCategories.map(c => c.name === name ? { ...c, name: newPath } : c);
    setQuickActionCategories(nextCats);
    const newAssignments = { ...assignments };
    let changed = false;
    for (const [k, v] of Object.entries(newAssignments)) {
      if (!k.startsWith('GLOBAL::QUICKACTION::')) continue;
      if (v.data?.category === name) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newPath } };
        changed = true;
      }
    }
    // One save carrying both keys. Two concurrent saves (helper + partial)
    // each re-read disk and merged independently, so the last writer dropped
    // the other's key.
    if (changed) {
      setAssignments(newAssignments);
      window.electronAPI?.saveConfig({ assignments: newAssignments, quickActionCategories: nextCats });
      syncEngine(newAssignments, activeProfile);
    } else {
      window.electronAPI?.saveConfig({ quickActionCategories: nextCats });
    }
  }, [quickActionCategories, assignments, activeProfile, syncEngine]);

  // ── Modifier toggling ─────────────────────────────────────
  const handleToggleModifier = useCallback((modId) => {
    setActiveModifiers(prev => {
      let next;
      if (modId === 'BARE') {
        next = prev.includes('BARE') ? [] : ['BARE'];
      } else {
        const base = prev.filter(m => m !== 'BARE');
        if (base.includes(modId)) next = base.filter(m => m !== modId);
        else if (base.length >= 3) next = base;
        else next = [...base, modId];
      }
      // Update sidebar filter to match keyboard modifier selection. Must use
      // comboString() — the canonical Ctrl/Shift/Alt/Win order also used in
      // assignment storage keys. A plain alphabetical sort would produce e.g.
      // "Alt+Ctrl" while assignments are keyed "Ctrl+Alt", causing the sidebar
      // filter to silently never match new multi-modifier assignments.
      if (next.length === 0) {
        setSidebarComboFilter(null);
      } else if (next.includes('BARE')) {
        setSidebarComboFilter('BARE');
      } else {
        setSidebarComboFilter(comboString(next));
      }
      return next;
    });
    // Deselect key when modifier layer changes
    setSelectedKey(null);
  }, []);

  // ── Key selection ─────────────────────────────────────────
  // Inner select — bypasses the reserved-shortcut hazard check. Used directly
  // by the modal's onContinue handler after the user accepts the warning.
  const commitKeySelect = useCallback((keyId) => {
    setSelectedKey(keyId);
    setSelectedLibraryId(null);
    if (draftAssignment) {
      const key = `${activeProfile}::${currentCombo}::${keyId}`;
      const doubleKey = key + '::double';
      if (assignments[key] || assignments[doubleKey]) {
        setDraftAssignment(null);
        setDraftDoubleAssignment(null);
      }
    }
  }, [draftAssignment, assignments, activeProfile, currentCombo]);

  const handleKeySelect = useCallback((keyId) => {
    if (activeModifiers.length === 0) return; // require a modifier layer
    // Clicking the currently-selected key deselects — skip the hazard check.
    if (selectedKey === keyId) {
      setSelectedKey(null);
      return;
    }
    // Reserved Windows shortcut? Show hazard modal before the user invests
    // time in the action editor. Cancel leaves selection unchanged.
    const reserved = findReservedShortcut(currentCombo, keyId);
    if (reserved) {
      setReservedShortcutPending({
        keyId,
        comboDisplay: formatComboDisplay(currentCombo, keyId),
        osFunction: reserved.osFunction,
        profileName: activeProfile,
      });
      return;
    }
    commitKeySelect(keyId);
  }, [activeModifiers, currentCombo, activeProfile, selectedKey, commitKeySelect]);

  // ── Assignment key format: "Profile::Ctrl+Alt::KeyE" ──────
  const makeAssignmentKey = useCallback((profile, combo, keyId) => {
    return `${profile}::${combo}::${keyId}`;
  }, []);

  const getKeyAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return null;
    return assignments[makeAssignmentKey(activeProfile, currentCombo, keyId)] || null;
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeAssignmentKey]);

  // ── Assign macro ──────────────────────────────────────────
  // ── Radial label propagation helper ────────────────────────
  // Apply `transformMap(itemsByProfile) -> itemsByProfile | same` to the
  // Default layout AND every extra layout, persisting whichever changed. Every
  // sweep that rewrites storageKeys or labels must go through here — a wedge
  // in a layout another device fires would otherwise silently go dead.
  const transformAllRadialMaps = useCallback((transformMap) => {
    setRadialItemsMap(prev => {
      const next = transformMap(prev);
      if (next === prev) return prev;
      window.electronAPI?.saveConfig({ radialMenuItemsByProfile: next });
      return next;
    });
    setRadialLayouts(prev => {
      let changed = false;
      const nextLayouts = prev.map(l => {
        const m = l.itemsByProfile || {};
        const nm = transformMap(m);
        if (nm === m) return l;
        changed = true;
        return { ...l, itemsByProfile: nm };
      });
      if (!changed) return prev;
      window.electronAPI?.saveConfig({ radialLayouts: nextLayouts });
      return nextLayouts;
    });
  }, []);

  // Sweeps every profile's radial items + folder children, updating any segment
  // whose storageKey matches AND whose label still equals oldLabel. Segments
  // the user independently renamed in the radial editor are preserved.
  // Declared before handleAssign / handleRenameAssignment so both useCallback
  // dep arrays can reference it without hitting a TDZ ReferenceError on render.
  const propagateLabelToRadialItems = useCallback((key, oldLabelRaw, newLabelRaw) => {
    const oldLabel = oldLabelRaw || '';
    const newLabel = newLabelRaw || '';
    if (oldLabel === newLabel) return;
    transformAllRadialMaps(prev => {
      let mapChanged = false;
      const nextMap = {};
      for (const [profileName, items] of Object.entries(prev)) {
        if (!Array.isArray(items)) { nextMap[profileName] = items; continue; }
        let profileChanged = false;
        const nextItems = items.map(item => {
          if (!item) return item;
          if (item.type === 'folder' && Array.isArray(item.children)) {
            let kidsChanged = false;
            const nextKids = item.children.map(child => {
              if (child && child.storageKey === key && (child.label || '') === oldLabel) {
                kidsChanged = true;
                return { ...child, label: newLabel };
              }
              return child;
            });
            if (kidsChanged) { profileChanged = true; return { ...item, children: nextKids }; }
            return item;
          }
          if (item.storageKey === key && (item.label || '') === oldLabel) {
            profileChanged = true;
            return { ...item, label: newLabel };
          }
          return item;
        });
        if (profileChanged) { mapChanged = true; nextMap[profileName] = nextItems; }
        else nextMap[profileName] = items;
      }
      if (!mapChanged) return prev;
      return nextMap;
    });
  }, [transformAllRadialMaps]);

  // Sweeps every profile's radial items + folder children, re-pointing any
  // segment whose storageKey appears in keyMap ({oldKey: newKey}). Radial
  // wedges reference assignments by storage key, so every operation that
  // rewrites a key (Unassign, Bind, Reassign/swap, displace-to-library) must
  // remap here or the wedge silently goes dead at fire time.
  const remapRadialStorageKeys = useCallback((keyMap) => {
    if (!keyMap || Object.keys(keyMap).length === 0) return;
    transformAllRadialMaps(prev => {
      let mapChanged = false;
      const nextMap = {};
      for (const [profileName, items] of Object.entries(prev)) {
        if (!Array.isArray(items)) { nextMap[profileName] = items; continue; }
        let profileChanged = false;
        const nextItems = items.map(item => {
          if (!item) return item;
          if (item.type === 'folder' && Array.isArray(item.children)) {
            let kidsChanged = false;
            const nextKids = item.children.map(child => {
              if (child && child.storageKey && keyMap[child.storageKey]) {
                kidsChanged = true;
                return { ...child, storageKey: keyMap[child.storageKey] };
              }
              return child;
            });
            if (kidsChanged) { profileChanged = true; return { ...item, children: nextKids }; }
            return item;
          }
          if (item.storageKey && keyMap[item.storageKey]) {
            profileChanged = true;
            return { ...item, storageKey: keyMap[item.storageKey] };
          }
          return item;
        });
        if (profileChanged) { mapChanged = true; nextMap[profileName] = nextItems; }
        else nextMap[profileName] = items;
      }
      if (!mapChanged) return prev;
      return nextMap;
    });
  }, [transformAllRadialMaps]);

  // Shared post-move selection: activate the target trigger's layer, select
  // the key, drop any library selection, and land on the right canvas.
  const selectTrigger = useCallback((combo, keyId) => {
    const mods = combo === 'BARE' ? ['BARE'] : (combo ? combo.split('+').filter(Boolean) : []);
    setActiveModifiers(mods);
    setSelectedKey(keyId);
    setSelectedLibraryId(null);
    setActiveView(keyId.startsWith('MOUSE_') ? 'mouse' : 'keyboard');
  }, []);

  const handleAssign = useCallback((keyId, macro) => {
    const key = makeAssignmentKey(activeProfile, currentCombo, keyId);
    const oldLabel = assignments[key]?.label || '';
    const newAssignments = { ...assignments, [key]: macro };
    // If a draft duplicate has a double-press counterpart, save that too
    if (draftDoubleAssignment) {
      const doubleKey = `${activeProfile}::${currentCombo}::${keyId}::double`;
      newAssignments[doubleKey] = draftDoubleAssignment;
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
    propagateLabelToRadialItems(key, oldLabel, macro?.label || '');
    showNotification(`Assigned to ${triggerLabel(currentCombo, keyId)}`);
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeAssignmentKey, draftDoubleAssignment, propagateLabelToRadialItems]);

  // ── Clear key (single-press only) ────────────────────────
  // Removes the single-press assignment for this combo+keyId. Double-press
  // assignment (if present) is preserved. For the "remove both single and
  // double" semantics, see handleDeleteKey.
  const handleClearKey = useCallback((keyId) => {
    const key = makeAssignmentKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Cleared ${triggerLabel(currentCombo, keyId)}`, 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Delete key (both single + double) ────────────────────
  // Wipes both single-press and double-press assignments for this combo+keyId
  // in one action. UI confirmation lives in MacroPanel; this handler trusts
  // the caller has already confirmed intent.
  const handleDeleteKey = useCallback((keyId) => {
    const key = makeAssignmentKey(activeProfile, currentCombo, keyId);
    const doubleKey = key + '::double';
    const holdKey = key + '::hold';
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    delete newAssignments[doubleKey];
    delete newAssignments[holdKey];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Deleted ${triggerLabel(currentCombo, keyId)}`, 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Rename assignment label (sidebar right-click → Rename) ───────────
  const handleRenameAssignment = useCallback((combo, keyId, newLabel) => {
    const base = `${activeProfile}::${combo}::${keyId}`;
    // Double-only / hold-only rows (and unassigned entries carrying only a
    // preserved variant) have no base entry — rename the first variant that
    // exists so the rename input doesn't silently no-op.
    const key = ASSIGNMENT_VARIANT_SUFFIXES.map(s => base + s).find(k => assignments[k]);
    if (!key) return;
    const existing = assignments[key];
    const oldLabel = existing.label || '';
    const newAssignments = { ...assignments, [key]: { ...existing, label: newLabel } };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    propagateLabelToRadialItems(key, oldLabel, newLabel);
  }, [assignments, activeProfile, profiles, saveConfig, propagateLabelToRadialItems]);

  // ── Clear assignment by combo+keyId (context menu) ────────
  const handleClearAssignment = useCallback((combo, keyId) => {
    const key = `${activeProfile}::${combo}::${keyId}`;
    const doubleKey = key + '::double';
    const holdKey = key + '::hold';
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    delete newAssignments[doubleKey];
    delete newAssignments[holdKey];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    syncEngine(newAssignments, activeProfile);
    if (selectedKey === keyId) setSelectedKey(null);
    // Sidebar Delete on an unassigned entry routes here with combo="UNASSIGNED"
    // and keyId = the entry's uuid.
    setSelectedLibraryId(prev => (prev === keyId ? null : prev));
    showNotification(combo === 'UNASSIGNED' ? 'Deleted from Unassigned' : `Cleared ${triggerLabel(combo, keyId)}`, 'info');
  }, [assignments, activeProfile, profiles, saveConfig, syncEngine, selectedKey, showNotification]);

  // ── Duplicate assignment via draft state ──────────────────
  // When the user right-clicks → Duplicate, the cloned action lives in
  // draftAssignment (plus draftDoubleAssignment, declared up top) until they
  // save it against a real key. No auto-recording — user picks the key on
  // their own via Record button or by clicking on the keyboard.
  const clearDraft = useCallback(() => {
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
  }, []);

  // Routes through the overlay-based duplicate (same flow as the editor's
  // Duplicate button): select the source item, then signal MacroPanel to open
  // its capture overlay. Unlike the old draft flow this carries ALL press-mode
  // variants (double-only and hold-only items included) and supports mouse /
  // scroll triggers as both source and destination.
  const handleDuplicateFromContext = useCallback((combo, keyId) => {
    const mods = combo === 'BARE' ? ['BARE'] : combo.split('+').filter(Boolean);
    setActiveModifiers(mods);
    setSelectedKey(keyId);
    setSelectedLibraryId(null);
    setActiveView(keyId.startsWith('MOUSE_') ? 'mouse' : 'keyboard');
    setDuplicateOverlaySignal(s => s + 1);
  }, []);

  // ── Double-tap assignment helpers ────────────────────────
  const makeDoubleKey = useCallback((profile, combo, keyId) => {
    return `${profile}::${combo}::${keyId}::double`;
  }, []);

  const getDoubleAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return null;
    return assignments[makeDoubleKey(activeProfile, currentCombo, keyId)] || null;
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeDoubleKey]);

  const hasDoubleAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return false;
    return !!assignments[makeDoubleKey(activeProfile, currentCombo, keyId)];
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeDoubleKey]);

  const handleAssignDouble = useCallback((keyId, macro) => {
    const key = makeDoubleKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments, [key]: macro };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Double-tap assigned to ${triggerLabel(currentCombo, keyId)}`);
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeDoubleKey]);

  const handleClearDouble = useCallback((keyId) => {
    const key = makeDoubleKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Double-tap cleared', 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeDoubleKey]);

  // ── Hold trigger assignment helpers (v0.5, Pro) ────────────
  const makeHoldKey = useCallback((profile, combo, keyId) => {
    return `${profile}::${combo}::${keyId}::hold`;
  }, []);

  const getHoldAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return null;
    return assignments[makeHoldKey(activeProfile, currentCombo, keyId)] || null;
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeHoldKey]);

  const hasHoldAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return false;
    return !!assignments[makeHoldKey(activeProfile, currentCombo, keyId)];
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeHoldKey]);

  const handleAssignHold = useCallback((keyId, macro) => {
    const key = makeHoldKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments, [key]: macro };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Hold assigned to ${triggerLabel(currentCombo, keyId)}`);
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeHoldKey]);

  const handleClearHold = useCallback((keyId) => {
    const key = makeHoldKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Hold cleared', 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeHoldKey]);

  // ── Profile management ────────────────────────────────────
  const handleProfileChange = useCallback((profile) => {
    setActiveProfile(profile);
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
    saveConfig(assignments, profiles, profile);
    showNotification(`Profile: ${profile}`, 'info');
  }, [assignments, profiles, profileSettings, saveConfig, showNotification]);

  const handleAddProfile = useCallback((name) => {
    if (!profiles.includes(name)) {
      const newProfiles = [...profiles, name];
      setProfiles(newProfiles);
      setActiveProfile(name);
      setSelectedKey(null);
      saveConfig(assignments, newProfiles, name);
      showNotification(`Profile "${name}" created`);
    }
  }, [profiles, assignments, saveConfig, showNotification]);

  const handleRenameProfile = useCallback((oldName, newName) => {
    if (!newName || newName === oldName || profiles.includes(newName)) return;
    // Rewrite all assignment keys from OldName:: to NewName::
    const newAssignments = {};
    const prefix = oldName + '::';
    for (const [k, v] of Object.entries(assignments)) {
      if (k.startsWith(prefix)) {
        newAssignments[newName + '::' + k.slice(prefix.length)] = v;
      } else {
        newAssignments[k] = v;
      }
    }
    // Rewrite profileSettings key
    const newProfileSettings = { ...profileSettings };
    if (newProfileSettings[oldName]) {
      newProfileSettings[newName] = newProfileSettings[oldName];
      delete newProfileSettings[oldName];
    }
    // Rewrite the radial layout: the per-profile map is keyed by profile
    // name AND every wedge's storageKey embeds the profile prefix. Rust
    // resolves the wheel by radialMenuItemsByProfile[active_profile], so
    // without this the renamed profile came up with an empty wheel and the
    // orphaned layout pointed at keys that no longer existed.
    const remapKey = (k) => (typeof k === 'string' && k.startsWith(prefix)) ? newName + '::' + k.slice(prefix.length) : k;
    const remapItem = (item) => {
      if (!item) return item;
      const next = item.storageKey ? { ...item, storageKey: remapKey(item.storageKey) } : { ...item };
      if (item.type === 'folder' && Array.isArray(item.children)) {
        next.children = item.children.map(remapItem);
      }
      return next;
    };
    const remapMap = (source) => {
      const out = {};
      for (const [profileName, items] of Object.entries(source || {})) {
        const targetName = profileName === oldName ? newName : profileName;
        out[targetName] = Array.isArray(items) ? items.map(remapItem) : items;
      }
      return out;
    };
    const newRadialMap = remapMap(radialItemsMap);
    // Extra per-device layouts carry the same per-profile shape.
    const newRadialLayouts = radialLayouts.map(l => ({ ...l, itemsByProfile: remapMap(l.itemsByProfile) }));
    const newProfiles = profiles.map(p => p === oldName ? newName : p);
    const newActive   = activeProfile === oldName ? newName : activeProfile;
    const newGlobal   = activeGlobalProfile === oldName ? newName : activeGlobalProfile;
    setAssignments(newAssignments);
    setProfiles(newProfiles);
    setActiveProfile(newActive);
    setProfileSettings(newProfileSettings);
    setRadialItemsMap(newRadialMap);
    if (radialLayouts.length) setRadialLayouts(newRadialLayouts);
    if (newGlobal !== activeGlobalProfile) {
      setActiveGlobalProfile(newGlobal);
      window.electronAPI?.setActiveGlobalProfile(newGlobal);
    }
    window.electronAPI?.updateProfileSettings(newProfileSettings);
    // Only the keys this handler actually changed. save_config shallow-merges,
    // and sending theme / expansionCategories / autocorrectEnabled from this
    // closure wrote back STALE copies (they weren't in the dep array), so a
    // category added moments earlier vanished on a profile rename.
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newActive, activeGlobalProfile: newGlobal, profileSettings: newProfileSettings, radialMenuItemsByProfile: newRadialMap, ...(radialLayouts.length ? { radialLayouts: newRadialLayouts } : {}), hasSeenWelcome: true });
    syncEngine(newAssignments, newActive);
    showNotification(`Renamed to "${newName}"`);
  }, [profiles, assignments, profileSettings, activeProfile, activeGlobalProfile, radialItemsMap, radialLayouts, syncEngine, showNotification]);

  // ── Toggle macros ─────────────────────────────────────────
  const handleToggleMacros = useCallback(() => {
    const newVal = !macrosEnabled;
    setMacrosEnabled(newVal);
    window.electronAPI?.toggleMacros(newVal);
    showNotification(newVal ? 'Macros active' : 'Macros paused', newVal ? 'success' : 'info');
  }, [macrosEnabled, showNotification]);

  // ── Theme setter (replaces binary toggle with 3-state setter) ──
  // Accepts 'auto' | 'light' | 'dark'. Resolves auto via matchMedia, applies
  // data-theme attribute synchronously, persists user's chosen mode (not the
  // resolved value) so a later OS-theme flip still works in auto mode.
  const handleSetTheme = useCallback((value) => {
    if (value !== 'auto' && value !== 'light' && value !== 'dark') return;
    setTheme(value);
    const resolved = value === 'auto'
      ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
      : value;
    setResolvedTheme(resolved);
    document.documentElement.setAttribute('data-theme', resolved);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme: value, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  // ── Live OS-theme tracking when theme === 'auto' ─────────────
  // Subscribes to prefers-color-scheme changes so Keyfire re-themes if the user
  // flips Windows light/dark without restarting Keyfire. No-op when theme is set
  // to an explicit value (user override).
  useEffect(() => {
    if (theme !== 'auto') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      const r = mq.matches ? 'dark' : 'light';
      setResolvedTheme(r);
      document.documentElement.setAttribute('data-theme', r);
    };
    apply();
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);

  // ── Text expansions (global — shared across all profiles) ─
  // Alias entries (data.isAlias === true) are shadow copies of a primary
  // expansion so the buffer matcher fires on any alias trigger without needing
  // per-key lookup logic. They are hidden from this list — the UI only shows
  // the primary. The primary carries `aliases: string[]` for editing.
  // Memoised: this rebuilt (and re-sorted) the full expansion list — every
  // body's text AND html — on every App render, i.e. on every toast, every
  // macro fire, every engine-status event, and handed TextExpansions a new
  // array identity each time.
  const expansions = useMemo(() => Object.entries(assignments)
    .filter(([k, v]) => k.startsWith('GLOBAL::EXPANSION::') && !v?.data?.isAlias)
    .map(([k, v]) => ({
      trigger: k.slice('GLOBAL::EXPANSION::'.length),
      html: v.data?.html || '',
      text: v.data?.text || '',
      category: v.data?.category || null,
      triggerMode: v.data?.triggerMode || 'space',
      displayName: v.data?.displayName || null,
      expansionType: v.data?.expansionType || 'text',
      imagePath: v.data?.imagePath || '',
      imageScale: v.data?.imageScale ?? 100,
      options: v.data?.options || [],
      randomVariant: v.data?.randomVariant === true,
      aliases: Array.isArray(v.data?.aliases) ? v.data.aliases : [],
      voicePhrases: readVoicePhrases(v.data),
    }))
    .sort((a, b) => a.trigger.localeCompare(b.trigger)), [assignments]);

  // editorValue is { html, text } from the rich text editor.
  // originalTrigger is provided when editing an existing expansion; if it differs
  // from trigger the old key is removed in the same update (single atomic write).
  // aliases is an optional string[] of additional triggers that fire the same
  // expansion — persisted on the primary as data.aliases, and each alias also
  // gets its own GLOBAL::EXPANSION::<alias> assignment entry marked isAlias:true
  // so the buffer matcher (which does exact-trigger lookup) picks them up.
  const handleAddExpansion = useCallback((trigger, editorValue, originalTrigger, category, triggerMode, displayName, expansionType, imagePath, imageScale, variantOptions, voicePhrases, aliases, randomVariant) => {
    const newAssignments = { ...assignments };
    // Sweep previous alias shadow entries so a rename or alias-list edit doesn't
    // leave stale keys behind. Applies to both the old-name AND new-name flavour
    // so we cover the rename case in one loop.
    const oldTriggerToClean = originalTrigger || trigger;
    Object.keys(newAssignments).forEach(k => {
      if (!k.startsWith('GLOBAL::EXPANSION::')) return;
      const d = newAssignments[k]?.data;
      if (d?.isAlias && (d.primaryTrigger === oldTriggerToClean || d.primaryTrigger === trigger)) {
        delete newAssignments[k];
      }
    });
    if (originalTrigger && originalTrigger !== trigger) {
      delete newAssignments[`GLOBAL::EXPANSION::${originalTrigger}`];
    }
    let cleanAliases = Array.isArray(aliases)
      ? Array.from(new Set(aliases.map(a => (a || '').trim().toLowerCase()).filter(a => a && a !== trigger)))
      : [];
    // Belt-and-braces behind the editor's clash checks: never overwrite a key
    // that belongs to a DIFFERENT expansion. After the sweep above, anything
    // still at these keys is another expansion's primary or alias shadow.
    const ownedByOther = (k) => {
      const e = newAssignments[k];
      if (!e) return null;
      const d = e.data || {};
      if (d.isAlias) return d.primaryTrigger === trigger ? null : (d.primaryTrigger || 'another expansion');
      return k === `GLOBAL::EXPANSION::${trigger}` && !originalTrigger ? (d.displayName || trigger) : null;
    };
    const primaryOwner = ownedByOther(`GLOBAL::EXPANSION::${trigger}`);
    if (primaryOwner) {
      showNotification(`"${trigger}" is already used by "${primaryOwner}". Choose a different trigger.`, 'error');
      return;
    }
    cleanAliases = cleanAliases.filter(a => {
      const owner = ownedByOther(`GLOBAL::EXPANSION::${a}`);
      if (owner) showNotification(`Alias "${a}" is already used by "${owner}" and was not added.`, 'info');
      return !owner;
    });
    const data = { category: category || null, triggerMode: triggerMode || 'space', displayName: displayName || null };
    if (expansionType === 'image') {
      data.expansionType = 'image';
      data.imagePath = imagePath;
      data.imageScale = imageScale ?? 100;
    } else {
      data.html = editorValue.html;
      data.text = editorValue.text;
    }
    if (variantOptions && variantOptions.length > 0) {
      data.options = variantOptions;
      // randomVariant only lives alongside options — a variantless expansion
      // never fires the random path so no need to keep the flag around.
      if (randomVariant === true) {
        data.randomVariant = true;
      }
    }
    if (cleanAliases.length > 0) {
      data.aliases = cleanAliases;
    }
    // Voice phrases: array with read fallback to legacy single string handled
    // by writeVoicePhrases — empty array deletes both fields so no orphan keys.
    writeVoicePhrases(data, voicePhrases);
    newAssignments[`GLOBAL::EXPANSION::${trigger}`] = {
      type: 'expansion',
      label: displayName || (expansionType === 'image' ? `Image: ${trigger}` : `Expand: ${trigger}`),
      data,
    };
    // Write one shadow entry per alias — identical data payload plus isAlias flag
    // and back-pointer so cleanup + `{{expansion:...}}` lookup work uniformly.
    for (const alias of cleanAliases) {
      newAssignments[`GLOBAL::EXPANSION::${alias}`] = {
        type: 'expansion',
        label: displayName || (expansionType === 'image' ? `Image: ${alias}` : `Expand: ${alias}`),
        data: { ...data, isAlias: true, primaryTrigger: trigger, aliases: undefined },
      };
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Expansion "${trigger}" saved`);
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  // Wired from ClipboardPanel's "Create Expansion" button. Seeds the pending
  // prefill, then jumps to the Expansions tab — TextExpansions consumes the
  // prefill on mount and clears it via onPrefillConsumed. The requestedAt
  // timestamp guarantees the effect re-fires when the same text is sent twice.
  const handleCreateExpansionFromClip = useCallback((text) => {
    if (!text) return;
    setPendingExpansionPrefill({ text, requestedAt: Date.now() });
    setActiveArea('expansions');
  }, []);

  const handleDeleteExpansion = useCallback((trigger) => {
    const newAssignments = { ...assignments };
    delete newAssignments[`GLOBAL::EXPANSION::${trigger}`];
    // Sweep shadow alias entries pointing at this primary.
    Object.keys(newAssignments).forEach(k => {
      if (!k.startsWith('GLOBAL::EXPANSION::')) return;
      const d = newAssignments[k]?.data;
      if (d?.isAlias && d.primaryTrigger === trigger) {
        delete newAssignments[k];
      }
    });
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Expansion "${trigger}" deleted`, 'info');
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  // Bulk delete from the multi-select selection bar — one state update, one
  // config save, one toast, however many rows were selected. Sweeps alias
  // shadow entries for each removed primary in the same pass.
  const handleDeleteExpansionsBulk = useCallback((triggers) => {
    if (!Array.isArray(triggers) || triggers.length === 0) return;
    const newAssignments = { ...assignments };
    const removedSet = new Set();
    let deleted = 0;
    for (const trigger of triggers) {
      const key = `GLOBAL::EXPANSION::${trigger}`;
      if (newAssignments[key]) {
        delete newAssignments[key];
        removedSet.add(trigger);
        deleted++;
      }
    }
    if (removedSet.size > 0) {
      Object.keys(newAssignments).forEach(k => {
        if (!k.startsWith('GLOBAL::EXPANSION::')) return;
        const d = newAssignments[k]?.data;
        if (d?.isAlias && removedSet.has(d.primaryTrigger)) {
          delete newAssignments[k];
        }
      });
    }
    if (deleted === 0) return;
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    const noun = deleted === 1 ? 'expansion' : 'expansions';
    showNotification(`${deleted} ${noun} deleted`, 'info');
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  // ── Expansion pack export / import ────────────────────────
  // Text-only export. Image expansions are skipped (their imagePath is local
  // to the exporter's machine). Shape mirrors profile export but with a
  // different discriminator so import flows can tell pack files apart.
  const handleExportExpansions = useCallback(async (scope, scopeKey) => {
    const allExpansions = Object.entries(assignments)
      .filter(([k]) => k.startsWith('GLOBAL::EXPANSION::'))
      .map(([k, v]) => ({
        trigger: k.slice('GLOBAL::EXPANSION::'.length),
        data: v?.data || {},
      }));

    let scoped = allExpansions;
    if (scope === 'category') {
      scoped = scoped.filter(e => (e.data.category || null) === (scopeKey || null));
    } else if (scope === 'single') {
      scoped = scoped.filter(e => e.trigger === scopeKey);
    }

    const textOnly = scoped.filter(e => (e.data.expansionType || 'text') !== 'image');
    const skippedImages = scoped.length - textOnly.length;

    if (textOnly.length === 0) {
      if (scope === 'single') {
        showNotification('Image expansions cannot be exported', 'info');
      } else if (scope === 'category') {
        showNotification(`No text expansions in "${scopeKey}" to export`, 'info');
      } else {
        showNotification('No text expansions to export', 'info');
      }
      return;
    }

    // Strip image-only fields defensively in case any leaked through.
    const cleanedExpansions = textOnly.map(e => {
      const data = { ...e.data };
      delete data.expansionType;
      delete data.imagePath;
      delete data.imageScale;
      return { trigger: e.trigger, data };
    });

    const referencedCats = new Set(
      cleanedExpansions.map(e => e.data.category).filter(Boolean)
    );
    const exportCategories = expansionCategories
      .filter(c => referencedCats.has(c.name))
      .map(c => ({ name: c.name, colour: c.colour || null }));

    const name = scope === 'all' ? 'All Expansions' : (scopeKey || 'Expansions');
    const payload = {
      trigr_expansion_pack: '1.0',
      scope,
      name,
      exportedAt: new Date().toISOString(),
      expansions: cleanedExpansions,
      categories: exportCategories,
    };

    const slug = (name || 'expansions')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
    const filenameHint = `${slug || 'expansions'}-trigr-expansions.json`;
    const content = JSON.stringify(payload, null, 2);

    try {
      const result = await window.electronAPI?.exportProfile(filenameHint, content);
      if (result?.ok) {
        let msg;
        if (scope === 'single') {
          msg = `Expansion "${scopeKey}" exported`;
        } else {
          const noun = textOnly.length === 1 ? 'expansion' : 'expansions';
          msg = `Exported ${textOnly.length} text ${noun}`;
          if (skippedImages > 0) {
            const imgNoun = skippedImages === 1 ? 'expansion' : 'expansions';
            msg += `. ${skippedImages} image ${imgNoun} skipped (images stay on your machine).`;
          }
        }
        showNotification(msg);
      } else if (result?.error) {
        showNotification(result.error, 'info');
      }
    } catch (e) {
      console.error('[Keyfire] Export expansions failed:', e);
    }
  }, [assignments, expansionCategories, showNotification]);

  // Applies an expansion pack to current state. `choice` is 'skip' or
  // 'overwrite' and controls how triggers that already exist locally are
  // handled. Categories referenced by the pack are added if missing; existing
  // categories keep their current colour.
  const applyExpansionImport = useCallback((packExpansions, packCategories, choice, extraNotes) => {
    const newAssignments = { ...assignments };
    let imported = 0;
    let skipped = 0;
    let overwritten = 0;

    for (const exp of packExpansions) {
      const trigger = exp?.trigger;
      if (!trigger) continue;
      const key = `GLOBAL::EXPANSION::${trigger}`;
      const existed = !!newAssignments[key];
      if (existed && choice === 'skip') { skipped++; continue; }

      const data = exp.data && typeof exp.data === 'object' ? { ...exp.data } : {};
      // Drop any image fields that snuck in — pack format is text-only.
      delete data.expansionType;
      delete data.imagePath;
      delete data.imageScale;

      const displayName = data.displayName || null;
      newAssignments[key] = {
        type: 'expansion',
        label: displayName || `Expand: ${trigger}`,
        data,
      };

      if (existed) overwritten++;
      else imported++;
    }

    const existingCatNames = new Set(expansionCategories.map(c => c.name));
    const newCategories = [...expansionCategories];
    for (const cat of packCategories || []) {
      if (cat && cat.name && !existingCatNames.has(cat.name)) {
        newCategories.push({ name: cat.name, colour: cat.colour || null });
        existingCatNames.add(cat.name);
      }
    }

    setAssignments(newAssignments);
    setExpansionCategories(newCategories);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({
      assignments: newAssignments,
      profiles, activeProfile, activeGlobalProfile, profileSettings, theme,
      expansionCategories: newCategories,
      autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true,
      globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories,
    });

    let msg;
    if (choice === 'skip') {
      const noun = imported === 1 ? 'expansion' : 'expansions';
      msg = `Imported ${imported} new ${noun}`;
      if (skipped > 0) {
        const sNoun = skipped === 1 ? 'expansion' : 'expansions';
        msg += `. ${skipped} ${sNoun} skipped (already existed).`;
      }
    } else {
      const total = imported + overwritten;
      const noun = total === 1 ? 'expansion' : 'expansions';
      msg = `Imported ${total} ${noun}`;
      if (overwritten > 0) {
        const oNoun = overwritten === 1 ? 'expansion' : 'expansions';
        msg += `. ${overwritten} existing ${oNoun} overwritten.`;
      }
    }
    // Lossy-conversion notes from third-party import adapters.
    if (Array.isArray(extraNotes) && extraNotes.length > 0) {
      msg += ` ${extraNotes.join(' ')}`;
    }
    showNotification(msg);
  }, [assignments, expansionCategories, syncEngine, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, autocorrectEnabled, macrosEnabledOnStartup, globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories, showNotification]);

  const handleImportExpansions = useCallback(async () => {
    try {
      const result = await window.electronAPI?.importProfile();
      if (!result?.ok) {
        if (result?.error) showNotification(result.error, 'info');
        return;
      }
      let parsed;
      try {
        parsed = JSON.parse(result.content);
      } catch {
        showNotification('Could not parse expansion file', 'info');
        return;
      }
      if (!parsed || !parsed.trigr_expansion_pack) {
        showNotification('Not a valid Keyfire expansion pack', 'info');
        return;
      }
      const packExpansions = Array.isArray(parsed.expansions) ? parsed.expansions : [];
      const packCategories = Array.isArray(parsed.categories) ? parsed.categories : [];
      if (packExpansions.length === 0) {
        showNotification('Expansion pack is empty', 'info');
        return;
      }

      const existingTriggers = new Set(
        Object.keys(assignments)
          .filter(k => k.startsWith('GLOBAL::EXPANSION::'))
          .map(k => k.slice('GLOBAL::EXPANSION::'.length))
      );
      const collisions = packExpansions
        .map(e => e?.trigger)
        .filter(t => t && existingTriggers.has(t));

      if (collisions.length === 0) {
        // No conflicts — import straight through with overwrite semantics
        // (no-op overwrite since nothing exists).
        applyExpansionImport(packExpansions, packCategories, 'overwrite');
        return;
      }
      setExpansionImportPrompt({
        expansions: packExpansions,
        categories: packCategories,
        collisions,
        totalCount: packExpansions.length,
      });
    } catch (e) {
      console.error('[Keyfire] Import expansions failed:', e);
      showNotification('Expansion import failed', 'info');
    }
  }, [assignments, applyExpansionImport, showNotification]);

  // ── Third-party expansion import (Import From ▾) ────────────────────────
  // One-way migration from other tools. Adapters in importAdapters.js are
  // pure parsers; this handler owns the file dialog, stamps every entry into
  // the "Imported" category (created if missing) so users can recategorise
  // at their own pace, and reuses the native collision flow + sink.
  const handleImportExpansionsFrom = useCallback(async (format) => {
    const FORMATS = {
      espanso: {
        label: 'Espanso',
        title: 'Import from Espanso',
        filterName: 'Espanso match files',
        extensions: ['yml', 'yaml'],
        parse: parseEspansoYaml,
      },
      ahk: {
        label: 'AutoHotkey',
        title: 'Import from AutoHotkey',
        filterName: 'AutoHotkey scripts',
        extensions: ['ahk'],
        parse: parseAhkHotstrings,
      },
      textexpander: {
        label: 'TextExpander',
        title: 'Import from TextExpander',
        filterName: 'TextExpander CSV exports',
        extensions: ['csv'],
        parse: parseTextExpanderCsv,
      },
      textblaze: {
        label: 'Text Blaze',
        title: 'Import from Text Blaze',
        filterName: 'Text Blaze JSON exports',
        extensions: ['json'],
        parse: parseTextBlazeJson,
      },
    };
    const meta = FORMATS[format];
    if (!meta) return;
    try {
      const result = await window.electronAPI?.importTextFile(meta.title, meta.filterName, meta.extensions);
      if (!result?.ok) {
        if (result?.error) showNotification(result.error, 'info');
        return;
      }
      const { expansions: parsed, warnings } = meta.parse(result.content);
      if (parsed.length === 0) {
        const detail = warnings.length > 0 ? ` ${warnings.join(' ')}` : '';
        showNotification(`No importable snippets found in that ${meta.label} file.${detail}`, 'info');
        return;
      }
      const packExpansions = parsed.map(e => ({
        trigger: e.trigger,
        data: { ...e.data, category: 'Imported' },
      }));
      const packCategories = [{ name: 'Imported', colour: '#4080E8' }];

      const existingTriggers = new Set(
        Object.keys(assignments)
          .filter(k => k.startsWith('GLOBAL::EXPANSION::'))
          .map(k => k.slice('GLOBAL::EXPANSION::'.length))
      );
      const collisions = packExpansions
        .map(e => e.trigger)
        .filter(t => existingTriggers.has(t));

      if (collisions.length === 0) {
        applyExpansionImport(packExpansions, packCategories, 'overwrite', warnings);
        return;
      }
      setExpansionImportPrompt({
        expansions: packExpansions,
        categories: packCategories,
        collisions,
        totalCount: packExpansions.length,
        warnings,
      });
    } catch (e) {
      console.error(`[Keyfire] Import from ${meta.label} failed:`, e);
      showNotification(`${meta.label} import failed`, 'info');
    }
  }, [assignments, applyExpansionImport, showNotification]);

  const handleExpansionImportResolve = useCallback((choice) => {
    if (!expansionImportPrompt) return;
    const { expansions: packExpansions, categories: packCategories, warnings } = expansionImportPrompt;
    setExpansionImportPrompt(null);
    if (choice === 'cancel') return;
    applyExpansionImport(packExpansions, packCategories, choice, warnings);
  }, [expansionImportPrompt, applyExpansionImport]);

  // ── Quick Action pack export/import (mirrors expansion pack flow) ──────
  // Pack envelope is `trigr_quick_action_pack: '1.0'`. Reuses the generic
  // exportProfile/importProfile Tauri commands (file dialog + JSON r/w).
  const handleExportQuickActions = useCallback(async (scope, scopeKey) => {
    const allActions = Object.entries(assignments)
      .filter(([k]) => k.startsWith('GLOBAL::QUICKACTION::'))
      .map(([k, v]) => ({
        id: k.slice('GLOBAL::QUICKACTION::'.length),
        type: v?.type,
        label: v?.label || '',
        data: v?.data || {},
      }));

    let scoped = allActions;
    if (scope === 'category') {
      scoped = scoped.filter(a => (a.data?.category || null) === (scopeKey || null));
    } else if (scope === 'single') {
      scoped = scoped.filter(a => a.id === scopeKey);
    }

    if (scoped.length === 0) {
      if (scope === 'category') showNotification(`No quick actions in "${scopeKey}" to export`, 'info');
      else showNotification('No quick actions to export', 'info');
      return;
    }

    const referencedCats = new Set(scoped.map(a => a.data?.category).filter(Boolean));
    const exportCategories = quickActionCategories
      .filter(c => referencedCats.has(c.name))
      .map(c => ({ name: c.name, colour: c.colour || null }));

    const name = scope === 'all'
      ? 'All Quick Actions'
      : scope === 'category'
      ? (scopeKey || 'Quick Actions')
      : (scoped[0]?.label || 'Quick Action');
    const payload = {
      trigr_quick_action_pack: '1.0',
      scope,
      name,
      exportedAt: new Date().toISOString(),
      quickActions: scoped,
      categories: exportCategories,
    };

    const slug = (name || 'quick-actions')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
    const filenameHint = `${slug || 'quick-actions'}-trigr-quick-actions.json`;
    const content = JSON.stringify(payload, null, 2);

    try {
      const result = await window.electronAPI?.exportProfile(filenameHint, content);
      if (result?.ok) {
        let msg;
        if (scope === 'single') {
          msg = `Quick action "${scoped[0]?.label || 'action'}" exported`;
        } else {
          const noun = scoped.length === 1 ? 'quick action' : 'quick actions';
          msg = `Exported ${scoped.length} ${noun}`;
        }
        showNotification(msg);
      } else if (result?.error) {
        showNotification(result.error, 'info');
      }
    } catch (e) {
      console.error('[Keyfire] Export quick actions failed:', e);
    }
  }, [assignments, quickActionCategories, showNotification]);

  // Universal sink for quick-action imports. Returns counts so adapters can
  // be added later (similar to applyExpansionImport per the future Import
  // From ▾ dropdown plan).
  const applyQuickActionImport = useCallback((packActions, packCategories, choice) => {
    const newAssignments = { ...assignments };
    let imported = 0;
    let skipped = 0;
    let overwritten = 0;

    for (const action of packActions) {
      if (!action || !action.type || !action.label) continue;
      // Collide on label within category (id is per-machine; not portable).
      const cat = action.data?.category || null;
      const existingEntry = Object.entries(newAssignments).find(([k, v]) =>
        k.startsWith('GLOBAL::QUICKACTION::') &&
        v?.label === action.label &&
        (v?.data?.category || null) === cat
      );

      if (existingEntry && choice === 'skip') { skipped++; continue; }

      const data = action.data && typeof action.data === 'object' ? { ...action.data } : {};
      if (existingEntry) {
        const [existingKey] = existingEntry;
        newAssignments[existingKey] = { type: action.type, label: action.label, data };
        overwritten++;
      } else {
        const newId = crypto.randomUUID();
        newAssignments[`GLOBAL::QUICKACTION::${newId}`] = { type: action.type, label: action.label, data };
        imported++;
      }
    }

    const existingCatNames = new Set(quickActionCategories.map(c => c.name));
    const newCategories = [...quickActionCategories];
    for (const cat of packCategories || []) {
      if (cat && cat.name && !existingCatNames.has(cat.name)) {
        newCategories.push({ name: cat.name, colour: cat.colour || null });
        existingCatNames.add(cat.name);
      }
    }

    setAssignments(newAssignments);
    setQuickActionCategories(newCategories);
    // Single save for both keys (see the QA category handlers above).
    window.electronAPI?.saveConfig({ assignments: newAssignments, quickActionCategories: newCategories });
    syncEngine(newAssignments, activeProfile);

    let msg;
    if (choice === 'skip') {
      const noun = imported === 1 ? 'quick action' : 'quick actions';
      msg = `Imported ${imported} new ${noun}`;
      if (skipped > 0) {
        const sNoun = skipped === 1 ? 'quick action' : 'quick actions';
        msg += `. ${skipped} ${sNoun} skipped (already existed).`;
      }
    } else {
      const total = imported + overwritten;
      const noun = total === 1 ? 'quick action' : 'quick actions';
      msg = `Imported ${total} ${noun}`;
      if (overwritten > 0) {
        const oNoun = overwritten === 1 ? 'quick action' : 'quick actions';
        msg += `. ${overwritten} existing ${oNoun} overwritten.`;
      }
    }
    showNotification(msg);
  }, [assignments, quickActionCategories, activeProfile, syncEngine, showNotification]);

  const handleImportQuickActions = useCallback(async () => {
    try {
      const result = await window.electronAPI?.importProfile();
      if (!result?.ok) {
        if (result?.error) showNotification(result.error, 'info');
        return;
      }
      let parsed;
      try {
        parsed = JSON.parse(result.content);
      } catch {
        showNotification('Could not parse quick action file', 'info');
        return;
      }
      if (!parsed || !parsed.trigr_quick_action_pack) {
        showNotification('Not a valid Keyfire quick action pack', 'info');
        return;
      }
      const packActions = Array.isArray(parsed.quickActions) ? parsed.quickActions : [];
      const packCategories = Array.isArray(parsed.categories) ? parsed.categories : [];
      if (packActions.length === 0) {
        showNotification('Quick action pack is empty', 'info');
        return;
      }

      // Collision = same label within the same category.
      const collisions = [];
      for (const action of packActions) {
        if (!action?.label) continue;
        const cat = action.data?.category || null;
        const hit = Object.entries(assignments).find(([k, v]) =>
          k.startsWith('GLOBAL::QUICKACTION::') &&
          v?.label === action.label &&
          (v?.data?.category || null) === cat
        );
        if (hit) collisions.push({ id: action.id, label: action.label });
      }

      if (collisions.length === 0) {
        applyQuickActionImport(packActions, packCategories, 'overwrite');
        return;
      }
      setQuickActionImportPrompt({
        actions: packActions,
        categories: packCategories,
        collisions,
        totalCount: packActions.length,
      });
    } catch (e) {
      console.error('[Keyfire] Import quick actions failed:', e);
      showNotification('Quick action import failed', 'info');
    }
  }, [assignments, applyQuickActionImport, showNotification]);

  const handleQuickActionImportResolve = useCallback((choice) => {
    if (!quickActionImportPrompt) return;
    const { actions: packActions, categories: packCategories } = quickActionImportPrompt;
    setQuickActionImportPrompt(null);
    if (choice === 'cancel') return;
    applyQuickActionImport(packActions, packCategories, choice);
  }, [quickActionImportPrompt, applyQuickActionImport]);

  // ── Expansion categories ──────────────────────────────────
  // Categories support one-level nesting via slash-delimited paths:
  // "Work" (top) or "Work/Client A" (child). Rename/move cascade to every
  // assignment whose data.category matches the old path OR starts with
  // "<old>/". Rust never reads data.category — the field is pure UI metadata.
  const handleAddCategory = useCallback((name, colour = null) => {
    if (!name || expansionCategories.some(c => c.name === name)) return;
    const newCategories = [...expansionCategories, { name, colour: colour || null }];
    setExpansionCategories(newCategories);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleReorderCategories = useCallback((newCategories) => {
    setExpansionCategories(newCategories);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleUpdateCategoryColour = useCallback((name, colour) => {
    const newCategories = expansionCategories.map(c => c.name === name ? { ...c, colour: colour || null } : c);
    setExpansionCategories(newCategories);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleRenameCategory = useCallback((oldName, newName) => {
    if (!newName || newName === oldName) return;
    if (expansionCategories.some(c => c.name === newName)) return; // duplicate guard
    const oldSlash = oldName + '/';
    const newSlash = newName + '/';
    const newCategories = expansionCategories.map(c => {
      if (c.name === oldName) return { ...c, name: newName };
      if (c.name.startsWith(oldSlash)) return { ...c, name: newName + '/' + c.name.slice(oldSlash.length) };
      return c;
    });
    const newAssignments = { ...assignments };
    for (const [k, v] of Object.entries(newAssignments)) {
      if (!k.startsWith('GLOBAL::EXPANSION::')) continue;
      const cat = v.data?.category;
      if (cat === oldName) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName } };
      } else if (typeof cat === 'string' && cat.startsWith(oldSlash)) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName + '/' + cat.slice(oldSlash.length) } };
      }
    }
    setExpansionCategories(newCategories);
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled, macrosEnabledOnStartup]);

  // mode: 'single' (default; leaves any children of a parent orphaned so the
  // caller must decide first), 'tree' (delete parent + all children), or
  // 'promote' (delete parent only; children become top-level, auto-suffix on
  // collision as "<child> (from <parent>)").
  const handleDeleteCategory = useCallback((name, mode = 'single') => {
    const slash = name + '/';
    const children = expansionCategories.filter(c => c.name.startsWith(slash));

    let newCategories;
    const newAssignments = { ...assignments };

    if (children.length === 0 || mode === 'tree') {
      const doomed = new Set([name, ...children.map(c => c.name)]);
      newCategories = expansionCategories.filter(c => !doomed.has(c.name));
      for (const [k, v] of Object.entries(newAssignments)) {
        if (!k.startsWith('GLOBAL::EXPANSION::')) continue;
        const cat = v.data?.category;
        if (typeof cat === 'string' && doomed.has(cat)) {
          newAssignments[k] = { ...v, data: { ...v.data, category: null } };
        }
      }
    } else {
      // Promote children to top-level. Collision map: old child path → new top-level name.
      const existingTop = new Set(expansionCategories.filter(c => !c.name.includes('/') && c.name !== name).map(c => c.name));
      const promoteMap = new Map();
      newCategories = expansionCategories
        .filter(c => c.name !== name)
        .map(c => {
          if (!c.name.startsWith(slash)) return c;
          const childBase = c.name.slice(slash.length);
          let promoted = childBase;
          if (existingTop.has(promoted)) promoted = `${childBase} (from ${name})`;
          // Rare double-collision: append counter
          let n = 2;
          while (existingTop.has(promoted)) promoted = `${childBase} (from ${name}) ${n++}`;
          existingTop.add(promoted);
          promoteMap.set(c.name, promoted);
          return { ...c, name: promoted };
        });
      for (const [k, v] of Object.entries(newAssignments)) {
        if (!k.startsWith('GLOBAL::EXPANSION::')) continue;
        const cat = v.data?.category;
        if (cat === name) {
          newAssignments[k] = { ...v, data: { ...v.data, category: null } };
        } else if (typeof cat === 'string' && promoteMap.has(cat)) {
          newAssignments[k] = { ...v, data: { ...v.data, category: promoteMap.get(cat) } };
        }
      }
    }
    setExpansionCategories(newCategories);
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled]);

  // Drop-target for the drag-and-drop "Uncategorised" row and any category
  // row on the sidebar. newCategory may be null (uncategorised) or a path.
  // Accepts a single trigger string or an array — a multi-selection drag from
  // the expansion list arrives here as an array in one save round-trip.
  const handleMoveExpansionToCategory = useCallback((triggerOrTriggers, newCategory) => {
    const triggers = Array.isArray(triggerOrTriggers) ? triggerOrTriggers : [triggerOrTriggers];
    if (triggers.length === 0) return;
    const next = newCategory || null;
    const newAssignments = { ...assignments };
    let changed = false;
    for (const trigger of triggers) {
      const key = `GLOBAL::EXPANSION::${trigger}`;
      const existing = newAssignments[key];
      if (!existing) continue;
      const current = existing.data?.category ?? null;
      if (current === next) continue;
      newAssignments[key] = { ...existing, data: { ...existing.data, category: next } };
      changed = true;
    }
    if (!changed) return;
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [assignments, expansionCategories, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled, macrosEnabledOnStartup]);

  // Move a category to a new location in the tree.
  // destination: 'top' promotes a child to top-level; otherwise a parent name to demote/move under.
  // Depth is capped at 1 — moving a top-level with children under another parent is rejected.
  const handleMoveCategoryTo = useCallback((name, destination) => {
    const isChild = name.includes('/');
    const baseName = isChild ? name.slice(name.lastIndexOf('/') + 1) : name;

    let newPath;
    if (destination === 'top') {
      if (!isChild) return;
      newPath = baseName;
      if (expansionCategories.some(c => c.name === newPath)) {
        const parentName = name.slice(0, name.lastIndexOf('/'));
        newPath = `${baseName} (from ${parentName})`;
        let n = 2;
        while (expansionCategories.some(c => c.name === newPath)) newPath = `${baseName} (from ${parentName}) ${n++}`;
      }
    } else {
      if (destination.includes('/')) return; // can't nest under a child
      // Guard depth cap: only childless top-levels can be demoted.
      if (!isChild) {
        const hasChildren = expansionCategories.some(c => c.name.startsWith(name + '/'));
        if (hasChildren) return;
      }
      if (!expansionCategories.some(c => c.name === destination)) return;
      newPath = `${destination}/${baseName}`;
      if (name === newPath) return;
      if (expansionCategories.some(c => c.name === newPath)) return;
    }

    const newCategories = expansionCategories.map(c => c.name === name ? { ...c, name: newPath } : c);
    const newAssignments = { ...assignments };
    for (const [k, v] of Object.entries(newAssignments)) {
      if (!k.startsWith('GLOBAL::EXPANSION::')) continue;
      if (v.data?.category === name) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newPath } };
      }
    }
    setExpansionCategories(newCategories);
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled]);

  // ── Autocorrect ───────────────────────────────────────────
  const autocorrections = useMemo(() => Object.entries(assignments)
    .filter(([k]) => k.startsWith('GLOBAL::AUTOCORRECT::'))
    .map(([k, v]) => ({
      typo: k.slice('GLOBAL::AUTOCORRECT::'.length),
      correction: v.data?.correction || '',
    })), [assignments]);

  // Unified settings patch: {enabled?, builtinTypos?, doubleCaps?, exceptions?}.
  // Syncs the engine and persists via shallow-merge saveConfig.
  const handleUpdateAutocorrectSettings = useCallback((patch) => {
    const next = {
      enabled: patch.enabled ?? autocorrectEnabled,
      builtinTypos: patch.builtinTypos ?? autocorrectBuiltinTypos,
      doubleCaps: patch.doubleCaps ?? autocorrectDoubleCaps,
      exceptions: patch.exceptions ?? autocorrectDoubleCapsExceptions,
      capsLockFix: patch.capsLockFix ?? autocorrectCapsLockFix,
      sentenceCaps: patch.sentenceCaps ?? autocorrectSentenceCaps,
      extendedTypos: patch.extendedTypos ?? autocorrectExtendedTypos,
      excludedApps: patch.excludedApps ?? autocorrectExcludedApps,
      disabledEntries: patch.disabledEntries ?? autocorrectDisabledEntries,
      days: patch.days ?? autocorrectDays,
      symbols: patch.symbols ?? autocorrectSymbols,
      emojis: patch.emojis ?? autocorrectEmojis,
    };
    // Normalize disabled entries: lowercase, dedupe, drop empties — mirrors
    // the Rust-side normalization in expansions::set_autocorrect_settings.
    next.disabledEntries = Array.from(new Set(
      (next.disabledEntries || [])
        .map(w => (w || '').toLowerCase().trim())
        .filter(Boolean)
    ));
    // Normalize excluded apps: lowercase, strip .exe, dedupe, drop empties —
    // mirrors the Rust-side normalization in expansions::set_autocorrect_settings.
    next.excludedApps = Array.from(new Set(
      (next.excludedApps || [])
        .map(a => (a || '').toLowerCase().replace(/\.exe$/, '').trim())
        .filter(Boolean)
    ));
    setAutocorrectEnabled(next.enabled);
    setAutocorrectBuiltinTypos(next.builtinTypos);
    setAutocorrectDoubleCaps(next.doubleCaps);
    setAutocorrectDoubleCapsExceptions(next.exceptions);
    setAutocorrectCapsLockFix(next.capsLockFix);
    setAutocorrectSentenceCaps(next.sentenceCaps);
    setAutocorrectExtendedTypos(next.extendedTypos);
    setAutocorrectDays(next.days);
    setAutocorrectSymbols(next.symbols);
    setAutocorrectEmojis(next.emojis);
    setAutocorrectExcludedApps(next.excludedApps);
    setAutocorrectDisabledEntries(next.disabledEntries);
    window.electronAPI?.updateAutocorrectSettings({
      enabled: next.enabled,
      builtinTypos: next.builtinTypos,
      extendedTypos: next.extendedTypos,
      days: next.days,
      symbols: next.symbols,
      emojis: next.emojis,
      doubleCaps: next.doubleCaps,
      doubleCapsExceptions: next.exceptions,
      capsLockFix: next.capsLockFix,
      sentenceCaps: next.sentenceCaps,
      excludedApps: next.excludedApps,
      disabledEntries: next.disabledEntries,
    });
    window.electronAPI?.saveConfig({
      autocorrectEnabled: next.enabled,
      autocorrectBuiltinTypos: next.builtinTypos,
      autocorrectDoubleCaps: next.doubleCaps,
      autocorrectDoubleCapsExceptions: next.exceptions,
      autocorrectCapsLockFix: next.capsLockFix,
      autocorrectSentenceCaps: next.sentenceCaps,
      autocorrectExtendedTypos: next.extendedTypos,
      autocorrectExcludedApps: next.excludedApps,
      autocorrectDisabledEntries: next.disabledEntries,
      autocorrectDays: next.days,
      autocorrectSymbols: next.symbols,
      autocorrectEmojis: next.emojis,
    });
  }, [autocorrectEnabled, autocorrectBuiltinTypos, autocorrectDoubleCaps, autocorrectDoubleCapsExceptions, autocorrectCapsLockFix, autocorrectSentenceCaps, autocorrectExtendedTypos, autocorrectExcludedApps, autocorrectDisabledEntries, autocorrectDays, autocorrectSymbols, autocorrectEmojis]);

  // Text-expansion excluded apps — separate list from autocorrect's.
  const handleUpdateExpansionExcludedApps = useCallback((apps) => {
    const next = Array.from(new Set(
      (apps || [])
        .map(a => (a || '').toLowerCase().replace(/\.exe$/, '').trim())
        .filter(Boolean)
    ));
    setExpansionExcludedApps(next);
    window.electronAPI?.updateExpansionExcludedApps(next);
    window.electronAPI?.saveConfig({ expansionExcludedApps: next });
  }, []);

  // Save one correct word with its full misspelling list. Storage is flat
  // (one GLOBAL::AUTOCORRECT::<typo> key per misspelling); typos dropped from
  // the list since the last save are deleted.
  const handleSaveAutocorrectGroup = useCallback((correction, typos, originalTypos = []) => {
    const newAssignments = { ...assignments };
    for (const t of originalTypos) {
      if (!typos.includes(t)) delete newAssignments[`GLOBAL::AUTOCORRECT::${t}`];
    }
    for (const t of typos) {
      newAssignments[`GLOBAL::AUTOCORRECT::${t}`] = {
        type: 'autocorrect',
        label: `Autocorrect: ${t}`,
        data: { correction },
      };
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Autocorrect "${correction}" saved (${typos.length} ${typos.length === 1 ? 'misspelling' : 'misspellings'})`);
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  const handleDeleteAutocorrectGroup = useCallback((correction, typos) => {
    const newAssignments = { ...assignments };
    for (const t of typos) delete newAssignments[`GLOBAL::AUTOCORRECT::${t}`];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Autocorrect "${correction}" deleted`, 'info');
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  // ── Autocorrect: learn-from-undo suggestions ─────────────────────────────
  // A word undone twice (and not muted or already handled) earns a banner in
  // the Autocorrect tab. Nothing is ever applied without a click.
  const acSuggestions = useMemo(() => {
    return Object.entries(autocorrectUndoCounts)
      .filter(([key, info]) => {
        if (!info || info.count < 2) return false;
        if (info.source === 'sentenceCaps') return false;
        if (autocorrectUndoMuted.includes(key)) return false;
        if (info.source === 'custom') return !!assignments[`GLOBAL::AUTOCORRECT::${key}`];
        if (info.source === 'doubleCaps' || info.source === 'capsLock') {
          return !autocorrectDoubleCapsExceptions.includes(key);
        }
        return !autocorrectDisabledEntries.includes(key);
      })
      .map(([key, info]) => ({ key, ...info }));
  }, [autocorrectUndoCounts, autocorrectUndoMuted, autocorrectDisabledEntries, autocorrectDoubleCapsExceptions, assignments]);

  const handleAcSuggestionResolve = useCallback((key, action) => {
    const info = autocorrectUndoCounts[key];
    if (action === 'stop' && info) {
      if (info.source === 'custom') {
        const k = `GLOBAL::AUTOCORRECT::${key}`;
        if (assignments[k]) {
          const newAssignments = { ...assignments };
          delete newAssignments[k];
          setAssignments(newAssignments);
          saveConfig(newAssignments, profiles, activeProfile);
        }
      } else if (info.source === 'doubleCaps' || info.source === 'capsLock') {
        handleUpdateAutocorrectSettings({ exceptions: [...autocorrectDoubleCapsExceptions, key] });
      } else {
        handleUpdateAutocorrectSettings({ disabledEntries: [...autocorrectDisabledEntries, key] });
      }
      showNotification(`Autocorrect will leave "${key}" alone`);
    }
    const nextCounts = { ...autocorrectUndoCounts };
    delete nextCounts[key];
    const nextMuted = autocorrectUndoMuted.includes(key) ? autocorrectUndoMuted : [...autocorrectUndoMuted, key];
    setAutocorrectUndoCounts(nextCounts);
    setAutocorrectUndoMuted(nextMuted);
    window.electronAPI?.saveConfig({ autocorrectUndoCounts: nextCounts, autocorrectUndoMuted: nextMuted });
  }, [autocorrectUndoCounts, autocorrectUndoMuted, autocorrectDisabledEntries, autocorrectDoubleCapsExceptions, assignments, profiles, activeProfile, saveConfig, showNotification, handleUpdateAutocorrectSettings]);

  // ── Autocorrect: CSV import/export ───────────────────────────────────────
  const handleExportAutocorrections = useCallback(async () => {
    const rows = Object.entries(assignments)
      .filter(([k]) => k.startsWith('GLOBAL::AUTOCORRECT::'))
      .map(([k, v]) => [k.slice('GLOBAL::AUTOCORRECT::'.length), v?.data?.correction || ''])
      .filter(([t, c]) => t && c)
      .sort((a, b) => a[1].localeCompare(b[1]) || a[0].localeCompare(b[0]));
    if (rows.length === 0) {
      showNotification('No corrections to export', 'info');
      return;
    }
    const content = 'typo,correction\n' + rows.map(([t, c]) => `${t},${c}`).join('\n') + '\n';
    try {
      const result = await window.electronAPI?.exportTextFile(
        'keyfire-corrections.csv', content, 'Export Corrections', 'CSV', ['csv']
      );
      if (result?.ok) {
        showNotification(`Exported ${rows.length} correction${rows.length === 1 ? '' : 's'}`);
      } else if (result?.error) {
        showNotification(result.error, 'info');
      }
    } catch (e) {
      console.error('[Keyfire] Export corrections failed:', e);
    }
  }, [assignments, showNotification]);

  const applyAcImport = useCallback((rows, choice) => {
    const newAssignments = { ...assignments };
    let imported = 0, overwritten = 0, skipped = 0;
    for (const { typo, correction } of rows) {
      const k = `GLOBAL::AUTOCORRECT::${typo}`;
      const existed = !!newAssignments[k];
      if (existed && choice === 'skip') { skipped++; continue; }
      if (existed) overwritten++; else imported++;
      newAssignments[k] = { type: 'autocorrect', label: `Autocorrect: ${typo}`, data: { correction } };
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    let msg = `Imported ${imported} correction${imported === 1 ? '' : 's'}`;
    if (overwritten) msg += `, updated ${overwritten}`;
    if (skipped) msg += `, skipped ${skipped}`;
    showNotification(msg);
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  const handleImportAutocorrections = useCallback(async () => {
    try {
      const result = await window.electronAPI?.importTextFile('Import Corrections', 'CSV', ['csv', 'txt']);
      if (!result?.ok) {
        if (result?.error) showNotification(result.error, 'info');
        return;
      }
      // One pair per line, comma- or tab-separated. Typos are single
      // lowercase words (mirrors what the engine buffer can ever match);
      // anything else is skipped rather than imported broken.
      const rows = [];
      const seen = new Set();
      for (const rawLine of String(result.content || '').split(/\r?\n/)) {
        const line = rawLine.trim();
        if (!line) continue;
        const sep = line.includes('\t') ? '\t' : ',';
        const idx = line.indexOf(sep);
        if (idx <= 0) continue;
        const typo = line.slice(0, idx).trim().toLowerCase().replace(/^"|"$/g, '');
        const correction = line.slice(idx + 1).trim().replace(/^"|"$/g, '');
        if (!typo || !correction || /\s/.test(typo)) continue;
        if (typo === 'typo' && correction.toLowerCase() === 'correction') continue; // header row
        if (seen.has(typo)) continue;
        seen.add(typo);
        rows.push({ typo, correction });
      }
      if (rows.length === 0) {
        showNotification('No corrections found in that file', 'info');
        return;
      }
      const collisions = rows
        .filter(r => {
          const existing = assignments[`GLOBAL::AUTOCORRECT::${r.typo}`];
          return existing && (existing.data?.correction || '') !== r.correction;
        })
        .map(r => r.typo);
      if (collisions.length > 0) {
        setAcImportPrompt({ rows, collisions, totalCount: rows.length });
      } else {
        applyAcImport(rows, 'overwrite');
      }
    } catch (e) {
      console.error('[Keyfire] Import corrections failed:', e);
    }
  }, [assignments, applyAcImport, showNotification]);

  const handleAcImportResolve = useCallback((choice) => {
    const prompt = acImportPrompt;
    setAcImportPrompt(null);
    if (!prompt || choice === 'cancel') return;
    applyAcImport(prompt.rows, choice);
  }, [acImportPrompt, applyAcImport]);

  // ── Profile settings (app-linking) ───────────────────────
  const handleUpdateProfileSettings = useCallback((profileName, updates) => {
    const merged = { ...(profileSettings[profileName] || {}), ...updates };
    // Drop null/undefined values
    Object.keys(merged).forEach(k => { if (merged[k] == null) delete merged[k]; });
    const next = { ...profileSettings };
    if (Object.keys(merged).length === 0) {
      delete next[profileName];
    } else {
      next[profileName] = merged;
    }
    setProfileSettings(next);
    window.electronAPI?.updateProfileSettings(next);
    // Save directly (not via saveConfig wrapper) so we include the new settings
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings: next, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [profileSettings, assignments, profiles, activeProfile, theme, expansionCategories, autocorrectEnabled]);

  // ── Profile reorder / duplicate / delete ─────────────────
  const handleReorderProfiles = useCallback((newProfiles) => {
    setProfiles(newProfiles);
    saveConfig(assignments, newProfiles, activeProfile);
  }, [assignments, activeProfile, saveConfig]);

  const handleDuplicateProfile = useCallback((name) => {
    let newName = `${name} (copy)`;
    let counter = 2;
    while (profiles.includes(newName)) newName = `${name} (copy ${counter++})`;
    // Copy all assignments for this profile
    const newAssignments = { ...assignments };
    const prefix = name + '::';
    for (const [k, v] of Object.entries(assignments)) {
      if (k.startsWith(prefix)) newAssignments[newName + '::' + k.slice(prefix.length)] = v;
    }
    const newProfiles = [...profiles, newName];
    setAssignments(newAssignments);
    setProfiles(newProfiles);
    setActiveProfile(newName);
    setSelectedKey(null);
    saveConfig(newAssignments, newProfiles, newName);
    showNotification(`Duplicated as "${newName}"`);
  }, [profiles, assignments, saveConfig, showNotification]);

  const handleExportProfile = useCallback(async (name) => {
    // Collect all assignments for this profile
    const prefix = name + '::';
    const profileAssignments = {};
    for (const [k, v] of Object.entries(assignments)) {
      if (k.startsWith(prefix)) profileAssignments[k] = v;
    }
    const payload = {
      trigr_profile: '1.0',
      name,
      assignments: profileAssignments,
      linkedApp: null,
    };
    const content = JSON.stringify(payload, null, 2);
    const filenameHint = `${name}-trigr-profile.json`;
    try {
      const result = await window.electronAPI?.exportProfile(filenameHint, content);
      if (result?.ok) {
        showNotification(`Profile "${name}" exported`);
      } else if (result?.error) {
        showNotification(result.error, 'info');
      }
    } catch (e) {
      console.error('[Keyfire] Export profile failed:', e);
    }
  }, [assignments, showNotification]);

  const handleImportProfile = useCallback(async () => {
    try {
      const result = await window.electronAPI?.importProfile();
      if (!result?.ok) {
        if (result?.error) showNotification(result.error, 'info');
        return;
      }
      let parsed;
      try {
        parsed = JSON.parse(result.content);
      } catch {
        showNotification('Could not parse profile file', 'info');
        return;
      }
      if (!parsed.trigr_profile) {
        showNotification('Not a valid Keyfire profile export', 'info');
        return;
      }
      const importName = parsed.name || 'Imported';
      if (profiles.includes(importName)) {
        // Name collision — show Copy/Overwrite prompt
        setImportPrompt({ name: importName, assignments: parsed.assignments || {} });
        return;
      }
      // No collision — import directly
      const importedAssignments = {};
      const originalName = parsed.name || '';
      for (const [k, v] of Object.entries(parsed.assignments || {})) {
        const parts = k.split('::');
        if (parts[0] === originalName) parts[0] = importName;
        importedAssignments[parts.join('::')] = v;
      }
      const assignmentCount = Object.keys(importedAssignments).filter(k => !isLibraryKey(k)).length;
      const newAssignments = { ...assignments, ...importedAssignments };
      const newProfiles = [...profiles, importName];
      setAssignments(newAssignments);
      setProfiles(newProfiles);
      setActiveProfile(importName);
      setSelectedKey(null);
      saveConfig(newAssignments, newProfiles, importName);
      showNotification(`Profile "${importName}" imported — ${assignmentCount} assignment${assignmentCount !== 1 ? 's' : ''} loaded`);
    } catch (e) {
      console.error('[Keyfire] Import profile failed:', e);
      showNotification('Profile import failed', 'info');
    }
  }, [profiles, assignments, saveConfig, showNotification]);

  const handleImportProfileResolve = useCallback((choice) => {
    if (!importPrompt) return;
    const { name: importName, assignments: importedRaw } = importPrompt;
    setImportPrompt(null);

    if (choice === 'copy') {
      // Deduplicate name
      let newName = importName;
      let counter = 1;
      while (profiles.includes(newName)) {
        newName = `${importName} (${counter++})`;
      }
      const importedAssignments = {};
      for (const [k, v] of Object.entries(importedRaw)) {
        const parts = k.split('::');
        if (parts[0] === importName) parts[0] = newName;
        importedAssignments[parts.join('::')] = v;
      }
      const assignmentCount = Object.keys(importedAssignments).filter(k => !isLibraryKey(k)).length;
      const newAssignments = { ...assignments, ...importedAssignments };
      const newProfiles = [...profiles, newName];
      setAssignments(newAssignments);
      setProfiles(newProfiles);
      setActiveProfile(newName);
      setSelectedKey(null);
      saveConfig(newAssignments, newProfiles, newName);
      showNotification(`Profile "${newName}" imported — ${assignmentCount} assignment${assignmentCount !== 1 ? 's' : ''} loaded`);
    } else if (choice === 'overwrite') {
      // Remove existing assignments for this profile, then write imported ones
      const prefix = importName + '::';
      const newAssignments = {};
      for (const [k, v] of Object.entries(assignments)) {
        if (!k.startsWith(prefix)) newAssignments[k] = v;
      }
      for (const [k, v] of Object.entries(importedRaw)) {
        const parts = k.split('::');
        if (parts[0] === importName) parts[0] = importName; // no-op but consistent
        newAssignments[parts.join('::')] = v;
      }
      const assignmentCount = Object.keys(importedRaw).filter(k => !isLibraryKey(k)).length;
      setAssignments(newAssignments);
      setActiveProfile(importName);
      setSelectedKey(null);
      saveConfig(newAssignments, profiles, importName);
      showNotification(`Profile "${importName}" updated — ${assignmentCount} assignment${assignmentCount !== 1 ? 's' : ''} replaced`);
    }
  }, [importPrompt, profiles, assignments, saveConfig, showNotification]);

  const handleDeleteProfile = useCallback((name) => {
    if (name === 'Default') return;
    const newProfiles = profiles.filter(p => p !== name);
    // Remove all assignments for the deleted profile
    const newAssignments = {};
    const prefix = name + '::';
    for (const [k, v] of Object.entries(assignments)) {
      if (!k.startsWith(prefix)) newAssignments[k] = v;
    }
    // Remove profile settings entry
    const newProfileSettings = { ...profileSettings };
    delete newProfileSettings[name];
    // Drop the profile's radial layout too — it was left as an orphan
    // (including up to ~1MB of custom icon data) and could never be reached.
    const newRadialMap = { ...radialItemsMap };
    const hadRadial = Object.prototype.hasOwnProperty.call(newRadialMap, name);
    delete newRadialMap[name];
    // Same for the extra per-device layouts.
    const layoutsHadRadial = radialLayouts.some(l => l.itemsByProfile && Object.prototype.hasOwnProperty.call(l.itemsByProfile, name));
    const newRadialLayouts = layoutsHadRadial
      ? radialLayouts.map(l => {
          if (!l.itemsByProfile || !Object.prototype.hasOwnProperty.call(l.itemsByProfile, name)) return l;
          const m = { ...l.itemsByProfile };
          delete m[name];
          return { ...l, itemsByProfile: m };
        })
      : radialLayouts;
    const newActive = activeProfile === name ? 'Default' : activeProfile;
    setAssignments(newAssignments);
    setProfiles(newProfiles);
    setActiveProfile(newActive);
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setProfileSettings(newProfileSettings);
    if (hadRadial) setRadialItemsMap(newRadialMap);
    if (layoutsHadRadial) setRadialLayouts(newRadialLayouts);
    // If the deleted profile was the active global profile, fall back to Default
    const newGlobal = activeGlobalProfile === name ? 'Default' : activeGlobalProfile;
    if (newGlobal !== activeGlobalProfile) {
      setActiveGlobalProfile(newGlobal);
      window.electronAPI?.setActiveGlobalProfile(newGlobal);
    }
    window.electronAPI?.updateProfileSettings(newProfileSettings);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newActive, activeGlobalProfile: newGlobal, profileSettings: newProfileSettings, ...(hadRadial ? { radialMenuItemsByProfile: newRadialMap } : {}), ...(layoutsHadRadial ? { radialLayouts: newRadialLayouts } : {}), theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
    syncEngine(newAssignments, newActive);
    showNotification(`Profile "${name}" deleted`, 'info');
  }, [profiles, assignments, profileSettings, activeProfile, activeGlobalProfile, radialItemsMap, radialLayouts, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, syncEngine, showNotification]);

  const handleSetActiveGlobalProfile = useCallback((name) => {
    setActiveGlobalProfile(name);
    window.electronAPI?.setActiveGlobalProfile(name);
    // Save to config
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, activeGlobalProfile: name, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
    // If no app-specific profile is currently overriding, switch the active editing profile too
    const currentIsAppSpecific = !!profileSettings[activeProfile]?.linkedApp;
    if (!currentIsAppSpecific) {
      setActiveProfile(name);
      setSelectedKey(null);
      syncEngine(assignments, name);
    }
    showNotification(`Global profile: ${name}`, 'info');
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, syncEngine, showNotification]);

  // ── Copy / Move assignment to another profile ─────────────
  // Copy/Move must carry every variant that exists (see
  // ASSIGNMENT_VARIANT_SUFFIXES at module scope), in any combination.
  // Bare character keys are only mappable in app-linked profiles (the engine
  // refuses them in static profiles via is_static_bare_allowed, so the copy
  // would show on the keyboard but never fire). Block the cross-profile
  // copy/move up front instead of creating a dead assignment.
  const bareBlockedInProfile = useCallback((targetProfile, combo, keyId) => {
    if (combo !== 'BARE' || !keyId || keyId.startsWith('MOUSE_')) return false;
    if (profileSettings[targetProfile]?.linkedApp) return false;
    if (STATIC_BARE_ALLOWED.has(keyId)) return false;
    showNotification(`${friendlyKeyName(keyId)} can't be a bare trigger in "${targetProfile}". Character keys only work bare in profiles linked to an app.`, 'error');
    return true;
  }, [profileSettings, showNotification]);

  const handleCopyToProfile = useCallback((targetProfile, combo, keyId) => {
    const srcCombo = combo || currentCombo;
    const srcKey   = keyId || selectedKey;
    if (bareBlockedInProfile(targetProfile, srcCombo, srcKey)) return;
    const oldKey = makeAssignmentKey(activeProfile, srcCombo, srcKey);
    const newKey = makeAssignmentKey(targetProfile, srcCombo, srcKey);
    const newAssignments = { ...assignments };
    let carried = 0;
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      if (assignments[oldKey + suffix]) {
        newAssignments[newKey + suffix] = assignments[oldKey + suffix];
        carried++;
      }
    }
    if (carried === 0) return;
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Copied to "${targetProfile}" profile`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey, bareBlockedInProfile]);

  const handleMoveToProfile = useCallback((targetProfile, combo, keyId) => {
    const srcCombo = combo || currentCombo;
    const srcKey   = keyId || selectedKey;
    if (bareBlockedInProfile(targetProfile, srcCombo, srcKey)) return;
    const oldKey = makeAssignmentKey(activeProfile, srcCombo, srcKey);
    const newKey = makeAssignmentKey(targetProfile, srcCombo, srcKey);
    const newAssignments = { ...assignments };
    let carried = 0;
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      if (newAssignments[oldKey + suffix]) {
        newAssignments[newKey + suffix] = newAssignments[oldKey + suffix];
        delete newAssignments[oldKey + suffix];
        carried++;
      }
    }
    if (carried === 0) return;
    setAssignments(newAssignments);
    setSelectedKey(null);
    setSelectedLibraryId(prev => (prev === srcKey ? null : prev));
    saveConfig(newAssignments, profiles, activeProfile);
    // Radial wedges reference assignments by storage key — re-point any
    // wedge that was bound to the moved key (Unassign/Bind/Reassign already
    // do this; Move-to was the one rewrite path that didn't).
    remapRadialStorageKeys(variantKeyMap(oldKey, newKey));
    showNotification(`Moved to "${targetProfile}" profile`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, remapRadialStorageKeys, showNotification, makeAssignmentKey]);

  // ── Reassign hotkey ───────────────────────────────────────
  // Moves ALL trigger variants (single + double + hold) to the new trigger.
  // Anything already living at the target swaps back to the old trigger,
  // variant by variant, so nothing is lost or orphaned.
  // moveAssignment is the parameterised core — handleReassign feeds it the
  // current selection; drag-and-drop feeds it the dragged row's trigger.
  const moveAssignment = useCallback((srcCombo, srcKeyId, newCombo, newKeyId) => {
    const oldKey = makeAssignmentKey(activeProfile, srcCombo, srcKeyId);
    const newKey = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const newAssignments = { ...assignments };
    const swapped = ASSIGNMENT_VARIANT_SUFFIXES.some(s => assignments[newKey + s]);

    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      const moving = assignments[oldKey + suffix];
      const atTarget = assignments[newKey + suffix];
      if (atTarget !== undefined) {
        newAssignments[oldKey + suffix] = atTarget;
      } else {
        delete newAssignments[oldKey + suffix];
      }
      if (moving !== undefined) {
        newAssignments[newKey + suffix] = moving;
      } else {
        delete newAssignments[newKey + suffix];
      }
    }

    setAssignments(newAssignments);
    selectTrigger(newCombo, newKeyId);
    saveConfig(newAssignments, profiles, activeProfile);
    // Radial wedges referencing either trigger must follow the rewrite —
    // on a swap both directions move.
    const radialRemap = variantKeyMap(oldKey, newKey);
    if (swapped) Object.assign(radialRemap, variantKeyMap(newKey, oldKey));
    remapRadialStorageKeys(radialRemap);
    showNotification(swapped ? 'Hotkeys swapped' : 'Hotkey reassigned');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeAssignmentKey, selectTrigger, remapRadialStorageKeys]);

  const handleReassign = useCallback((newCombo, newKeyId) => {
    moveAssignment(currentCombo, selectedKey, newCombo, newKeyId);
  }, [moveAssignment, currentCombo, selectedKey]);

  // ── Duplicate assignment to a new hotkey ─────────────────
  // Copies every trigger variant that exists (single / double / hold) —
  // double-only and hold-only items duplicate too.
  const handleDuplicateAssignment = useCallback((newCombo, newKeyId) => {
    const oldKey = makeAssignmentKey(activeProfile, currentCombo, selectedKey);
    if (!ASSIGNMENT_VARIANT_SUFFIXES.some(s => assignments[oldKey + s])) return;
    const newKey = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const newAssignments = { ...assignments };
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      const existing = assignments[oldKey + suffix];
      if (!existing) continue;
      newAssignments[newKey + suffix] = {
        ...existing,
        label: (existing.label || '') + ' (copy)',
        data: JSON.parse(JSON.stringify(existing.data || {})),
      };
    }
    setAssignments(newAssignments);
    const newMods = newCombo === 'BARE' ? ['BARE'] : (newCombo ? newCombo.split('+').filter(Boolean) : []);
    setActiveModifiers(newMods);
    setSelectedKey(newKeyId);
    setActiveView(newKeyId.startsWith('MOUSE_') ? 'mouse' : 'keyboard');
    saveConfig(newAssignments, profiles, activeProfile);
    const keyLabel = friendlyKeyName(newKeyId);
    const comboLabel = newCombo === 'BARE' ? keyLabel : `${newCombo}+${keyLabel}`;
    showNotification(`Duplicated to ${comboLabel}`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Unassigned library ─────────────────────────────────────
  // Entries live at "{Profile}::UNASSIGNED::{uuid}" (+ ::double / ::hold),
  // in the same assignments map as everything else. See the selectedLibraryId
  // declaration for the engine-reachability invariant. The literal segment
  // must stay "UNASSIGNED" — never a real combo string and never "BARE".
  const makeLibraryKey = useCallback((profile, id) => {
    return `${profile}::UNASSIGNED::${id}`;
  }, []);

  const getLibraryEntry = useCallback((id, suffix = '') => {
    if (!id) return null;
    return assignments[makeLibraryKey(activeProfile, id) + suffix] || null;
  }, [assignments, activeProfile, makeLibraryKey]);

  // Unassign: free the trigger, keep the action. Moves ALL press-mode
  // variants under a fresh library uuid, then selects the new entry so the
  // editor stays open on the action the user just unassigned.
  const handleUnassignKey = useCallback((combo, keyId) => {
    const oldKey = makeAssignmentKey(activeProfile, combo, keyId);
    const id = crypto.randomUUID();
    const newKey = makeLibraryKey(activeProfile, id);
    const newAssignments = { ...assignments };
    if (moveVariantsInMap(newAssignments, oldKey, newKey) === 0) return;
    setAssignments(newAssignments);
    setSelectedKey(null);
    setSelectedLibraryId(id);
    saveConfig(newAssignments, profiles, activeProfile);
    remapRadialStorageKeys(variantKeyMap(oldKey, newKey));
    showNotification('Moved to Unassigned, at the top of the sidebar');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeAssignmentKey, makeLibraryKey, remapRadialStorageKeys]);
  // Move the saved action at one press mode (single / double / hold) to
  // another press mode on the SAME trigger — e.g. a hold action the user now
  // wants on double press. An occupied destination is swapped, never
  // overwritten: both records survive. baseKey is either a real trigger key or
  // an Unassigned-library key; only the suffix changes. Radial wedges reference
  // assignments by full storage key, so the remap keeps any segment pointing
  // at either record alive after the move.
  const movePressVariant = useCallback((baseKey, fromMode, toMode) => {
    if (fromMode === toMode) return false;
    const fromKey = baseKey + PRESS_MODE_SUFFIX[fromMode];
    const toKey = baseKey + PRESS_MODE_SUFFIX[toMode];
    const moving = assignments[fromKey];
    if (!moving) return false;
    const displaced = assignments[toKey] || null;
    const newAssignments = { ...assignments, [toKey]: moving };
    if (displaced) newAssignments[fromKey] = displaced;
    else delete newAssignments[fromKey];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    remapRadialStorageKeys(displaced ? { [fromKey]: toKey, [toKey]: fromKey } : { [fromKey]: toKey });
    showNotification(displaced
      ? `Swapped the ${PRESS_MODE_LABEL[fromMode]} and ${PRESS_MODE_LABEL[toMode]} actions`
      : `Moved to ${PRESS_MODE_LABEL[toMode]}`);
    return true;
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, remapRadialStorageKeys]);
  const handleMovePressVariant = useCallback((keyId, fromMode, toMode) => {
    return movePressVariant(makeAssignmentKey(activeProfile, currentCombo, keyId), fromMode, toMode);
  }, [movePressVariant, makeAssignmentKey, activeProfile, currentCombo]);
  const handleMoveLibraryVariant = useCallback((id, fromMode, toMode) => {
    return movePressVariant(makeLibraryKey(activeProfile, id), fromMode, toMode);
  }, [movePressVariant, makeLibraryKey, activeProfile]);

  // Save / clear one press-mode variant of a library entry. MacroPanel's
  // onAssign/onAssignDouble/onAssignHold (and clear counterparts) are routed
  // here with the matching suffix when a library entry is selected.
  const handleAssignLibraryVariant = useCallback((id, macro, suffix = '') => {
    const key = makeLibraryKey(activeProfile, id) + suffix;
    const oldLabel = assignments[key]?.label || '';
    const newAssignments = { ...assignments, [key]: macro };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    if (suffix === '') propagateLabelToRadialItems(key, oldLabel, macro?.label || '');
    showNotification('Saved to Unassigned');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeLibraryKey, propagateLabelToRadialItems]);

  const handleClearLibraryVariant = useCallback((id, suffix = '') => {
    const key = makeLibraryKey(activeProfile, id) + suffix;
    if (!assignments[key]) return;
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    // If nothing is left under this uuid, drop the selection too.
    if (!ASSIGNMENT_VARIANT_SUFFIXES.some(s => newAssignments[makeLibraryKey(activeProfile, id) + s])) {
      setSelectedLibraryId(prev => (prev === id ? null : prev));
    }
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Cleared', 'info');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeLibraryKey]);

  const handleDeleteLibraryEntry = useCallback((id) => {
    const base = makeLibraryKey(activeProfile, id);
    const newAssignments = { ...assignments };
    let removed = 0;
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      if (newAssignments[base + suffix]) { delete newAssignments[base + suffix]; removed++; }
    }
    if (removed === 0) return;
    setAssignments(newAssignments);
    setSelectedLibraryId(prev => (prev === id ? null : prev));
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Deleted from Unassigned', 'info');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeLibraryKey]);

  // Bind a library entry to a real trigger. If the target already holds an
  // action, the displaced action moves INTO Unassigned — this feature never
  // destroys an action without an explicit confirmed Delete.
  const handleBindLibrary = useCallback((id, newCombo, newKeyId) => {
    const libKey = makeLibraryKey(activeProfile, id);
    const targetKey = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const newAssignments = { ...assignments };
    const displacedKey = displaceToLibraryInMap(newAssignments, targetKey, activeProfile);
    if (moveVariantsInMap(newAssignments, libKey, targetKey) === 0) return;
    setAssignments(newAssignments);
    selectTrigger(newCombo, newKeyId);
    saveConfig(newAssignments, profiles, activeProfile);
    const radialRemap = variantKeyMap(libKey, targetKey);
    if (displacedKey) Object.assign(radialRemap, variantKeyMap(targetKey, displacedKey));
    remapRadialStorageKeys(radialRemap);
    const comboLabel = triggerLabel(newCombo, newKeyId);
    showNotification(displacedKey
      ? `Bound to ${comboLabel}. The key's previous action moved to Unassigned.`
      : `Bound to ${comboLabel}`);
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeAssignmentKey, makeLibraryKey, selectTrigger, remapRadialStorageKeys]);

  // Duplicate a library entry onto a trigger, keeping the library original.
  // Like handleBindLibrary, a displaced action moves into Unassigned.
  const handleDuplicateLibraryToKey = useCallback((id, newCombo, newKeyId) => {
    const libKey = makeLibraryKey(activeProfile, id);
    if (!ASSIGNMENT_VARIANT_SUFFIXES.some(s => assignments[libKey + s])) return;
    const targetKey = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const newAssignments = { ...assignments };
    const displacedKey = displaceToLibraryInMap(newAssignments, targetKey, activeProfile);
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      const existing = newAssignments[libKey + suffix];
      if (!existing) continue;
      newAssignments[targetKey + suffix] = {
        ...existing,
        data: JSON.parse(JSON.stringify(existing.data || {})),
      };
    }
    setAssignments(newAssignments);
    selectTrigger(newCombo, newKeyId);
    saveConfig(newAssignments, profiles, activeProfile);
    if (displacedKey) remapRadialStorageKeys(variantKeyMap(targetKey, displacedKey));
    showNotification(`Duplicated to ${triggerLabel(newCombo, newKeyId)}`);
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeAssignmentKey, makeLibraryKey, selectTrigger, remapRadialStorageKeys]);

  // Sidebar context menu → Duplicate on an unassigned entry: copy in place.
  const handleDuplicateLibraryInPlace = useCallback((id) => {
    const srcKey = makeLibraryKey(activeProfile, id);
    if (!ASSIGNMENT_VARIANT_SUFFIXES.some(s => assignments[srcKey + s])) return;
    const newId = crypto.randomUUID();
    const dstKey = makeLibraryKey(activeProfile, newId);
    const newAssignments = { ...assignments };
    for (const suffix of ASSIGNMENT_VARIANT_SUFFIXES) {
      const existing = assignments[srcKey + suffix];
      if (!existing) continue;
      newAssignments[dstKey + suffix] = {
        ...existing,
        label: (existing.label || '') + ' (copy)',
        data: JSON.parse(JSON.stringify(existing.data || {})),
      };
    }
    setAssignments(newAssignments);
    setSelectedLibraryId(newId);
    setSelectedKey(null);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Duplicated in Unassigned');
  }, [assignments, activeProfile, profiles, saveConfig, showNotification, makeLibraryKey]);

  const handleSelectLibraryEntry = useCallback((id) => {
    setSelectedLibraryId(id);
    setSelectedKey(null);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
  }, []);

  // "New Action" — select a fresh uuid with no saved entry; the entry is
  // created on first Save from the editor.
  const handleNewLibraryAction = useCallback(() => {
    handleSelectLibraryEntry(crypto.randomUUID());
  }, [handleSelectLibraryEntry]);

  // Sidebar context menu → "Bind to key…": select the entry, then signal
  // MacroPanel to open its bind-capture overlay.
  const handleBindFromContext = useCallback((id) => {
    handleSelectLibraryEntry(id);
    setBindOverlaySignal(s => s + 1);
  }, [handleSelectLibraryEntry]);

  // ── View switching (keyboard ↔ mouse, within Mapping area) ─
  const handleSetView = useCallback((view) => {
    setActiveView(view);
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setSelectedRadialSegment(null);
    setSelectedRadialChild(null);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
  }, []);

  // ── Hotkey recording ──────────────────────────────────────
  const handleStartRecord = useCallback(() => {
    setIsRecording(true);
    window.electronAPI?.startHotkeyRecording();
  }, []);

  const handleStopRecord = useCallback(() => {
    setIsRecording(false);
    window.electronAPI?.stopHotkeyRecording();
  }, []);

  // ── List view toggle ─────────────────────────────────────────
  const wasInKeyboardModeRef = useRef(false);

  const handleToggleListView = useCallback(() => {
    wasInKeyboardModeRef.current = false; // manual toggle overrides auto-restore
    setListViewActive(prev => {
      const next = !prev;
      try { localStorage.setItem('trigr_list_view', String(next)); } catch {}
      return next;
    });
  }, []);

  // ── New Shortcut button — wipe selection state so the user gets a clean slate
  // for creating a new hotkey from scratch. Cheaper than asking the user to
  // manually deselect each modifier + key. Per Ailin round-1 feedback.
  // newTriggerHint is the "next step" prompt — pulses the Record button until
  // the user takes any forward action (pick modifier, click key, start record).
  const [newTriggerHint, setNewTriggerHint] = useState(false);
  const handleNewShortcut = useCallback(() => {
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setActiveModifiers([]);
    setSidebarComboFilter(null);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
    setNewTriggerHint(true);
  }, []);

  // Clear the New Trigger hint as soon as the user advances the flow.
  useEffect(() => {
    if (!newTriggerHint) return;
    if (selectedKey || isRecording || activeModifiers.length > 0) {
      setNewTriggerHint(false);
    }
  }, [newTriggerHint, selectedKey, isRecording, activeModifiers]);

  // Also clear on any pointer-down anywhere in the document — covers clicks on
  // unrelated UI (sidebar tabs, top nav, canvas blank space) so the pulse
  // doesn't hang around once the user's attention has moved on. Skip clicks on
  // the New Trigger button itself so the prompt doesn't kill itself on arrival.
  useEffect(() => {
    if (!newTriggerHint) return;
    function onDown(e) {
      if (e.target.closest?.('.new-shortcut-btn')) return;
      setNewTriggerHint(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [newTriggerHint]);

  // ── Narrow-window tracker (< 1200px) — controls right-panel auto-hide ────
  // When narrow, the MacroPanel is hidden unless there's an active selection,
  // freeing horizontal space for the keyboard/mouse/radial view. As soon as
  // the user clicks a key (or radial segment), MacroPanel slides in.
  const [isNarrow, setIsNarrow] = useState(() => window.innerWidth < 1200);
  useEffect(() => {
    const onResize = () => setIsNarrow(window.innerWidth < 1200);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // ── Auto list view below ~900px with state memory ───────────
  // Threshold chosen so the modifier-bar pills (label + 4 mods + Bare Keys)
  // never get visibly cramped on the primary row. Record + New Shortcut
  // already wrap to a second row via CSS flex-wrap before this point.
  useEffect(() => {
    const BREAKPOINT = 900;
    let lastNarrow = window.innerWidth < BREAKPOINT;

    function onResize() {
      const narrow = window.innerWidth < BREAKPOINT;
      if (narrow === lastNarrow) return;
      lastNarrow = narrow;

      if (narrow) {
        // Going narrow — auto-switch to list view if currently in keyboard mode
        setListViewActive(prev => {
          if (prev) return prev; // already in list view
          wasInKeyboardModeRef.current = true;
          return true;
        });
      } else {
        // Going wide — restore keyboard mode if auto-switched
        if (wasInKeyboardModeRef.current) {
          wasInKeyboardModeRef.current = false;
          setListViewActive(false);
        }
      }
    }

    window.addEventListener('resize', onResize);
    // Check on mount in case window is already narrow
    onResize();
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // ── Top-level area switching (Mapping ↔ Text Expansions) ──
  // Optional `view` param lets callers (e.g. the onboarding tour) jump to a
  // specific sub-view within the area in one go (e.g. mapping + radial).
  const handleSetArea = useCallback((area, view) => {
    setActiveArea(area);
    if (view) setActiveView(view);
    if (area !== 'mapping') {
      setSelectedKey(null);
      setSelectedLibraryId(null);
      setDraftAssignment(null);
      setDraftDoubleAssignment(null);
    }
    // Clear stale sub-panel editing flags. The child components remount when
    // the user navigates back and re-push their actual editing state, so we
    // just need to drop stale `true` values from a previous visit.
    if (area !== 'expansions') setExpansionEditing(false);
    if (area !== 'templates') setQuickActionEditing(false);
  }, []);

  // ── Select assignment from sidebar ────────────────────────
  // Modifier selection is intentionally NOT changed here — the modifier bar
  // buttons are the only way to change which layer is active.  Clicking a
  // sidebar item should only focus that key/view without disturbing the
  // current modifier state.
  const handleSelectAssignment = useCallback((keyId, combo) => {
    // Activate the modifier layer that matches this assignment so that
    // getKeyAssignment() (which guards on activeModifiers.length > 0) can look
    // up the correct entry and MacroPanel receives the right assignment object.
    if (combo === 'BARE') {
      setActiveModifiers(['BARE']);
    } else if (combo) {
      // combo is already sorted by comboString(), so splitting and re-sorting is safe
      setActiveModifiers(combo.split('+'));
    }
    // User is navigating to an existing assignment — discard any pending
    // duplicate draft so the editor shows the clicked shortcut's data.
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
    setSelectedKey(keyId);
    setSelectedLibraryId(null);
    setSelectedRadialSegment(null);
    setSelectedRadialChild(null);
    // Stay on radial view when in radial mode; otherwise switch to keyboard/mouse
    if (activeView !== 'radial') {
      setActiveView(keyId.startsWith('MOUSE_') ? 'mouse' : 'keyboard');
    }
  }, [activeView]);

  // Sidebar combo-tab clicks update the sidebar's display filter only.
  // Modifier state is left untouched so clicking a tab never clears the
  // modifier layer the user selected via the keyboard modifier bar.
  const handleSelectCombo = useCallback((comboStr) => {
    setSelectedKey(null);
    setDraftAssignment(null);
    setDraftDoubleAssignment(null);
    // Sync the keyboard modifier layer + sidebar combo filter when the user
    // picks a tab from the assignment-list filter strip. "All" clears both.
    if (!comboStr || comboStr === 'All') {
      setActiveModifiers([]);
      setSidebarComboFilter(null);
    } else if (comboStr === 'BARE') {
      setActiveModifiers(['BARE']);
      setSidebarComboFilter('BARE');
    } else {
      setActiveModifiers(comboStr.split('+'));
      setSidebarComboFilter(comboStr);
    }
  }, []);

  // ── Settings handlers ─────────────────────────────────────
  const handleUpdateGlobalSettings = useCallback((patch) => {
    const next = {
      globalInputMethod:  patch.globalInputMethod  ?? globalInputMethod,
      macroSpeed:         patch.macroSpeed         ?? macroSpeed,
      keystrokeDelay:     patch.keystrokeDelay     ?? keystrokeDelay,
      macroTriggerDelay:  patch.macroTriggerDelay  ?? macroTriggerDelay,
      doubleTapWindow:    patch.doubleTapWindow     ?? doubleTapWindow,
      holdThresholdMs:    patch.holdThresholdMs    ?? holdThresholdMs,
      fireOnPress:        patch.fireOnPress        ?? fireOnPress,
      defaultDateFormat:  patch.defaultDateFormat  ?? defaultDateFormat,
    };
    setGlobalInputMethod(next.globalInputMethod);
    setMacroSpeed(next.macroSpeed);
    setKeystrokeDelay(next.keystrokeDelay);
    setMacroTriggerDelay(next.macroTriggerDelay);
    setDoubleTapWindow(next.doubleTapWindow);
    setHoldThresholdMs(next.holdThresholdMs);
    setFireOnPress(next.fireOnPress);
    setDefaultDateFormat(next.defaultDateFormat);
    window.electronAPI?.updateGlobalSettings(next);
    window.electronAPI?.saveConfig(next);
  }, [globalInputMethod, macroSpeed, keystrokeDelay, macroTriggerDelay, doubleTapWindow, holdThresholdMs, fireOnPress, defaultDateFormat]);

  // ── Global pause toggle ───────────────────────────────────
  const handleSetPauseKey = useCallback(async (combo) => {
    setGlobalPauseToggleKey(combo);
    await window.electronAPI?.setPauseHotkey(combo);
    window.electronAPI?.saveConfig({ globalPauseToggleKey: combo });
  }, []);

  const handleClearPauseKey = useCallback(() => {
    setGlobalPauseToggleKey(null);
    window.electronAPI?.clearPauseHotkey();
    window.electronAPI?.saveConfig({ globalPauseToggleKey: null });
  }, []);

  // ── Clipboard paste hotkey ────────────────────────
  const handleSetClipboardPasteKey = useCallback(async (combo) => {
    setClipboardPasteHotkey(combo);
    await window.electronAPI?.setClipboardPasteHotkey(combo);
    window.electronAPI?.saveConfig({ clipboardPasteHotkey: combo });
  }, []);

  const handleClearClipboardPasteKey = useCallback(() => {
    setClipboardPasteHotkey('');
    window.electronAPI?.clearClipboardPasteHotkey();
    window.electronAPI?.saveConfig({ clipboardPasteHotkey: null });
  }, []);

  // ── Voice enabled toggle ────────────────────────
  const handleToggleVoiceEnabled = useCallback((val) => {
    setVoiceEnabled(val);
    if (val && voiceHotkey) {
      window.electronAPI?.setVoiceHotkey(voiceHotkey);
    } else {
      window.electronAPI?.clearVoiceHotkey();
    }
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, voiceEnabled: val, voiceHotkey });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, voiceHotkey]);

  // ── Voice hotkey ───────────────────────────────
  const handleSetVoiceKey = useCallback((combo) => {
    setVoiceHotkey(combo);
    window.electronAPI?.setVoiceHotkey(combo);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, voiceEnabled: true, voiceHotkey: combo });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleClearVoiceKey = useCallback(() => {
    setVoiceHotkey('');
    window.electronAPI?.clearVoiceHotkey();
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, voiceHotkey: '' });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleSetVoiceMic = useCallback((micId) => {
    setVoiceMicId(micId);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, voiceMicId: micId });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  // ── Radial Menu ────────────────────────────────────────────
  const handleSetRadialMenuHotkey = useCallback((combo) => {
    setRadialMenuHotkey(combo);
    window.electronAPI?.setRadialMenuHotkey(combo);
    window.electronAPI?.saveConfig({ radialMenuHotkey: combo });
  }, []);

  const handleSetRadialHoldToSelect = useCallback((enabled) => {
    setRadialHoldToSelect(enabled);
    window.electronAPI?.setRadialHoldToSelect(enabled);
    window.electronAPI?.saveConfig({ radialHoldToSelect: enabled });
  }, []);

  const handleClearRadialMenuHotkey = useCallback(() => {
    setRadialMenuHotkey(null);
    window.electronAPI?.clearRadialMenuHotkey();
    window.electronAPI?.saveConfig({ radialMenuHotkey: null });
  }, []);

  // Auto-fetch app icon for Open App assignments and store on the radial item.
  // Optional assignmentOverride: pass the assignment directly when state hasn't flushed yet.
  // Target priority: iconSource (Start Menu shortcut path — needed for Steam
  // .url shortcuts whose AppID is a bare steam:// URL) → path (Browse-for-file)
  // → appId (AUMID resolved via SHParseDisplayName + SHGFI_PIDL).
  const fetchAndSetAppIcon = useCallback(async (itemId, storageKey, assignmentOverride) => {
    const assignment = assignmentOverride || assignments[storageKey];
    if (!assignment || assignment.type !== 'app') return;
    const target = assignment.data?.iconSource || assignment.data?.path || assignment.data?.appId;
    if (!target) return;
    try {
      const dataUrl = await window.electronAPI?.getAppIcon(target);
      if (dataUrl) {
        setRadialMenuItems(prev => prev.map(item => {
          if (!item || item.id !== itemId) return item;
          return { ...item, appIcon: dataUrl };
        }));
      }
    } catch (e) {
      // Silent fail — icon extraction is best-effort
    }
  }, [assignments]);

  // Also fetch for folder children
  const fetchAndSetChildAppIcon = useCallback(async (folderId, childId, storageKey) => {
    const assignment = assignments[storageKey];
    if (!assignment || assignment.type !== 'app') return;
    const target = assignment.data?.iconSource || assignment.data?.path || assignment.data?.appId;
    if (!target) return;
    try {
      const dataUrl = await window.electronAPI?.getAppIcon(target);
      if (dataUrl) {
        setRadialMenuItems(prev => prev.map(item => {
          if (!item || item.id !== folderId || item.type !== 'folder') return item;
          return { ...item, children: item.children.map(c => c.id === childId ? { ...c, appIcon: dataUrl } : c) };
        }));
      }
    } catch (e) {}
  }, [assignments]);

  // Retroactive backfill: any Open App segment (top-level or folder child) that
  // has no appIcon gets one on next reconcile. Idempotent — items that already
  // carry an appIcon skip the fetch. Runs whenever the wheel or assignments
  // change so a fresh profile switch also converges.
  useEffect(() => {
    if (!radialMenuItems.length || !assignments || Object.keys(assignments).length === 0) return;
    for (const item of radialMenuItems) {
      if (!item) continue;
      if (item.type !== 'folder' && !item.appIcon && item.storageKey) {
        const a = assignments[item.storageKey];
        if (a?.type === 'app' && (a.data?.iconSource || a.data?.path || a.data?.appId)) fetchAndSetAppIcon(item.id, item.storageKey, a);
      }
      if (item.type === 'folder' && Array.isArray(item.children)) {
        for (const c of item.children) {
          if (!c || c.appIcon || !c.storageKey) continue;
          const a = assignments[c.storageKey];
          if (a?.type === 'app' && (a.data?.iconSource || a.data?.path || a.data?.appId)) fetchAndSetChildAppIcon(item.id, c.id, c.storageKey);
        }
      }
    }
  }, [radialMenuItems, assignments, fetchAndSetAppIcon, fetchAndSetChildAppIcon]);

  // Retroactive backfill for Quick Actions — same shape as the radial one but
  // walks assignments directly since QAs store appIcon on data (they don't have
  // a parallel items list). Convergent: once data.appIcon is set, the entry is
  // skipped on subsequent runs.
  useEffect(() => {
    if (!assignments || Object.keys(assignments).length === 0) return;
    for (const [key, a] of Object.entries(assignments)) {
      if (!key.startsWith('GLOBAL::QUICKACTION::')) continue;
      if (!a || a.type !== 'app') continue;
      if (a.data?.appIcon) continue;
      const target = a.data?.iconSource || a.data?.path || a.data?.appId;
      if (!target) continue;
      const qaId = key.slice('GLOBAL::QUICKACTION::'.length);
      fetchAndSetQuickActionAppIcon(qaId, a);
    }
  }, [assignments, fetchAndSetQuickActionAppIcon]);

  const handleAddRadialMenuItem = useCallback((storageKey, label = null, targetIndex = -1) => {
    const resolvedLabel = label || assignments[storageKey]?.label || storageKey.split('::').pop() || '';
    const itemId = crypto.randomUUID();
    setRadialMenuItems(prev => {
      if (prev.filter(Boolean).length >= MAX_SLOTS) return prev;
      if (prev.some(item => item && item.storageKey === storageKey)) return prev;
      const newItem = { id: itemId, storageKey, label: resolvedLabel };
      let next;
      if (targetIndex >= 0 && targetIndex < MAX_SLOTS) {
        next = [...prev];
        while (next.length <= targetIndex) next.push(null);
        if (next[targetIndex] != null) return prev;
        next[targetIndex] = newItem;
      } else {
        next = [...prev, newItem];
      }
      return next;
    });
    // Auto-fetch app icon for Open App assignments
    fetchAndSetAppIcon(itemId, storageKey);
  }, [assignments, fetchAndSetAppIcon]);

  const handleRemoveRadialMenuItem = useCallback((id) => {
    // Radial-only actions (GLOBAL::RADIAL:: keys) exist solely for their
    // wedge. Removing the wedge used to leave them as invisible, unreachable
    // assignments forever; delete them (and a folder's children's) along with
    // the slot. Library-linked / key-linked segments are references and are
    // left alone. handleRadialClear does the same and stays idempotent.
    const radialOnlyKeys = [];
    const collect = (item) => {
      if (!item) return;
      if (item.storageKey?.startsWith('GLOBAL::RADIAL::')) radialOnlyKeys.push(item.storageKey);
      if (item.type === 'folder' && Array.isArray(item.children)) item.children.forEach(collect);
    };
    setRadialMenuItems(prev => {
      prev.forEach(item => { if (item && item.id === id) collect(item); });
      return prev.map(item => (item && item.id === id) ? null : item);
    });
    if (radialOnlyKeys.length) {
      setAssignments(prev => {
        if (!radialOnlyKeys.some(k => prev[k])) return prev;
        const next = { ...prev };
        radialOnlyKeys.forEach(k => { delete next[k]; });
        window.electronAPI?.saveConfig({ assignments: next });
        window.electronAPI?.updateAssignments(next, activeProfile);
        return next;
      });
    }
  }, [activeProfile]);

  const handleReorderRadialMenuItems = useCallback((items) => {
    setRadialMenuItems(items);
  }, []);

  const handleAddRadialMenuFolder = useCallback((label, targetIndex = -1) => {
    setRadialMenuItems(prev => {
      if (prev.filter(Boolean).length >= MAX_SLOTS) return prev;
      const newItem = { id: crypto.randomUUID(), type: 'folder', label, children: [] };
      let next;
      if (targetIndex >= 0 && targetIndex < MAX_SLOTS) {
        next = [...prev];
        while (next.length <= targetIndex) next.push(null);
        if (next[targetIndex] != null) return prev;
        next[targetIndex] = newItem;
      } else {
        next = [...prev, newItem];
      }
      return next;
    });
  }, []);

  const handleAddChildToFolder = useCallback((folderId, storageKey, label = null) => {
    const resolvedLabel = label || assignments[storageKey]?.label || storageKey.split('::').pop() || '';
    const childId = crypto.randomUUID();
    setRadialMenuItems(prev => {
      const next = prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        if (item.children.length >= 8) return item;
        if (item.children.some(c => c.storageKey === storageKey)) return item;
        return { ...item, children: [...item.children, { id: childId, storageKey, label: resolvedLabel }] };
      });
      return next;
    });
    fetchAndSetChildAppIcon(folderId, childId, storageKey);
  }, [assignments, fetchAndSetChildAppIcon]);

  const handleRemoveChildFromFolder = useCallback((folderId, childId) => {
    setRadialMenuItems(prev => {
      const next = prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        return { ...item, children: item.children.filter(c => c.id !== childId) };
      });
      return next;
    });
  }, []);

  // Move a main segment into a folder, preserving icon/iconColor/appIcon
  const handleMoveItemToFolder = useCallback((sourceIndex, folderId) => {
    setRadialMenuItems(prev => {
      const source = prev[sourceIndex];
      if (!source || source.type === 'folder' || !source.storageKey) return prev;
      const folderIdx = prev.findIndex(i => i && i.id === folderId);
      if (folderIdx < 0) return prev;
      const folder = prev[folderIdx];
      if (folder.children.length >= 8) return prev;
      if (folder.children.some(c => c.storageKey === source.storageKey)) return prev;
      const child = {
        id: crypto.randomUUID(),
        storageKey: source.storageKey,
        label: source.label || '',
        ...(source.icon ? { icon: source.icon } : {}),
        ...(source.iconColor ? { iconColor: source.iconColor } : {}),
        ...(source.appIcon ? { appIcon: source.appIcon } : {}),
      };
      const next = prev.map((item, i) => {
        if (i === sourceIndex) return null; // remove from main
        if (i === folderIdx) return { ...item, children: [...item.children, child] };
        return item;
      });
      return next;
    });
  }, []);

  // Move a folder child out to a main segment slot
  const handleMoveChildToMain = useCallback((folderId, childId, targetIndex) => {
    setRadialMenuItems(prev => {
      const folderIdx = prev.findIndex(i => i && i.id === folderId);
      if (folderIdx < 0) return prev;
      const folder = prev[folderIdx];
      const child = folder.children.find(c => c.id === childId);
      if (!child) return prev;
      // Target slot must be empty
      const next = [...prev];
      while (next.length <= targetIndex) next.push(null);
      if (next[targetIndex] != null) return prev;
      // Create main item from child data
      next[targetIndex] = {
        id: crypto.randomUUID(),
        storageKey: child.storageKey,
        label: child.label || '',
        ...(child.icon ? { icon: child.icon } : {}),
        ...(child.iconColor ? { iconColor: child.iconColor } : {}),
        ...(child.appIcon ? { appIcon: child.appIcon } : {}),
      };
      // Remove child from folder
      next[folderIdx] = { ...folder, children: folder.children.filter(c => c.id !== childId) };
      return next;
    });
  }, []);

  const handleReorderFolderChildren = useCallback((folderId, newChildren) => {
    setRadialMenuItems(prev => {
      const next = prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        return { ...item, children: newChildren };
      });
      return next;
    });
  }, []);

  const handleRenameFolder = useCallback((folderId, newName) => {
    setRadialMenuItems(prev => {
      const next = prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        return { ...item, label: newName };
      });
      return next;
    });
  }, []);

  const handleRenameRadialMenuItem = useCallback((id, newLabel) => {
    setRadialMenuItems(prev => {
      return prev.map(item => {
        if (!item || item.id !== id) return item;
        return { ...item, label: newLabel };
      });
    });
  }, []);

  const handleRenameChildInFolder = useCallback((folderId, childId, newLabel) => {
    setRadialMenuItems(prev => {
      return prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        return { ...item, children: item.children.map(c => c.id === childId ? { ...c, label: newLabel } : c) };
      });
    });
  }, []);

  const handleSetRadialMenuItemIcon = useCallback((id, iconName, iconColor) => {
    setRadialMenuItems(prev => {
      return prev.map(item => {
        if (!item || item.id !== id) return item;
        const patch = { ...item };
        if (iconName !== undefined) patch.icon = iconName || undefined;
        if (iconColor !== undefined) patch.iconColor = iconColor || undefined;
        return patch;
      });
    });
  }, []);

  const handleSetRadialChildIcon = useCallback((folderId, childId, iconName, iconColor) => {
    setRadialMenuItems(prev => {
      return prev.map(item => {
        if (!item || item.id !== folderId || item.type !== 'folder') return item;
        return { ...item, children: item.children.map(c => {
          if (c.id !== childId) return c;
          const patch = { ...c };
          if (iconName !== undefined) patch.icon = iconName || undefined;
          if (iconColor !== undefined) patch.iconColor = iconColor || undefined;
          return patch;
        }) };
      });
    });
  }, []);

  const handleCreateRadialAction = useCallback((actionType, actionData, label, targetIndex) => {
    const id = crypto.randomUUID();
    const storageKey = `GLOBAL::RADIAL::${id}`;
    const assignment = { type: actionType, label, data: actionData };
    const newAssignments = { ...assignments, [storageKey]: assignment };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    handleAddRadialMenuItem(storageKey, label, targetIndex);
    // Fetch app icon immediately — pass assignment directly since state hasn't flushed.
    // Handles filesystem paths, AUMIDs, and Start Menu shortcuts (Steam .url etc).
    if (actionType === 'app' && (actionData?.iconSource || actionData?.path || actionData?.appId)) {
      // Need to find the item ID that handleAddRadialMenuItem just created
      // Use a microtask to let the state update flush, then find the item
      queueMicrotask(() => {
        setRadialMenuItems(prev => {
          const item = prev.find(i => i && i.storageKey === storageKey);
          if (item) fetchAndSetAppIcon(item.id, storageKey, assignment);
          return prev; // no mutation — just reading
        });
      });
    }
  }, [assignments, profiles, activeProfile, saveConfig, handleAddRadialMenuItem, fetchAndSetAppIcon]);

  // Assign action to radial segment (from MacroPanel save)
  const handleRadialAssign = useCallback((_keyId, macro) => {
    if (selectedRadialSegment == null) return;
    const idx = selectedRadialSegment;
    const existingItem = idx < radialMenuItems.length ? radialMenuItems[idx] : null;
    const existingKey = existingItem?.storageKey;

    if (existingKey && existingKey.startsWith('GLOBAL::RADIAL::')) {
      // Update existing radial-only assignment in place
      const newAssignments = { ...assignments, [existingKey]: macro };
      setAssignments(newAssignments);
      saveConfig(newAssignments, profiles, activeProfile);
      if (macro.label) {
        setRadialMenuItems(prev => prev.map((item, i) => (item && i === idx) ? { ...item, label: macro.label } : item));
      }
      // Re-fetch app icon if type is app — pass macro directly since state hasn't flushed
      if (macro.type === 'app' && (macro.data?.iconSource || macro.data?.path || macro.data?.appId) && existingItem?.id) {
        fetchAndSetAppIcon(existingItem.id, existingKey, macro);
      }
    } else if (existingItem && existingKey) {
      // Segment is bound to a library/sidebar action — the wheel label is a
      // per-segment display override, the library action stays canonical.
      // Only update item.label; do NOT create a radial-only copy of the
      // action, and do NOT rename the underlying library entry.
      const nextLabel = (macro.label || '').trim();
      if (nextLabel) {
        setRadialMenuItems(prev => prev.map((item, i) => (item && i === idx) ? { ...item, label: nextLabel } : item));
      }
    } else {
      // Truly empty slot — create a new GLOBAL::RADIAL:: assignment
      handleCreateRadialAction(macro.type, macro.data, macro.label || '', idx);
    }
    showNotification('Radial segment updated');
  }, [selectedRadialSegment, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleCreateRadialAction, showNotification, setRadialMenuItems, fetchAndSetAppIcon]);

  // Clear radial segment (from MacroPanel clear)
  const handleRadialClear = useCallback((_keyId) => {
    if (selectedRadialSegment == null) return;
    const idx = selectedRadialSegment;
    const existingItem = idx < radialMenuItems.length ? radialMenuItems[idx] : null;
    if (existingItem) {
      // Remove from wheel
      handleRemoveRadialMenuItem(existingItem.id);
      // If it's a GLOBAL::RADIAL:: key, also delete the assignment
      if (existingItem.storageKey?.startsWith('GLOBAL::RADIAL::')) {
        const newAssignments = { ...assignments };
        delete newAssignments[existingItem.storageKey];
        setAssignments(newAssignments);
        saveConfig(newAssignments, profiles, activeProfile);
      }
    }
    setSelectedRadialSegment(null);
    showNotification('Radial segment cleared', 'info');
  }, [selectedRadialSegment, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleRemoveRadialMenuItem, showNotification]);

  // Assign action to a folder child (from MacroPanel save)
  const handleRadialChildAssign = useCallback((_keyId, macro) => {
    if (!selectedRadialChild) return;
    const { folderId, childIndex } = selectedRadialChild;
    // Find the folder and check if child slot has an existing item
    const folder = radialMenuItems.find(i => i && i.id === folderId);
    const existingChild = folder?.children?.[childIndex];
    const existingKey = existingChild?.storageKey;

    if (existingKey && existingKey.startsWith('GLOBAL::RADIAL::')) {
      // Update existing radial-only child assignment in place
      const newAssignments = { ...assignments, [existingKey]: macro };
      setAssignments(newAssignments);
      saveConfig(newAssignments, profiles, activeProfile);
      if (macro.label) {
        setRadialMenuItems(prev => prev.map(item => {
          if (!item || item.id !== folderId || item.type !== 'folder') return item;
          return { ...item, children: item.children.map((c, ci) => ci === childIndex ? { ...c, label: macro.label } : c) };
        }));
      }
      // Re-fetch app icon if type is app — pass macro directly since state hasn't flushed.
      // Mirrors the top-level reassign branch in handleRadialAssign. Priority
      // matches fetchAndSetAppIcon: iconSource → path → appId.
      const iconTarget = macro.data?.iconSource || macro.data?.path || macro.data?.appId;
      if (macro.type === 'app' && iconTarget && existingChild?.id) {
        (async () => {
          try {
            const dataUrl = await window.electronAPI?.getAppIcon(iconTarget);
            if (!dataUrl) return;
            setRadialMenuItems(prev => prev.map(item => {
              if (!item || item.id !== folderId || item.type !== 'folder') return item;
              return { ...item, children: item.children.map(c => c.id === existingChild.id ? { ...c, appIcon: dataUrl } : c) };
            }));
          } catch (e) {}
        })();
      }
    } else if (existingChild && existingKey) {
      // Child is bound to a library/sidebar action — the wheel label is a
      // per-child display override, the library action stays canonical.
      // Only update child.label; do NOT create a radial-only copy of the
      // action, and do NOT rename the underlying library entry.
      const nextLabel = (macro.label || '').trim();
      if (nextLabel) {
        setRadialMenuItems(prev => prev.map(item => {
          if (!item || item.id !== folderId || item.type !== 'folder') return item;
          return { ...item, children: item.children.map((c, ci) => ci === childIndex ? { ...c, label: nextLabel } : c) };
        }));
      }
    } else {
      // Truly empty child slot — create GLOBAL::RADIAL:: assignment and add to folder
      const id = crypto.randomUUID();
      const storageKey = `GLOBAL::RADIAL::${id}`;
      const newAssignments = { ...assignments, [storageKey]: macro };
      setAssignments(newAssignments);
      saveConfig(newAssignments, profiles, activeProfile);
      handleAddChildToFolder(folderId, storageKey, macro.label || '');
    }
    showNotification('Folder child updated');
  }, [selectedRadialChild, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleAddChildToFolder, showNotification, setRadialMenuItems]);

  // Clear a folder child (from MacroPanel clear)
  const handleRadialChildClear = useCallback((_keyId) => {
    if (!selectedRadialChild) return;
    const { folderId, childIndex } = selectedRadialChild;
    const folder = radialMenuItems.find(i => i && i.id === folderId);
    const existingChild = folder?.children?.[childIndex];
    if (existingChild) {
      handleRemoveChildFromFolder(folderId, existingChild.id);
      if (existingChild.storageKey?.startsWith('GLOBAL::RADIAL::')) {
        const newAssignments = { ...assignments };
        delete newAssignments[existingChild.storageKey];
        setAssignments(newAssignments);
        saveConfig(newAssignments, profiles, activeProfile);
      }
    }
    setSelectedRadialChild(null);
    showNotification('Folder child cleared', 'info');
  }, [selectedRadialChild, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleRemoveChildFromFolder, showNotification]);

  // Select a radial segment — route to normal MacroPanel for sidebar items,
  // radial MacroPanel for radial-only / new items
  const handleSelectRadialSegment = useCallback((index) => {
    const item = index < radialMenuItems.length ? radialMenuItems[index] : null;
    const storageKey = item?.storageKey;

    if (storageKey && !storageKey.startsWith('GLOBAL::') && storageKey.startsWith(activeProfile + '::')) {
      // Existing sidebar assignment — open normally with sidebar highlight
      const parts = storageKey.split('::');
      if (parts.length >= 3) {
        const combo = parts[1];
        const keyId = parts[2];
        if (combo === 'BARE') {
          setActiveModifiers(['BARE']);
        } else if (combo) {
          setActiveModifiers(combo.split('+'));
        }
        setSelectedKey(keyId);
        setSelectedRadialSegment(null);
        setSelectedRadialChild(null);
        return;
      }
    }

    // Radial-only item, global item, or empty segment — use radial MacroPanel
    setSelectedRadialSegment(index);
    setSelectedKey(null);
    setSelectedRadialChild(null);
  }, [radialMenuItems, activeProfile]);

  const handleSwapRadialMenuItems = useCallback((fromIndex, toIndex) => {
    setRadialMenuItems(prev => {
      const next = [...prev];
      while (next.length <= Math.max(fromIndex, toIndex)) next.push(null);
      const temp = next[fromIndex];
      next[fromIndex] = next[toIndex];
      next[toIndex] = temp;
      return next;
    });
  }, []);

  // Copy a radial segment to another profile's same slot.
  // Returns 'conflict' + existing item label if the slot is occupied, otherwise copies directly.
  const handleCopyRadialSegmentToProfile = useCallback((targetProfile, segmentIndex) => {
    const sourceItems = editingRadialMap[activeProfile] || [];
    const sourceItem = sourceItems[segmentIndex];
    if (!sourceItem) return null;
    const targetItems = editingRadialMap[targetProfile] || [];
    const existing = targetItems[segmentIndex];
    if (existing) {
      return { conflict: true, existingLabel: existing.label || existing.type || 'item' };
    }
    // Deep copy with new UUID
    const copied = JSON.parse(JSON.stringify(sourceItem));
    copied.id = crypto.randomUUID();
    if (copied.children) copied.children = copied.children.map(c => c ? { ...c, id: crypto.randomUUID() } : c);
    const newTarget = [...targetItems];
    while (newTarget.length <= segmentIndex) newTarget.push(null);
    newTarget[segmentIndex] = copied;
    const newMap = { ...editingRadialMap, [targetProfile]: newTarget };
    persistEditingRadialMap(newMap);
    showNotification(`Copied to "${targetProfile}"`);
    return null;
  }, [editingRadialMap, activeProfile, persistEditingRadialMap, showNotification]);

  const handleForceOverwriteRadialSegment = useCallback((targetProfile, segmentIndex) => {
    const sourceItems = editingRadialMap[activeProfile] || [];
    const sourceItem = sourceItems[segmentIndex];
    if (!sourceItem) return;
    const copied = JSON.parse(JSON.stringify(sourceItem));
    copied.id = crypto.randomUUID();
    if (copied.children) copied.children = copied.children.map(c => c ? { ...c, id: crypto.randomUUID() } : c);
    const targetItems = editingRadialMap[targetProfile] || [];
    const newTarget = [...targetItems];
    while (newTarget.length <= segmentIndex) newTarget.push(null);
    newTarget[segmentIndex] = copied;
    const newMap = { ...editingRadialMap, [targetProfile]: newTarget };
    persistEditingRadialMap(newMap);
    showNotification(`Copied to "${targetProfile}" (overwritten)`);
  }, [editingRadialMap, activeProfile, persistEditingRadialMap, showNotification]);

  // ── Radial layouts (Pro, per-device) ──────────────────────────────────
  // Layouts are pointers to the shared actions, so creating, renaming or
  // deleting one never touches assignments. Which layout THIS device fires
  // lives in trigr-local-settings.json (Rust); the editor opens on it.
  // One selector: the picked layout is both what the editor shows and what
  // this device's radial hotkey opens, so a pick persists as the machine's
  // choice (trigr-local-settings.json via Rust).
  const handleSelectRadialLayout = useCallback((id) => {
    const value = id && id !== 'default' ? id : 'default';
    setEditingRadialLayoutId(value);
    setDeviceRadialLayoutId(value);
    window.electronAPI?.setRadialLayoutId?.(value === 'default' ? null : value);
    setSelectedRadialSegment(null);
    setSelectedRadialChild(null);
    setExpandedRadialFolder(null);
  }, []);

  const handleCreateRadialLayout = useCallback((name, duplicateCurrent) => {
    const id = crypto.randomUUID();
    let itemsByProfile = {};
    if (duplicateCurrent) {
      // Deep copy with fresh item ids — folder ids key the editor's
      // expanded / selected state, so two layouts must not share them.
      itemsByProfile = JSON.parse(JSON.stringify(editingRadialMap));
      for (const items of Object.values(itemsByProfile)) {
        if (!Array.isArray(items)) continue;
        for (const it of items) {
          if (!it) continue;
          it.id = crypto.randomUUID();
          if (Array.isArray(it.children)) it.children = it.children.map(c => c ? { ...c, id: crypto.randomUUID() } : c);
        }
      }
    }
    const trimmed = (name || '').trim();
    const layout = { id, name: trimmed || `Layout ${radialLayouts.length + 2}`, itemsByProfile };
    const next = [...radialLayouts, layout];
    setRadialLayouts(next);
    window.electronAPI?.saveConfig({ radialLayouts: next });
    handleSelectRadialLayout(id);
    showNotification(`Layout "${layout.name}" created`);
  }, [editingRadialMap, radialLayouts, handleSelectRadialLayout, showNotification]);

  const handleRenameRadialLayout = useCallback((id, name) => {
    const trimmed = (name || '').trim();
    if (!trimmed) return;
    const next = radialLayouts.map(l => l.id === id ? { ...l, name: trimmed } : l);
    setRadialLayouts(next);
    window.electronAPI?.saveConfig({ radialLayouts: next });
  }, [radialLayouts]);

  const handleDeleteRadialLayout = useCallback((id) => {
    const next = radialLayouts.filter(l => l.id !== id);
    setRadialLayouts(next);
    window.electronAPI?.saveConfig({ radialLayouts: next });
    if (deviceRadialLayoutId === id) {
      setDeviceRadialLayoutId('default');
      window.electronAPI?.setRadialLayoutId?.(null);
    }
    if (editingRadialLayoutId === id) handleSelectRadialLayout('default');
    showNotification('Layout deleted', 'info');
  }, [radialLayouts, deviceRadialLayoutId, editingRadialLayoutId, handleSelectRadialLayout, showNotification]);

  // A layout deleted on another device (or dropped by a sync) must not leave
  // the editor pointing at nothing.
  useEffect(() => {
    if (editingRadialLayoutId !== 'default' && !radialLayouts.some(l => l.id === editingRadialLayoutId)) {
      setEditingRadialLayoutId('default');
    }
  }, [editingRadialLayoutId, radialLayouts]);

  // ── Radial drag state + handlers (cross-container DndContext) ──
  const [radialActiveDrag, setRadialActiveDrag] = useState(null);
  const [radialDropTarget, setRadialDropTarget] = useState(-1);    // inner ring target
  const [radialDropTargetOuter, setRadialDropTargetOuter] = useState(-1); // outer ring target
  const [radialRejectIndex, setRadialRejectIndex] = useState(-1);
  const wheelRef = useRef(null);
  const radialDragActivatorRef = useRef(null); // stores the pointerdown event from drag start

  // ── Bind/move drag state (sidebar rows → canvas keys) ──
  // kind 'bind-action' drags ride the same DndContext as the radial drags;
  // the shared handlers branch on active.data.current.kind.
  const [bindActiveDrag, setBindActiveDrag] = useState(null); // { source: 'unassigned'|'bound', id?, combo?, keyId?, label }
  const [pendingCanvasDrop, setPendingCanvasDrop] = useState(null); // occupied-target confirm
  // Drag payload mirror: spring-toggling a modifier layer mid-drag can
  // re-filter the sidebar and unmount the dragged row, after which dnd-kit's
  // active.data.current may be gone at drop time. The ref keeps the payload
  // alive for the whole drag.
  const bindDragRef = useRef(null);
  // Spring-loaded modifier switching: hovering a ModifierBar button for 450ms
  // mid-drag toggles that layer, so "pick up macro → hover Ctrl+Alt → drop on
  // K" works in one gesture.
  const springModRef = useRef({ overId: null, timer: null });
  const clearSpringMod = useCallback(() => {
    if (springModRef.current.timer) clearTimeout(springModRef.current.timer);
    springModRef.current = { overId: null, timer: null };
  }, []);

  const radialSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  const radialUsedKeys = useMemo(() => {
    const s = new Set();
    radialMenuItems.forEach(i => {
      if (!i) return;
      if (i.storageKey) s.add(i.storageKey);
      if (i.children) i.children.forEach(c => { if (c && c.storageKey) s.add(c.storageKey); });
    });
    return s;
  }, [radialMenuItems]);

  // Hit test returning { ring: 'inner'|'outer', index } or null
  const hitTestWedge = useCallback((clientX, clientY) => {
    if (!wheelRef.current) return null;
    const rect = wheelRef.current.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const svgX = ((clientX - rect.left) / rect.width) * 420;
    const svgY = ((clientY - rect.top) / rect.height) * 420;
    const CX = 210, CY = 210;
    const INNER_R = 55, OUTER_R = 105;
    const OUTER_INNER = 113, OUTER_OUTER = 163;
    const dx = svgX - CX, dy = svgY - CY;
    const dist = Math.sqrt(dx * dx + dy * dy);

    // Raw atan2 angle in degrees — same coordinate system as RadialWheel
    const atan2Deg = Math.atan2(dy, dx) * (180 / Math.PI);

    // Slot angle — offset by +90 (0=top) and +halfStep to match centred wedge layout
    const step = 360 / MAX_SLOTS;
    const slotAngle = ((atan2Deg + 90 + step / 2) % 360 + 360) % 360;

    // Inner ring
    if (dist >= INNER_R && dist <= OUTER_R) {
      return { ring: 'inner', index: Math.floor(slotAngle / step) };
    }

    // Outer ring — only when a folder is expanded. Geometry MUST match
    // RadialWheel.jsx outerWedges exactly (drawing centres the assigned run
    // on the parent bisector at its own wedge width, then appends the "+"
    // empty slot at the same wedge width — NOT one big equal-slice arc).
    if (dist >= OUTER_INNER && dist <= OUTER_OUTER && expandedRadialFolder) {
      const folderIdx = radialMenuItems.findIndex(i => i?.id === expandedRadialFolder);
      if (folderIdx < 0) return null;
      const folder = radialMenuItems[folderIdx];
      if (folder?.type !== 'folder' || !folder.children) return null;

      const childCount = folder.children.length;
      const slotStep = 360 / MAX_SLOTS;
      const parentStart = slotStep * folderIdx - 90 - slotStep / 2;
      const parentBisector = parentStart + slotStep / 2;
      const parentArc = slotStep;

      const minArcPerChild = 22;
      const hasEmpty = childCount < 8;
      const assignedCount = Math.max(childCount, 1);
      const desiredArc = Math.max(parentArc, assignedCount * minArcPerChild);
      const assignedArc = Math.min(desiredArc, 160);
      const childWedgeAngle = assignedArc / assignedCount;
      const assignedStart = parentBisector - assignedArc / 2;

      const assignedFilledArc = childCount * childWedgeAngle;
      const totalArc = assignedFilledArc + (hasEmpty ? childWedgeAngle : 0);
      const relAngle = ((atan2Deg - assignedStart) % 360 + 360) % 360;
      if (relAngle < totalArc) {
        const childIdx = Math.floor(relAngle / childWedgeAngle);
        const maxIdx = hasEmpty ? childCount : childCount - 1;
        if (childIdx >= 0 && childIdx <= maxIdx) {
          return { ring: 'outer', index: childIdx, folderId: expandedRadialFolder };
        }
      }
    }

    return null;
  }, [expandedRadialFolder, radialMenuItems]);

  const handleRadialDragStart = useCallback((event) => {
    const { active, activatorEvent } = event;
    const data = active.data?.current;
    if (data?.kind === 'bind-action') {
      // Recording lock: the layer buttons and canvas are disabled while the
      // hotkey recorder is armed — drags must not mutate layer/selection
      // state behind it either.
      if (isRecording) return;
      const payload = { ...data };
      bindDragRef.current = payload;
      setBindActiveDrag(payload);
      return;
    }
    if (activeView !== 'radial') return;
    radialDragActivatorRef.current = activatorEvent || null;
    setRadialActiveDrag({
      id: active.id,
      kind: data?.kind || 'library-card',
      label: data?.folderName || String(active.id).split('::').pop() || '',
    });
  }, [activeView, isRecording]);

  // Spring-load layer selection — ADDITIVE only, unlike the click path's
  // handleToggleModifier. Dwelling on an already-active button must never
  // deselect it (the user is en route to a key, not toggling), BARE swaps the
  // layer to bare, and the 3-modifier cap keeps extra dwells inert. Also
  // deliberately does NOT touch selectedKey or sidebarComboFilter mid-drag —
  // rewriting the filter would unmount the dragged sidebar row.
  const springSelectModifier = useCallback((mod) => {
    setActiveModifiers(prev => {
      if (mod === 'BARE') return prev.includes('BARE') ? prev : ['BARE'];
      if (prev.includes(mod)) return prev;
      const next = [...prev.filter(m => m !== 'BARE'), mod];
      return next.length > 3 ? prev : next;
    });
  }, []);

  // Spring-loaded modifier switch — fires while a bind-action drag hovers a
  // ModifierBar button. Droppable ids: modlayer-<Ctrl|Shift|Alt|Win|BARE>.
  // Keyed off bindDragRef (not active.data.current, which can vanish if the
  // dragged row unmounts mid-drag).
  const handleCanvasDragOver = useCallback((event) => {
    if (!bindDragRef.current) return;
    if (isRecording) { clearSpringMod(); return; }
    const overData = event.over?.data?.current;
    if (!overData || overData.dropKind !== 'modlayer') { clearSpringMod(); return; }
    const overId = String(event.over.id);
    if (springModRef.current.overId === overId) return; // timer already pending
    clearSpringMod();
    springModRef.current = {
      overId,
      timer: setTimeout(() => {
        springModRef.current = { overId: null, timer: null };
        springSelectModifier(overData.mod);
      }, 450),
    };
  }, [clearSpringMod, springSelectModifier, isRecording]);

  const handleRadialDragMove = useCallback((event) => {
    if (activeView !== 'radial') return;
    const { activatorEvent, delta } = event;
    const origin = activatorEvent || radialDragActivatorRef.current;
    if (!origin) return;
    const clientX = (origin.clientX || 0) + (delta?.x || 0);
    const clientY = (origin.clientY || 0) + (delta?.y || 0);
    const hit = hitTestWedge(clientX, clientY);
    setRadialDropTarget(hit?.ring === 'inner' ? hit.index : -1);
    setRadialDropTargetOuter(hit?.ring === 'outer' ? hit.index : -1);
  }, [activeView, hitTestWedge]);

  const handleRadialDragEnd = useCallback((event) => {
    // Prefer the ref mirror — active.data.current can vanish if the dragged
    // row unmounted mid-drag (spring layer switch re-filters the sidebar).
    const bindData = bindDragRef.current
      || (event.active?.data?.current?.kind === 'bind-action' ? event.active.data.current : null);
    if (bindData) {
      bindDragRef.current = null;
      setBindActiveDrag(null);
      clearSpringMod();
      const target = event.over?.data?.current;
      if (!target || target.dropKind !== 'canvas-key') return;
      const targetKeyId = target.keyId;
      const targetCombo = currentCombo;
      if (!targetCombo) return; // no modifier layer — key droppables are disabled anyway
      const targetBase = makeAssignmentKey(activeProfile, targetCombo, targetKeyId);
      const occupied = ASSIGNMENT_VARIANT_SUFFIXES.map(s => assignments[targetBase + s]).find(Boolean);
      // Same hazard guard as the click path (handleKeySelect) — a drop must
      // not silently hijack a reserved Windows shortcut.
      const reserved = findReservedShortcut(targetCombo, targetKeyId);
      if (bindData.source === 'unassigned') {
        if (occupied || reserved) {
          setPendingCanvasDrop({
            mode: 'bind', id: bindData.id, targetCombo, targetKeyId,
            conflictLabel: occupied ? (occupied.label || 'an action') : null,
            reservedOsFunction: reserved?.osFunction || null,
          });
        } else {
          handleBindLibrary(bindData.id, targetCombo, targetKeyId);
        }
      } else if (bindData.source === 'bound') {
        if (bindData.combo === targetCombo && bindData.keyId === targetKeyId) return;
        if (occupied || reserved) {
          setPendingCanvasDrop({
            mode: 'move', srcCombo: bindData.combo, srcKeyId: bindData.keyId, targetCombo, targetKeyId,
            conflictLabel: occupied ? (occupied.label || 'an action') : null,
            reservedOsFunction: reserved?.osFunction || null,
          });
        } else {
          moveAssignment(bindData.combo, bindData.keyId, targetCombo, targetKeyId);
        }
      }
      return;
    }
    if (activeView !== 'radial') { setRadialActiveDrag(null); setRadialDropTarget(-1); setRadialDropTargetOuter(-1); return; }
    const { active, delta } = event;
    const data = active.data?.current;
    const activator = radialDragActivatorRef.current;

    setRadialActiveDrag(null);
    setRadialDropTarget(-1);
    setRadialDropTargetOuter(-1);
    radialDragActivatorRef.current = null;

    // Re-compute hit from final cursor position
    const clientX = (activator?.clientX || 0) + (delta?.x || 0);
    const clientY = (activator?.clientY || 0) + (delta?.y || 0);
    const hit = hitTestWedge(clientX, clientY);

    // Outer ring drop — add as child to expanded folder
    if (hit?.ring === 'outer' && hit.folderId && (data?.kind === 'library-card') && data?.storageKey) {
      const folder = radialMenuItems.find(i => i?.id === hit.folderId);
      const existingChild = folder?.children?.[hit.index];
      if (existingChild) return; // slot filled
      handleAddChildToFolder(hit.folderId, data.storageKey, null);
      return;
    }

    // Inner ring drop
    if (!hit || hit.ring !== 'inner') return;
    const idx = hit.index;
    if (idx < 0 || idx >= MAX_SLOTS) return;

    const existingItem = idx < radialMenuItems.length ? radialMenuItems[idx] : null;
    if (existingItem) {
      setRadialRejectIndex(idx);
      setTimeout(() => setRadialRejectIndex(-1), 250);
      return;
    }

    if ((data?.kind === 'library-card') && data?.storageKey) {
      handleAddRadialMenuItem(data.storageKey, null, idx);
    }
  }, [activeView, radialMenuItems, expandedRadialFolder, hitTestWedge, handleAddRadialMenuItem, handleAddChildToFolder, currentCombo, activeProfile, assignments, makeAssignmentKey, handleBindLibrary, moveAssignment, clearSpringMod]);

  const handleRadialDragCancel = useCallback(() => {
    setRadialActiveDrag(null);
    setRadialDropTarget(-1);
    setRadialDropTargetOuter(-1);
    setBindActiveDrag(null);
    bindDragRef.current = null;
    clearSpringMod();
  }, [clearSpringMod]);

  // ── Search overlay settings ───────────────────────────────
  const handleUpdateSearchSettings = useCallback((patch) => {
    if (patch.searchOverlayHotkey      !== undefined) setSearchOverlayHotkey(patch.searchOverlayHotkey);
    if (patch.searchOverlayEnabled     !== undefined) setSearchOverlayEnabled(patch.searchOverlayEnabled);
    if (patch.overlayShowAll           !== undefined) setOverlayShowAll(patch.overlayShowAll);
    if (patch.overlayCloseAfterFiring  !== undefined) setOverlayCloseAfterFiring(patch.overlayCloseAfterFiring);
    if (patch.overlayIncludeAutocorrect !== undefined) setOverlayIncludeAutocorrect(patch.overlayIncludeAutocorrect);
    // Engine registration: always send enabled + hotkey together so a
    // re-enable restores the user's combo and a hotkey change while disabled
    // doesn't accidentally re-arm the overlay.
    window.electronAPI?.updateSearchSettings({
      ...patch,
      searchOverlayEnabled: patch.searchOverlayEnabled ?? searchOverlayEnabled,
      searchOverlayHotkey:  patch.searchOverlayHotkey  ?? searchOverlayHotkey,
    });
    // Persist. save_config shallow-merges, so the partial patch is enough.
    // (Sibling handleUpdateGlobalSettings always saved; this one never did,
    // so the overlay toggles and custom hotkey reverted on every restart.)
    window.electronAPI?.saveConfig(patch);
  }, [searchOverlayEnabled, searchOverlayHotkey]);

  const handleToggleMacrosOnStartup = useCallback((val) => {
    setMacrosEnabledOnStartup(val);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup: val, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled]);
  // ── Physical keyboard shape (Settings → General → Keyboard shape) ─────────
  const handleSetPhysicalKeyboardLayout = useCallback((val) => {
    const next = ['ansi', 'iso'].includes(val) ? val : 'auto';
    setPhysicalKeyboardLayout(next);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, physicalKeyboardLayout: next, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);
  // Both capture paths land here: the LL hook (Rust emits iso-key-detected
  // once per run when scancode 0x56 is pressed) and the WebView keydown
  // listener (e.code === 'IntlBackslash' while Keyfire itself is focused).
  // Persisted so the shape survives restarts without re-detection.
  const handleIsoKeyDetected = useCallback(() => {
    if (isoKeyDetected) return;
    setIsoKeyDetected(true);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, isoKeyDetected: true, hasSeenWelcome: true });
    if (physicalKeyboardLayout === 'auto' && keyboardLayoutHint !== 'iso') {
      showNotification('ISO keyboard detected (extra key beside Shift). The on-screen keyboard now uses the ISO shape. Change it in Settings, General.', 'info');
    }
  }, [isoKeyDetected, physicalKeyboardLayout, keyboardLayoutHint, assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, showNotification]);
  useEffect(() => {
    window.electronAPI?.getKeyboardLayoutHint?.()
      .then(h => { if (h === 'iso' || h === 'ansi') setKeyboardLayoutHint(h); })
      .catch(() => {});
  }, []);
  // Legends: fetched on mount and again whenever the window regains focus,
  // which is when a user who switched input language will next look at the
  // canvas. On failure (or an empty answer) the hard-coded US legends stay.
  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      window.electronAPI?.getKeyboardLegends?.()
        .then(list => {
          if (cancelled || !Array.isArray(list) || list.length === 0) return;
          const bySlot = {};
          const byKeyId = {};
          for (const l of list) {
            bySlot[l.slot] = { keyId: l.key_id, base: l.base, shift: l.shift };
            if (l.base) byKeyId[l.key_id] = l.base;
          }
          setLiveKeyLegends(byKeyId);
          setKeyboardLegends(prev => (JSON.stringify(prev) === JSON.stringify(bySlot) ? prev : bySlot));
        })
        .catch(() => {});
    };
    refresh();
    window.addEventListener('focus', refresh);
    return () => { cancelled = true; window.removeEventListener('focus', refresh); };
  }, []);
  useEffect(() => {
    let unlisten = null;
    let disposed = false;
    window.electronAPI?.onIsoKeyDetected?.(handleIsoKeyDetected)
      ?.then(u => { if (disposed) u?.(); else unlisten = u; });
    const onKeyDown = (e) => { if (e.code === 'IntlBackslash') handleIsoKeyDetected(); };
    document.addEventListener('keydown', onKeyDown);
    return () => { disposed = true; unlisten?.(); document.removeEventListener('keydown', onKeyDown); };
  }, [handleIsoKeyDetected]);

  const handleToggleClipboardCapture = useCallback((enabled) => {
    setClipboardCaptureEnabled(enabled);
    window.electronAPI?.setClipboardCaptureEnabled(enabled);
    window.electronAPI?.saveConfig({
      assignments, profiles, activeProfile, profileSettings, theme, expansionCategories,
      autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true,
      clipboardCaptureEnabled: enabled,
      clipboardExcludedApps,
    });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, clipboardExcludedApps]);

  // Telemetry opt-in toggle. NOT saved to the shared config (machine-local
  // preference per [[reference_live_config_shared_path]] convention) — the
  // Rust side persists to trigr-local-settings.json directly.
  const handleToggleTelemetry = useCallback((enabled) => {
    setTelemetryEnabled(enabled);
    window.electronAPI?.setTelemetryEnabled?.(enabled);
  }, []);

  const handleUpdateClipboardExcludedApps = useCallback((apps) => {
    // Normalize: lowercase, strip .exe, dedupe, drop empties. Mirrors the
    // Rust-side normalization in clipboard::normalize_proc_name.
    const normalized = Array.from(new Set(
      (apps || [])
        .map(a => (a || '').toLowerCase().replace(/\.exe$/, '').trim())
        .filter(Boolean)
    ));
    setClipboardExcludedApps(normalized);
    window.electronAPI?.setClipboardExcludedApps(normalized);
    window.electronAPI?.saveConfig({
      assignments, profiles, activeProfile, profileSettings, theme, expansionCategories,
      autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true,
      clipboardCaptureEnabled,
      clipboardExcludedApps: normalized,
    });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, clipboardCaptureEnabled]);

  // ── Grace period: manual "Switch to local now" ────────────
  const handleMigrateSharedToLocalNow = useCallback(async () => {
    const confirmed = window.confirm(
      'Switch to local config now?\n\n'
      + 'Your current shared config will be copied to local storage on this machine. '
      + 'Your data is preserved. The shared file in your cloud folder is not deleted, '
      + 'so other machines (if any) can keep using it.'
    );
    if (!confirmed) return;
    const result = await window.electronAPI?.migrateSharedToLocalNow();
    if (result?.ok) {
      // Rust emits 'shared-config-migrated' which the existing listener
      // picks up to refresh state + show the post-migration notice.
      showNotification('Shared config moved to local storage.');
    } else {
      showNotification(result?.error || 'Migration failed. Check the log.', 'error');
      // Refresh state so the banner reflects deferred mode if applicable.
      window.electronAPI?.getGracePeriodState?.().then(g => setGracePeriodState(g));
    }
  }, [showNotification]);

  const handleDismissWelcome = useCallback(() => {
    setShowWelcome(false);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  // "Get Started" inside WelcomeModal — proceed to the onboarding tour.
  // Marks welcome seen but keeps onboarding incomplete so the tour fires.
  const handleWelcomeContinue = useCallback(() => {
    setShowWelcome(false);
    window.electronAPI?.saveConfig({ hasSeenWelcome: true });
    if (!onboardingCompleteRef.current) {
      setShowOnboarding(true);
    }
  }, []);

  // "Skip the tour" inside WelcomeModal — bypass both and mark everything seen.
  const handleWelcomeSkip = useCallback(() => {
    setShowWelcome(false);
    onboardingCompleteRef.current = true;
    window.electronAPI?.saveConfig({
      hasSeenWelcome: true,
      onboarding_complete: true,
      onboarding_version_seen: ONBOARDING_VERSION,
    });
    // The trial has been live since launch; skipping the tour must not
    // also skip the announcement.
    if (trialAnnouncePending(licenceStatus)) setShowProTrialModal(true);
  }, [licenceStatus]);

  // Dev-only: replay the first-launch experience without uninstalling.
  // Resets the welcome + onboarding flags, snaps to keyboard mapping (so the
  // tour can render its highlights), then re-fires the welcome modal.
  const handleReplayWelcome = useCallback(() => {
    window.electronAPI?.hideSettingsWindow();
    window.electronAPI?.resetOnboarding();
    setTemplatesNudgeSeen(false);
    setActiveModifiers([]);
    setSidebarComboFilter(null);
    setActiveArea('mapping');
    setActiveView('keyboard');
    window.electronAPI?.saveConfig({
      hasSeenWelcome: false,
      templates_nudge_seen: false,
    });
    onboardingCompleteRef.current = false;
    setShowWelcome(true);
  }, []);

  const handleOnboardingComplete = useCallback(() => {
    setShowOnboarding(false);
    onboardingCompleteRef.current = true;
    window.electronAPI?.saveConfig({
      onboarding_complete: true,
      hasSeenWelcome: true,
      onboarding_version_seen: ONBOARDING_VERSION,
    });
    // Announce the 14-day Pro trial right after the tour finishes (also on
    // skip). Rust started it at first launch; this modal is an announcement,
    // not an offer. Suppressed once announced or if a real key is entered.
    if (trialAnnouncePending(licenceStatus)) setShowProTrialModal(true);
  }, [licenceStatus]);

  const handleRestartOnboarding = useCallback(() => {
    window.electronAPI?.hideSettingsWindow();
    window.electronAPI?.resetOnboarding();
    // Reset the templates coachmark so it re-fires after the replayed tour
    // (and after any trial offer that follows). Mirrors the new-user flow.
    setTemplatesNudgeSeen(false);
    window.electronAPI?.saveConfig({ templates_nudge_seen: false });
    // Clear any active modifier layer + sidebar combo filter (same as ESC) so
    // the tour starts on a clean keyboard canvas. Otherwise the user lands in
    // Step 2/3 with a pre-selected modifier from before they clicked Restart
    // Tour, which makes the "press your hotkey" prompt confusing.
    setActiveModifiers([]);
    setSidebarComboFilter(null);
    // Also drop any open editor. With a key still selected the tour skipped
    // Step 2b (the panel was already non-empty) and could dead-end at 2c.
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setDraftAssignment(null);
    setSelectedRadialSegment(null);
    setSelectedRadialChild(null);
    // Snap back to the keyboard mapping UI. If the user clicked Restart Tour
    // while on Expansions / Analytics / Mouse / Radial / Clipboard, Step 2
    // can't render its highlights (modifier bar + keyboard canvas don't
    // exist outside the mapping/keyboard area) and the tour stalls.
    setActiveArea('mapping');
    setActiveView('keyboard');
    setShowOnboarding(true);
    onboardingCompleteRef.current = false;
  }, []);

  const handleDismissTips = useCallback(() => {
    setTipsHidden(true);
    window.electronAPI?.saveConfig({ tipsHidden: true });
  }, []);

  // Hide a single feature TIP box for good (until reset in Settings).
  const handleHideTip = useCallback((key) => {
    setHiddenTips(prev => {
      if (prev.includes(key)) return prev;
      const next = [...prev, key];
      window.electronAPI?.saveConfig({ hiddenTips: next });
      return next;
    });
  }, []);

  // Settings "Show feature tips again" — restores every dismissed TIP box
  // and the keyboard-view quick tips.
  const handleResetHiddenTips = useCallback(() => {
    setHiddenTips([]);
    setTipsHidden(false);
    window.electronAPI?.saveConfig({ hiddenTips: [], tipsHidden: false });
  }, []);

  // ── Templates coachmark fire / dismiss ─────────────────────
  const handleDismissTemplatesNudge = useCallback(() => {
    setShowTemplatesNudge(false);
    setTemplatesNudgeSeen(true);
    window.electronAPI?.saveConfig({ templates_nudge_seen: true });
  }, []);

  const handleOpenTemplatesFromCoachmark = useCallback(() => {
    setOpenTemplatesSignal(n => n + 1);
    handleDismissTemplatesNudge();
  }, [handleDismissTemplatesNudge]);

  // Fire the coachmark once the user lands on the main UI with the Templates
  // pill visible. Guard rails:
  //   - onboarding tour, welcome modal, and trial modal must all be closed
  //   - user must be on the Triggers (mapping) tab so the pill is rendered
  //   - the pill itself must not have been right-click-dismissed (localStorage)
  //   - templates_nudge_seen must be false (one-shot)
  //   - onboarding_complete must be true (don't fire if user quit mid-tour)
  useEffect(() => {
    if (!licenceChecked) return; // wait until any migration trial popup has had a chance to open
    if (showOnboarding || showWelcome || showProTrialModal || showTrialEndModal) return;
    if (templatesNudgeSeen || showTemplatesNudge) return;
    if (activeArea !== 'mapping') return;
    if (!onboardingCompleteRef.current) return;
    try {
      if (localStorage.getItem('trigr_templates_dismissed') === 'true') return;
    } catch {}

    // Defer one frame + a small beat so the TitleBar pill has laid out (and
    // any modal close animation has run) before we measure its rect.
    const t = setTimeout(() => {
      const el = templatesPillRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0) return; // not yet rendered
      setTemplatesPillRect(rect);
      setShowTemplatesNudge(true);
    }, 350);
    return () => clearTimeout(t);
  }, [licenceChecked, showOnboarding, showWelcome, showProTrialModal, showTrialEndModal, templatesNudgeSeen, showTemplatesNudge, activeArea]);

  // Keep the coachmark's anchor rect in sync with window resizes while it's open.
  useEffect(() => {
    if (!showTemplatesNudge) return;
    function onResize() {
      const el = templatesPillRef.current;
      if (el) setTemplatesPillRect(el.getBoundingClientRect());
    }
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [showTemplatesNudge]);

  // ── Template import (additive) ─────────────────────────────
  const handleImportTemplate = useCallback((templateAssignments) => {
    const newAssignments = { ...assignments };
    let added = 0;
    let skipped = 0;
    for (const [key, value] of Object.entries(templateAssignments)) {
      if (newAssignments[key]) {
        skipped++;
      } else {
        newAssignments[key] = value;
        added++;
      }
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    syncEngine(newAssignments, activeProfile);
    return { added, skipped };
  }, [assignments, profiles, activeProfile, saveConfig, syncEngine]);

  // ── CAD template import (creates app-specific profile) ─────
  const handleImportCadTemplate = useCallback((exeName, expansionAssignments, bareKeyAssignments) => {
    const profileName = `CAD — ${exeName}`;
    // Create the profile if it doesn't exist
    const newProfiles = profiles.includes(profileName) ? [...profiles] : [...profiles, profileName];
    // Set up profile settings with linkedApp
    const newProfileSettings = { ...profileSettings, [profileName]: { linkedApp: exeName } };
    // Merge all assignments
    const newAssignments = { ...assignments };
    let added = 0;
    let skipped = 0;
    // Bare keys go into the CAD profile
    for (const [key, value] of Object.entries(bareKeyAssignments)) {
      if (newAssignments[key]) { skipped++; } else { newAssignments[key] = value; added++; }
    }
    const bareAdded = added;
    // Expansions are global
    for (const [key, value] of Object.entries(expansionAssignments)) {
      if (newAssignments[key]) { skipped++; } else { newAssignments[key] = value; added++; }
    }
    const expAdded = added - bareAdded;
    // Save everything
    setProfiles(newProfiles);
    setProfileSettings(newProfileSettings);
    setAssignments(newAssignments);
    window.electronAPI?.updateProfileSettings(newProfileSettings);
    window.electronAPI?.saveConfig({
      assignments: newAssignments, profiles: newProfiles, activeProfile, activeGlobalProfile,
      profileSettings: newProfileSettings, theme, expansionCategories, autocorrectEnabled,
      macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables,
    });
    syncEngine(newAssignments, activeProfile);
    return { added, skipped, profileName, bareAdded, expAdded };
  }, [assignments, profiles, profileSettings, activeProfile, activeGlobalProfile, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, globalVariables, syncEngine]);

  const handleExportConfig = useCallback(async () => {
    const result = await window.electronAPI?.exportConfig();
    if (result?.ok) {
      showNotification('Config exported successfully');
    } else if (result?.error) {
      showNotification(`Export failed: ${result.error}`, 'info');
    }
  }, [showNotification]);

  const handleImportConfig = useCallback(async () => {
    const result = await window.electronAPI?.importConfig();
    if (!result?.ok) {
      if (result?.error) showNotification(result.error, 'info');
      return;
    }
    const confirmed = window.confirm(
      'This will replace your current config with the imported backup.\nAre you sure?'
    );
    if (!confirmed) return;

    const cfg = result.config;
    // Nothing has touched disk yet — import_config only read + validated the
    // file. Commit now that the user has confirmed (Cancel above really
    // cancels; previously the file and last-known-good were already replaced).
    const commit = await window.electronAPI?.commitImportConfig?.(cfg);
    if (!commit?.ok) {
      showNotification(commit?.error || 'Could not write the imported config to disk.', 'error');
      return;
    }
    // Reset interaction state so the sidebar and MacroPanel start clean
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setActiveModifiers([]);
    const imported = cfg.assignments || {};
    const importedHotkeyCount    = Object.keys(imported).filter(k => !k.startsWith('GLOBAL::EXPANSION::')).length;
    const importedExpansionCount = Object.keys(imported).length - importedHotkeyCount;
    console.log(`[Keyfire] Import applied — ${Object.keys(imported).length} assignments (${importedHotkeyCount} hotkeys, ${importedExpansionCount} expansions)`);
    // Apply EVERY section to state + engine (not just assignments/profiles/
    // theme) so the next full-object save can't write stale variables/
    // templates/radial back.
    applyLoadedConfig(cfg, { useSavedActiveProfile: true });
    setBackupRestoredFrom(null);
    showNotification('Config imported successfully');
    window.electronAPI?.hideSettingsWindow();
  }, [showNotification, applyLoadedConfig]);

  const handleRestoreBackup = useCallback(async (filename) => {
    const result = await window.electronAPI?.restoreBackup(filename);
    if (!result?.ok) {
      showNotification(result?.error || 'Restore failed', 'info');
      return;
    }
    const cfg = result.config;
    // Reset interaction state so the sidebar and MacroPanel start clean
    setSelectedKey(null);
    setSelectedLibraryId(null);
    setActiveModifiers([]);
    // Same full-section apply as Import Config (see applyLoadedConfig).
    applyLoadedConfig(cfg, { useSavedActiveProfile: true });
    window.electronAPI?.saveConfig({ ...cfg, hasSeenWelcome: true });
    setBackupRestoredFrom(null);
    showNotification('Config restored from backup');
    window.electronAPI?.hideSettingsWindow();
  }, [showNotification, applyLoadedConfig]);

  // Whether the active profile has an app linked (enables Bare Keys mode)
  const profileLinked = !!(profileSettings[activeProfile]?.linkedApp);

  // True when at least one non-expansion, non-autocorrect assignment exists
  // (any profile/layer). Unassigned library entries don't count — a config
  // holding only unassigned actions still deserves the first-run hint,
  // because zero keys can actually fire.
  const hasAnyAssignments = Object.keys(assignments).some(
    k => !k.includes('::EXPANSION::') && !k.includes('::AUTOCORRECT::') && !isLibraryKey(k)
  );

  // Show tips only in keyboard/mapping view, not dismissed, and within first 7 days
  const showTips = !tipsHidden && activeArea === 'mapping' && activeView === 'keyboard' && (() => {
    if (!firstLaunchDate) return true;
    const days = (Date.now() - new Date(firstLaunchDate).getTime()) / 86400000;
    return days < 7;
  })();

  // Auto-updater listeners
  // phase: 'available' | 'downloading' | 'ready' | 'dismissed'
  useEffect(() => {
    if (!window.electronAPI) return;

    let fallbackTimer = null;

    const clearFallback = () => {
      if (fallbackTimer) { clearTimeout(fallbackTimer); fallbackTimer = null; }
    };

    window.electronAPI.onUpdateAvailable(({ version }) => {
      // Do NOT store downloadSize from the manifest — that's the full installer size (~114 MB),
      // not the differential download size. Real size comes from progress.total once download starts.
      setUpdateInfo({ version, percent: 0, bytesPerSecond: 0, total: 0, phase: 'available' });
    });

    window.electronAPI.onDownloadProgress(({ percent, transferred, total, bytesPerSecond }) => {
      setUpdateInfo(prev => {
        if (!prev) return prev;
        const updated = { ...prev, percent, transferred, total, bytesPerSecond };
        // At 100%, arm a 5-second fallback in case update-downloaded never fires
        if (percent >= 100 && prev.phase === 'downloading') {
          clearFallback();
          fallbackTimer = setTimeout(() => {
            setUpdateInfo(cur => cur && cur.phase !== 'ready' ? { ...cur, phase: 'ready' } : cur);
          }, 5000);
        }
        return updated;
      });
    });

    window.electronAPI.onUpdateDownloaded(() => {
      clearFallback();
      setUpdateInfo(prev => prev ? { ...prev, phase: 'ready' } : prev);
    });

    return () => {
      clearFallback();
      window.electronAPI.removeAllListeners('update-available');
      window.electronAPI.removeAllListeners('download-progress');
      window.electronAPI.removeAllListeners('update-downloaded');
    };
  }, []);

  // Count assignments for current profile (all combos, excluding expansions
  // and unassigned library entries — those have no trigger to count)
  const profileAssignmentCount = Object.keys(assignments)
    .filter(k => k.startsWith(activeProfile + '::') && !k.includes('::EXPANSION::') && !isLibraryKey(k)).length;

  // ── Update banner helpers ─────────────────────────────────
  function fmtBytes(bytes) {
    if (!bytes || bytes <= 0) return null;
    if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
    if (bytes >= 1024 * 1024)        return `${Math.round(bytes / (1024 * 1024))} MB`;
    return `${Math.round(bytes / 1024)} KB`;
  }

  function fmtEta(bytesRemaining, bytesPerSecond) {
    if (!bytesPerSecond || bytesPerSecond <= 0 || !bytesRemaining) return null;
    const secs = Math.round(bytesRemaining / bytesPerSecond);
    if (secs < 5)   return 'almost done';
    if (secs < 60)  return `${secs}s remaining`;
    const mins = Math.ceil(secs / 60);
    return `${mins} min remaining`;
  }

  // ── Settings window bridge ─────────────────────────────────────────────
  // The standalone Settings window (?settings=1) is a remote control: this
  // window stays the single owner of settings state + config persistence.
  // State flows out as "settings-state" broadcasts; the settings window sends
  // fire-and-forget "settings-action" { action, args } events back, dispatched
  // to the existing handlers below. Refs are refreshed every render so the
  // once-registered listeners never see stale closures.
  const settingsStateRef = useRef(null);
  const settingsActionsRef = useRef({});
  useEffect(() => {
    settingsStateRef.current = {
      macrosEnabledOnStartup,
      physicalKeyboardLayout,
      resolvedPhysicalLayout,
      expansionExcludedApps,
      globalInputMethod,
      macroSpeed,
      keystrokeDelay,
      macroTriggerDelay,
      doubleTapWindow,
      holdThresholdMs,
      fireOnPress,
      defaultDateFormat,
      searchOverlayHotkey,
      searchOverlayEnabled,
      overlayShowAll,
      overlayCloseAfterFiring,
      overlayIncludeAutocorrect,
      globalPauseToggleKey,
      voiceEnabled,
      voiceHotkey,
      hiddenTipsCount: hiddenTips.length + (tipsHidden ? 1 : 0),
      activeProfile,
      isPro,
      licenceStatus,
      clipboardCaptureEnabled,
      clipboardExcludedApps,
      clipboardPasteHotkey,
      telemetryEnabled,
      theme: resolvedTheme,
    };
    // Modal-opening actions pull the main window forward first — the modal
    // renders here, and the user is looking at the settings window.
    const focusMain = () => window.electronAPI?.showMainWindow?.();
    settingsActionsRef.current = {
      toggleMacrosOnStartup: handleToggleMacrosOnStartup,
      setPhysicalKeyboardLayout: handleSetPhysicalKeyboardLayout,
      exportConfig: handleExportConfig,
      importConfig: handleImportConfig,
      restoreBackup: handleRestoreBackup,
      updateExpansionExcludedApps: handleUpdateExpansionExcludedApps,
      updateGlobalSettings: handleUpdateGlobalSettings,
      updateSearchSettings: handleUpdateSearchSettings,
      setPauseKey: handleSetPauseKey,
      clearPauseKey: handleClearPauseKey,
      toggleVoiceEnabled: handleToggleVoiceEnabled,
      setVoiceKey: handleSetVoiceKey,
      clearVoiceKey: handleClearVoiceKey,
      restartOnboarding: () => { focusMain(); handleRestartOnboarding(); },
      replayWelcome: () => { focusMain(); handleReplayWelcome(); },
      resetHiddenTips: handleResetHiddenTips,
      importTemplate: handleImportTemplate,
      importCadTemplate: handleImportCadTemplate,
      licenceStatusChange: (s) => setLicenceStatus(s),
      showUpgrade: (...a) => { focusMain(); showUpgrade(...a); },
      // Dev-only: fresh 14 days, announcement + end modal re-armed (Rust).
      resetTrial: () => {
        window.electronAPI?.resetTrial?.().then((s) => {
          if (s) setLicenceStatus(s);
        });
      },
      toggleClipboardCapture: handleToggleClipboardCapture,
      updateClipboardExcludedApps: handleUpdateClipboardExcludedApps,
      setClipboardPasteKey: handleSetClipboardPasteKey,
      clearClipboardPasteKey: handleClearClipboardPasteKey,
      toggleTelemetry: handleToggleTelemetry,
    };
  });
  // Live-sync: broadcast whenever any bridged value changes (the ref-refresh
  // effect above is declared first, so the payload here is always fresh).
  useEffect(() => {
    emitEvent('settings-state', settingsStateRef.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    macrosEnabledOnStartup, expansionExcludedApps, globalInputMethod,
    macroSpeed, keystrokeDelay, macroTriggerDelay, doubleTapWindow,
    holdThresholdMs, fireOnPress, defaultDateFormat, searchOverlayHotkey, searchOverlayEnabled,
    overlayShowAll, overlayCloseAfterFiring, overlayIncludeAutocorrect,
    globalPauseToggleKey, voiceEnabled, voiceHotkey, hiddenTips, tipsHidden,
    activeProfile, isPro, licenceStatus, clipboardCaptureEnabled,
    clipboardExcludedApps, clipboardPasteHotkey, telemetryEnabled,
    resolvedTheme,
  ]);
  useEffect(() => {
    const unlisteners = [];
    const respond = () => emitEvent('settings-state', settingsStateRef.current);
    listenEvent('settings-request-state', respond).then(u => unlisteners.push(u));
    listenEvent('settings-shown', respond).then(u => unlisteners.push(u));
    listenEvent('settings-action', (e) => {
      const { action, args = [] } = e.payload || {};
      const fn = settingsActionsRef.current[action];
      if (fn) fn(...args);
      else console.warn('[SettingsBridge] unknown action:', action);
    }).then(u => unlisteners.push(u));
    return () => unlisteners.forEach(u => { if (typeof u === 'function') u(); });
  }, []);

  //    Dev-only UI test bridge ("Claude Test")                              
  // Exposes App-level setters to src/devBridge.js so scripts/ui-shot.ps1 can
  // drive the running dev app deterministically (no screen-coordinate clicks).
  // import.meta.env.DEV is a build-time constant, so this block is dropped from
  // production bundles entirely. Add setters here as tests need them.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    window.__kf_dev = {
      ...(window.__kf_dev || {}),
      setArea: handleSetArea,                       // ('mapping'|'expansions'|'templates'|'clipboard'|'analytics', view?)
      setView: handleSetView,                       // ('keyboard'|'mouse'|'radial')
      setTheme: handleSetTheme,                     // ('auto'|'light'|'dark') — persists like the menu does
      setListView: (on) => setListViewActive(!!on),
      toggleListView: handleToggleListView,
      selectKey: handleKeySelect,                   // (keyId) as the canvas would
      clearSelection: () => { setSelectedKey(null); setSelectedLibraryId(null); setSelectedRadialSegment(null); },
      setModifiers: (mods) => setActiveModifiers(Array.isArray(mods) ? mods : []),
      setProfile: handleProfileChange,
      setRadialLayout: handleSelectRadialLayout,    // (layoutId | 'default') active radial for this device (edit + fire)
      getRadialLayouts: () => radialLayouts.map(l => ({ id: l.id, name: l.name })),
      hideTip: handleHideTip,
      setHiddenTips: (keys) => setHiddenTips(Array.isArray(keys) ? keys : []),
      setPro: (pro) => window.electronAPI?.devSetProOverride(pro),  // true | false | null (clear)
      // Trial modals for UI testing. showTrialEnd(usage?) opens the end modal
      // with a mock `get_trial_usage` payload ({ triggers:[{trigger_key,
      // action_type,count}], autocorrect }); omit for the generic fallback.
      // Neither touches the persisted trial flags.
      showTrialAnnounce: () => setShowProTrialModal(true),
      showTrialEnd: (usage) => { setTrialUsage(usage || { triggers: [], autocorrect: 0 }); setShowTrialEndModal(true); },
      hideTrialEnd: () => { setShowTrialEndModal(false); trialEndOpenRef.current = false; },
      setPhysicalLayout: handleSetPhysicalKeyboardLayout, // ('auto'|'ansi'|'iso') persists like Settings does
      getAssignments: () => assignments,             // read-only snapshot of the storage map
      friendlyKeyName,                                // live instance (consults Windows-layout legends)
      assign: (keyId, macro, mode = 'single') => (mode === 'double' ? handleAssignDouble : mode === 'hold' ? handleAssignHold : handleAssign)(keyId, macro),
      deleteKey: handleDeleteKey,                   // (keyId) all variants on the active combo
      getState: () => ({
        activeArea, activeView, theme, resolvedTheme, listViewActive, selectedKey, selectedLibraryId,
        selectedRadialSegment, activeModifiers, activeProfile, isPro, macrosEnabled, hiddenTips,
        editingRadialLayoutId, deviceRadialLayoutId, radialLayoutCount: radialLayouts.length,
        physicalKeyboardLayout, resolvedPhysicalLayout, isoKeyDetected, keyboardLayoutHint,
        keyboardLegends,
        width: window.innerWidth, height: window.innerHeight, dpr: window.devicePixelRatio,
      }),
    };
  });

  return (
    <div className="app">
      {showOnboarding && (
        <OnboardingTour
          assignments={assignments}
          onComplete={handleOnboardingComplete}
          onSkip={handleOnboardingComplete}
          onAreaChange={handleSetArea}
          onShowUpgrade={showUpgrade}
          searchOverlayHotkey={searchOverlayHotkey}
          searchOverlayEnabled={searchOverlayEnabled}
          clipboardPasteHotkey={clipboardPasteHotkey || null}
          radialMenuHotkey={radialMenuHotkey || null}
          globalPauseToggleKey={globalPauseToggleKey || null}
        />
      )}
      {showWelcome && !showOnboarding && (
        <WelcomeModal
          onContinue={handleWelcomeContinue}
          onSkip={handleWelcomeSkip}
          onDismiss={handleDismissWelcome}
          searchOverlayHotkey={searchOverlayEnabled ? searchOverlayHotkey : null}
          clipboardPasteHotkey={clipboardPasteHotkey || null}
        />
      )}
      {upgradePrompt && (
        <UpgradeModal
          featureName={upgradePrompt}
          onClose={() => setUpgradePrompt(null)}
          onOpenSettings={() => window.electronAPI?.showSettingsWindow('licence')}
        />
      )}
      {appMissingModal && (
        <AppNotRunningModal
          exe={appMissingModal.exe}
          hint={appMissingModal.hint}
          onClose={() => setAppMissingModal(null)}
        />
      )}
      {pendingCanvasDrop && (
        <DropConfirmModal
          drop={pendingCanvasDrop}
          onCancel={() => setPendingCanvasDrop(null)}
          onConfirm={() => {
            const p = pendingCanvasDrop;
            setPendingCanvasDrop(null);
            if (p.mode === 'bind') handleBindLibrary(p.id, p.targetCombo, p.targetKeyId);
            else moveAssignment(p.srcCombo, p.srcKeyId, p.targetCombo, p.targetKeyId);
          }}
        />
      )}
      {reservedShortcutPending && (
        <ReservedShortcutModal
          comboDisplay={reservedShortcutPending.comboDisplay}
          osFunction={reservedShortcutPending.osFunction}
          profileName={reservedShortcutPending.profileName}
          onContinue={() => {
            commitKeySelect(reservedShortcutPending.keyId);
            setReservedShortcutPending(null);
          }}
          onCancel={() => setReservedShortcutPending(null)}
        />
      )}
      {showProTrialModal && (
        <ProTrialModal
          onClose={() => {
            setShowProTrialModal(false);
            window.electronAPI?.markTrialOfferShown?.().then((s) => {
              if (s) setLicenceStatus(s);
            });
          }}
        />
      )}
      {showTrialEndModal && (
        <TrialEndModal
          usage={trialUsage}
          assignments={assignments}
          profileSettings={profileSettings}
          radialLayouts={radialLayouts}
          sharedActive={!!gracePeriodState?.shared_active}
          onKeepPro={() => { closeTrialEnd(); showUpgrade('Keep Keyfire Pro'); }}
          onClose={closeTrialEnd}
        />
      )}
      {showTemplatesNudge && (
        <TemplatesCoachmark
          anchorRect={templatesPillRect}
          onOpenTemplates={handleOpenTemplatesFromCoachmark}
          onDismiss={handleDismissTemplatesNudge}
        />
      )}
      {backupRestoredFrom && (
        <div className="backup-restored-banner">
          <span className="backup-restored-icon">⚠</span>
          <span className="backup-restored-text">
            Config was restored from backup
            {backupRestoredFrom === 'keyforge-config-last-known-good.json'
              ? ' (last known good)'
              : ` (${backupRestoredFrom.replace('keyforge-config-', '').replace('.json', '')})`
            }.
            Your most recent changes may not be included.
          </span>
          <button
            className="backup-restored-dismiss"
            onClick={() => setBackupRestoredFrom(null)}
            type="button"
          >Dismiss</button>
        </div>
      )}
      {gracePeriodState?.pro_expired_at && gracePeriodState?.shared_active && (
        <div className={`grace-banner${gracePeriodState.migration_deferred ? ' grace-banner--deferred' : (gracePeriodState.days_remaining ?? 7) <= 2 ? ' grace-banner--urgent' : ''}`}>
          <span className="grace-banner-icon">⚠</span>
          <span className="grace-banner-text">
            {gracePeriodState.migration_deferred ? (
              <>Couldn't reach your shared config file to move it to local right now. Your data is safe — Keyfire is using a local snapshot and will keep retrying until the shared file is reachable again. The shared file in your cloud folder is never modified or deleted by Keyfire.</>
            ) : (gracePeriodState.days_remaining ?? 7) <= 0 ? (
              <>Your Pro grace period has ended. Keyfire will move your shared config to local on next restart.</>
            ) : (
              <>Pro is required for shared config. Sync continues for {gracePeriodState.days_remaining} more day{gracePeriodState.days_remaining === 1 ? '' : 's'}, then your config will move to local storage automatically.</>
            )}
          </span>
          <div className="grace-banner-actions">
            <button
              type="button"
              className="grace-banner-btn grace-banner-btn--primary"
              onClick={() => showUpgrade('Shared config (cross-machine sync)')}
            >Upgrade</button>
            <button
              type="button"
              className="grace-banner-btn"
              onClick={handleMigrateSharedToLocalNow}
            >Switch to local now</button>
          </div>
        </div>
      )}
      {postMigrationNotice && (
        <div className="grace-banner grace-banner--info">
          <span className="grace-banner-icon">ⓘ</span>
          <span className="grace-banner-text">
            Your shared config has been moved to local storage. Re-enable Pro any time to resume cross-machine sync.
          </span>
          <button
            type="button"
            className="grace-banner-dismiss"
            onClick={() => setPostMigrationNotice(false)}
          >Dismiss</button>
        </div>
      )}
      {updateInfo && updateInfo.phase !== 'dismissed' && (() => {
        // Only show size once download-progress fires and progress.total is known — that is the
        // real (possibly differential) download size, not the full installer size from the manifest.
        const displaySize = fmtBytes(updateInfo.total);
        const eta          = fmtEta(
          (updateInfo.total || 0) - (updateInfo.transferred || 0),
          updateInfo.bytesPerSecond
        );
        return (
          <div className="update-banner">
            {updateInfo.phase === 'ready' ? (
              <>
                <span className="update-banner__text">Keyfire {updateInfo.version} ready — click to install and relaunch</span>
                <button
                  className="update-banner__btn update-banner__btn--restart"
                  // CRITICAL — DO NOT MODIFY: must be fire-and-forget, no await, no state changes
                  onClick={() => { window.electronAPI?.installUpdate(); }}
                  type="button"
                >Restart Now</button>
                <button
                  className="update-banner__btn update-banner__btn--later"
                  onClick={() => setUpdateInfo(prev => ({ ...prev, phase: 'dismissed' }))}
                  type="button"
                >Later</button>
              </>
            ) : updateInfo.phase === 'downloading' ? (
              <>
                <span className="update-banner__text">
                  Downloading Keyfire {updateInfo.version}
                  {displaySize ? ` — ${displaySize}` : ''}
                  {eta ? ` · ${eta}` : ''}
                </span>
                <span className="update-banner__progress">
                  <span
                    className="update-banner__progress-bar"
                    style={{ width: `${Math.round(updateInfo.percent)}%` }}
                  />
                </span>
                <span className="update-banner__pct">{Math.round(updateInfo.percent)}%</span>
              </>
            ) : (
              <>
                <span className="update-banner__text">
                  Keyfire {updateInfo.version} available
                </span>
                <button
                  className="update-banner__btn update-banner__btn--restart"
                  onClick={() => {
                    console.log('[UpdateBanner] Download clicked — updateInfo.version:', updateInfo?.version, '| full updateInfo:', JSON.stringify(updateInfo));
                    setUpdateInfo(prev => ({ ...prev, phase: 'downloading' }));
                    window.electronAPI?.startDownload(updateInfo.version);
                  }}
                  type="button"
                >Download &amp; Install</button>
                <button
                  className="update-banner__btn update-banner__btn--later"
                  onClick={() => setUpdateInfo(prev => ({ ...prev, phase: 'dismissed' }))}
                  type="button"
                >Later</button>
              </>
            )}
          </div>
        );
      })()}
      <TitleBar
        macrosEnabled={macrosEnabled}
        onToggleMacros={handleToggleMacros}
        theme={theme}
        resolvedTheme={resolvedTheme}
        onSetTheme={handleSetTheme}
        onOpenSettings={() => window.electronAPI?.toggleSettingsWindow()}
        activeArea={activeArea}
        onAreaChange={handleSetArea}
        listViewActive={listViewActive}
        onToggleListView={handleToggleListView}
        activeProfile={activeProfile}
        onImportTemplate={handleImportTemplate}
        onImportCadTemplate={handleImportCadTemplate}
        onShowNotification={showNotification}
        templatesPillRef={templatesPillRef}
        templatesPillPulse={showTemplatesNudge}
        openTemplatesSignal={openTemplatesSignal}
      />
      <DndContext
        sensors={radialSensors}
        collisionDetection={pointerWithin}
        onDragStart={handleRadialDragStart}
        onDragMove={handleRadialDragMove}
        onDragOver={handleCanvasDragOver}
        onDragEnd={handleRadialDragEnd}
        onDragCancel={handleRadialDragCancel}
      >
      <div className="app-body">
        {activeArea === 'mapping' && (
          /* Triggers frame: the same contained card shell as Text Expansions /
             Quick Search / Clipboard (.text-expansions / .stp-panel / .cbg-panel).
             The header row carries the Keyboard | Mouse | Radial mode tabs
             top-left; the row below holds the profile Sidebar, the canvas the
             keyboard / mouse / radial editor paint on, and the MacroPanel editor. */
          <div className="main-area trig-shell">
          <div className="trig-panel">
          {!listViewActive && (
          <div className="trig-header">
            <div className="view-switcher">
              <button
                className={`view-tab${activeView === 'keyboard' ? ' active' : ''}`}
                onClick={() => handleSetView('keyboard')}
                type="button"
                title="Keyboard"
                aria-label="Keyboard"
              >
                <span className="view-tab-icon" aria-hidden="true">⌨</span>
                <span className="view-tab-label">Keyboard</span>
              </button>
              <button
                className={`view-tab${activeView === 'mouse' ? ' active' : ''}`}
                onClick={() => handleSetView('mouse')}
                type="button"
                title="Mouse"
                aria-label="Mouse"
              >
                <span className="view-tab-icon" aria-hidden="true">🖱</span>
                <span className="view-tab-label">Mouse</span>
              </button>
              <button
                className={`view-tab${activeView === 'radial' ? ' active' : ''}`}
                onClick={() => handleSetView('radial')}
                type="button"
                title="Radial"
                aria-label="Radial"
              >
                <span className="view-tab-icon" aria-hidden="true">&#x25ce;</span>
                <span className="view-tab-label">Radial</span>
              </button>
            </div>
          {!hiddenTips.includes('profile-link') && (
            <div className="profile-link-tip" title="Right-click any profile in the sidebar to link it to a specific app. Keyfire auto-switches profiles when that app gains focus.">
              <span className="profile-link-tip-badge">TIP</span>
              <span className="profile-link-tip-text">
                Right-click any profile in the sidebar to link it to a specific app. Keyfire auto-switches profiles when that app gains focus.
              </span>
              <button type="button" className="profile-link-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => handleHideTip('profile-link')}>&#10005;</button>
            </div>
          )}
          </div>
          )}
          <div className="trig-row">
          <Sidebar
            activeProfile={activeProfile}
            assignments={assignments}
            activeModifiers={activeModifiers}
            sidebarComboFilter={sidebarComboFilter}
            currentCombo={currentCombo}
            selectedKey={selectedKey}
            onSelectAssignment={handleSelectAssignment}
            onSelectCombo={handleSelectCombo}
            profileLinked={profileLinked}
            profiles={profiles}
            activeGlobalProfile={activeGlobalProfile}
            profileSettings={profileSettings}
            onProfileChange={handleProfileChange}
            onAddProfile={handleAddProfile}
            onRenameProfile={handleRenameProfile}
            onDeleteProfile={handleDeleteProfile}
            onReorderProfiles={handleReorderProfiles}
            onDuplicateProfile={handleDuplicateProfile}
            onSetActiveGlobalProfile={handleSetActiveGlobalProfile}
            onUpdateProfileSettings={handleUpdateProfileSettings}
            onExportProfile={handleExportProfile}
            onImportProfile={handleImportProfile}
            importPrompt={importPrompt}
            onImportProfileResolve={handleImportProfileResolve}
            onImportPromptDismiss={() => setImportPrompt(null)}
            listViewActive={listViewActive}
            isRecording={isRecording}
            onStartRecord={handleStartRecord}
            onStopRecord={handleStopRecord}
            recordCapture={recordCapture}
            onToggleModifier={handleToggleModifier}
            onRenameAssignment={handleRenameAssignment}
            onClearAssignment={handleClearAssignment}
            onDuplicateFromContext={handleDuplicateFromContext}
            onCopyToProfile={handleCopyToProfile}
            onMoveToProfile={handleMoveToProfile}
            selectedLibraryId={selectedLibraryId}
            onSelectLibraryEntry={handleSelectLibraryEntry}
            onBindFromContext={handleBindFromContext}
            onDuplicateLibraryInPlace={handleDuplicateLibraryInPlace}
            onUnassign={handleUnassignKey}
            activeView={activeView}
            radialMenuItems={radialMenuItems}
            isPro={isPro}
            onShowUpgrade={showUpgrade}
          />
          <main className={`main-area trig-canvas trig-canvas--${activeView}${listViewActive ? ' main-area--hidden' : ''}`}>
          {activeView === 'keyboard' && !listViewActive && (
            <div className="keyboard-numpad-wrap">
              <KeyboardCanvas
                selectedKey={selectedKey}
                physicalLayout={resolvedPhysicalLayout}
                legends={keyboardLegends}
                onKeySelect={handleKeySelect}
                getKeyAssignment={getKeyAssignment}
                getDoubleAssignment={getDoubleAssignment}
                getHoldAssignment={getHoldAssignment}
                lastFired={lastFired}
                activeModifiers={activeModifiers}
                onToggleModifier={handleToggleModifier}
                profileLinked={profileLinked}
                isRecording={isRecording}
                onStartRecord={handleStartRecord}
                onStopRecord={handleStopRecord}
                recordCapture={recordCapture}
                hasAnyAssignments={hasAnyAssignments}
                currentCombo={currentCombo}
                onRenameAssignment={handleRenameAssignment}
                onClearAssignment={handleClearAssignment}
                onDuplicateFromContext={handleDuplicateFromContext}
                onUnassign={handleUnassignKey}
                profiles={profiles}
                activeProfile={activeProfile}
                onCopyToProfile={handleCopyToProfile}
                onMoveToProfile={handleMoveToProfile}
                onNewShortcut={handleNewShortcut}
                newTriggerHint={newTriggerHint}
                bindDragActive={!!bindActiveDrag}
              />
            </div>
          )}
          {showTips && (
            <QuickTips onDismiss={handleDismissTips} searchOverlayHotkey={searchOverlayHotkey} searchOverlayEnabled={searchOverlayEnabled} />
          )}
          {activeView === 'mouse' && (
            <MouseCanvas
              selectedKey={selectedKey}
              onKeySelect={handleKeySelect}
              getKeyAssignment={getKeyAssignment}
              hasDoubleAssignment={hasDoubleAssignment}
              hasHoldAssignment={hasHoldAssignment}
              lastFired={lastFired}
              activeModifiers={activeModifiers}
              onToggleModifier={handleToggleModifier}
              profileLinked={profileLinked}
              onAddProfile={handleAddProfile}
              isRecording={isRecording}
              onStartRecord={handleStartRecord}
              onStopRecord={handleStopRecord}
              recordCapture={recordCapture}
              onNewShortcut={handleNewShortcut}
              newTriggerHint={newTriggerHint}
              bindDragActive={!!bindActiveDrag}
            />
          )}
            {activeView === 'radial' && (
              <Suspense fallback={null}>
              <RadialEditorView
                hiddenTips={hiddenTips}
                onHideTip={handleHideTip}
                radialMenuHotkey={radialMenuHotkey}
                onSetRadialMenuHotkey={handleSetRadialMenuHotkey}
                onClearRadialMenuHotkey={handleClearRadialMenuHotkey}
                radialHoldToSelect={radialHoldToSelect}
                onSetRadialHoldToSelect={handleSetRadialHoldToSelect}
                radialMenuItems={radialMenuItems}
                onAddRadialMenuItem={handleAddRadialMenuItem}
                onRemoveRadialMenuItem={handleRemoveRadialMenuItem}
                onReorderRadialMenuItems={handleReorderRadialMenuItems}
                onAddRadialMenuFolder={handleAddRadialMenuFolder}
                onAddChildToFolder={handleAddChildToFolder}
                onRemoveChildFromFolder={handleRemoveChildFromFolder}
                onMoveItemToFolder={handleMoveItemToFolder}
                onMoveChildToMain={handleMoveChildToMain}
                onReorderFolderChildren={handleReorderFolderChildren}
                onRenameFolder={handleRenameFolder}
                onRenameRadialMenuItem={handleRenameRadialMenuItem}
                onRenameChildInFolder={handleRenameChildInFolder}
                onSwapRadialMenuItems={handleSwapRadialMenuItems}
                onCreateRadialAction={handleCreateRadialAction}
                onSetRadialMenuItemIcon={handleSetRadialMenuItemIcon}
                onSetRadialChildIcon={handleSetRadialChildIcon}
                selectedRadialSegment={selectedRadialSegment}
                onSelectRadialSegment={handleSelectRadialSegment}
                onSelectRadialChild={(folderId, childIndex) => {
                  setSelectedRadialChild({ folderId, childIndex });
                  setSelectedRadialSegment(null);
                  setSelectedKey(null);
                }}
                assignments={assignments}
                dropTargetIndex={radialActiveDrag ? radialDropTarget : -1}
                dropTargetOuterIndex={radialActiveDrag ? radialDropTargetOuter : -1}
                rejectIndex={radialRejectIndex}
                wheelRef={wheelRef}
                usedKeys={radialUsedKeys}
                expandedFolder={expandedRadialFolder}
                onExpandedFolderChange={setExpandedRadialFolder}
                profiles={profiles}
                activeProfile={activeProfile}
                onCopyRadialSegmentToProfile={handleCopyRadialSegmentToProfile}
                onForceOverwriteRadialSegment={handleForceOverwriteRadialSegment}
                radialLayouts={radialLayouts}
                editingRadialLayoutId={effectiveEditingLayoutId}
                onSelectRadialLayout={handleSelectRadialLayout}
                onCreateRadialLayout={handleCreateRadialLayout}
                onRenameRadialLayout={handleRenameRadialLayout}
                onDeleteRadialLayout={handleDeleteRadialLayout}
                isPro={isPro}
                onShowUpgrade={showUpgrade}
              />
              </Suspense>
            )}
          </main>
        {/* Right panel: MacroPanel (Settings lives in its own window) */}
        {activeView === 'radial' && selectedRadialChild != null ? (
          <MacroPanel
            selectedKey={'Folder Child'}
            activeModifiers={[]}
            currentCombo=""
            assignment={radialChildAssignment}
            doubleAssignment={null}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleRadialChildAssign}
            onClear={handleRadialChildClear}
            onClose={() => setSelectedRadialChild(null)}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
            hiddenTips={hiddenTips}
            onHideTip={handleHideTip}
          />
        ) : activeView === 'radial' && selectedRadialSegment != null ? (
          <MacroPanel
            selectedKey={'Radial Segment'}
            activeModifiers={[]}
            currentCombo=""
            assignment={radialSegmentAssignment}
            doubleAssignment={null}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleRadialAssign}
            onClear={handleRadialClear}
            onClose={() => setSelectedRadialSegment(null)}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
            hiddenTips={hiddenTips}
            onHideTip={handleHideTip}
          />
        ) : selectedLibraryId ? (
          // Unassigned library editor — same MacroPanel, library mode. The
          // uuid rides in selectedKey so the save/select plumbing works
          // unchanged; App routes every assign/clear callback to the
          // {Profile}::UNASSIGNED::{uuid} storage keys.
          <MacroPanel
            libraryMode={true}
            selectedKey={selectedLibraryId}
            activeModifiers={[]}
            currentCombo="UNASSIGNED"
            assignment={getLibraryEntry(selectedLibraryId)}
            doubleAssignment={getLibraryEntry(selectedLibraryId, '::double')}
            holdAssignment={getLibraryEntry(selectedLibraryId, '::hold')}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={(id, macro) => handleAssignLibraryVariant(id, macro, '')}
            onClear={(id) => handleClearLibraryVariant(id, '')}
            onDelete={handleDeleteLibraryEntry}
            onAssignDouble={(id, macro) => handleAssignLibraryVariant(id, macro, '::double')}
            onClearDouble={(id) => handleClearLibraryVariant(id, '::double')}
            onAssignHold={(id, macro) => handleAssignLibraryVariant(id, macro, '::hold')}
            onClearHold={(id) => handleClearLibraryVariant(id, '::hold')}
            onMoveVariant={handleMoveLibraryVariant}
            onClose={() => setSelectedLibraryId(null)}
            onCancelDraft={() => {}}
            onReassign={(combo, keyId) => handleBindLibrary(selectedLibraryId, combo, keyId)}
            onDuplicate={(combo, keyId) => handleDuplicateLibraryToKey(selectedLibraryId, combo, keyId)}
            duplicateOverlaySignal={duplicateOverlaySignal}
            bindOverlaySignal={bindOverlaySignal}
            isPro={isPro}
            voiceEnabled={false}
            onShowUpgrade={showUpgrade}
            hiddenTips={hiddenTips}
            onHideTip={handleHideTip}
          />
        ) : (!isNarrow || selectedKey != null || draftAssignment != null) ? (
          // Below 1200px the MacroPanel is hidden until the user picks a key or
          // starts a draft. The selection / draft check covers the keyboard +
          // mouse views; the radial branches above have their own guards on
          // selectedRadialSegment / selectedRadialChild so they're unaffected.
          <MacroPanel
            selectedKey={selectedKey}
            activeModifiers={activeModifiers}
            currentCombo={currentCombo}
            assignment={selectedKey ? getKeyAssignment(selectedKey) : null}
            doubleAssignment={selectedKey ? getDoubleAssignment(selectedKey) : null}
            holdAssignment={selectedKey ? getHoldAssignment(selectedKey) : null}
            draftAssignment={draftAssignment}
            draftDoubleAssignment={draftDoubleAssignment}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleAssign}
            onClear={handleClearKey}
            onDelete={handleDeleteKey}
            onAssignDouble={handleAssignDouble}
            onClearDouble={handleClearDouble}
            onAssignHold={handleAssignHold}
            onClearHold={handleClearHold}
            onMoveVariant={handleMovePressVariant}
            onClose={() => { clearDraft(); setSelectedKey(null); }}
            onCancelDraft={clearDraft}
            onReassign={handleReassign}
            onDuplicate={handleDuplicateAssignment}
            onUnassign={(keyId) => handleUnassignKey(currentCombo, keyId)}
            onNewLibraryAction={handleNewLibraryAction}
            duplicateOverlaySignal={duplicateOverlaySignal}
            bindOverlaySignal={bindOverlaySignal}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
            hiddenTips={hiddenTips}
            onHideTip={handleHideTip}
          />
        ) : null}
          </div>
          </div>
          </div>
        )}
        {activeArea !== 'mapping' && (
        <main className="main-area main-area--expansions">
          {showTips && (
            <QuickTips onDismiss={handleDismissTips} searchOverlayHotkey={searchOverlayHotkey} searchOverlayEnabled={searchOverlayEnabled} />
          )}
          {activeArea === 'analytics' && (
            <Suspense fallback={null}>
              <AnalyticsPanel isPro={isPro} onShowUpgrade={showUpgrade} />
            </Suspense>
          )}
          {activeArea === 'clipboard' && (
            <Suspense fallback={null}>
            <ClipboardPanel
              hiddenTips={hiddenTips}
              onHideTip={handleHideTip}
              clipboardPasteHotkey={clipboardPasteHotkey}
              previewWidth={clipboardPreviewWidth}
              onChangePreviewWidth={(w) => {
                const clamped = Math.max(320, Math.min(1200, Math.round(w)));
                setClipboardPreviewWidth(clamped);
                window.electronAPI?.saveConfig({ clipboardPreviewWidth: clamped });
              }}
              columnMode={clipboardColumnMode}
              onChangeColumnMode={(m) => {
                const next = (m === 'one' || m === 'two') ? m : 'auto';
                setClipboardColumnMode(next);
                window.electronAPI?.saveConfig({ clipboardColumnMode: next });
              }}
              onCreateExpansion={handleCreateExpansionFromClip}
              isPro={isPro}
              onShowUpgrade={showUpgrade}
            />
            </Suspense>
          )}
          {activeArea === 'templates' && (
            <Suspense fallback={null}>
            <SearchTemplatesPanel
              hiddenTips={hiddenTips}
              onHideTip={handleHideTip}
              searchTemplates={searchTemplates}
              categories={searchTemplateCategories}
              searchOverlayHotkey={searchOverlayHotkey}
              isPro={isPro}
              onAdd={handleAddSearchTemplate}
              onUpdate={handleUpdateSearchTemplate}
              onDelete={handleDeleteSearchTemplate}
              onAddCategory={handleAddSearchTemplateCategory}
              onRenameCategory={handleRenameSearchTemplateCategory}
              onDeleteCategory={handleDeleteSearchTemplateCategory}
              onUpdateCategoryColour={handleUpdateSearchTemplateCategoryColour}
              onReorderCategories={handleReorderSearchTemplateCategories}
              onMoveCategoryTo={handleMoveSearchTemplateCategoryTo}
              onMoveTemplateToCategory={handleMoveSearchTemplateToCategory}
              quickActions={Object.entries(assignments).filter(([k]) => k.startsWith('GLOBAL::QUICKACTION::')).map(([k, v]) => ({ id: k.slice('GLOBAL::QUICKACTION::'.length), ...v }))}
              onAddQuickAction={handleAddQuickAction}
              onUpdateQuickAction={handleUpdateQuickAction}
              onDeleteQuickAction={handleDeleteQuickAction}
              qaCategories={quickActionCategories}
              onAddQaCategory={handleAddQaCategory}
              onRenameQaCategory={handleRenameQaCategory}
              onDeleteQaCategory={handleDeleteQaCategory}
              onUpdateQaCategoryColour={handleUpdateQaCategoryColour}
              onReorderQaCategories={handleReorderQaCategories}
              onMoveQaCategoryTo={handleMoveQaCategoryTo}
              onMoveQuickActionToCategory={handleMoveQuickActionToCategory}
              onExportQuickActions={handleExportQuickActions}
              onImportQuickActions={handleImportQuickActions}
              quickActionImportPrompt={quickActionImportPrompt}
              onQuickActionImportResolve={handleQuickActionImportResolve}
              globalInputMethod={globalInputMethod}
              assignments={assignments}
              profiles={profiles}
              onShowNotification={showNotification}
              onShowUpgrade={showUpgrade}
              onEditingChange={setQuickActionEditing}
            />
            </Suspense>
          )}
          {activeArea === 'expansions' && (
            // Phase 3: Text Expansions will eventually support its own profile bar
            // for per-app or team expansion profiles.  For now a single global set.
            <Suspense fallback={null}>
            <TextExpansions
              hiddenTips={hiddenTips}
              onHideTip={handleHideTip}
              expansions={expansions}
              onAdd={handleAddExpansion}
              onDelete={handleDeleteExpansion}
              onDeleteMany={handleDeleteExpansionsBulk}
              categories={expansionCategories}
              onAddCategory={handleAddCategory}
              onDeleteCategory={handleDeleteCategory}
              onReorderCategories={handleReorderCategories}
              onUpdateCategoryColour={handleUpdateCategoryColour}
              onRenameCategory={handleRenameCategory}
              onMoveCategoryTo={handleMoveCategoryTo}
              onMoveExpansionToCategory={handleMoveExpansionToCategory}
              autocorrectEnabled={autocorrectEnabled}
              autocorrectBuiltinTypos={autocorrectBuiltinTypos}
              autocorrectDoubleCaps={autocorrectDoubleCaps}
              autocorrectDoubleCapsExceptions={autocorrectDoubleCapsExceptions}
              autocorrectCapsLockFix={autocorrectCapsLockFix}
              autocorrectSentenceCaps={autocorrectSentenceCaps}
              autocorrectExtendedTypos={autocorrectExtendedTypos}
              autocorrectDays={autocorrectDays}
              autocorrectSymbols={autocorrectSymbols}
              autocorrectEmojis={autocorrectEmojis}
              autocorrectExcludedApps={autocorrectExcludedApps}
              onUpdateAutocorrectSettings={handleUpdateAutocorrectSettings}
              autocorrections={autocorrections}
              onSaveAutocorrectGroup={handleSaveAutocorrectGroup}
              onDeleteAutocorrectGroup={handleDeleteAutocorrectGroup}
              autocorrectDisabledEntries={autocorrectDisabledEntries}
              acSuggestions={acSuggestions}
              onAcSuggestionResolve={handleAcSuggestionResolve}
              onExportAutocorrections={handleExportAutocorrections}
              onImportAutocorrections={handleImportAutocorrections}
              acImportPrompt={acImportPrompt}
              onAcImportResolve={handleAcImportResolve}
              globalVariables={globalVariables}
              onSaveGlobalVariables={handleSaveGlobalVariables}
              isPro={isPro}
              onShowUpgrade={showUpgrade}
              prefill={pendingExpansionPrefill}
              onPrefillConsumed={() => setPendingExpansionPrefill(null)}
              onExportExpansions={handleExportExpansions}
              onImportExpansions={handleImportExpansions}
              onImportExpansionsFrom={handleImportExpansionsFrom}
              expansionImportPrompt={expansionImportPrompt}
              onExpansionImportResolve={handleExpansionImportResolve}
              onEditingChange={setExpansionEditing}
            />
            </Suspense>
          )}
        </main>
        )}
      </div>
      <DragOverlay>
        {radialActiveDrag && (
          <div className="rmp-card rmp-card-overlay">
            <span className="rmp-card-label">{radialActiveDrag.label}</span>
          </div>
        )}
        {bindActiveDrag && (
          <div className="rmp-card rmp-card-overlay">
            <span className="rmp-card-label">{bindActiveDrag.label || 'Action'}</span>
          </div>
        )}
      </DragOverlay>
      </DndContext>
      <StatusBar
        selectedKey={selectedKey}
        currentCombo={currentCombo}
        macrosEnabled={macrosEnabled}
        assignmentCount={profileAssignmentCount}
        engineStatus={engineStatus}
        lastFired={lastFired}
        appVersion={appVersion}
        globalPauseToggleKey={globalPauseToggleKey}
      />
      <Toaster toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}

export default App;
