// Asserts that every CSS custom property the theme engine can set on <html>
// is declared in the semantic layer of src/styles/global.css. Catches a
// typo'd token (a `--bg-primary` that renders transparent) before it ships.
// Run: node scripts/check-theme-tokens.mjs   (exit 1 on any miss)
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');

const engineSrc = readFileSync(resolve(root, 'src/theme/engine.js'), 'utf8');
const cssSrc = readFileSync(resolve(root, 'src/styles/global.css'), 'utf8');

// Pull the THEME_TOKENS array literal out of engine.js without importing it
// (the engine imports colour.js; keeping this a plain text check avoids
// pulling ESM resolution into the lint step).
const m = engineSrc.match(/export const THEME_TOKENS = \[([\s\S]*?)\];/);
if (!m) {
  console.error('check-theme-tokens: THEME_TOKENS array not found in src/theme/engine.js');
  process.exit(1);
}
const tokens = [...m[1].matchAll(/'(--[a-z0-9-]+)'/g)].map(x => x[1]);
const declared = new Set([...cssSrc.matchAll(/^\s*(--[a-z0-9-]+)\s*:/gm)].map(x => x[1]));

const missing = tokens.filter(t => !declared.has(t));
if (missing.length) {
  console.error('check-theme-tokens: engine sets tokens that global.css never declares:');
  for (const t of missing) console.error(`  ${t}`);
  process.exit(1);
}
console.log(`check-theme-tokens: ${tokens.length} engine tokens all declared in global.css`);
