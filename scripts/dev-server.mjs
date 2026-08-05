// Reuse-or-detach Vite launcher for `cargo tauri dev` (beforeDevCommand).
//
// The demo/profile relaunch flow (Settings -> Launch Demo Mode / Studio
// Profile) exits the app and starts a new instance OUTSIDE the cargo tauri
// dev session, so the Vite dev server must outlive that session.
//
// This script makes that work in a SINGLE terminal:
//   - If something already serves 5173, exit 0 and let tauri reuse it.
//   - Otherwise spawn Vite DETACHED (survives cargo tauri dev exiting),
//     wait until it responds, then exit 0. Vite logs go to a temp file.
//
// The detached Vite keeps running after the dev session ends - subsequent
// `cargo tauri dev` runs reuse it (faster startup). To stop it, kill the
// process on port 5173 (the usual dev-restart routine).
import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const DEV_URL = 'http://localhost:5173/';
const projectRoot = join(dirname(fileURLToPath(import.meta.url)), '..');

async function viteUp() {
  try {
    const res = await fetch(DEV_URL, { signal: AbortSignal.timeout(1000) });
    return res.status > 0;
  } catch {
    return false;
  }
}

if (await viteUp()) {
  console.log('[dev-server] Vite already serving 5173 - reusing it.');
  process.exit(0);
}

const logPath = join(tmpdir(), 'keyfire-vite-dev.log');
const log = openSync(logPath, 'a');
console.log(`[dev-server] Starting detached Vite (survives this dev session, so demo/profile relaunches keep working). Logs: ${logPath}`);
const child = spawn('npx', ['vite'], {
  cwd: projectRoot,
  detached: true,
  stdio: ['ignore', log, log],
  shell: process.platform === 'win32',
  windowsHide: true,
});
child.unref();

for (let i = 0; i < 40; i++) {
  await new Promise(r => setTimeout(r, 500));
  if (await viteUp()) {
    console.log('[dev-server] Vite is up.');
    process.exit(0);
  }
}
console.error(`[dev-server] Vite did not respond within 20s - check ${logPath}`);
process.exit(1);
