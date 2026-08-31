// Dev-only UI test bridge (window half). Pairs with scripts/vite-dev-bridge.mjs.
//
// Runs in EVERY Keyfire window (main, settings, overlay, ...). Listens for
// `kf:dev` messages on the Vite HMR websocket, executes the request if the
// `target` label matches this window, and replies on `kf:dev:result`.
//
// Built-in functions (all take a CSS selector where noted):
//   __ping                         -> { label, url, visible, width, height, dpr }
//   click(sel, nth=0)              -> real pointer/mouse event sequence + .click()
//   dblclick(sel, nth=0) / contextmenu(sel, nth=0)
//   type(sel, text, nth=0)         -> sets the value the React way (native setter + input event)
//   key(sel|null, key, opts)       -> keydown/keyup on the element (or document.activeElement)
//   text(sel, nth=0) / html(sel, nth=0) / attr(sel, name, nth=0)
//   exists(sel) / count(sel)
//   rect(sel, nth=0)               -> { x, y, width, height, dpr } in CSS px + devicePixelRatio
//   wait(sel, timeoutMs=5000)      -> resolves when the selector matches
//   sleep(ms)
//   scroll(sel, nth=0)             -> scrollIntoView
//   eval(code)                     -> evaluates an expression (awaits promises)
//
// Anything else is looked up on window.__kf_dev, which App.jsx populates in
// dev with its state setters (setArea, setView, setTheme, ...). Apps can add
// more setters there at any time.
//
// `import.meta.hot` only exists under the Vite dev server, so this whole file
// is dead code in production builds — never add a non-HMR transport here.

const WINDOW_PARAMS = ['overlay', 'fillin', 'radialmenu', 'clipboardoverlay', 'settings', 'report', 'countdown', 'snipoverlay'];

function windowLabel() {
  const params = new URLSearchParams(window.location.search);
  for (const p of WINDOW_PARAMS) if (params.get(p) === '1') return p;
  return 'main';
}

function pick(sel, nth = 0) {
  const list = document.querySelectorAll(sel);
  if (!list.length) throw new Error(`no element matches "${sel}"`);
  const el = list[nth < 0 ? list.length + nth : nth];
  if (!el) throw new Error(`"${sel}" has ${list.length} match(es); index ${nth} out of range`);
  return el;
}

function fire(el, type, init = {}) {
  const Ctor = type.startsWith('pointer') ? (window.PointerEvent || MouseEvent) : MouseEvent;
  el.dispatchEvent(new Ctor(type, { bubbles: true, cancelable: true, view: window, ...init }));
}

function clickSequence(el, button = 0) {
  const r = el.getBoundingClientRect();
  const init = { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button, buttons: button === 2 ? 2 : 1 };
  fire(el, 'pointerdown', init);
  fire(el, 'mousedown', init);
  fire(el, 'pointerup', { ...init, buttons: 0 });
  fire(el, 'mouseup', { ...init, buttons: 0 });
}

function setNativeValue(el, value) {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
  if (setter) setter.call(el, value); else el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

const builtins = {
  __ping: () => ({
    label: windowLabel(),
    url: window.location.href,
    visible: document.visibilityState,
    width: window.innerWidth,
    height: window.innerHeight,
    dpr: window.devicePixelRatio,
  }),
  click: (sel, nth = 0) => { const el = pick(sel, nth); el.focus?.(); clickSequence(el); el.click(); return describe(el); },
  dblclick: (sel, nth = 0) => { const el = pick(sel, nth); clickSequence(el); el.click(); clickSequence(el); el.click(); fire(el, 'dblclick'); return describe(el); },
  contextmenu: (sel, nth = 0) => { const el = pick(sel, nth); clickSequence(el, 2); fire(el, 'contextmenu', { button: 2 }); return describe(el); },
  type: (sel, text, nth = 0) => {
    const el = pick(sel, nth);
    el.focus?.();
    if (el.isContentEditable) {
      el.textContent = String(text);
      el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: String(text) }));
    } else {
      setNativeValue(el, String(text));
    }
    return describe(el);
  },
  key: (sel, key, opts = {}) => {
    const el = sel ? pick(sel) : (document.activeElement || document.body);
    const init = { key, code: opts.code || key, bubbles: true, cancelable: true, ctrlKey: !!opts.ctrl, altKey: !!opts.alt, shiftKey: !!opts.shift, metaKey: !!opts.meta };
    el.dispatchEvent(new KeyboardEvent('keydown', init));
    el.dispatchEvent(new KeyboardEvent('keyup', init));
    return describe(el);
  },
  text: (sel, nth = 0) => pick(sel, nth).textContent,
  html: (sel, nth = 0) => pick(sel, nth).outerHTML,
  attr: (sel, name, nth = 0) => pick(sel, nth).getAttribute(name),
  exists: (sel) => document.querySelector(sel) != null,
  count: (sel) => document.querySelectorAll(sel).length,
  rect: (sel, nth = 0) => {
    const r = pick(sel, nth).getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height, dpr: window.devicePixelRatio };
  },
  wait: (sel, timeoutMs = 5000) => new Promise((resolve, reject) => {
    const t0 = performance.now();
    const tick = () => {
      if (document.querySelector(sel)) return resolve(true);
      if (performance.now() - t0 > timeoutMs) return reject(new Error(`timed out waiting for "${sel}"`));
      setTimeout(tick, 50);
    };
    tick();
  }),
  sleep: (ms = 100) => new Promise((r) => setTimeout(r, ms)),
  scroll: (sel, nth = 0) => { pick(sel, nth).scrollIntoView({ block: 'center' }); return true; },
  // eslint-disable-next-line no-new-func
  eval: (code) => new Function('return (async () => (' + code + '))()')(),
};

function describe(el) {
  return { tag: el.tagName.toLowerCase(), id: el.id || undefined, className: typeof el.className === 'string' ? el.className : undefined, text: (el.textContent || '').trim().slice(0, 80) };
}

function safe(value) {
  try {
    return JSON.parse(JSON.stringify(value, (_k, v) => {
      if (typeof v === 'function') return `[function ${v.name || 'anonymous'}]`;
      if (v instanceof Element) return describe(v);
      if (typeof v === 'bigint') return String(v);
      return v;
    }));
  } catch {
    return String(value);
  }
}

if (import.meta.hot) {
  const label = windowLabel();
  import.meta.hot.on('kf:dev', async (msg) => {
    if (!msg || (msg.target !== '*' && msg.target !== label)) {
      if (msg && msg.fn === '__windows') import.meta.hot.send('kf:dev:result', { id: msg.id, ok: true, result: builtins.__ping() });
      return;
    }
    if (msg.fn === '__windows') {
      import.meta.hot.send('kf:dev:result', { id: msg.id, ok: true, result: builtins.__ping() });
      return;
    }
    try {
      const fn = builtins[msg.fn] || window.__kf_dev?.[msg.fn];
      if (typeof fn !== 'function') {
        const known = Object.keys(builtins).concat(Object.keys(window.__kf_dev || {}));
        throw new Error(`unknown fn "${msg.fn}" in window "${label}". Known: ${known.join(', ')}`);
      }
      const result = await fn(...(msg.args || []));
      import.meta.hot.send('kf:dev:result', { id: msg.id, ok: true, result: safe(result === undefined ? null : result) });
    } catch (e) {
      import.meta.hot.send('kf:dev:result', { id: msg.id, ok: false, error: e && e.message ? e.message : String(e) });
    }
  });
  window.__kf_dev_label = label;
}
