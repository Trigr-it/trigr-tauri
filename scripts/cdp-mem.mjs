// Per-page renderer memory attribution for Keyfire dev builds (RAM wave 2).
//
// Talks to the WebView2 remote-debugging port that lib.rs opens in debug
// builds (--remote-debugging-port=9223, see run()). Dev only: release builds
// never open the port.
//
//   node scripts/cdp-mem.mjs            # DOM counters per page + heap
//   node scripts/cdp-mem.mjs gc         # ...then force a full GC, re-read heap
//   node scripts/cdp-mem.mjs pressure   # ...then send a CRITICAL memory-pressure
//                                       #    signal to every page (purges Blink
//                                       #    caches + V8), re-read heap
//
// Pair with scripts/kf-mem.ps1 before/after to see what the OS actually gets
// back. V8 heap numbers are per renderer PROCESS (all pages share one isolate
// under --process-per-site); DOM counters are per page.

const PORT = process.env.KF_CDP_PORT || 9223;
const mode = process.argv[2] || 'dom';

function labelOf(url) {
  const q = new URL(url).searchParams;
  for (const k of ['overlay', 'fillin', 'radialmenu', 'clipboardoverlay', 'settings', 'report', 'countdown', 'snipoverlay']) {
    if (q.get(k) === '1') return k;
  }
  return 'main';
}

class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.ready = new Promise((res, rej) => {
      this.ws.addEventListener('open', () => res());
      this.ws.addEventListener('error', (e) => rej(new Error(`ws error ${e.message || ''}`)));
    });
    this.ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(typeof ev.data === 'string' ? ev.data : ev.data.toString());
      if (msg.id && this.pending.has(msg.id)) {
        const { res, rej } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? rej(new Error(`${msg.error.message} (${msg.error.code})`)) : res(msg.result);
      }
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      this.ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); rej(new Error(`${method} timed out`)); } }, 15000);
    });
  }
  close() { this.ws.close(); }
}

const mb = (b) => (b / 1048576).toFixed(1);

async function readPage(cdp) {
  const dom = await cdp.send('Memory.getDOMCounters').catch((e) => ({ error: e.message }));
  const heap = await cdp.send('Runtime.getHeapUsage').catch((e) => ({ error: e.message }));
  return { dom, heap };
}

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
const pages = targets.filter((t) => t.type === 'page' && t.webSocketDebuggerUrl);
if (!pages.length) {
  console.error(`No pages on port ${PORT}. Is the dev app running a build with the remote-debugging port?`);
  process.exit(1);
}

const conns = [];
for (const t of pages) {
  const cdp = new Cdp(t.webSocketDebuggerUrl);
  await cdp.ready;
  conns.push({ label: labelOf(t.url), cdp, url: t.url });
}
conns.sort((a, b) => a.label.localeCompare(b.label));

console.log(`\n== ${pages.length} pages, mode=${mode} ==`);
const before = [];
for (const c of conns) {
  const r = await readPage(c.cdp);
  before.push({ label: c.label, ...r });
  const d = r.dom.error ? r.dom.error : `documents ${r.dom.documents}  nodes ${r.dom.nodes}  listeners ${r.dom.jsEventListeners}`;
  console.log(`${c.label.padEnd(17)} ${d}`);
}
const h0 = before.find((b) => !b.heap.error)?.heap;
if (h0) console.log(`V8 heap (renderer-wide): used ${mb(h0.usedSize)} MB / total ${mb(h0.totalSize)} MB`);

if (mode === 'gc' || mode === 'pressure') {
  for (const c of conns) {
    if (mode === 'gc') {
      await c.cdp.send('HeapProfiler.collectGarbage').catch((e) => console.log(`${c.label}: collectGarbage ${e.message}`));
    } else {
      await c.cdp.send('Memory.simulatePressureNotification', { level: 'critical' }).catch((e) => console.log(`${c.label}: pressure ${e.message}`));
    }
  }
  await new Promise((r) => setTimeout(r, 3000));
  const h1 = await conns[0].cdp.send('Runtime.getHeapUsage').catch(() => null);
  if (h1) console.log(`after ${mode}: V8 heap used ${mb(h1.usedSize)} MB / total ${mb(h1.totalSize)} MB`);
  console.log('Now run scripts/kf-mem.ps1 to see the process-level effect.');
}

for (const c of conns) c.cdp.close();
