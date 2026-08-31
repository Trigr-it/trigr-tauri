// Dev-only UI test bridge (Vite plugin half).
//
// Exposes POST /__kf_dev on the Vite dev server. Each request is relayed over
// the HMR websocket to every connected Keyfire window (main, settings, overlay,
// clipboardoverlay, radialmenu, fillin, ...); the window whose label matches
// `target` executes it (see src/devBridge.js) and replies. This lets
// scripts/ui-shot.ps1 (or curl) drive the running dev app deterministically
// instead of clicking screen coordinates.
//
// Request body (JSON):
//   { "target": "main", "fn": "setView", "args": ["radial"], "timeout": 5000 }
//   { "target": "main", "fn": "click", "args": [".view-tab[title='Mouse']"] }
//   { "target": "main", "fn": "eval", "args": ["document.title"] }
//   { "fn": "__windows" }            -> every connected window's label + url
//
// Response: { ok: true, result } | { ok: false, error }
//
// Only registered with `apply: 'serve'`, so nothing here exists in
// `vite build` / `cargo tauri build`. The dev server binds localhost only.

const PATH = '/__kf_dev';
const DEFAULT_TIMEOUT_MS = 5000;
const WINDOWS_COLLECT_MS = 400;

export default function keyfireDevBridge() {
  let nextId = 1;
  const pending = new Map(); // id -> { resolve, collect: [] | null }

  return {
    name: 'keyfire-dev-bridge',
    apply: 'serve',
    configureServer(server) {
      server.ws.on('kf:dev:result', (data) => {
        const entry = data && pending.get(data.id);
        if (!entry) return;
        if (entry.collect) { entry.collect.push(data); return; }
        pending.delete(data.id);
        entry.resolve(data);
      });

      server.middlewares.use(PATH, (req, res) => {
        if (req.method !== 'POST') {
          res.statusCode = 405;
          res.setHeader('Content-Type', 'application/json');
          res.end(JSON.stringify({ ok: false, error: 'POST JSON { target, fn, args }' }));
          return;
        }
        let body = '';
        req.on('data', (c) => { body += c; });
        req.on('end', async () => {
          let msg;
          try {
            msg = body ? JSON.parse(body) : {};
          } catch (e) {
            res.statusCode = 400;
            res.setHeader('Content-Type', 'application/json');
            res.end(JSON.stringify({ ok: false, error: 'invalid JSON: ' + e.message }));
            return;
          }
          const id = nextId++;
          const fn = String(msg.fn || '');
          const target = msg.target === undefined ? 'main' : msg.target;
          const timeout = Number(msg.timeout) > 0 ? Number(msg.timeout) : DEFAULT_TIMEOUT_MS;
          const payload = { id, target, fn, args: Array.isArray(msg.args) ? msg.args : [] };

          const reply = await new Promise((resolve) => {
            const collect = fn === '__windows' ? [] : null;
            pending.set(id, { resolve, collect });
            server.ws.send('kf:dev', payload);
            if (collect) {
              setTimeout(() => {
                pending.delete(id);
                resolve({ ok: true, result: collect.map((r) => r.result) });
              }, WINDOWS_COLLECT_MS);
            } else {
              setTimeout(() => {
                if (!pending.has(id)) return;
                pending.delete(id);
                resolve({ ok: false, error: `no reply from window "${target}" within ${timeout} ms (is the dev app running and that window loaded?)` });
              }, timeout);
            }
          });

          res.statusCode = 200;
          res.setHeader('Content-Type', 'application/json');
          res.end(JSON.stringify(reply));
        });
      });
    },
  };
}
