import React, { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import './styles/global.css';
import './styles/app.css';
import TitleBar from './components/TitleBar';
import Sidebar from './components/Sidebar';
import KeyboardCanvas, { comboString } from './components/KeyboardCanvas';
import MouseCanvas from './components/MouseCanvas';
import MacroPanel from './components/MacroPanel';
import SettingsPanel from './components/SettingsPanel';
import StatusBar from './components/StatusBar';
import TextExpansions from './components/TextExpansions';
import WelcomeModal from './components/WelcomeModal';
import UpgradeModal from './components/UpgradeModal';
import OnboardingTour from './components/OnboardingTour';
import QuickTips from './components/QuickTips';
import AnalyticsPanel from './components/AnalyticsPanel';
import ClipboardPanel from './components/ClipboardPanel';
import SearchTemplatesPanel from './components/SearchTemplatesPanel';
import RadialEditorView from './components/RadialEditorView';
import { DndContext, PointerSensor, useSensor, useSensors, DragOverlay } from '@dnd-kit/core';
import { MAX_SLOTS } from './components/RadialWheel';
import { friendlyKeyName } from './components/keyboardLayout';

function App() {
  const [assignments, setAssignments]       = useState({});
  const [selectedKey, setSelectedKey]       = useState(null);
  const [activeProfile, setActiveProfile]   = useState('Default');
  const [profiles, setProfiles]             = useState(['Default']);
  const [profileSettings, setProfileSettings] = useState({}); // { profileName: { linkedApp: '...' } }
  const [macrosEnabled, setMacrosEnabled]   = useState(true);
  const [notification, setNotification]     = useState(null);
  const [activeModifiers, setActiveModifiers] = useState([]);  // e.g. ['Ctrl', 'Alt']
  const [sidebarComboFilter, setSidebarComboFilter] = useState(null); // null = show all, string = filter by combo
  const [engineStatus, setEngineStatus]     = useState({ uiohookAvailable: false, nutjsAvailable: false });
  const [lastFired, setLastFired]           = useState(null);
  const [theme, setTheme]                   = useState('dark');
  const [expansionCategories, setExpansionCategories] = useState([]);
  const [globalVariables, setGlobalVariables]         = useState({});   // { 'my.name': 'Trigr', … }
  const [activeView, setActiveView]                 = useState('keyboard'); // 'keyboard' | 'mouse'
  const [activeArea, setActiveArea]                 = useState('mapping');  // 'mapping' | 'expansions' | 'analytics'
  const [isRecording, setIsRecording]               = useState(false);
  const [recordCapture, setRecordCapture]           = useState(null);
  const [tipsHidden, setTipsHidden]                 = useState(false);
  const [firstLaunchDate, setFirstLaunchDate]       = useState(null);
  const [backupRestoredFrom, setBackupRestoredFrom] = useState(null); // non-null = show banner
  const [activeGlobalProfile, setActiveGlobalProfile] = useState('Default');
  const [autocorrectEnabled, setAutocorrectEnabled] = useState(false);
  const [showSettings, setShowSettings]             = useState(false);
  const [showWelcome, setShowWelcome]               = useState(false);
  const [showOnboarding, setShowOnboarding]         = useState(false);
  const [macrosEnabledOnStartup, setMacrosEnabledOnStartup] = useState(true);
  const [globalInputMethod,  setGlobalInputMethod]  = useState('direct');
  const [macroSpeed,         setMacroSpeed]         = useState('safe');
  const [keystrokeDelay,     setKeystrokeDelay]     = useState(30);
  const [macroTriggerDelay,  setMacroTriggerDelay]  = useState(150);
  const [searchOverlayHotkey,       setSearchOverlayHotkey]       = useState('Ctrl+Space');
  const [voiceEnabled,              setVoiceEnabled]              = useState(false);
  const [voiceHotkey,               setVoiceHotkey]               = useState('Alt+Space');
  const [voiceMicId,               setVoiceMicId]               = useState('');
  const [overlayShowAll,             setOverlayShowAll]             = useState(true);
  const [overlayCloseAfterFiring,    setOverlayCloseAfterFiring]    = useState(true);
  const [overlayIncludeAutocorrect,  setOverlayIncludeAutocorrect]  = useState(false);
  const [clipboardPreviewWidth,      setClipboardPreviewWidth]      = useState(480);
  const [doubleTapWindow,            setDoubleTapWindow]            = useState(300);
  const [updateInfo,     setUpdateInfo]     = useState(null);   // { version, percent, ready, dismissed }
  const [appVersion,     setAppVersion]     = useState('');
  const [globalPauseToggleKey, setGlobalPauseToggleKey] = useState(null);
  const [importPrompt, setImportPrompt]                 = useState(null); // { name, assignments }
  const [licenceStatus, setLicenceStatus]               = useState({ is_pro: false, key_entered: false, status: 'no_key', product_name: '', expires_at: null });
  const [upgradePrompt, setUpgradePrompt]               = useState(null); // feature name string, or null
  const [listViewActive, setListViewActive]             = useState(() => {
    try { return localStorage.getItem('trigr_list_view') === 'true'; } catch { return false; }
  });
  const [searchTemplates, setSearchTemplates]           = useState([]);
  const [searchTemplateCategories, setSearchTemplateCategories] = useState([]);
  const [quickActionCategories, setQuickActionCategories] = useState([]);
  const [radialItemsMap, setRadialItemsMap]                 = useState({}); // { profileName: items[] }
  const [radialMenuHotkey, setRadialMenuHotkey]           = useState(null);
  const [selectedRadialSegment, setSelectedRadialSegment] = useState(null); // index or null
  const [selectedRadialChild, setSelectedRadialChild] = useState(null);   // { folderId, childIndex } or null
  const [expandedRadialFolder, setExpandedRadialFolder] = useState(null); // folder item id or null


  // Current modifier combo string e.g. "Ctrl+Alt"
  const currentCombo = comboString(activeModifiers);
  const isPro = licenceStatus.is_pro;

  // Show the upgrade modal for a named Pro feature.
  const showUpgrade = useCallback((featureName) => setUpgradePrompt(featureName), []);

  // ── Per-profile radial menu items ──────────────────────────
  const radialMenuItems = radialItemsMap[activeProfile] || [];

  // Ref tracks activeProfile so the wrapper below has a stable identity.
  // Without this, every handler that captures setRadialMenuItems would need
  // it in its dependency array, and a stale closure on profile switch causes
  // items to be written to the wrong profile — the root cause of drag-drop
  // failing on app-specific profiles.
  const activeProfileRef = useRef(activeProfile);
  activeProfileRef.current = activeProfile;

  // Drop-in wrapper: updates the per-profile map and syncs the flat
  // radialMenuItems key that Rust reads on overlay show.
  // Stable identity (empty deps) — reads activeProfile from ref at call time.
  const setRadialMenuItems = useCallback((updater) => {
    setRadialItemsMap(map => {
      const profile = activeProfileRef.current;
      const prev = map[profile] || [];
      const next = typeof updater === 'function' ? updater(prev) : updater;
      if (next === prev) return map;
      const newMap = { ...map, [profile]: next };
      window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap, radialMenuItems: next });
      return newMap;
    });
  }, []);

  // Sync flat radialMenuItems to config when profile switches (for Rust overlay)
  const prevRadialProfileRef = useRef(activeProfile);
  useEffect(() => {
    if (prevRadialProfileRef.current === activeProfile) return;
    prevRadialProfileRef.current = activeProfile;
    const items = radialItemsMap[activeProfile] || [];
    window.electronAPI?.saveConfig({ radialMenuItems: items });
  }, [activeProfile, radialItemsMap]);

  // ── Load config on mount ──────────────────────────────────
  useEffect(() => {
    const init = async () => {
      if (!window.electronAPI) return;
      window.electronAPI.getAppVersion().then(v => { if (v) setAppVersion(v); });
      const config = await window.electronAPI.loadConfig();
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
        const savedTheme = config.theme || 'dark';
        setTheme(savedTheme);
        document.documentElement.setAttribute('data-theme', savedTheme);
        // Migrate old string[] format to object[] format — treat missing colour as null
        const rawCats = config.expansionCategories || [];
        setExpansionCategories(rawCats.map(c => typeof c === 'string' ? { name: c, colour: null } : c));
        setGlobalVariables(config.globalVariables || {});
        const savedAcEnabled = config.autocorrectEnabled ?? false;
        setAutocorrectEnabled(savedAcEnabled);
        if (savedAcEnabled) {
          window.electronAPI?.updateAutocorrectEnabled(savedAcEnabled);
        }
        const savedMacrosOnStartup = config.macrosEnabledOnStartup ?? true;
        setMacrosEnabledOnStartup(savedMacrosOnStartup);
        setGlobalInputMethod(config.globalInputMethod   || 'direct');
        setMacroSpeed(       config.macroSpeed          || 'safe');
        setKeystrokeDelay(   config.keystrokeDelay      ?? 30);
        setMacroTriggerDelay(config.macroTriggerDelay   ?? 150);
        setDoubleTapWindow(  config.doubleTapWindow     ?? 300);
        // Always start on the Mapping view — do not restore last-used view/area
        setSearchOverlayHotkey(     config.searchOverlayHotkey      || 'Ctrl+Space');
        setVoiceEnabled(            config.voiceEnabled             ?? false);
        setVoiceHotkey(             config.voiceHotkey              || 'Alt+Space');
        setVoiceMicId(              config.voiceMicId               || '');
        setGlobalPauseToggleKey(    config.globalPauseToggleKey     ?? null);
        setOverlayShowAll(          config.overlayShowAll            ?? true);
        setOverlayCloseAfterFiring( config.overlayCloseAfterFiring   ?? true);
        setOverlayIncludeAutocorrect(config.overlayIncludeAutocorrect ?? false);
        setClipboardPreviewWidth(   Math.max(320, Math.min(1200, config.clipboardPreviewWidth ?? 480)));
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
            const flat = map[globalProfile] || [];
            window.electronAPI?.saveConfig({ radialMenuItemsByProfile: map, radialMenuItems: flat });
          }
          setRadialItemsMap(map);
        }
        setRadialMenuHotkey(config.radialMenuHotkey || null);
        // Sync new settings to engine on load
        window.electronAPI?.updateGlobalSettings({
          globalInputMethod: config.globalInputMethod  || 'direct',
          macroSpeed:        config.macroSpeed         || 'safe',
          keystrokeDelay:    config.keystrokeDelay     ?? 30,
          macroTriggerDelay: config.macroTriggerDelay  ?? 150,
          doubleTapWindow:   config.doubleTapWindow    ?? 300,
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
        // Register pause hotkey with Rust backend if one is stored in config
        if (config.globalPauseToggleKey) {
          window.electronAPI?.setPauseHotkey(config.globalPauseToggleKey);
        }
        // Register voice hotkey with Rust backend only if voice is enabled
        if ((config.voiceEnabled ?? false) && config.voiceHotkey) {
          window.electronAPI?.setVoiceHotkey(config.voiceHotkey);
        }
        // Register radial menu hotkey with Rust backend
        if (config.radialMenuHotkey) {
          window.electronAPI?.setRadialMenuHotkey(config.radialMenuHotkey);
        }
        // If the main process auto-restored from a backup, surface that to the user
        if (config._restoredFrom) setBackupRestoredFrom(config._restoredFrom);

        // Tips — load hidden flag; record first launch date if not yet stored
        setTipsHidden(config.tipsHidden ?? false);
        const fld = config.firstLaunchDate || new Date().toISOString();
        setFirstLaunchDate(fld);
        if (!config.firstLaunchDate) needsSave = true;

        // Onboarding migration: existing users who already saw the welcome
        // should not see the new onboarding tour after updating.
        let onboardingComplete = config.onboarding_complete;
        if (onboardingComplete === undefined && config.hasSeenWelcome) {
          onboardingComplete = true;
          needsSave = true;
        }

        if (!onboardingComplete) {
          // New user — show onboarding tour (replaces WelcomeModal)
          setShowOnboarding(true);
        } else if (!config.hasSeenWelcome) {
          // Edge case: onboarding complete but welcome not set
          setShowWelcome(true);
        }

        needsSave = needsSave || !config.hasSeenWelcome;
        if (needsSave) {
          window.electronAPI.saveConfig({
            ...config,
            assignments: migrated,
            hasSeenWelcome: true,
            firstLaunchDate: fld,
            onboarding_complete: onboardingComplete ?? false,
          });
        }
      }
      const status = await window.electronAPI.getEngineStatus();
      setEngineStatus(status);

      // Check licence status (revalidates if >24h since last check)
      window.electronAPI.checkLicenceRevalidation?.().then(ls => {
        if (ls) setLicenceStatus(ls);
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
      // Engine auto-switched profile (foreground app matched a linked profile)
      window.electronAPI.onProfileSwitched(({ profile }) => {
        setActiveProfile(profile);
        setSelectedKey(null);
      });
      window.electronAPI.onOverlayFired?.((data) => {
        showNotification(`⚡ ${data.label || 'Macro fired'}`);
      });
      window.electronAPI.onHotkeyRecorded?.((data) => {
        setIsRecording(false);
        if (!data) {
          // Escape — cancelled. Discard any pending duplicate.
          pendingDuplicateRef.current = null;
          return;
        }
        const { modifiers, keyId } = data;
        // No modifiers → treat as BARE key layer
        const mods = modifiers.length === 0 ? ['BARE'] : modifiers;
        setActiveModifiers(mods);
        setSelectedKey(keyId);
        if (keyId.startsWith('MOUSE_')) setActiveView('mouse');
        else setActiveView('keyboard');
        const modsLabel = modifiers.length === 0 ? 'Bare' : modifiers.join('+');
        setRecordCapture(`${modsLabel}+${friendlyKeyName(keyId)}`);
        setTimeout(() => setRecordCapture(null), 2000);
      });

      // Shared config — listen for sync reload events from file watcher
      window.electronAPI.onConfigReloadedFromSync?.((config) => {
        if (!config) return;
        const raw = config.assignments || {};
        setAssignments(raw);
        setProfiles(config.profiles?.length ? config.profiles : ['Default']);
        const globalProfile = config.activeGlobalProfile || 'Default';
        setActiveProfile(globalProfile);
        setActiveGlobalProfile(globalProfile);
        setProfileSettings(config.profileSettings || {});
        const savedTheme = config.theme || 'dark';
        setTheme(savedTheme);
        document.documentElement.setAttribute('data-theme', savedTheme);
        const rawCats = config.expansionCategories || [];
        setExpansionCategories(rawCats.map(c => typeof c === 'string' ? { name: c, colour: null } : c));
        setGlobalVariables(config.globalVariables || {});
        setAutocorrectEnabled(config.autocorrectEnabled ?? false);
        setMacrosEnabledOnStartup(config.macrosEnabledOnStartup ?? true);
        setGlobalInputMethod(config.globalInputMethod   || 'direct');
        setMacroSpeed(       config.macroSpeed          || 'safe');
        setKeystrokeDelay(   config.keystrokeDelay      ?? 30);
        setMacroTriggerDelay(config.macroTriggerDelay   ?? 150);
        setDoubleTapWindow(  config.doubleTapWindow     ?? 300);
        setSearchOverlayHotkey(     config.searchOverlayHotkey      || 'Ctrl+Space');
        setVoiceEnabled(            config.voiceEnabled             ?? false);
        setVoiceHotkey(             config.voiceHotkey              || 'Alt+Space');
        setVoiceMicId(              config.voiceMicId               || '');
        setGlobalPauseToggleKey(    config.globalPauseToggleKey     ?? null);
        setOverlayShowAll(          config.overlayShowAll            ?? true);
        setOverlayCloseAfterFiring( config.overlayCloseAfterFiring   ?? true);
        setOverlayIncludeAutocorrect(config.overlayIncludeAutocorrect ?? false);
        setClipboardPreviewWidth(   Math.max(320, Math.min(1200, config.clipboardPreviewWidth ?? 480)));
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
        }
        setRadialMenuHotkey(config.radialMenuHotkey || null);
        // Re-sync engine with updated config
        window.electronAPI?.updateAssignments(raw, globalProfile);
        window.electronAPI?.updateProfileSettings(config.profileSettings || {});
        window.electronAPI?.setActiveGlobalProfile(globalProfile);
        window.electronAPI?.updateGlobalVariables(config.globalVariables || {});
        window.electronAPI?.updateGlobalSettings({
          globalInputMethod: config.globalInputMethod  || 'direct',
          macroSpeed:        config.macroSpeed         || 'safe',
          keystrokeDelay:    config.keystrokeDelay     ?? 30,
          macroTriggerDelay: config.macroTriggerDelay  ?? 150,
          doubleTapWindow:   config.doubleTapWindow    ?? 300,
        });
        showNotification('Config updated from sync', 'info');
      });
    };
    init();
    return () => {
      window.electronAPI?.removeAllListeners('macro-fired');
      window.electronAPI?.removeAllListeners('engine-status');
      window.electronAPI?.removeAllListeners('profile-switched');
      window.electronAPI?.removeAllListeners('overlay-fired');
      window.electronAPI?.removeAllListeners('hotkey-recorded');
    };
  }, []);

  // ── Licence re-validation on window focus ──
  useEffect(() => {
    const handleFocus = () => {
      window.electronAPI?.checkLicenceRevalidation?.().then(ls => {
        if (ls) setLicenceStatus(ls);
      });
    };
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, []);

  // ── UPDATER — DO NOT MODIFY WITHOUT EXPLICIT INSTRUCTION ──
  // Permissions required: updater:allow-check, updater:default (default.json)
  // process:allow-restart required for relaunch after install
  // Removing any of these permissions will cause silent failure
  // Test any changes with cargo tauri dev before releasing
  // Both x64 and ARM64 builds required in release.yml matrix
  useEffect(() => {
    async function checkForUpdates() {
      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        const { relaunch } = await import('@tauri-apps/plugin-process');
        const { confirm } = await import('@tauri-apps/plugin-dialog');
        const update = await check();
        if (update?.available) {
          const confirmed = await confirm(
            `Trigr ${update.version} is available. Install now?`,
            { title: 'Update Available', kind: 'info' }
          );
          if (confirmed) {
            await update.downloadAndInstall();
            await relaunch();
          }
        }
      } catch (e) {
        console.error('Update check failed:', e);
      }
    }
    checkForUpdates();
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

  const saveConfig = useCallback((newAssignments, newProfiles, newProfile) => {
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories });
    syncEngine(newAssignments, newProfile);
  }, [syncEngine, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, globalVariables, searchTemplates, searchTemplateCategories, quickActionCategories]);

  const handleSaveGlobalVariables = useCallback((newVars) => {
    setGlobalVariables(newVars);
    window.electronAPI?.updateGlobalVariables(newVars);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables: newVars, searchTemplates, searchTemplateCategories, quickActionCategories });
  }, [assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, searchTemplates, searchTemplateCategories, quickActionCategories]);

  // ── Notifications ─────────────────────────────────────────
  const showNotification = useCallback((msg, type = 'success') => {
    setNotification({ msg, type });
    setTimeout(() => setNotification(null), 2500);
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

  // ── Search Template Category CRUD ────────────────────────
  const handleAddSearchTemplateCategory = useCallback((name, colour = null) => {
    const next = [...searchTemplateCategories, { name, colour: colour || null }];
    setSearchTemplateCategories(next);
    window.electronAPI?.saveConfig({ searchTemplateCategories: next });
  }, [searchTemplateCategories]);

  const handleRenameSearchTemplateCategory = useCallback((oldName, newName) => {
    const nextCats = searchTemplateCategories.map(c => c.name === oldName ? { ...c, name: newName } : c);
    const nextTemplates = searchTemplates.map(t => t.category === oldName ? { ...t, category: newName } : t);
    setSearchTemplateCategories(nextCats);
    setSearchTemplates(nextTemplates);
    window.electronAPI?.saveConfig({ searchTemplateCategories: nextCats, searchTemplates: nextTemplates });
  }, [searchTemplateCategories, searchTemplates]);

  const handleDeleteSearchTemplateCategory = useCallback((name) => {
    const nextCats = searchTemplateCategories.filter(c => c.name !== name);
    const nextTemplates = searchTemplates.map(t => t.category === name ? { ...t, category: null } : t);
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

  // ── Quick Action CRUD (stored in assignments as GLOBAL::QUICKACTION::uuid) ──
  const handleAddQuickAction = useCallback((action) => {
    const key = `GLOBAL::QUICKACTION::${action.id}`;
    const newAssignments = { ...assignments, [key]: { type: action.type, label: action.label, data: action.data } };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, profiles, activeProfile, saveConfig]);

  const handleUpdateQuickAction = useCallback((id, updates) => {
    const key = `GLOBAL::QUICKACTION::${id}`;
    const existing = assignments[key];
    if (!existing) return;
    const newAssignments = { ...assignments, [key]: { ...existing, ...updates } };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, profiles, activeProfile, saveConfig]);

  const handleDeleteQuickAction = useCallback((id) => {
    const key = `GLOBAL::QUICKACTION::${id}`;
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, profiles, activeProfile, saveConfig]);

  // ── Quick Action Category CRUD ───────────────────────────
  const handleAddQaCategory = useCallback((name, colour = null) => {
    const next = [...quickActionCategories, { name, colour: colour || null }];
    setQuickActionCategories(next);
    window.electronAPI?.saveConfig({ quickActionCategories: next });
  }, [quickActionCategories]);

  const handleRenameQaCategory = useCallback((oldName, newName) => {
    const nextCats = quickActionCategories.map(c => c.name === oldName ? { ...c, name: newName } : c);
    setQuickActionCategories(nextCats);
    // Update category on all quick actions that use the old name
    const newAssignments = { ...assignments };
    let changed = false;
    for (const [k, v] of Object.entries(newAssignments)) {
      if (k.startsWith('GLOBAL::QUICKACTION::') && v.data?.category === oldName) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName } };
        changed = true;
      }
    }
    if (changed) { setAssignments(newAssignments); saveConfig(newAssignments, profiles, activeProfile); }
    window.electronAPI?.saveConfig({ quickActionCategories: nextCats });
  }, [quickActionCategories, assignments, profiles, activeProfile, saveConfig]);

  const handleDeleteQaCategory = useCallback((name) => {
    const nextCats = quickActionCategories.filter(c => c.name !== name);
    setQuickActionCategories(nextCats);
    // Move quick actions in deleted category to uncategorised
    const newAssignments = { ...assignments };
    let changed = false;
    for (const [k, v] of Object.entries(newAssignments)) {
      if (k.startsWith('GLOBAL::QUICKACTION::') && v.data?.category === name) {
        newAssignments[k] = { ...v, data: { ...v.data, category: null } };
        changed = true;
      }
    }
    if (changed) { setAssignments(newAssignments); saveConfig(newAssignments, profiles, activeProfile); }
    window.electronAPI?.saveConfig({ quickActionCategories: nextCats });
  }, [quickActionCategories, assignments, profiles, activeProfile, saveConfig]);

  const handleUpdateQaCategoryColour = useCallback((name, colour) => {
    const next = quickActionCategories.map(c => c.name === name ? { ...c, colour } : c);
    setQuickActionCategories(next);
    window.electronAPI?.saveConfig({ quickActionCategories: next });
  }, [quickActionCategories]);

  const handleReorderQaCategories = useCallback((newOrder) => {
    setQuickActionCategories(newOrder);
    window.electronAPI?.saveConfig({ quickActionCategories: newOrder });
  }, []);

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
      // Update sidebar filter to match keyboard modifier selection
      if (next.length === 0) {
        setSidebarComboFilter(null);
      } else if (next.includes('BARE')) {
        setSidebarComboFilter('BARE');
      } else {
        setSidebarComboFilter([...next].sort().join('+'));
      }
      return next;
    });
    // Deselect key when modifier layer changes
    setSelectedKey(null);
  }, []);

  // ── Key selection ─────────────────────────────────────────
  const handleKeySelect = useCallback((keyId) => {
    if (activeModifiers.length === 0) return; // require a modifier layer
    setSelectedKey(prev => prev === keyId ? null : keyId);
  }, [activeModifiers]);

  // ── Assignment key format: "Profile::Ctrl+Alt::KeyE" ──────
  const makeAssignmentKey = useCallback((profile, combo, keyId) => {
    return `${profile}::${combo}::${keyId}`;
  }, []);

  const getKeyAssignment = useCallback((keyId) => {
    if (activeModifiers.length === 0) return null;
    return assignments[makeAssignmentKey(activeProfile, currentCombo, keyId)] || null;
  }, [assignments, activeProfile, currentCombo, activeModifiers, makeAssignmentKey]);

  // ── Assign macro ──────────────────────────────────────────
  const handleAssign = useCallback((keyId, macro) => {
    const key = makeAssignmentKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments, [key]: macro };
    // If pending duplicate has a double press, save it too
    if (pendingDuplicateRef.current?.double) {
      const doubleKey = `${activeProfile}::${currentCombo}::${keyId}::double`;
      newAssignments[doubleKey] = pendingDuplicateRef.current.double;
    }
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    pendingDuplicateRef.current = null;
    showNotification(`Assigned to ${currentCombo}+${keyId}`);
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Clear key ─────────────────────────────────────────────
  const handleClearKey = useCallback((keyId) => {
    const key = makeAssignmentKey(activeProfile, currentCombo, keyId);
    const doubleKey = key + '::double';
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    delete newAssignments[doubleKey];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Cleared ${currentCombo}+${keyId}`, 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Rename assignment label ────────────────────────────────
  const handleRenameAssignment = useCallback((combo, keyId, newLabel) => {
    const key = `${activeProfile}::${combo}::${keyId}`;
    const existing = assignments[key];
    if (!existing) return;
    const newAssignments = { ...assignments, [key]: { ...existing, label: newLabel } };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
  }, [assignments, activeProfile, profiles, saveConfig]);

  // ── Clear assignment by combo+keyId (context menu) ────────
  const handleClearAssignment = useCallback((combo, keyId) => {
    const key = `${activeProfile}::${combo}::${keyId}`;
    const doubleKey = key + '::double';
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    delete newAssignments[doubleKey];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    syncEngine(newAssignments, activeProfile);
    if (selectedKey === keyId) setSelectedKey(null);
    showNotification(`Cleared ${combo}+${keyId}`, 'info');
  }, [assignments, activeProfile, profiles, saveConfig, syncEngine, selectedKey, showNotification]);

  // ── Duplicate assignment to pending (context menu) ────────
  const pendingDuplicateRef = useRef(null);

  const handleDuplicateFromContext = useCallback((combo, keyId) => {
    const key = `${activeProfile}::${combo}::${keyId}`;
    const existing = assignments[key];
    if (!existing) return;
    // Deep clone single press
    const single = {
      ...existing,
      label: (existing.label || '') + ' (copy)',
      data: JSON.parse(JSON.stringify(existing.data || {})),
    };
    // Deep clone double press if it exists
    const doubleKey = `${activeProfile}::${combo}::${keyId}::double`;
    const existingDouble = assignments[doubleKey];
    const double = existingDouble ? {
      ...existingDouble,
      label: (existingDouble.label || '') + ' (copy)',
      data: JSON.parse(JSON.stringify(existingDouble.data || {})),
    } : null;
    pendingDuplicateRef.current = { single, double };
    // Deselect any current key so MacroPanel shows the pending duplicate, not the original
    setSelectedKey(null);
    // Start recording so user picks a new key for the duplicate
    setIsRecording(true);
    window.electronAPI?.startHotkeyRecording();
  }, [assignments, activeProfile]);

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
    showNotification(`Double-tap assigned to ${currentCombo}+${keyId}`);
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeDoubleKey]);

  const handleClearDouble = useCallback((keyId) => {
    const key = makeDoubleKey(activeProfile, currentCombo, keyId);
    const newAssignments = { ...assignments };
    delete newAssignments[key];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Double-tap cleared', 'info');
  }, [assignments, activeProfile, currentCombo, profiles, saveConfig, showNotification, makeDoubleKey]);

  // ── Profile management ────────────────────────────────────
  const handleProfileChange = useCallback((profile) => {
    setActiveProfile(profile);
    setSelectedKey(null);
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
    const newProfiles = profiles.map(p => p === oldName ? newName : p);
    const newActive   = activeProfile === oldName ? newName : activeProfile;
    setAssignments(newAssignments);
    setProfiles(newProfiles);
    setActiveProfile(newActive);
    setProfileSettings(newProfileSettings);
    window.electronAPI?.updateProfileSettings(newProfileSettings);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newActive, profileSettings: newProfileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
    syncEngine(newAssignments, newActive);
    showNotification(`Renamed to "${newName}"`);
  }, [profiles, assignments, profileSettings, activeProfile, syncEngine, showNotification]);

  // ── Toggle macros ─────────────────────────────────────────
  const handleToggleMacros = useCallback(() => {
    const newVal = !macrosEnabled;
    setMacrosEnabled(newVal);
    window.electronAPI?.toggleMacros(newVal);
    showNotification(newVal ? 'Macros active' : 'Macros paused', newVal ? 'success' : 'info');
  }, [macrosEnabled, showNotification]);

  // ── Theme toggle ──────────────────────────────────────────
  const handleToggleTheme = useCallback(() => {
    setTheme(prev => {
      const next = prev === 'dark' ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', next);
      window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme: next, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
      return next;
    });
  }, [assignments, profiles, activeProfile, profileSettings]);

  // ── Text expansions (global — shared across all profiles) ─
  const expansions = Object.entries(assignments)
    .filter(([k]) => k.startsWith('GLOBAL::EXPANSION::'))
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
      voicePhrase: v.data?.voicePhrase || '',
    }))
    .sort((a, b) => a.trigger.localeCompare(b.trigger));

  // editorValue is { html, text } from the rich text editor.
  // originalTrigger is provided when editing an existing expansion; if it differs
  // from trigger the old key is removed in the same update (single atomic write).
  const handleAddExpansion = useCallback((trigger, editorValue, originalTrigger, category, triggerMode, displayName, expansionType, imagePath, imageScale, variantOptions, voicePhrase) => {
    const newAssignments = { ...assignments };
    if (originalTrigger && originalTrigger !== trigger) {
      delete newAssignments[`GLOBAL::EXPANSION::${originalTrigger}`];
    }
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
    }
    if (voicePhrase) data.voicePhrase = voicePhrase;
    newAssignments[`GLOBAL::EXPANSION::${trigger}`] = {
      type: 'expansion',
      label: displayName || (expansionType === 'image' ? `Image: ${trigger}` : `Expand: ${trigger}`),
      data,
    };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Expansion "${trigger}" saved`);
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  const handleDeleteExpansion = useCallback((trigger) => {
    const newAssignments = { ...assignments };
    delete newAssignments[`GLOBAL::EXPANSION::${trigger}`];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Expansion "${trigger}" deleted`, 'info');
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  // ── Expansion categories ──────────────────────────────────
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
    const newCategories = expansionCategories.map(c =>
      c.name === oldName ? { ...c, name: newName } : c
    );
    // Rewrite every expansion that belongs to this category
    const newAssignments = { ...assignments };
    for (const [k, v] of Object.entries(newAssignments)) {
      if (k.startsWith('GLOBAL::EXPANSION::') && v.data?.category === oldName) {
        newAssignments[k] = { ...v, data: { ...v.data, category: newName } };
      }
    }
    setExpansionCategories(newCategories);
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleDeleteCategory = useCallback((name) => {
    const newCategories = expansionCategories.filter(c => c.name !== name);
    // Move all expansions in this category to uncategorised
    const newAssignments = { ...assignments };
    for (const [k, v] of Object.entries(newAssignments)) {
      if (k.startsWith('GLOBAL::EXPANSION::') && v.data?.category === name) {
        newAssignments[k] = { ...v, data: { ...v.data, category: null } };
      }
    }
    setExpansionCategories(newCategories);
    setAssignments(newAssignments);
    syncEngine(newAssignments, activeProfile);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles, activeProfile, profileSettings, theme, expansionCategories: newCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [expansionCategories, assignments, profiles, activeProfile, profileSettings, theme, syncEngine, autocorrectEnabled]);

  // ── Autocorrect ───────────────────────────────────────────
  const autocorrections = Object.entries(assignments)
    .filter(([k]) => k.startsWith('GLOBAL::AUTOCORRECT::'))
    .map(([k, v]) => ({
      typo: k.slice('GLOBAL::AUTOCORRECT::'.length),
      correction: v.data?.correction || '',
    }));

  const handleToggleAutocorrect = useCallback(() => {
    const next = !autocorrectEnabled;
    setAutocorrectEnabled(next);
    window.electronAPI?.updateAutocorrectEnabled(next);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled: next, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [autocorrectEnabled, assignments, profiles, activeProfile, profileSettings, theme, expansionCategories]);

  const handleAddAutocorrect = useCallback((typo, correction, originalTypo) => {
    const newAssignments = { ...assignments };
    if (originalTypo && originalTypo !== typo) {
      delete newAssignments[`GLOBAL::AUTOCORRECT::${originalTypo}`];
    }
    newAssignments[`GLOBAL::AUTOCORRECT::${typo}`] = {
      type: 'autocorrect',
      label: `Autocorrect: ${typo}`,
      data: { correction },
    };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Autocorrect "${typo}" saved`);
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

  const handleDeleteAutocorrect = useCallback((typo) => {
    const newAssignments = { ...assignments };
    delete newAssignments[`GLOBAL::AUTOCORRECT::${typo}`];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Autocorrect "${typo}" deleted`, 'info');
  }, [assignments, profiles, activeProfile, saveConfig, showNotification]);

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
      console.error('[Trigr] Export profile failed:', e);
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
        showNotification('Not a valid Trigr profile export', 'info');
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
      const assignmentCount = Object.keys(importedAssignments).length;
      const newAssignments = { ...assignments, ...importedAssignments };
      const newProfiles = [...profiles, importName];
      setAssignments(newAssignments);
      setProfiles(newProfiles);
      setActiveProfile(importName);
      setSelectedKey(null);
      saveConfig(newAssignments, newProfiles, importName);
      showNotification(`Profile "${importName}" imported — ${assignmentCount} assignment${assignmentCount !== 1 ? 's' : ''} loaded`);
    } catch (e) {
      console.error('[Trigr] Import profile failed:', e);
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
      const assignmentCount = Object.keys(importedAssignments).length;
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
      const assignmentCount = Object.keys(importedRaw).length;
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
    const newActive = activeProfile === name ? 'Default' : activeProfile;
    setAssignments(newAssignments);
    setProfiles(newProfiles);
    setActiveProfile(newActive);
    setSelectedKey(null);
    setProfileSettings(newProfileSettings);
    // If the deleted profile was the active global profile, fall back to Default
    const newGlobal = activeGlobalProfile === name ? 'Default' : activeGlobalProfile;
    if (newGlobal !== activeGlobalProfile) {
      setActiveGlobalProfile(newGlobal);
      window.electronAPI?.setActiveGlobalProfile(newGlobal);
    }
    window.electronAPI?.updateProfileSettings(newProfileSettings);
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newActive, activeGlobalProfile: newGlobal, profileSettings: newProfileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
    syncEngine(newAssignments, newActive);
    showNotification(`Profile "${name}" deleted`, 'info');
  }, [profiles, assignments, profileSettings, activeProfile, activeGlobalProfile, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, syncEngine, showNotification]);

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
  const handleCopyToProfile = useCallback((targetProfile, combo, keyId) => {
    const srcCombo = combo || currentCombo;
    const srcKey   = keyId || selectedKey;
    const oldKey = makeAssignmentKey(activeProfile, srcCombo, srcKey);
    const newKey = makeAssignmentKey(targetProfile, srcCombo, srcKey);
    const oldDouble = oldKey + '::double';
    const newDouble = newKey + '::double';
    const newAssignments = { ...assignments };
    if (assignments[oldKey]) newAssignments[newKey] = assignments[oldKey];
    if (assignments[oldDouble]) newAssignments[newDouble] = assignments[oldDouble];
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Copied to "${targetProfile}" profile`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  const handleMoveToProfile = useCallback((targetProfile, combo, keyId) => {
    const srcCombo = combo || currentCombo;
    const srcKey   = keyId || selectedKey;
    const oldKey = makeAssignmentKey(activeProfile, srcCombo, srcKey);
    const newKey = makeAssignmentKey(targetProfile, srcCombo, srcKey);
    const oldDouble = oldKey + '::double';
    const newDouble = newKey + '::double';
    const newAssignments = { ...assignments };
    // Move single-press (if exists)
    if (newAssignments[oldKey]) {
      newAssignments[newKey] = newAssignments[oldKey];
      delete newAssignments[oldKey];
    }
    // Move double-press (if exists)
    if (newAssignments[oldDouble]) {
      newAssignments[newDouble] = newAssignments[oldDouble];
      delete newAssignments[oldDouble];
    }
    setAssignments(newAssignments);
    setSelectedKey(null);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Moved to "${targetProfile}" profile`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Reassign hotkey ───────────────────────────────────────
  const handleReassign = useCallback((newCombo, newKeyId) => {
    const oldKey       = makeAssignmentKey(activeProfile, currentCombo, selectedKey);
    const oldDoubleKey = oldKey + '::double';
    const newKey       = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const newDoubleKey = newKey + '::double';
    const newAssignments = { ...assignments };

    // If the target key already has an assignment, save it to the old key
    // so the user doesn't lose their existing macro/action
    if (newAssignments[newKey]) {
      newAssignments[oldKey] = newAssignments[newKey];
    }
    if (newAssignments[newDoubleKey]) {
      newAssignments[oldDoubleKey] = newAssignments[newDoubleKey];
    } else {
      delete newAssignments[oldDoubleKey];
    }

    // Move the original assignment to the new key
    newAssignments[newKey] = assignments[oldKey];
    if (assignments[oldDoubleKey]) {
      newAssignments[newDoubleKey] = assignments[oldDoubleKey];
    }

    // If target had no assignment, clean up old key
    if (!assignments[newKey]) {
      delete newAssignments[oldKey];
    }
    if (!assignments[newDoubleKey]) {
      delete newAssignments[oldDoubleKey];
    }

    setAssignments(newAssignments);
    const newMods = newCombo ? newCombo.split('+').filter(Boolean) : [];
    setActiveModifiers(newMods);
    setSelectedKey(newKeyId);
    if (!newKeyId.startsWith('MOUSE_')) setActiveView('keyboard');
    saveConfig(newAssignments, profiles, activeProfile);
    const swapped = assignments[newKey];
    showNotification(swapped ? 'Hotkeys swapped' : 'Hotkey reassigned');
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── Duplicate assignment to a new hotkey ─────────────────
  const handleDuplicateAssignment = useCallback((newCombo, newKeyId) => {
    const oldKey = makeAssignmentKey(activeProfile, currentCombo, selectedKey);
    const existing = assignments[oldKey];
    if (!existing) return;
    const newKey = makeAssignmentKey(activeProfile, newCombo, newKeyId);
    const duplicated = {
      ...existing,
      label: (existing.label || '') + ' (copy)',
      data: JSON.parse(JSON.stringify(existing.data || {})),
    };
    const newAssignments = { ...assignments, [newKey]: duplicated };
    // Copy double press if it exists
    const oldDoubleKey = `${activeProfile}::${currentCombo}::${selectedKey}::double`;
    const existingDouble = assignments[oldDoubleKey];
    if (existingDouble) {
      const newDoubleKey = `${activeProfile}::${newCombo}::${newKeyId}::double`;
      newAssignments[newDoubleKey] = {
        ...existingDouble,
        label: (existingDouble.label || '') + ' (copy)',
        data: JSON.parse(JSON.stringify(existingDouble.data || {})),
      };
    }
    setAssignments(newAssignments);
    const newMods = newCombo === 'BARE' ? ['BARE'] : (newCombo ? newCombo.split('+').filter(Boolean) : []);
    setActiveModifiers(newMods);
    setSelectedKey(newKeyId);
    if (!newKeyId.startsWith('MOUSE_')) setActiveView('keyboard');
    saveConfig(newAssignments, profiles, activeProfile);
    const keyLabel = friendlyKeyName(newKeyId);
    const comboLabel = newCombo === 'BARE' ? keyLabel : `${newCombo}+${keyLabel}`;
    showNotification(`Duplicated to ${comboLabel}`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── View switching (keyboard ↔ mouse, within Mapping area) ─
  const handleSetView = useCallback((view) => {
    setActiveView(view);
    setSelectedKey(null);
    setSelectedRadialSegment(null);
    setSelectedRadialChild(null);
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

  // ── Auto list view below 800px with state memory ────────────
  useEffect(() => {
    const BREAKPOINT = 800;
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
  const handleSetArea = useCallback((area) => {
    setActiveArea(area);
    if (area !== 'mapping') setSelectedKey(null);
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
    setSelectedKey(keyId);
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
    };
    setGlobalInputMethod(next.globalInputMethod);
    setMacroSpeed(next.macroSpeed);
    setKeystrokeDelay(next.keystrokeDelay);
    setMacroTriggerDelay(next.macroTriggerDelay);
    setDoubleTapWindow(next.doubleTapWindow);
    window.electronAPI?.updateGlobalSettings(next);
    window.electronAPI?.saveConfig(next);
  }, [globalInputMethod, macroSpeed, keystrokeDelay, macroTriggerDelay, doubleTapWindow]);

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

  const handleClearRadialMenuHotkey = useCallback(() => {
    setRadialMenuHotkey(null);
    window.electronAPI?.clearRadialMenuHotkey();
    window.electronAPI?.saveConfig({ radialMenuHotkey: null });
  }, []);

  // Auto-fetch app icon for Open App assignments and store on the radial item
  // Optional assignmentOverride: pass the assignment directly when state hasn't flushed yet
  const fetchAndSetAppIcon = useCallback(async (itemId, storageKey, assignmentOverride) => {
    const assignment = assignmentOverride || assignments[storageKey];
    if (!assignment || assignment.type !== 'app') return;
    const appPath = assignment.data?.path;
    if (!appPath) return;
    try {
      const dataUrl = await window.electronAPI?.getAppIcon(appPath);
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
    const appPath = assignment.data?.path;
    if (!appPath) return;
    try {
      const dataUrl = await window.electronAPI?.getAppIcon(appPath);
      if (dataUrl) {
        setRadialMenuItems(prev => prev.map(item => {
          if (!item || item.id !== folderId || item.type !== 'folder') return item;
          return { ...item, children: item.children.map(c => c.id === childId ? { ...c, appIcon: dataUrl } : c) };
        }));
      }
    } catch (e) {}
  }, [assignments]);

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
    setRadialMenuItems(prev => {
      return prev.map(item => (item && item.id === id) ? null : item);
    });
  }, []);

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
    // Fetch app icon immediately — pass assignment directly since state hasn't flushed
    if (actionType === 'app' && actionData?.path) {
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
      if (macro.type === 'app' && macro.data?.path && existingItem?.id) {
        fetchAndSetAppIcon(existingItem.id, existingKey, macro);
      }
    } else {
      // New segment or segment with a sidebar key — create a GLOBAL::RADIAL:: assignment
      handleCreateRadialAction(macro.type, macro.data, macro.label || '', idx);
    }
    showNotification('Radial segment updated');
  }, [selectedRadialSegment, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleCreateRadialAction, showNotification]);

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
    } else {
      // New child — create GLOBAL::RADIAL:: assignment and add to folder
      const id = crypto.randomUUID();
      const storageKey = `GLOBAL::RADIAL::${id}`;
      const newAssignments = { ...assignments, [storageKey]: macro };
      setAssignments(newAssignments);
      saveConfig(newAssignments, profiles, activeProfile);
      handleAddChildToFolder(folderId, storageKey, macro.label || '');
    }
    showNotification('Folder child updated');
  }, [selectedRadialChild, radialMenuItems, assignments, profiles, activeProfile, saveConfig, handleAddChildToFolder, showNotification]);

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
    const sourceItems = radialItemsMap[activeProfile] || [];
    const sourceItem = sourceItems[segmentIndex];
    if (!sourceItem) return null;
    const targetItems = radialItemsMap[targetProfile] || [];
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
    const newMap = { ...radialItemsMap, [targetProfile]: newTarget };
    setRadialItemsMap(newMap);
    const flatItems = newMap[activeProfile] || [];
    window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap, radialMenuItems: flatItems });
    showNotification(`Copied to "${targetProfile}"`);
    return null;
  }, [radialItemsMap, activeProfile, showNotification]);

  const handleForceOverwriteRadialSegment = useCallback((targetProfile, segmentIndex) => {
    const sourceItems = radialItemsMap[activeProfile] || [];
    const sourceItem = sourceItems[segmentIndex];
    if (!sourceItem) return;
    const copied = JSON.parse(JSON.stringify(sourceItem));
    copied.id = crypto.randomUUID();
    if (copied.children) copied.children = copied.children.map(c => c ? { ...c, id: crypto.randomUUID() } : c);
    const targetItems = radialItemsMap[targetProfile] || [];
    const newTarget = [...targetItems];
    while (newTarget.length <= segmentIndex) newTarget.push(null);
    newTarget[segmentIndex] = copied;
    const newMap = { ...radialItemsMap, [targetProfile]: newTarget };
    setRadialItemsMap(newMap);
    const flatItems = newMap[activeProfile] || [];
    window.electronAPI?.saveConfig({ radialMenuItemsByProfile: newMap, radialMenuItems: flatItems });
    showNotification(`Copied to "${targetProfile}" (overwritten)`);
  }, [radialItemsMap, activeProfile, showNotification]);

  // ── Radial drag state + handlers (cross-container DndContext) ──
  const [radialActiveDrag, setRadialActiveDrag] = useState(null);
  const [radialDropTarget, setRadialDropTarget] = useState(-1);    // inner ring target
  const [radialDropTargetOuter, setRadialDropTargetOuter] = useState(-1); // outer ring target
  const [radialRejectIndex, setRadialRejectIndex] = useState(-1);
  const wheelRef = useRef(null);
  const radialDragActivatorRef = useRef(null); // stores the pointerdown event from drag start

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

    // Outer ring — only when a folder is expanded
    if (dist >= OUTER_INNER && dist <= OUTER_OUTER && expandedRadialFolder) {
      const folderIdx = radialMenuItems.findIndex(i => i?.id === expandedRadialFolder);
      if (folderIdx < 0) return null;
      const folder = radialMenuItems[folderIdx];
      if (folder?.type !== 'folder' || !folder.children) return null;

      // Mirror exact RadialWheel.jsx outerWedges geometry
      const slotStep = 360 / MAX_SLOTS;
      const parentStart = slotStep * folderIdx - 90 - slotStep / 2;
      const parentEnd = parentStart + slotStep;
      const parentBisector = (parentStart + parentEnd) / 2;
      const childCount = folder.children.length;
      const totalChildren = Math.max(childCount + 1, 1);
      const minArcPerChild = 22;
      const desiredArc = Math.max(slotStep, totalChildren * minArcPerChild);
      const childArc = Math.min(desiredArc, 180);
      const arcStart = parentBisector - childArc / 2;

      const relAngle = ((atan2Deg - arcStart) % 360 + 360) % 360;
      if (relAngle < childArc) {
        const childIdx = Math.floor(relAngle / (childArc / totalChildren));
        if (childIdx >= 0 && childIdx < totalChildren) {
          return { ring: 'outer', index: childIdx, folderId: expandedRadialFolder };
        }
      }
    }

    return null;
  }, [expandedRadialFolder, radialMenuItems]);

  const handleRadialDragStart = useCallback((event) => {
    if (activeView !== 'radial') return;
    const { active, activatorEvent } = event;
    const data = active.data?.current;
    radialDragActivatorRef.current = activatorEvent || null;
    setRadialActiveDrag({
      id: active.id,
      kind: data?.kind || 'library-card',
      label: data?.folderName || String(active.id).split('::').pop() || '',
    });
  }, [activeView]);

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

    if (data?.kind === 'library-folder') {
      handleAddRadialMenuFolder(data.folderName || 'New folder', idx);
    } else if ((data?.kind === 'library-card') && data?.storageKey) {
      handleAddRadialMenuItem(data.storageKey, null, idx);
    }
  }, [activeView, radialMenuItems, expandedRadialFolder, hitTestWedge, handleAddRadialMenuItem, handleAddRadialMenuFolder, handleAddChildToFolder]);

  const handleRadialDragCancel = useCallback(() => {
    setRadialActiveDrag(null);
    setRadialDropTarget(-1);
    setRadialDropTargetOuter(-1);
  }, []);

  // ── Search overlay settings ───────────────────────────────
  const handleUpdateSearchSettings = useCallback((patch) => {
    if (patch.searchOverlayHotkey      !== undefined) setSearchOverlayHotkey(patch.searchOverlayHotkey);
    if (patch.overlayShowAll           !== undefined) setOverlayShowAll(patch.overlayShowAll);
    if (patch.overlayCloseAfterFiring  !== undefined) setOverlayCloseAfterFiring(patch.overlayCloseAfterFiring);
    if (patch.overlayIncludeAutocorrect !== undefined) setOverlayIncludeAutocorrect(patch.overlayIncludeAutocorrect);
    window.electronAPI?.updateSearchSettings(patch);
  }, []);

  const handleToggleMacrosOnStartup = useCallback((val) => {
    setMacrosEnabledOnStartup(val);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup: val, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled]);

  const handleDismissWelcome = useCallback(() => {
    setShowWelcome(false);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true });
  }, [assignments, profiles, activeProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  const handleOnboardingComplete = useCallback(() => {
    setShowOnboarding(false);
    window.electronAPI?.saveConfig({ onboarding_complete: true, hasSeenWelcome: true });
  }, []);

  const handleRestartOnboarding = useCallback(() => {
    setShowSettings(false);
    window.electronAPI?.resetOnboarding();
    setShowOnboarding(true);
  }, []);

  const handleDismissTips = useCallback(() => {
    setTipsHidden(true);
    window.electronAPI?.saveConfig({ tipsHidden: true });
  }, []);

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
    // Reset interaction state so the sidebar and MacroPanel start clean
    setSelectedKey(null);
    setActiveModifiers([]);
    // Apply all imported state
    const imported = cfg.assignments || {};
    const importedHotkeyCount    = Object.keys(imported).filter(k => !k.startsWith('GLOBAL::EXPANSION::')).length;
    const importedExpansionCount = Object.keys(imported).length - importedHotkeyCount;
    console.log(`[KeyForge] Import applied — ${Object.keys(imported).length} assignments (${importedHotkeyCount} hotkeys, ${importedExpansionCount} expansions)`);
    setAssignments(imported);
    setProfiles(cfg.profiles?.length ? cfg.profiles : ['Default']);
    setActiveProfile(cfg.activeProfile || 'Default');
    setProfileSettings(cfg.profileSettings || {});
    const importedTheme = cfg.theme || 'dark';
    setTheme(importedTheme);
    document.documentElement.setAttribute('data-theme', importedTheme);
    setExpansionCategories(cfg.expansionCategories || []);
    const importedAc = cfg.autocorrectEnabled ?? false;
    setAutocorrectEnabled(importedAc);
    window.electronAPI?.updateAutocorrectEnabled(importedAc);
    setMacrosEnabledOnStartup(cfg.macrosEnabledOnStartup ?? true);
    // main.js already wrote the imported config to disk — only sync the engine
    window.electronAPI?.updateAssignments(imported, cfg.activeProfile || 'Default');
    window.electronAPI?.updateProfileSettings(cfg.profileSettings || {});
    showNotification('Config imported successfully');
    setShowSettings(false);
  }, [showNotification]);

  const handleRestoreBackup = useCallback(async (filename) => {
    const result = await window.electronAPI?.restoreBackup(filename);
    if (!result?.ok) {
      showNotification(result?.error || 'Restore failed', 'info');
      return;
    }
    const cfg = result.config;
    // Reset interaction state so the sidebar and MacroPanel start clean
    setSelectedKey(null);
    setActiveModifiers([]);
    const restored = cfg.assignments || {};
    setAssignments(restored);
    setProfiles(cfg.profiles?.length ? cfg.profiles : ['Default']);
    setActiveProfile(cfg.activeProfile || 'Default');
    setProfileSettings(cfg.profileSettings || {});
    const restoredTheme = cfg.theme || 'dark';
    setTheme(restoredTheme);
    document.documentElement.setAttribute('data-theme', restoredTheme);
    setExpansionCategories(cfg.expansionCategories || []);
    const restoredAc = cfg.autocorrectEnabled ?? false;
    setAutocorrectEnabled(restoredAc);
    window.electronAPI?.updateAutocorrectEnabled(restoredAc);
    setMacrosEnabledOnStartup(cfg.macrosEnabledOnStartup ?? true);
    window.electronAPI?.saveConfig({ ...cfg, hasSeenWelcome: true });
    window.electronAPI?.updateAssignments(restored, cfg.activeProfile || 'Default');
    window.electronAPI?.updateProfileSettings(cfg.profileSettings || {});
    setBackupRestoredFrom(null);
    showNotification('Config restored from backup');
    setShowSettings(false);
  }, [showNotification]);

  // Whether the active profile has an app linked (enables Bare Keys mode)
  const profileLinked = !!(profileSettings[activeProfile]?.linkedApp);

  // True when at least one non-expansion, non-autocorrect assignment exists (any profile/layer)
  const hasAnyAssignments = Object.keys(assignments).some(
    k => !k.includes('::EXPANSION::') && !k.includes('::AUTOCORRECT::')
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

  // Count assignments for current profile (all combos, excluding expansions)
  const profileAssignmentCount = Object.keys(assignments)
    .filter(k => k.startsWith(activeProfile + '::') && !k.includes('::EXPANSION::')).length;

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

  return (
    <div className="app">
      {showOnboarding && (
        <OnboardingTour
          assignments={assignments}
          onComplete={handleOnboardingComplete}
          onSkip={handleOnboardingComplete}
          onAreaChange={handleSetArea}
        />
      )}
      {showWelcome && !showOnboarding && (
        <WelcomeModal onDismiss={handleDismissWelcome} />
      )}
      {upgradePrompt && (
        <UpgradeModal featureName={upgradePrompt} onClose={() => setUpgradePrompt(null)} />
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
                <span className="update-banner__text">Trigr {updateInfo.version} ready — click to install and relaunch</span>
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
                  Downloading Trigr {updateInfo.version}
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
                  Trigr {updateInfo.version} available
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
        onToggleTheme={handleToggleTheme}
        onOpenSettings={() => setShowSettings(v => !v)}
        settingsOpen={showSettings}
        activeArea={activeArea}
        onAreaChange={handleSetArea}
        listViewActive={listViewActive}
        onToggleListView={handleToggleListView}
        activeProfile={activeProfile}
        onImportTemplate={handleImportTemplate}
        onImportCadTemplate={handleImportCadTemplate}
        onShowNotification={showNotification}
      />
      <DndContext
        sensors={radialSensors}
        onDragStart={handleRadialDragStart}
        onDragMove={handleRadialDragMove}
        onDragEnd={handleRadialDragEnd}
        onDragCancel={handleRadialDragCancel}
      >
      <div className="app-body">
        {/* Sidebar only visible in Mapping area */}
        {activeArea === 'mapping' && (
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
            activeView={activeView}
            radialMenuItems={radialMenuItems}
            isPro={isPro}
            onShowUpgrade={showUpgrade}
          />
        )}
        <main className={`main-area${activeArea !== 'mapping' ? ' main-area--expansions' : ''}${listViewActive && activeArea === 'mapping' ? ' main-area--hidden' : ''}`}>
          {activeArea === 'mapping' && !listViewActive && (
            <div className="view-switcher">
              <button
                className={`view-tab${activeView === 'keyboard' ? ' active' : ''}`}
                onClick={() => handleSetView('keyboard')}
                type="button"
              >
                ⌨ Keyboard
              </button>
              <button
                className={`view-tab${activeView === 'mouse' ? ' active' : ''}`}
                onClick={() => handleSetView('mouse')}
                type="button"
              >
                🖱 Mouse
              </button>
              <button
                className={`view-tab${activeView === 'radial' ? ' active' : ''}`}
                onClick={() => handleSetView('radial')}
                type="button"
              >
                &#x25ce; Radial
              </button>
            </div>
          )}
          {activeArea === 'mapping' && activeView === 'keyboard' && !listViewActive && (
            <div className="keyboard-numpad-wrap">
              <KeyboardCanvas
                selectedKey={selectedKey}
                onKeySelect={handleKeySelect}
                getKeyAssignment={getKeyAssignment}
                hasDoubleAssignment={hasDoubleAssignment}
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
              />
            </div>
          )}
          {showTips && (
            <QuickTips onDismiss={handleDismissTips} />
          )}
          {activeArea === 'mapping' && activeView === 'mouse' && (
            <MouseCanvas
              selectedKey={selectedKey}
              onKeySelect={handleKeySelect}
              getKeyAssignment={getKeyAssignment}
              hasDoubleAssignment={hasDoubleAssignment}
              lastFired={lastFired}
              activeModifiers={activeModifiers}
              onToggleModifier={handleToggleModifier}
              profileLinked={profileLinked}
              onAddProfile={handleAddProfile}
              isRecording={isRecording}
              onStartRecord={handleStartRecord}
              onStopRecord={handleStopRecord}
              recordCapture={recordCapture}
            />
          )}
          {activeArea === 'analytics' && (
            <AnalyticsPanel isPro={isPro} />
          )}
          {activeArea === 'clipboard' && (
            <ClipboardPanel
              previewWidth={clipboardPreviewWidth}
              onChangePreviewWidth={(w) => {
                const clamped = Math.max(320, Math.min(1200, Math.round(w)));
                setClipboardPreviewWidth(clamped);
                window.electronAPI?.saveConfig({ clipboardPreviewWidth: clamped });
              }}
            />
          )}
          {activeArea === 'mapping' && activeView === 'radial' && (
            <RadialEditorView
              radialMenuHotkey={radialMenuHotkey}
              onSetRadialMenuHotkey={handleSetRadialMenuHotkey}
              onClearRadialMenuHotkey={handleClearRadialMenuHotkey}
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
            />
          )}
          {activeArea === 'templates' && (
            <SearchTemplatesPanel
              searchTemplates={searchTemplates}
              categories={searchTemplateCategories}
              isPro={isPro}
              onAdd={handleAddSearchTemplate}
              onUpdate={handleUpdateSearchTemplate}
              onDelete={handleDeleteSearchTemplate}
              onAddCategory={handleAddSearchTemplateCategory}
              onRenameCategory={handleRenameSearchTemplateCategory}
              onDeleteCategory={handleDeleteSearchTemplateCategory}
              onUpdateCategoryColour={handleUpdateSearchTemplateCategoryColour}
              onReorderCategories={handleReorderSearchTemplateCategories}
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
              globalInputMethod={globalInputMethod}
              onShowNotification={showNotification}
              onShowUpgrade={showUpgrade}
            />
          )}
          {activeArea === 'expansions' && (
            // Phase 3: Text Expansions will eventually support its own profile bar
            // for per-app or team expansion profiles.  For now a single global set.
            <TextExpansions
              expansions={expansions}
              onAdd={handleAddExpansion}
              onDelete={handleDeleteExpansion}
              categories={expansionCategories}
              onAddCategory={handleAddCategory}
              onDeleteCategory={handleDeleteCategory}
              onReorderCategories={handleReorderCategories}
              onUpdateCategoryColour={handleUpdateCategoryColour}
              onRenameCategory={handleRenameCategory}
              autocorrectEnabled={autocorrectEnabled}
              onToggleAutocorrect={handleToggleAutocorrect}
              autocorrections={autocorrections}
              onAddAutocorrect={handleAddAutocorrect}
              onDeleteAutocorrect={handleDeleteAutocorrect}
              globalVariables={globalVariables}
              onSaveGlobalVariables={handleSaveGlobalVariables}
              isPro={isPro}
              onShowUpgrade={showUpgrade}
            />
          )}
        </main>
        {/* Right panel: Settings always accessible; MacroPanel only in Mapping area */}
        {showSettings ? (
          <SettingsPanel
            onClose={() => setShowSettings(false)}
            macrosEnabledOnStartup={macrosEnabledOnStartup}
            onToggleMacrosOnStartup={handleToggleMacrosOnStartup}
            onExportConfig={handleExportConfig}
            onImportConfig={handleImportConfig}
            onRestoreBackup={handleRestoreBackup}
            globalInputMethod={globalInputMethod}
            macroSpeed={macroSpeed}
            keystrokeDelay={keystrokeDelay}
            macroTriggerDelay={macroTriggerDelay}
            doubleTapWindow={doubleTapWindow}
            onUpdateGlobalSettings={handleUpdateGlobalSettings}
            searchOverlayHotkey={searchOverlayHotkey}
            overlayShowAll={overlayShowAll}
            overlayCloseAfterFiring={overlayCloseAfterFiring}
            overlayIncludeAutocorrect={overlayIncludeAutocorrect}
            onUpdateSearchSettings={handleUpdateSearchSettings}
            globalPauseToggleKey={globalPauseToggleKey}
            onSetPauseKey={handleSetPauseKey}
            onClearPauseKey={handleClearPauseKey}
            voiceEnabled={voiceEnabled}
            onToggleVoiceEnabled={handleToggleVoiceEnabled}
            voiceHotkey={voiceHotkey}
            onSetVoiceKey={handleSetVoiceKey}
            onClearVoiceKey={handleClearVoiceKey}
            onRestartOnboarding={handleRestartOnboarding}
            activeProfile={activeProfile}
            onImportTemplate={handleImportTemplate}
            onImportCadTemplate={handleImportCadTemplate}
            isPro={isPro}
            licenceStatus={licenceStatus}
            onLicenceStatusChange={setLicenceStatus}
            onShowUpgrade={showUpgrade}
          />
        ) : activeArea === 'mapping' && activeView === 'radial' && selectedRadialChild != null ? (
          <MacroPanel
            selectedKey={'Folder Child'}
            activeModifiers={[]}
            currentCombo=""
            assignment={(() => {
              const folder = radialMenuItems.find(i => i && i.id === selectedRadialChild.folderId);
              const child = folder?.children?.[selectedRadialChild.childIndex];
              if (!child?.storageKey) return null;
              return assignments[child.storageKey] || null;
            })()}
            doubleAssignment={null}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleRadialChildAssign}
            onClear={handleRadialChildClear}
            onAssignDouble={() => {}}
            onClearDouble={() => {}}
            onClose={() => setSelectedRadialChild(null)}
            onReassign={() => {}}
            onDuplicate={() => {}}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
          />
        ) : activeArea === 'mapping' && activeView === 'radial' && selectedRadialSegment != null ? (
          <MacroPanel
            selectedKey={'Radial Segment'}
            activeModifiers={[]}
            currentCombo=""
            assignment={(() => {
              const item = selectedRadialSegment < radialMenuItems.length ? radialMenuItems[selectedRadialSegment] : null;
              if (!item?.storageKey) return null;
              return assignments[item.storageKey] || null;
            })()}
            doubleAssignment={null}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleRadialAssign}
            onClear={handleRadialClear}
            onAssignDouble={() => {}}
            onClearDouble={() => {}}
            onClose={() => setSelectedRadialSegment(null)}
            onReassign={() => {}}
            onDuplicate={() => {}}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
          />
        ) : activeArea === 'mapping' ? (
          <MacroPanel
            selectedKey={selectedKey}
            activeModifiers={activeModifiers}
            currentCombo={currentCombo}
            assignment={pendingDuplicateRef.current?.single || (selectedKey ? getKeyAssignment(selectedKey) : null)}
            doubleAssignment={pendingDuplicateRef.current?.double || (selectedKey ? getDoubleAssignment(selectedKey) : null)}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleAssign}
            onClear={handleClearKey}
            onAssignDouble={handleAssignDouble}
            onClearDouble={handleClearDouble}
            onClose={() => { pendingDuplicateRef.current = null; setSelectedKey(null); }}
            onReassign={handleReassign}
            onDuplicate={handleDuplicateAssignment}
            isPro={isPro}
            voiceEnabled={voiceEnabled}
            onShowUpgrade={showUpgrade}
          />
        ) : null}
      </div>
      <DragOverlay>
        {radialActiveDrag && (
          <div className="rmp-card rmp-card-overlay">
            <span className="rmp-card-label">{radialActiveDrag.label}</span>
          </div>
        )}
      </DragOverlay>
      </DndContext>
      <StatusBar
        selectedKey={selectedKey}
        currentCombo={currentCombo}
        macrosEnabled={macrosEnabled}
        assignmentCount={profileAssignmentCount}
        notification={notification}
        engineStatus={engineStatus}
        lastFired={lastFired}
        appVersion={appVersion}
        globalPauseToggleKey={globalPauseToggleKey}
      />
    </div>
  );
}

export default App;
