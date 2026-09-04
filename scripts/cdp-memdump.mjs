// Chromium memory-infra dump for Keyfire dev builds (RAM wave 2).
//
// Uses the WebView2 remote-debugging port lib.rs opens in debug builds. Starts
// a trace with the memory-infra category, requests one detailed global memory
// dump (covers browser, GPU, renderer and utility processes), stops the trace
// and prints each process's allocator breakdown, largest first.
//
//   node scripts/cdp-memdump.mjs            # top 25 allocators per process
//   node scripts/cdp-memdump.mjs 60         # top 60
//   node scripts/cdp-memdump.mjs 40 v8      # only allocators whose path contains "v8"
//   node scripts/cdp-memdump.mjs 40 "" 3    # show paths up to depth 3 (default 2)
//   KF_PROC=Renderer node scripts/cdp-memdump.mjs 40 "" 2   # one process only

const PORT = process.env.KF_CDP_PORT || 9223;
const TOP = Number(process.argv[2] || 25);
const FILTER = process.argv[3] || '';
const DEPTH = Number(process.argv[4] || 2); // max path depth shown (1 = top-level allocators only)
const ONLY = process.env.KF_PROC || ''; // e.g. Renderer, "GPU Process", Browser

class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.handlers = new Map();
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
      } else if (msg.method && this.handlers.has(msg.method)) {
        this.handlers.get(msg.method)(msg.params);
      }
    });
  }
  on(method, cb) { this.handlers.set(method, cb); }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      this.ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); rej(new Error(`${method} timed out`)); } }, 30000);
    });
  }
  close() { this.ws.close(); }
}

const mb = (b) => (b / 1048576).toFixed(1).padStart(7);
const hex = (v) => (typeof v === 'string' ? parseInt(v, 16) : Number(v || 0));

// Browser-level endpoint: the Tracing domain and global memory dumps live there.
const version = await (await fetch(`http://127.0.0.1:${PORT}/json/version`)).json();
if (!version.webSocketDebuggerUrl) { console.error(`No browser endpoint on port ${PORT}.`); process.exit(1); }

const cdp = new Cdp(version.webSocketDebuggerUrl);
await cdp.ready;
cdp.ws.addEventListener('close', (e) => console.error(`ws closed (${e.code})`));

// Collect via ReturnAsStream + IO.read: the renderer's detailed dump is several
// MB and ReportEvents chunks over the Node WebSocket closed the socket (1006).
let streamHandle = null;
let complete;
const done = new Promise((r) => { complete = r; });
cdp.on('Tracing.tracingComplete', (p) => { streamHandle = p.stream; complete(); });
const fallback = setTimeout(() => complete(), 30000);

await cdp.send('Tracing.start', {
  transferMode: 'ReturnAsStream',
  streamFormat: 'json',
  traceConfig: {
    recordMode: 'recordUntilFull',
    includedCategories: ['disabled-by-default-memory-infra'],
    excludedCategories: ['*'],
    memoryDumpConfig: { triggers: [] },
  },
});
const dump = await cdp.send('Tracing.requestMemoryDump', { deterministic: false, levelOfDetail: 'detailed' });
if (!dump.success) console.error('memory dump reported success=false');
await new Promise((r) => setTimeout(r, 4000));
await cdp.send('Tracing.end');
await done;
clearTimeout(fallback);
if (!streamHandle) { console.error('tracingComplete carried no stream'); process.exit(1); }
let text = '';
for (;;) {
  const chunk = await cdp.send('IO.read', { handle: streamHandle, size: 1 << 20 });
  text += chunk.base64Encoded ? Buffer.from(chunk.data, 'base64').toString('utf8') : chunk.data;
  if (chunk.eof) break;
}
await cdp.send('IO.close', { handle: streamHandle }).catch(() => {});
cdp.close();
const parsed = JSON.parse(text);
const events = Array.isArray(parsed) ? parsed : parsed.traceEvents || [];
console.error(`collected ${events.length} trace events (${(text.length / 1048576).toFixed(1)} MB)`);

// Process names come from metadata events; dumps are ph:"v" events.
const names = new Map();
for (const e of events) {
  if (e.ph === 'M' && e.name === 'process_name') names.set(e.pid, e.args?.name);
}
const dumps = events.filter((e) => e.ph === 'v' && e.args?.dumps);
if (!dumps.length) { console.error(`No memory dump events (got ${events.length} trace events).`); process.exit(1); }

for (const d of dumps) {
  const allocators = d.args.dumps.allocators || {};
  const totals = d.args.dumps.process_totals || {};
  const rows = [];
  for (const [path, node] of Object.entries(allocators)) {
    const attrs = node.attrs || {};
    const size = hex(attrs.effective_size?.value ?? attrs.size?.value);
    if (!size) continue;
    if (FILTER && !path.includes(FILTER)) continue;
    if (path.split('/').length > DEPTH) continue;
    rows.push({ path, size, depth: path.split('/').length });
  }
  // Show the top-level allocators plus the biggest sub-nodes, no double counting confusion:
  rows.sort((a, b) => b.size - a.size);
  const title = `${names.get(d.pid) || 'process'} pid ${d.pid}`;
  if (ONLY && !title.includes(ONLY)) continue;
  const priv = totals.private_footprint_bytes ? ` private footprint ${mb(hex(totals.private_footprint_bytes)).trim()} MB` : '';
  const rss = totals.resident_set_bytes ? ` resident ${mb(hex(totals.resident_set_bytes)).trim()} MB` : '';
  console.log(`\n== ${title}${priv}${rss} ==`);
  for (const r of rows.slice(0, TOP)) console.log(`${mb(r.size)} MB  ${r.path}`);
}
