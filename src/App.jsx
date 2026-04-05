import React, { useState, useCallback, useEffect, useRef } from 'react';
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
import OnboardingTour from './components/OnboardingTour';
import QuickTips from './components/QuickTips';
import AnalyticsPanel from './components/AnalyticsPanel';

function App() {
  const [assignments, setAssignments]       = useState({});
  const [selectedKey, setSelectedKey]       = useState(null);
  const [activeProfile, setActiveProfile]   = useState('Default');
  const [profiles, setProfiles]             = useState(['Default']);
  const [profileSettings, setProfileSettings] = useState({}); // { profileName: { linkedApp: '...' } }
  const [macrosEnabled, setMacrosEnabled]   = useState(true);
  const [notification, setNotification]     = useState(null);
  const [activeModifiers, setActiveModifiers] = useState([]);  // e.g. ['Ctrl', 'Alt']
  const [engineStatus, setEngineStatus]     = useState({ uiohookAvailable: false, nutjsAvailable: false });
  const [lastFired, setLastFired]           = useState(null);
  const [theme, setTheme]                   = useState('dark');
  const [expansionCategories, setExpansionCategories] = useState([]);
  const [globalVariables, setGlobalVariables]         = useState({});   // { 'my.name': 'Rory Brady', … }
  const [activeView, setActiveView]                 = useState('keyboard'); // 'keyboard' | 'mouse'
  const [activeArea, setActiveArea]                 = useState('mapping');  // 'mapping' | 'expansions' | 'analytics'
  const [numpadOpen, setNumpadOpen]                 = useState(false);
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
  const [keystrokeDelay,     setKeystrokeDelay]     = useState(30);
  const [macroTriggerDelay,  setMacroTriggerDelay]  = useState(150);
  const [searchOverlayHotkey,       setSearchOverlayHotkey]       = useState('Ctrl+Space');
  const [overlayShowAll,             setOverlayShowAll]             = useState(true);
  const [overlayCloseAfterFiring,    setOverlayCloseAfterFiring]    = useState(true);
  const [overlayIncludeAutocorrect,  setOverlayIncludeAutocorrect]  = useState(false);
  const [doubleTapWindow,            setDoubleTapWindow]            = useState(300);
  const [updateInfo,     setUpdateInfo]     = useState(null);   // { version, percent, ready, dismissed }
  const [appVersion,     setAppVersion]     = useState('');
  const [globalPauseToggleKey, setGlobalPauseToggleKey] = useState(null);
  const [listViewActive, setListViewActive]             = useState(() => {
    try { return localStorage.getItem('trigr_list_view') === 'true'; } catch { return false; }
  });


  // Current modifier combo string e.g. "Ctrl+Alt"
  const currentCombo = comboString(activeModifiers);

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
        setKeystrokeDelay(   config.keystrokeDelay      ?? 30);
        setMacroTriggerDelay(config.macroTriggerDelay   ?? 150);
        setDoubleTapWindow(  config.doubleTapWindow     ?? 300);
        // Always start on the Mapping view — do not restore last-used view/area
        setNumpadOpen(              config.numpadOpen               ?? false);
        setSearchOverlayHotkey(     config.searchOverlayHotkey      || 'Ctrl+Space');
        setGlobalPauseToggleKey(    config.globalPauseToggleKey     ?? null);
        setOverlayShowAll(          config.overlayShowAll            ?? true);
        setOverlayCloseAfterFiring( config.overlayCloseAfterFiring   ?? true);
        setOverlayIncludeAutocorrect(config.overlayIncludeAutocorrect ?? false);
        // Sync new settings to engine on load
        window.electronAPI?.updateGlobalSettings({
          globalInputMethod: config.globalInputMethod  || 'direct',
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
      window.electronAPI.onProfileSwitched(({ profile, profileSettings: ps }) => {
        setActiveProfile(profile);
        setSelectedKey(null);
        setActiveModifiers(prev =>
          prev.includes('BARE') && !(ps || {})[profile]?.linkedApp ? [] : prev
        );
      });
      window.electronAPI.onOverlayFired?.((data) => {
        showNotification(`⚡ ${data.label || 'Macro fired'}`);
      });
      window.electronAPI.onHotkeyRecorded?.((data) => {
        setIsRecording(false);
        if (!data) return; // Escape — cancelled
        const { modifiers, keyId } = data;
        // No modifiers → treat as BARE key layer
        const mods = modifiers.length === 0 ? ['BARE'] : modifiers;
        setActiveModifiers(mods);
        setSelectedKey(keyId);
        if (keyId.startsWith('MOUSE_')) setActiveView('mouse');
        else setActiveView('keyboard');
        const modsLabel = modifiers.length === 0 ? 'Bare' : modifiers.join('+');
        const keyLabel  = keyId.startsWith('Key') ? keyId.slice(3)
          : keyId.startsWith('Digit') ? keyId.slice(5) : keyId;
        setRecordCapture(`${modsLabel}+${keyLabel}`);
        setTimeout(() => setRecordCapture(null), 2000);
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
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [selectedKey, activeModifiers]);

  // ── Sync to engine whenever assignments/profile changes ───
  const syncEngine = useCallback((newAssignments, profile) => {
    window.electronAPI?.updateAssignments(newAssignments, profile);
  }, []);

  const saveConfig = useCallback((newAssignments, newProfiles, newProfile) => {
    window.electronAPI?.saveConfig({ assignments: newAssignments, profiles: newProfiles, activeProfile: newProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables });
    syncEngine(newAssignments, newProfile);
  }, [syncEngine, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, globalVariables]);

  const handleSaveGlobalVariables = useCallback((newVars) => {
    setGlobalVariables(newVars);
    window.electronAPI?.updateGlobalVariables(newVars);
    window.electronAPI?.saveConfig({ assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup, hasSeenWelcome: true, globalVariables: newVars });
  }, [assignments, profiles, activeProfile, activeGlobalProfile, profileSettings, theme, expansionCategories, autocorrectEnabled, macrosEnabledOnStartup]);

  // ── Notifications ─────────────────────────────────────────
  const showNotification = useCallback((msg, type = 'success') => {
    setNotification({ msg, type });
    setTimeout(() => setNotification(null), 2500);
  }, []);

  // ── Modifier toggling ─────────────────────────────────────
  const handleToggleModifier = useCallback((modId) => {
    setActiveModifiers(prev => {
      if (modId === 'BARE') {
        // BARE is exclusive — toggle on/off, can't combine with regular modifiers
        return prev.includes('BARE') ? [] : ['BARE'];
      }
      // Regular modifier — strip BARE first if active, then toggle
      const base = prev.filter(m => m !== 'BARE');
      if (base.includes(modId)) return base.filter(m => m !== modId);
      if (base.length >= 3) return base;
      return [...base, modId];
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
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
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
    // Clear BARE layer if the new profile has no linked app
    setActiveModifiers(prev =>
      prev.includes('BARE') && !profileSettings[profile]?.linkedApp ? [] : prev
    );
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
    }))
    .sort((a, b) => a.trigger.localeCompare(b.trigger));

  // editorValue is { html, text } from the rich text editor.
  // originalTrigger is provided when editing an existing expansion; if it differs
  // from trigger the old key is removed in the same update (single atomic write).
  const handleAddExpansion = useCallback((trigger, editorValue, originalTrigger, category, triggerMode, displayName) => {
    const newAssignments = { ...assignments };
    if (originalTrigger && originalTrigger !== trigger) {
      delete newAssignments[`GLOBAL::EXPANSION::${originalTrigger}`];
    }
    newAssignments[`GLOBAL::EXPANSION::${trigger}`] = {
      type: 'expansion',
      label: displayName || `Expand: ${trigger}`,
      data: { html: editorValue.html, text: editorValue.text, category: category || null, triggerMode: triggerMode || 'space', displayName: displayName || null },
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
  const handleCopyToProfile = useCallback((targetProfile) => {
    const oldKey = makeAssignmentKey(activeProfile, currentCombo, selectedKey);
    const newKey = makeAssignmentKey(targetProfile, currentCombo, selectedKey);
    const newAssignments = { ...assignments, [newKey]: assignments[oldKey] };
    setAssignments(newAssignments);
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification(`Copied to "${targetProfile}" profile`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  const handleMoveToProfile = useCallback((targetProfile) => {
    const oldKey = makeAssignmentKey(activeProfile, currentCombo, selectedKey);
    const newKey = makeAssignmentKey(targetProfile, currentCombo, selectedKey);
    const newAssignments = { ...assignments, [newKey]: assignments[oldKey] };
    delete newAssignments[oldKey];
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
    // Move single assignment
    newAssignments[newKey] = newAssignments[oldKey];
    delete newAssignments[oldKey];
    // Move double assignment if it exists
    if (newAssignments[oldDoubleKey]) {
      newAssignments[newDoubleKey] = newAssignments[oldDoubleKey];
      delete newAssignments[oldDoubleKey];
    }
    setAssignments(newAssignments);
    const newMods = newCombo ? newCombo.split('+').filter(Boolean) : [];
    setActiveModifiers(newMods);
    setSelectedKey(newKeyId);
    // Always show keyboard view after reassigning a keyboard key
    if (!newKeyId.startsWith('MOUSE_')) setActiveView('keyboard');
    saveConfig(newAssignments, profiles, activeProfile);
    showNotification('Hotkey reassigned');
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
    setAssignments(newAssignments);
    const newMods = newCombo === 'BARE' ? ['BARE'] : (newCombo ? newCombo.split('+').filter(Boolean) : []);
    setActiveModifiers(newMods);
    setSelectedKey(newKeyId);
    if (!newKeyId.startsWith('MOUSE_')) setActiveView('keyboard');
    saveConfig(newAssignments, profiles, activeProfile);
    const keyLabel = newKeyId.startsWith('Key') ? newKeyId.slice(3)
      : newKeyId.startsWith('Digit') ? newKeyId.slice(5)
      : newKeyId;
    const comboLabel = newCombo === 'BARE' ? keyLabel : `${newCombo}+${keyLabel}`;
    showNotification(`Duplicated to ${comboLabel}`);
  }, [assignments, activeProfile, currentCombo, selectedKey, profiles, saveConfig, showNotification, makeAssignmentKey]);

  // ── View switching (keyboard ↔ mouse, within Mapping area) ─
  const handleSetView = useCallback((view) => {
    setActiveView(view);
    setSelectedKey(null);
  }, []);

  // ── Numpad slide-out toggle ───────────────────────────────
  const handleToggleNumpad = useCallback(() => {
    const next = !numpadOpen;
    setNumpadOpen(next);
    window.electronAPI?.saveConfig({ numpadOpen: next });
  }, [numpadOpen]);

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
    // Switch view to match the selected key type
    setActiveView(keyId.startsWith('MOUSE_') ? 'mouse' : 'keyboard');
  }, []);

  // Sidebar combo-tab clicks update the sidebar's display filter only.
  // Modifier state is left untouched so clicking a tab never clears the
  // modifier layer the user selected via the keyboard modifier bar.
  const handleSelectCombo = useCallback((_comboStr) => {
    setSelectedKey(null);
  }, []);

  // ── Settings handlers ─────────────────────────────────────
  const handleUpdateGlobalSettings = useCallback((patch) => {
    const next = {
      globalInputMethod:  patch.globalInputMethod  ?? globalInputMethod,
      keystrokeDelay:     patch.keystrokeDelay     ?? keystrokeDelay,
      macroTriggerDelay:  patch.macroTriggerDelay  ?? macroTriggerDelay,
      doubleTapWindow:    patch.doubleTapWindow     ?? doubleTapWindow,
    };
    setGlobalInputMethod(next.globalInputMethod);
    setKeystrokeDelay(next.keystrokeDelay);
    setMacroTriggerDelay(next.macroTriggerDelay);
    setDoubleTapWindow(next.doubleTapWindow);
    window.electronAPI?.updateGlobalSettings(next);
  }, [globalInputMethod, keystrokeDelay, macroTriggerDelay, doubleTapWindow]);

  // ── Global pause toggle ───────────────────────────────────
  const handleSetPauseKey = useCallback(async (combo) => {
    setGlobalPauseToggleKey(combo);
    await window.electronAPI?.setPauseHotkey(combo);
  }, []);

  const handleClearPauseKey = useCallback(() => {
    setGlobalPauseToggleKey(null);
    window.electronAPI?.clearPauseHotkey();
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
        />
      )}
      {showWelcome && !showOnboarding && (
        <WelcomeModal onDismiss={handleDismissWelcome} />
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
      />
      <div className="app-body">
        {/* Sidebar only visible in Mapping area */}
        {activeArea === 'mapping' && (
          <Sidebar
            activeProfile={activeProfile}
            assignments={assignments}
            activeModifiers={activeModifiers}
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
            listViewActive={listViewActive}
            isRecording={isRecording}
            onStartRecord={handleStartRecord}
            onStopRecord={handleStopRecord}
            recordCapture={recordCapture}
            onToggleModifier={handleToggleModifier}
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
                numpadOpen={numpadOpen}
                onToggleNumpad={handleToggleNumpad}
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
            <AnalyticsPanel />
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
            onRestartOnboarding={handleRestartOnboarding}
          />
        ) : activeArea === 'mapping' ? (
          <MacroPanel
            selectedKey={selectedKey}
            activeModifiers={activeModifiers}
            currentCombo={currentCombo}
            assignment={selectedKey ? getKeyAssignment(selectedKey) : null}
            doubleAssignment={selectedKey ? getDoubleAssignment(selectedKey) : null}
            assignments={assignments}
            activeProfile={activeProfile}
            profiles={profiles}
            profileLinked={profileLinked}
            globalInputMethod={globalInputMethod}
            onAssign={handleAssign}
            onClear={handleClearKey}
            onAssignDouble={handleAssignDouble}
            onClearDouble={handleClearDouble}
            onClose={() => setSelectedKey(null)}
            onReassign={handleReassign}
            onDuplicate={handleDuplicateAssignment}
            onCopyToProfile={handleCopyToProfile}
            onMoveToProfile={handleMoveToProfile}
          />
        ) : null}
      </div>
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
