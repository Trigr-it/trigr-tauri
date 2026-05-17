import React, { useState, useEffect, useRef, useCallback } from 'react';
import { Sparkles, LayoutGrid, Keyboard as KeyboardIcon, MessageSquare, Sun, Moon, Monitor, Check } from 'lucide-react';
import './TitleBar.css';
import TemplatesPanel from './TemplatesPanel';
import { openFeedback } from '../utils/feedback';

const AREA_TABS = [
  { key: 'mapping',    label: 'Triggers' },
  { key: 'expansions', label: 'Text Expansion' },
  { key: 'templates',  label: 'Quick Search' },
  { key: 'clipboard',  label: 'Clipboard' },
  { key: 'analytics',  label: 'Analytics' },
];

export default function TitleBar({
  macrosEnabled,
  onToggleMacros,
  // theme is the user's chosen mode: 'auto' | 'light' | 'dark'.
  // resolvedTheme is what's currently rendered: 'light' | 'dark'.
  theme = 'auto',
  resolvedTheme = 'dark',
  onSetTheme,
  onOpenSettings,
  settingsOpen = false,
  activeArea = 'mapping',
  onAreaChange,
  listViewActive = false,
  onToggleListView,
  activeProfile = 'Default',
  onImportTemplate,
  onImportCadTemplate,
  onShowNotification,
  templatesPillRef,
  templatesPillPulse = false,
  openTemplatesSignal = 0,
}) {
  const handleMinimize = () => window.electronAPI?.minimize();
  const handleMaximize = () => window.electronAPI?.maximize();
  const handleClose    = () => window.electronAPI?.close();

  // Theme picker popover state
  const [themePickerOpen, setThemePickerOpen] = useState(false);
  const themePickerRef = useRef(null);
  useEffect(() => {
    if (!themePickerOpen) return;
    function onDocDown(e) {
      if (themePickerRef.current && !themePickerRef.current.contains(e.target)) {
        setThemePickerOpen(false);
      }
    }
    function onKey(e) {
      if (e.key === 'Escape') setThemePickerOpen(false);
    }
    document.addEventListener('mousedown', onDocDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDocDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [themePickerOpen]);

  const themeIcon = theme === 'auto' ? Monitor : (theme === 'light' ? Sun : Moon);
  const ThemeIconComponent = themeIcon;
  const themeOptions = [
    { value: 'auto',  label: 'Follow System', Icon: Monitor },
    { value: 'light', label: 'Light',         Icon: Sun },
    { value: 'dark',  label: 'Dark',          Icon: Moon },
  ];

  // Templates dropdown
  const [templatesDismissed, setTemplatesDismissed] = useState(() => {
    try { return localStorage.getItem('trigr_templates_dismissed') === 'true'; } catch { return false; }
  });
  const [templatesOpen, setTemplatesOpen] = useState(false);
  const [tplCtxMenu, setTplCtxMenu] = useState(null); // { x, y } or null
  const templatesRef = useRef(null);
  const tplCtxRef = useRef(null);

  useEffect(() => {
    if (!templatesOpen && !tplCtxMenu) return;
    function onDown(e) {
      if (templatesOpen && templatesRef.current && !templatesRef.current.contains(e.target)) setTemplatesOpen(false);
      if (tplCtxMenu && tplCtxRef.current && !tplCtxRef.current.contains(e.target)) setTplCtxMenu(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [templatesOpen, tplCtxMenu]);

  const handleDismissTemplates = () => {
    setTemplatesOpen(false);
    setTplCtxMenu(null);
    setTemplatesDismissed(true);
    try { localStorage.setItem('trigr_templates_dismissed', 'true'); } catch {}
    onShowNotification?.('Templates can always be found in Settings', 'info');
  };

  // External "open templates" signal (coachmark "Browse templates" button).
  // Increments a nonce, so each click re-opens even after manual close.
  useEffect(() => {
    if (openTemplatesSignal > 0 && !templatesDismissed) {
      setTemplatesOpen(true);
    }
  }, [openTemplatesSignal]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Responsive tabs → dropdown ──────────────────────────────────────────
  const [tabsCollapsed, setTabsCollapsed] = useState(false);
  const [navDropdownOpen, setNavDropdownOpen] = useState(false);
  const tabsRef = useRef(null);
  const tabsInnerRef = useRef(null);
  const tabsNaturalWidthRef = useRef(0);
  const lastContainerWidthRef = useRef(0);
  const navDropRef = useRef(null);

  // Measure the inner tabs element's actual content width, then compare the
  // wrapper's available width on resize. The inner element is inline-flex so
  // its offsetWidth reflects the true content width, not the flex-stretched
  // container width.
  useEffect(() => {
    const el = tabsRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const containerW = el.clientWidth;
      if (Math.abs(containerW - lastContainerWidthRef.current) < 1) return;
      lastContainerWidthRef.current = containerW;

      // Re-measure the inner tabs content width whenever tabs are visible
      if (!tabsCollapsed && tabsInnerRef.current) {
        const measured = tabsInnerRef.current.offsetWidth;
        if (measured > 0) tabsNaturalWidthRef.current = measured;
      }

      if (tabsNaturalWidthRef.current > 0) {
        setTabsCollapsed(containerW < tabsNaturalWidthRef.current);
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [tabsCollapsed]);

  // Close nav dropdown on outside click
  useEffect(() => {
    if (!navDropdownOpen) return;
    function onDown(e) {
      if (navDropRef.current && !navDropRef.current.contains(e.target)) setNavDropdownOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [navDropdownOpen]);

  const handleNavSelect = useCallback((key) => {
    onAreaChange?.(key);
    setNavDropdownOpen(false);
  }, [onAreaChange]);

  const activeLabel = AREA_TABS.find(t => t.key === activeArea)?.label || 'Triggers';

  return (
    <div className="titlebar" data-drag="true">
      <div className="titlebar-left">
        <div className="app-logo">
          <span className="trigr-mark">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="Trigr">
              <defs>
                <linearGradient id="trigr-base" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#f0b942"/>
                  <stop offset="100%" stopColor="#c8860a"/>
                </linearGradient>
                <linearGradient id="trigr-keytop" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#ffffff"/>
                  <stop offset="100%" stopColor="#e8e5dc"/>
                </linearGradient>
              </defs>
              <rect x="0" y="0" width="64" height="64" rx="9" fill="url(#trigr-base)"/>
              <rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5" fill="url(#trigr-keytop)"/>
              <rect x="7.68" y="46.5" width="48.64" height="3.42" rx="1.5" fill="#000000" opacity="0.06"/>
              <rect x="19" y="20" width="26" height="8" rx="1.5" fill="#c8860a"/>
              <rect x="28" y="24" width="8" height="11" rx="1.5" fill="#c8860a"/>
            </svg>
          </span>
          <span className="app-name">Trigr</span>
        </div>

        <div className="titlebar-divider" />

        {/* Area tabs — collapse to dropdown when space runs out */}
        <div className={`area-tabs-wrap${tabsCollapsed ? ' collapsed' : ''}`} ref={tabsRef}>
          {tabsCollapsed ? (
            <div className="area-nav-dropdown" ref={navDropRef} data-drag="false">
              <button
                className="area-nav-btn"
                onClick={() => setNavDropdownOpen(v => !v)}
                type="button"
              >
                {activeLabel}
                <span className="area-nav-chevron">{navDropdownOpen ? '▴' : '▾'}</span>
              </button>
              {navDropdownOpen && (
                <div className="area-nav-menu">
                  {AREA_TABS.map(t => (
                    <button
                      key={t.key}
                      className={`area-nav-item${t.key === activeArea ? ' active' : ''}`}
                      onClick={() => handleNavSelect(t.key)}
                      type="button"
                    >
                      {t.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className="area-tabs" data-drag="false" ref={tabsInnerRef}>
              {AREA_TABS.map(t => (
                <button
                  key={t.key}
                  className={`area-tab${t.key === activeArea ? ' active' : ''}`}
                  onClick={() => onAreaChange?.(t.key)}
                  type="button"
                >
                  {t.label}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="titlebar-right" data-drag="false">
        {activeArea === 'mapping' && !templatesDismissed && (
          <div className="tb-templates-wrap" ref={templatesRef} data-drag="false">
            <button
              ref={templatesPillRef}
              className={`tb-templates-btn${templatesOpen ? ' active' : ''}${templatesPillPulse ? ' coachmark-pulse' : ''}`}
              onClick={() => setTemplatesOpen(v => !v)}
              onContextMenu={e => { e.preventDefault(); setTplCtxMenu({ x: e.clientX, y: e.clientY }); }}
              title="Starter templates — right-click to dismiss"
              type="button"
            >
<Sparkles size={13} strokeWidth={1.75} style={{ marginRight: 6, verticalAlign: -2 }} /> Templates
            </button>
            {templatesOpen && (
              <div className="tb-templates-dropdown">
                <TemplatesPanel
                  activeProfile={activeProfile}
                  onImportTemplate={onImportTemplate}
                  onImportCadTemplate={onImportCadTemplate}
                />
              </div>
            )}
            {tplCtxMenu && (
              <div
                ref={tplCtxRef}
                className="tb-tpl-ctx-menu"
                style={{ top: tplCtxMenu.y, left: tplCtxMenu.x }}
              >
                <button className="tb-tpl-ctx-item" type="button" onClick={handleDismissTemplates}>
                  Don't show this again
                </button>
              </div>
            )}
          </div>
        )}
        <button
          className="tb-feedback-btn"
          onClick={openFeedback}
          title="Send feedback or vote on the roadmap"
          data-drag="false"
          type="button"
          aria-label="Send feedback"
        >
          <MessageSquare size={13} strokeWidth={1.75} style={{ marginRight: 6, verticalAlign: -2 }} /> Feedback
        </button>
        {activeArea === 'mapping' && (
          <button
            className={`tb-list-toggle${listViewActive ? ' active' : ''}`}
            onClick={onToggleListView}
            title={listViewActive ? 'Switch to keyboard view' : 'Switch to list view'}
            data-drag="false"
            type="button"
            aria-label={listViewActive ? 'Switch to keyboard view' : 'Switch to list view'}
          >
            {listViewActive
              ? <KeyboardIcon size={14} strokeWidth={1.75} />
              : <LayoutGrid size={14} strokeWidth={1.75} />}
          </button>
        )}
        <button
          className={`macro-toggle ${macrosEnabled ? 'enabled' : 'disabled'}`}
          onClick={onToggleMacros}
          title={macrosEnabled ? 'Macros Active — Click to Disable' : 'Macros Paused — Click to Enable'}
        >
          <span className="toggle-dot" />
          {macrosEnabled ? 'ACTIVE' : 'PAUSED'}
        </button>

        <div className="theme-picker-wrap" ref={themePickerRef} data-drag="false">
          <button
            className="theme-toggle-btn"
            onClick={() => setThemePickerOpen(v => !v)}
            title={`Theme: ${theme === 'auto' ? 'Follow System' : theme === 'light' ? 'Light' : 'Dark'}`}
            aria-label="Change theme"
            aria-haspopup="true"
            aria-expanded={themePickerOpen}
            data-drag="false"
          >
            <ThemeIconComponent size={15} strokeWidth={2} />
          </button>
          {themePickerOpen && (
            <div className="theme-picker-popover" role="menu">
              {themeOptions.map(opt => {
                const Icon = opt.Icon;
                const selected = theme === opt.value;
                return (
                  <button
                    key={opt.value}
                    className={`theme-picker-option${selected ? ' selected' : ''}`}
                    onClick={() => { onSetTheme?.(opt.value); setThemePickerOpen(false); }}
                    role="menuitemradio"
                    aria-checked={selected}
                    type="button"
                  >
                    <Icon size={13} strokeWidth={2} className="theme-picker-icon" />
                    <span className="theme-picker-label">{opt.label}</span>
                    {selected && <Check size={12} strokeWidth={2.5} className="theme-picker-check" />}
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <button
          className={`tb-settings-btn${settingsOpen ? ' active' : ''}`}
          onClick={onOpenSettings}
          title="Settings"
          aria-label="Settings"
          data-drag="false"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
          </svg>
        </button>

        <div className="window-controls">
          <button className="wc-btn minimize" onClick={handleMinimize} aria-label="Minimize">
            <svg width="10" height="2" viewBox="0 0 10 2"><rect width="10" height="2" rx="1" fill="currentColor"/></svg>
          </button>
          <button className="wc-btn maximize" onClick={handleMaximize} aria-label="Maximize">
            <svg width="10" height="10" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1.5" stroke="currentColor" strokeWidth="1.5" fill="none"/></svg>
          </button>
          <button className="wc-btn close" onClick={handleClose} aria-label="Close">
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            </svg>
          </button>
        </div>
      </div>
    </div>
  );
}
