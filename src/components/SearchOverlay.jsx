import React, { useState, useEffect, useRef, useLayoutEffect, useMemo, useCallback } from 'react';
import {
  Mic, Check, X, AlertTriangle, Search, CornerDownLeft,
  Type, Keyboard, AppWindow, Globe, FolderOpen, Layers, Edit2, GripVertical,
} from 'lucide-react';
import './SearchOverlay.css';
import { friendlyKeyName } from './keyboardLayout';
import { readVoicePhrases } from '../voicePhrases';
import useOverlayDrag from './useOverlayDrag';

// Seed the theme from the last session before first paint so a cold boot or a
// lost data payload doesn't flash (or stay) dark for a light-theme user. Same
// key the clipboard popup writes — one theme cache for all overlays.
try {
  document.documentElement.setAttribute('data-theme', localStorage.getItem('trigr_overlay_theme') || 'dark');
} catch { /* storage unavailable — CSS :root default applies */ }

// Per-item search haystack cache (see the scoring loop).
const HAYSTACK_CACHE = new WeakMap();

// ── Type metadata ──────────────────────────────────────────────────────────────

const TYPE_META = {
  text:       { Icon: Type,        color: 'var(--type-text)' },
  hotkey:     { Icon: Keyboard,    color: 'var(--type-hotkey)' },
  app:        { Icon: AppWindow,   color: 'var(--type-app)' },
  url:        { Icon: Globe,       color: 'var(--type-url)' },
  folder:     { Icon: FolderOpen,  color: 'var(--type-folder)' },
  macro:      { Icon: Layers,      color: 'var(--type-macro)' },
  expansion:  { Icon: CornerDownLeft, color: 'var(--type-expansion)' },
  autocorrect:{ Icon: Edit2,       color: 'var(--text-muted)' },
};

const GROUP_ORDER = ['assignment', 'quickaction', 'expansion', 'autocorrect'];
const GROUP_LABELS = {
  assignment:  'MACROS & HOTKEYS',
  quickaction: 'QUICK ACTIONS',
  expansion:   'TEXT EXPANSIONS',
  autocorrect: 'AUTOCORRECT',
};

// ── comboLabel builder ─────────────────────────────────────────────────────────

function buildComboLabel(combo, keyId) {
  const keyPart = friendlyKeyName(keyId);
  if (combo === 'BARE' || combo === '') {
    return `${keyPart} (bare)`;
  }
  return `${combo}+${keyPart}`;
}

// ── preview builder ────────────────────────────────────────────────────────────

function buildPreview(macro) {
  if (!macro || !macro.data) return '';
  const d = macro.data;
  switch (macro.type) {
    case 'text':
      return (d.text || '').substring(0, 40);
    case 'hotkey': {
      const parts = [];
      if (d.ctrl)  parts.push('Ctrl');
      if (d.alt)   parts.push('Alt');
      if (d.shift) parts.push('Shift');
      if (d.win)   parts.push('Win');
      if (d.key)   parts.push(d.key);
      return parts.join('+');
    }
    case 'app':
      if (d.appName) return d.appName;
      if (d.appPath) return d.appPath.split(/[\\/]/).pop();
      return '';
    case 'url':
      return d.urlName || d.url || '';
    case 'folder':
      if (d.folderName) return d.folderName;
      if (d.folderPath) return d.folderPath.split(/[\\/]/).pop();
      return '';
    case 'macro': {
      const steps = Array.isArray(d.steps) ? d.steps.length : 0;
      return `Sequence (${steps} step${steps !== 1 ? 's' : ''})`;
    }
    default:
      return '';
  }
}

// ── buildItems ─────────────────────────────────────────────────────────────────

function buildItems(data) {
  const { assignments, activeProfile, globalInputMethod, settings } = data;
  const { includeAutocorrect } = settings || {};
  const items = [];

  for (const [storageKey, macro] of Object.entries(assignments || {})) {
    if (storageKey.startsWith(`${activeProfile}::`)) {
      // Regular key assignment: Profile::combo::keyId
      const parts = storageKey.split('::');
      if (parts.length < 3) continue;
      // parts[0] = profile, parts[1] = combo, parts[2] = keyId
      const combo   = parts[1];
      const keyId   = parts[2];
      // Unassigned library entries ("{Profile}::UNASSIGNED::{uuid}") have no
      // trigger but fire fine by storage key — list them with a plain
      // "Unassigned" tag. Variant stubs (::double / ::hold) are represented
      // by their base entry when one exists; an entry unassigned from a
      // double-only or hold-only key has NO base, so its first variant
      // stands in (full suffixed key still resolves at fire time).
      if (combo === 'UNASSIGNED') {
        if (!macro) continue;
        if (parts.length > 3) {
          const baseKey = parts.slice(0, 3).join('::');
          if (assignments[baseKey]) continue; // base entry represents it
          if (parts[3] === 'hold' && assignments[baseKey + '::double']) continue;
        }
        items.push({
          type:       'assignment',
          storageKey,
          combo,
          keyId,
          comboLabel: 'Unassigned',
          assignType: macro.type,
          label:      macro.label || '',
          preview:    buildPreview(macro),
          voicePhrases: [],
        });
        continue;
      }
      const comboLabel = buildComboLabel(combo, keyId);
      items.push({
        type:       'assignment',
        storageKey,
        combo,
        keyId,
        comboLabel,
        assignType: macro.type,
        label:      macro.label || '',
        preview:    buildPreview(macro),
        voicePhrases: readVoicePhrases(macro.data),
      });
    } else if (storageKey.startsWith('GLOBAL::EXPANSION::')) {
      const trigger = storageKey.slice('GLOBAL::EXPANSION::'.length);
      const isImage = macro.data?.expansionType === 'image';
      items.push({
        type:    'expansion',
        storageKey,
        trigger,
        label:   macro.data?.displayName || trigger,
        preview: isImage
          ? `[IMG] ${(macro.data?.imagePath || '').split(/[/\\]/).pop() || 'No image'}`
          : (macro.data?.text || '').substring(0, 60),
        text:    macro.data?.text,
        html:    macro.data?.html,
        voicePhrases: readVoicePhrases(macro.data),
      });
    } else if (storageKey.startsWith('GLOBAL::QUICKACTION::')) {
      items.push({
        type:       'quickaction',
        storageKey,
        assignType: macro.type,
        label:      macro.label || '',
        preview:    buildPreview(macro),
        voicePhrases: readVoicePhrases(macro.data),
        appIcon:    macro.data?.appIcon || null,
      });
    } else if (storageKey.startsWith('GLOBAL::AUTOCORRECT::')) {
      if (!includeAutocorrect) continue;
      const typo = storageKey.slice('GLOBAL::AUTOCORRECT::'.length);
      items.push({
        type:    'autocorrect',
        storageKey,
        label:   typo,
        preview: `→ ${macro.data?.correction || ''}`,
        text:    macro.data?.correction,
      });
    }
  }

  return items;
}

// ── scoreMatch ─────────────────────────────────────────────────────────────────

function scoreMatch(text, query) {
  if (!text || !query) return 0;
  const t = text.toLowerCase();
  const q = query.toLowerCase();

  if (t === q) return 5;
  if (t.startsWith(q)) return 4;
  if (t.includes(q)) return 3;

  return 0;
}

// ── searchItems ────────────────────────────────────────────────────────────────

function searchItems(items, query, showAll) {
  if (!query) {
    return [];
  }

  // Tokenize on whitespace; every token must match somewhere in the searched
  // fields (AND logic). This lets queries like "marker pla" match
  // "Marker - Plan Layout" — a contiguous-substring match would miss it
  // because " - " breaks the substring.
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) {
    return [];
  }

  const scored = items
    .map(item => {
      // Fields + joined lowercase haystack are computed once per item object
      // (items are rebuilt per open, so a WeakMap cache is exact). Before,
      // every keystroke re-joined and lowercased every expansion's full body.
      let cached = HAYSTACK_CACHE.get(item);
      if (!cached) {
        const fields = [
          item.label      || '',
          item.preview    || '',
          item.comboLabel || '',
          item.trigger    || '',
          item.text       || '',
        ];
        cached = { fields, haystack: fields.join(' ').toLowerCase() };
        HAYSTACK_CACHE.set(item, cached);
      }
      const { fields, haystack } = cached;

      // Every token must appear somewhere in the joined haystack.
      const allMatch = tokens.every(tok => haystack.includes(tok));
      if (!allMatch) return { item, bestScore: 0 };

      // Ranking signal: best per-field score across tokens. For single-token
      // queries this is identical to the previous scoreMatch behavior, so the
      // existing ordering is preserved for cases that already worked.
      let bestScore = 0;
      for (const f of fields) {
        for (const tok of tokens) {
          const s = scoreMatch(f, tok);
          if (s > bestScore) bestScore = s;
        }
      }

      return { item, bestScore };
    })
    .filter(({ bestScore }) => bestScore > 0)
    .sort((a, b) => {
      // Primary: group order (matches renderGroups visual layout)
      const groupDiff = GROUP_ORDER.indexOf(a.item.type) - GROUP_ORDER.indexOf(b.item.type);
      if (groupDiff !== 0) return groupDiff;
      // Secondary: best score within group
      return b.bestScore - a.bestScore;
    });

  return scored.slice(0, 8).map(({ item }) => item);
}

// ── HighlightMatch ─────────────────────────────────────────────────────────────

function HighlightMatch({ text, query }) {
  if (!query || !text) return <>{text}</>;

  const t = text.toLowerCase();
  const q = query.toLowerCase();

  // Substring match
  const idx = t.indexOf(q);
  if (idx !== -1) {
    return (
      <>
        {text.slice(0, idx)}
        <span className="hl">{text.slice(idx, idx + q.length)}</span>
        {text.slice(idx + q.length)}
      </>
    );
  }

  return <>{text}</>;
}

// ── Levenshtein distance (for fuzzy voice matching) ───────────────────────────

function levenshtein(a, b) {
  const m = a.length, n = b.length;
  const dp = Array.from({ length: m + 1 }, (_, i) => i);
  for (let j = 1; j <= n; j++) {
    let prev = dp[0];
    dp[0] = j;
    for (let i = 1; i <= m; i++) {
      const tmp = dp[i];
      dp[i] = a[i - 1] === b[j - 1] ? prev : 1 + Math.min(prev, dp[i], dp[i - 1]);
      prev = tmp;
    }
  }
  return dp[m];
}

function findBestVoiceMatch(transcript, phraseMap) {
  const t = transcript.toLowerCase().trim();
  if (!t) return null;
  // Exact match
  if (phraseMap[t]) return phraseMap[t];
  // Starts-with match
  for (const [phrase, item] of Object.entries(phraseMap)) {
    if (t.startsWith(phrase) || phrase.startsWith(t)) return item;
  }
  // Contains match
  for (const [phrase, item] of Object.entries(phraseMap)) {
    if (t.includes(phrase) || phrase.includes(t)) return item;
  }
  // Fuzzy: Levenshtein distance < 30% of phrase length
  let bestItem = null, bestDist = Infinity;
  for (const [phrase, item] of Object.entries(phraseMap)) {
    const dist = levenshtein(t, phrase);
    if (dist < phrase.length * 0.3 && dist < bestDist) {
      bestDist = dist;
      bestItem = item;
    }
  }
  return bestItem;
}

// ── Main component ─────────────────────────────────────────────────────────────

export default function SearchOverlay() {
  const { onGripPointerDown, onGripDoubleClick } = useOverlayDrag('search');
  const [query,         setQuery]         = useState('');
  const [allItems,      setAllItems]      = useState([]);
  const [displayItems,  setDisplayItems]  = useState([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [settings,      setSettings]      = useState({
    showAll: false, closeAfterFiring: true, includeAutocorrect: false,
  });
  const [flipUp, setFlipUp] = useState(false);
  const [ready, setReady] = useState(false);

  // ── Search Template state machine ──
  // mode: 'main' (normal search) | 'query' (typing a search template query) | 'voice' (listening)
  const [mode, setMode]                     = useState('main');
  const [activeTemplate, setActiveTemplate] = useState(null);
  const [triggerToken, setTriggerToken]     = useState('');
  const [searchTemplates, setSearchTemplates] = useState([]);

  // ── Voice mode state ──
  const [voiceState, setVoiceState]         = useState('idle'); // 'idle' | 'listening' | 'matched' | 'no-match' | 'error' | 'unsupported'
  const [isSpeaking, setIsSpeaking]         = useState(false);  // gated on WinRT SoundStarted/SoundEnded — drives waveform bars
  const [interimText, setInterimText]       = useState('');
  const [matchedLabel, setMatchedLabel]     = useState('');
  const [examplePhrases, setExamplePhrases] = useState([]); // shown after a no-match
  const [voiceContinuous, _setVoiceContinuousState] = useState(false); // double-tap stay-active mode
  const speakingTailRef     = useRef(null); // setTimeout id for 300ms grace tail after SoundEnded
  const recognitionRef      = useRef(false);  // boolean: is WinRT recognition running
  const voiceTimeoutRef     = useRef(null);
  const voiceContinuousRef  = useRef(false);
  // Synchronized setter: ref updated immediately (synchronous), state queued for re-render.
  // Do NOT do voiceContinuousRef.current = voiceContinuous here — that only runs on render
  // commit and creates a one-cycle lag that causes the "second click closes" race.
  const setVoiceContinuous = useCallback((val) => {
    voiceContinuousRef.current = val;
    _setVoiceContinuousState(val);
  }, []);
  const startListeningRef   = useRef(null);   // ref so async callbacks can call startListening
  const modeRef             = useRef(mode);
  modeRef.current = mode;
  const voiceStateRef       = useRef(voiceState);
  voiceStateRef.current = voiceState;

  const inputRef   = useRef(null);
  const resultsRef = useRef(null);
  const rowRefs    = useRef([]);

  // ── Voice phrase map (built from items with voicePhrases) ──
  // One item can map from multiple alias phrases; each alias lookups to the same item.
  const voicePhraseMap = useMemo(() => {
    const map = {};
    for (const item of allItems) {
      for (const phrase of (item.voicePhrases || [])) {
        const k = phrase.toLowerCase().trim();
        if (k) map[k] = item;
      }
    }
    return map;
  }, [allItems]);

  // Pick N random phrases from the current grammar to surface on a no-match.
  // Helps users learn what's available without exposing the full list.
  const pickExamplePhrases = useCallback((count = 3) => {
    const keys = Object.keys(voicePhraseMapRef.current || {});
    if (keys.length === 0) return [];
    // Fisher–Yates partial shuffle to take min(count, keys.length)
    const out = keys.slice();
    for (let i = out.length - 1; i > 0 && i >= out.length - count; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [out[i], out[j]] = [out[j], out[i]];
    }
    return out.slice(-Math.min(count, out.length)).reverse();
  }, []);

  // When example phrases get set, grow the overlay window so the banner fits.
  // Guarded against firing on stale state by checking voiceState/continuous —
  // a fresh overlay open with examplePhrases=[] doesn't trigger this, and any
  // residual state from before the reset can't accidentally resize the window.
  useEffect(() => {
    if (examplePhrases.length === 0) return;
    const isNoMatch = voiceStateRef.current === 'no-match';
    const isContinuousListening = voiceStateRef.current === 'listening' && voiceContinuousRef.current === true;
    if (isNoMatch || isContinuousListening) {
      window.electronAPI?.voiceOverlayExamplesExpand();
    }
  }, [examplePhrases]);

  const voicePhraseMapRef = useRef(voicePhraseMap);
  voicePhraseMapRef.current = voicePhraseMap;

  // ── Voice recognition functions (WinRT backend via Rust IPC) ──
  const stopListening = useCallback(() => {
    window.electronAPI?.stopVoiceRecognition();
    clearTimeout(voiceTimeoutRef.current);
    recognitionRef.current = false;
  }, []);

  // Ref for fireItem so speech callbacks always have the latest version
  const fireItemRef = useRef(null);

  const startListening = useCallback(() => {
    if (recognitionRef.current) return; // already running

    const phrases = Object.keys(voicePhraseMap);
    if (phrases.length === 0) {
      setVoiceState('error');
      setInterimText('No voice commands configured');
      window.electronAPI?.voiceOverlayErrorExpand();
      setTimeout(() => window.electronAPI?.closeOverlay(), 6000);
      return;
    }

    recognitionRef.current = true;
    setVoiceState('listening');
    setInterimText('');
    setMatchedLabel('');

    // Send phrases to Rust WinRT recognizer
    window.electronAPI?.startVoiceRecognition(phrases);

    // ── Dual-layer voice timeout (Stage 7 of voice overhaul) ──
    // WinRT InitialSilenceTimeout fires at 8s when audio frames arrive but the
    // user stays silent. It does NOT fire if audio frames never arrive at all —
    // e.g. Bluetooth mic mid-session dropout, exclusive mic capture by another
    // app (Teams call starting), OS-level permission revocation during recognition,
    // or USB driver hang. In those edge cases RecognizeAsync.get() blocks
    // indefinitely on the Rust side.
    //
    // This JS-side backstop at 11s (3s past WinRT's 8s) is the only escape from
    // that hung state — it force-stops the recognizer via stopVoiceRecognition().
    // DO NOT delete this timer thinking it's redundant with WinRT's silence
    // timeout; the two layers cover different failure modes.
    voiceTimeoutRef.current = setTimeout(() => {
      stopListening();
      if (voiceContinuousRef.current) {
        setVoiceState('listening');
        setInterimText('');
        recognitionRef.current = false;
        startListeningRef.current?.();
      } else {
        setVoiceState('no-match');
        setInterimText('No speech detected');
        setTimeout(() => window.electronAPI?.closeOverlay(), 1200);
      }
    }, 11000);
  }, [voicePhraseMap, stopListening]);

  // ── Receive data from main process ──
  // One applier for the pushed overlay-search-data payload AND the self-heal
  // pull below, so both paths reset exactly the same state.
  const applySearchData = useCallback((data) => {
    if (!data) return;
    // Apply theme before rendering so colours are correct on first paint
    const theme = data.theme || 'dark';
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem('trigr_overlay_theme', theme); } catch { /* ignore */ }
    const { settings: newSettings } = data;
    setSettings(newSettings || { showAll: false, closeAfterFiring: true, includeAutocorrect: false });
    // flipUp is show-time geometry; the pull payload omits it so the bar keeps
    // whatever the last show decided.
    if (typeof data.flipUp === 'boolean') setFlipUp(data.flipUp);
    const items = buildItems(data);
    setAllItems(items);
    setSearchTemplates(data.searchTemplates || []);
    setQuery('');
    setSelectedIndex(0);
    setMode('main');
    setActiveTemplate(null);
    setTriggerToken('');
    setVoiceState('idle');
    setInterimText('');
    setMatchedLabel('');
    setReady(true);

    // Focus the input each time the overlay opens (data arrives on every show)
    setTimeout(() => inputRef.current?.focus(), 0);
  }, []);

  useEffect(() => {
    if (!window.electronAPI?.onOverlaySearchData) return;
    window.electronAPI.onOverlaySearchData(applySearchData);
  }, [applySearchData]);

  // ── Self-heal pull ────────────────────────────────────────────────────────
  // The pushed payload is fire-and-forget with two known loss windows (same as
  // the clipboard popup, fixed there in v0.8.5): (1) cold start — this lazy
  // chunk may not have registered its listener when the first show emits;
  // (2) resume from webview_mem TrySuspend — the resume/IPC-reconnect race can
  // drop events emitted right after Resume(). Either left Quick Search blank,
  // stale, or dark until closed and reopened. So the overlay pulls its own
  // data at mount (fills only while empty, never clobbers a fresher push) and
  // on the visibilitychange that only fires on a suspend→resume (forced: the
  // pushed payload for that show is exactly what may have been lost). Voice
  // mode is left alone — its payload comes down a different event.
  const allItemsLenRef = useRef(0);
  useEffect(() => { allItemsLenRef.current = allItems.length; }, [allItems]);
  // modeRef (declared above, live mirror of `mode`) gates the voice case.
  const selfHealPull = useCallback((force) => {
    window.electronAPI?.getSearchOverlayData?.()
      .then((data) => {
        if (!data || !data.assignments) return;
        if (!force && allItemsLenRef.current > 0) return;
        if (modeRef.current === 'voice') return;
        applySearchData(data);
      })
      .catch(() => {});
  }, [applySearchData]);

  useEffect(() => { selfHealPull(false); }, [selfHealPull]);

  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState !== 'visible') return;
      selfHealPull(true);
    };
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [selfHealPull]);

  // Mid-session flip change — currently only fired by a position reset while
  // the overlay is open (the bar returns to the default spot, which never
  // flips). Un-flipping re-runs the measure effect, which resizes the fixed
  // full-height flip window back down to its content.
  useEffect(() => {
    window.electronAPI?.onOverlayFlip?.((v) => setFlipUp(!!v));
  }, []);

  // ── Arrow-key handler on window ──
  useEffect(() => {
    function handleKeyDown(e) {
      // In query mode, suppress arrow keys (no result list)
      if (mode === 'query') return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex(i => Math.min(i + 1, displayItems.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex(i => Math.max(i - 1, 0));
      }
    }
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [displayItems.length, mode]);

  // ── Update displayItems when query/allItems/settings change (Main mode only) ──
  useEffect(() => {
    if (mode === 'query' || mode === 'voice') {
      setDisplayItems([]);
      return;
    }
    const results = searchItems(allItems, query, settings.showAll);
    setDisplayItems(results);
    setSelectedIndex(0);
  }, [query, allItems, settings.showAll, mode]);

  // ── Start/stop voice recognition when mode changes ──
  useEffect(() => {
    if (mode === 'voice') {
      // Delay to let overlay render first
      const t = setTimeout(() => startListening(), 200);
      return () => { clearTimeout(t); stopListening(); };
    } else {
      stopListening();
    }
    return () => stopListening();
  }, [mode, startListening, stopListening]);

  // ── Voice mode: press to activate, speak, Keyfire matches and fires ──
  useEffect(() => {
    if (!window.electronAPI?.onOverlayVoiceData) return;
    window.electronAPI.onOverlayVoiceData((data) => {
      document.documentElement.setAttribute('data-theme', data.theme || 'dark');
      const items = buildItems(data);
      setAllItems(items);
      setQuery('');
      setSelectedIndex(0);
      setDisplayItems([]);
      setMode('voice');
      // Reset all voice-session-scoped state so nothing leaks between overlay opens.
      setVoiceState('idle');
      setInterimText('');
      setMatchedLabel('');
      setVoiceContinuous(false);
      setExamplePhrases([]);
      recognitionRef.current = false;
      clearTimeout(voiceTimeoutRef.current);
      setReady(true);
    });
  }, []);

  // ── Continuous mode: click voice pill to go persistent, click again to close ──
  // Backed by SpeechContinuousRecognitionSession on the Rust side — one long-running
  // session emits voice-result events as they happen, no per-utterance restart needed.
  // Cached recognizer is reused (no constraint re-compile).
  const handleVoicePillClick = useCallback(() => {
    if (voiceContinuousRef.current) {
      setVoiceContinuous(false);
      window.electronAPI?.stopVoiceContinuous();
      window.electronAPI?.closeOverlay();
    } else {
      setVoiceContinuous(true);
      window.electronAPI?.setVoiceContinuous(true);
      window.electronAPI?.stopVoiceRecognition();
      const phrases = Object.keys(voicePhraseMapRef.current);
      if (phrases.length > 0) {
        recognitionRef.current = true;
        setVoiceState('listening');
        window.electronAPI?.startVoiceContinuous(phrases);
      }
    }
  }, []);

  // ── Continuous mode: restart listening after each command fires ──
  useEffect(() => {
    if (!window.electronAPI?.onVoiceContinuousRestart) return;
    window.electronAPI.onVoiceContinuousRestart(() => {
      clearTimeout(voiceTimeoutRef.current);
      setVoiceState('listening');
      setInterimText('');
      recognitionRef.current = false;
      startListeningRef.current?.();
    });
  }, []);

  // ── Listen for WinRT voice recognition results from Rust ──
  useEffect(() => {
    if (!window.electronAPI?.onVoiceResult) return;
    window.electronAPI.onVoiceResult((data) => {
      if (modeRef.current !== 'voice') return;
      clearTimeout(voiceTimeoutRef.current);
      recognitionRef.current = false;
      const text = (data.text || '').toLowerCase().trim();
      if (!text) {
        setVoiceState('no-match');
        setInterimText('');
        setExamplePhrases(pickExamplePhrases(3));
        if (voiceContinuousRef.current) {
          setTimeout(() => {
            setVoiceState('listening');
            setInterimText('');
          }, 1000);
        } else {
          setTimeout(() => window.electronAPI?.closeOverlay(), 3000);
        }
        return;
      }
      setInterimText(text);
      const phraseMap = voicePhraseMapRef.current;
      const match = phraseMap[text] || findBestVoiceMatch(text, phraseMap);
      if (match) {
        setVoiceState('matched');
        setMatchedLabel(match.label || match.trigger || '(matched)');
        setExamplePhrases([]);
        setTimeout(() => {
          fireItemRef.current?.(match);
          if (voiceContinuousRef.current) {
            setTimeout(() => {
              setVoiceState('listening');
              setInterimText('');
              setMatchedLabel('');
            }, 600);
          }
        }, 500);
      } else {
        setVoiceState('no-match');
        setExamplePhrases(pickExamplePhrases(3));
        if (voiceContinuousRef.current) {
          setTimeout(() => {
            setVoiceState('listening');
            setInterimText('');
          }, 1200);
        } else {
          setTimeout(() => window.electronAPI?.closeOverlay(), 3000);
        }
      }
    });
  }, []);

  useEffect(() => {
    if (!window.electronAPI?.onVoiceError) return;
    window.electronAPI.onVoiceError((data) => {
      if (modeRef.current !== 'voice') return;
      clearTimeout(voiceTimeoutRef.current);
      recognitionRef.current = false;
      if (data.error === 'no-speech') {
        if (voiceContinuousRef.current) {
          // Continuous session keeps running on its own — just refresh visual state.
          setVoiceState('listening');
          setInterimText('');
        } else {
          setVoiceState('no-match');
          setInterimText('');
          setExamplePhrases(pickExamplePhrases(3));
          setTimeout(() => window.electronAPI?.closeOverlay(), 3000);
        }
      } else {
        // Hard error (mic unavailable, permission, session Completed with non-Success).
        // In continuous mode this means the session died — exit continuous and close.
        if (voiceContinuousRef.current) {
          setVoiceContinuous(false);
          window.electronAPI?.stopVoiceContinuous();
        }
        setVoiceState('error');
        setInterimText(data.error || 'Voice unavailable');
        window.electronAPI?.voiceOverlayErrorExpand();
        setTimeout(() => window.electronAPI?.closeOverlay(), 6000);
      }
    });
  }, []);

  // ── Voice waveform gating (WinRT SoundStarted/SoundEnded events) ──
  // Bars animate only while WinRT reports the user is actively producing sound.
  // 300ms grace tail prevents flicker on mid-word silences. Cleared by voice-mode
  // exit and by transitions out of 'listening' state (match/no-match/error).
  useEffect(() => {
    if (!window.electronAPI?.onVoiceSoundStarted) return;
    window.electronAPI.onVoiceSoundStarted(() => {
      if (modeRef.current !== 'voice') return;
      if (speakingTailRef.current) {
        clearTimeout(speakingTailRef.current);
        speakingTailRef.current = null;
      }
      setIsSpeaking(true);
    });
  }, []);

  useEffect(() => {
    if (!window.electronAPI?.onVoiceSoundEnded) return;
    window.electronAPI.onVoiceSoundEnded(() => {
      if (modeRef.current !== 'voice') return;
      if (speakingTailRef.current) clearTimeout(speakingTailRef.current);
      speakingTailRef.current = setTimeout(() => {
        setIsSpeaking(false);
        speakingTailRef.current = null;
      }, 300);
    });
  }, []);

  // Clear isSpeaking whenever we leave the listening state (matched/no-match/error/idle).
  useEffect(() => {
    if (voiceState !== 'listening') {
      if (speakingTailRef.current) {
        clearTimeout(speakingTailRef.current);
        speakingTailRef.current = null;
      }
      setIsSpeaking(false);
    }
  }, [voiceState]);

  // ── Resize overlay window whenever displayItems or mode change ──
  const panelRef = useRef(null);
  useEffect(() => {
    // Voice mode: Rust already set the correct size/position — skip JS resize
    if (mode === 'voice') return;
    // Flip mode: the window is fixed at full height and the list grows
    // inside the DOM — resizing the window's top edge per keystroke is what
    // made the bar jitter. Rust's overlay_resize also ignores calls while
    // flipped; this guard just avoids the pointless IPC.
    if (flipUp) return;

    // Double-rAF ensures React has committed the DOM update before we measure.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = panelRef.current;
        if (!el) return;
        const scrollH = el.scrollHeight;
        const rect = el.getBoundingClientRect();
        // scrollHeight = full content height (not clipped by overflow)
        // 12 = top margin (matches .search-panel margin); 16 = bottom shadow breathing
        // room — the 0 4px 12px panel shadow reaches ~16px below the panel.
        const windowH = Math.ceil(scrollH + 12 + 16);
        import('@tauri-apps/api/core').then(({ invoke: inv }) =>
          inv('log_debug', { message: `[OVERLAY-JS] scrollH=${scrollH} rectH=${rect.height} top=${rect.top} → windowH=${windowH}` })
        ).catch(() => {});
        window.electronAPI?.resizeOverlay(windowH);
      });
    });
  }, [displayItems, mode, flipUp]);

  // ── Scroll selected row into view ──
  useLayoutEffect(() => {
    const el = rowRefs.current[selectedIndex];
    if (el) el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedIndex]);

  // ── Fire an item ──
  function fireItem(item) {
    if (!item) return;

    const payload = { type: item.type };
    if (item.storageKey) payload.storageKey = item.storageKey;
    if (item.label)      payload.label      = item.label;
    if (item.text  != null) payload.text  = item.text;
    if (item.html  != null) payload.html  = item.html;

    if (voiceContinuousRef.current) {
      // Continuous voice mode — fire without closing overlay
      window.electronAPI?.executeSearchResult(payload);
    } else if (settings.closeAfterFiring) {
      window.electronAPI?.closeOverlay();
      window.electronAPI?.executeSearchResult(payload);
    } else {
      window.electronAPI?.executeSearchResult(payload);
    }
  }
  fireItemRef.current = fireItem;
  startListeningRef.current = startListening;

  // ── Fire search template ──
  function fireSearchTemplate() {
    if (!activeTemplate || !query.trim()) return;
    const payload = {
      type: 'search_template',
      url_template: activeTemplate.url_template,
      query: query.trim(),
      encode_query: activeTemplate.encode_query ?? true,
      label: activeTemplate.label,
      trigger: triggerToken,
    };
    window.electronAPI?.closeOverlay();
    window.electronAPI?.executeSearchResult(payload);
  }

  // ── Input change handler — trigger detection ──
  function handleInputChange(e) {
    const value = e.target.value;

    if (mode === 'query') {
      // Backspace-to-Main: if user clears the entire query, restore trigger in main mode
      if (value === '') {
        setMode('main');
        setQuery(triggerToken);
        setActiveTemplate(null);
        setTriggerToken('');
        return;
      }
      setQuery(value);
      return;
    }

    // Main mode — check for trigger + space
    if (value.endsWith(' ') && searchTemplates.length > 0) {
      const candidate = value.slice(0, -1).trim().toLowerCase();
      if (candidate) {
        const match = searchTemplates.find(t => t.trigger.toLowerCase() === candidate);
        if (match) {
          // Transition to query mode
          setMode('query');
          setActiveTemplate(match);
          setTriggerToken(candidate);
          setQuery('');
          return;
        }
      }
    }

    setQuery(value);
  }

  // ── Input keydown ──
  function handleInputKeyDown(e) {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (mode === 'query') {
        fireSearchTemplate();
      } else {
        fireItem(displayItems[selectedIndex]);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (mode === 'voice') {
        stopListening();
        setMode('main');
        setVoiceState('idle');
        setTimeout(() => inputRef.current?.focus(), 0);
      } else if (mode === 'query') {
        // First Escape: return to Main mode, don't close
        setMode('main');
        setQuery('');
        setActiveTemplate(null);
        setTriggerToken('');
        setTimeout(() => inputRef.current?.focus(), 0);
      } else {
        window.electronAPI?.closeOverlay();
      }
    }
    // ArrowUp/Down handled by window listener
  }

  // ── Grouped rendering ──
  function renderGroups() {
    // Build ordered groups
    const groups = {};
    for (const type of GROUP_ORDER) groups[type] = [];
    for (const item of displayItems) {
      if (groups[item.type]) groups[item.type].push(item);
      else groups[item.type] = [item];
    }

    const nodes = [];
    let rowIdx  = 0;  // flat index into displayItems for selectedIndex tracking

    for (const type of GROUP_ORDER) {
      const groupItems = groups[type];
      if (!groupItems || groupItems.length === 0) continue;

      nodes.push(
        <div className="search-group-header" key={`hdr-${type}`}>
          {GROUP_LABELS[type]}
        </div>
      );

      for (const item of groupItems) {
        const idx      = rowIdx;
        const isSelected = idx === selectedIndex;
        const meta     = item.type === 'assignment'
          ? (TYPE_META[item.assignType] || { Icon: Layers, color: 'var(--text-muted)' })
          : (TYPE_META[item.type]       || { Icon: Layers, color: 'var(--text-muted)' });
        const MetaIcon = meta.Icon;

        nodes.push(
          <div
            key={item.storageKey || `${item.type}-${item.label}`}
            className={`search-result-row${isSelected ? ' selected' : ''}`}
            onClick={() => fireItem(item)}
            ref={el => { rowRefs.current[idx] = el; }}
          >
            {item.appIcon ? (
              <span className="result-type-icon">
                <img src={item.appIcon} alt="" width="14" height="14" style={{ display: 'block' }} draggable={false} />
              </span>
            ) : (
              <span className="result-type-icon" style={{ color: meta.color }}>
                <MetaIcon size={14} strokeWidth={1.75} />
              </span>
            )}
            <div className="result-content">
              <div className="result-label">
                <HighlightMatch text={item.label} query={query} />
              </div>
              {item.preview && (
                <div className="result-preview">
                  <HighlightMatch text={item.preview} query={query} />
                </div>
              )}
            </div>
            {item.comboLabel && item.type === 'assignment' && (
              <span className="result-combo">{item.comboLabel}</span>
            )}
            {item.trigger && item.type === 'expansion' && (
              <span className="result-combo">{item.trigger} + Space</span>
            )}
          </div>
        );

        rowIdx++;
      }
    }

    // Clear stale refs beyond current count
    rowRefs.current = rowRefs.current.slice(0, rowIdx);

    return nodes;
  }

  return (
    <div
      className={`search-overlay${flipUp && mode !== 'voice' ? ' flip-up' : ''}`}
      onMouseDown={e => {
        // Flip mode keeps the window at full height, so there's transparent
        // backdrop above short lists. A click there reads as click-outside —
        // dismiss, matching the mouse hook's behaviour for true outside
        // clicks.
        if (flipUp && mode !== 'voice' && e.target === e.currentTarget) {
          window.electronAPI?.closeOverlay();
        }
      }}
    >
      <div className="search-panel" ref={panelRef}>
        {/* Query mode: template label bar */}
        {mode === 'query' && activeTemplate && (
          <div className="search-template-bar">
            <span className="search-template-label">{activeTemplate.label}</span>
          </div>
        )}

        {/* Voice mode UI — compact square + optional examples banner */}
        {mode === 'voice' ? (
          <div className="search-voice-frame">
            {examplePhrases.length > 0 && (
              <div className="search-voice-examples">
                <div className="search-voice-examples-title">Couldn't catch that — try:</div>
                {examplePhrases.map((p, i) => (
                  <div className="search-voice-examples-row" key={i}>
                    <span className="search-voice-examples-dot">·</span>
                    <span className="search-voice-examples-phrase">"{p}"</span>
                  </div>
                ))}
              </div>
            )}
          <div
            className={`search-voice-pill${isSpeaking ? ' is-speaking' : ''}`}
            onClick={handleVoicePillClick}
            onKeyDown={handleInputKeyDown}
            tabIndex={0}
            role="button"
            aria-label={voiceContinuous ? 'Voice continuous mode — click to close' : 'Voice listening — click for continuous mode'}
            title={voiceContinuous ? 'Click to close' : 'Click for continuous mode'}
          >
            {voiceContinuous && (
              <div className="search-voice-continuous-badge" aria-hidden="true">∞</div>
            )}
            {(voiceState === 'listening' || voiceState === 'idle') && (
              isSpeaking ? (
                /* Waveform — bars dance while WinRT reports SoundStarted */
                <div className="search-voice-bars is-active" aria-hidden="true">
                  {[0, 1, 2, 3, 4].map(i => (
                    <span
                      key={i}
                      className="search-voice-bar"
                      style={{ '--bar-i': i }}
                    />
                  ))}
                </div>
              ) : (
                /* Static mic — shown when ready / between phrases */
                <div className="search-voice-pill-mic" aria-hidden="true">
                  <Mic size={22} strokeWidth={1.75} />
                </div>
              )
            )}
            {voiceState === 'matched' && (
              <span className="search-voice-pill-match-icon" aria-label="Matched">
                <Check size={26} strokeWidth={2.25} />
              </span>
            )}
            {voiceState === 'no-match' && (
              <span className="search-voice-pill-label" aria-label="No match">
                <X size={24} strokeWidth={2} />
              </span>
            )}
            {voiceState === 'error' && (
              <div className="search-voice-error-row">
                <span className="search-voice-error-icon" aria-hidden="true">
                  <AlertTriangle size={16} strokeWidth={2} />
                </span>
                <span className="search-voice-error-text">{interimText || 'Voice error'}</span>
              </div>
            )}
            {voiceState === 'unsupported' && (
              <span className="search-voice-pill-label" aria-label="Unsupported">
                <AlertTriangle size={24} strokeWidth={2} />
              </span>
            )}
          </div>
          </div>
        ) : (
          <>
            <div className="search-input-row">
              <span
                className="search-grip"
                title="Drag to move · Double-click to reset position"
                onPointerDown={onGripPointerDown}
                onDoubleClick={onGripDoubleClick}
                aria-hidden="true"
              >
                <GripVertical size={14} strokeWidth={1.75} />
              </span>
              {mode === 'query' ? (
                <span className="search-back-hint" title="Esc to go back" aria-label="Back">
                  <CornerDownLeft size={16} strokeWidth={1.75} />
                </span>
              ) : (
                <span className="search-icon" aria-hidden="true">
                  <Search size={16} strokeWidth={1.75} />
                </span>
              )}
              <input
                ref={inputRef}
                className="search-input"
                type="text"
                placeholder={
                  mode === 'query' && activeTemplate
                    ? `Search ${activeTemplate.label}…`
                    : 'Search macros, hotkeys, expansions…'
                }
                value={query}
                onChange={handleInputChange}
                onKeyDown={handleInputKeyDown}
                spellCheck={false}
                autoComplete="off"
                autoCorrect="off"
              />
              <span className="search-esc-hint">Esc</span>
            </div>

            {mode === 'main' && displayItems.length > 0 && (
              <div className="search-results" ref={resultsRef}>
                {renderGroups()}
              </div>
            )}

            {mode === 'main' && query && displayItems.length === 0 && ready && (
              <div className="search-empty">No results for "{query}"</div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
