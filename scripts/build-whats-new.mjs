#!/usr/bin/env node
// Build the per-release changelog content in docs/roadmap.html ("What's New"
// page) from Featurebase. The page has a two-column layout: left column
// (this script's output) is the flowing changelog grouped by major version;
// right column stays hand-written as the sticky-scroll mockup gallery.
//
// Why server-side: FB's read endpoint requires Bearer auth (verified 403
// unauth), so we can't ship the API key to the browser. This script runs
// locally (or in CI) with the key from .env.local, fetches all changelog
// entries, regenerates the LEFT column between sentinel comments, and
// writes the result back to docs/roadmap.html.
//
// Run after every /release publish:  npm run build:whats-new
//
// Sentinels in docs/roadmap.html:
//   <!-- WHATS-NEW:START --> ... auto-generated ... <!-- WHATS-NEW:END -->
// Static content (mockups, hand-written intros, "What's next" section) is
// outside the sentinels and survives every regeneration.

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, '..');

// ── Major-version metadata ───────────────────────────────────────────────────
// One entry per phase that's already shipped. The mockup gallery has a
// data-phase slot for each of these; the IO observer keys off data-phase on
// .phase elements in our output. Names + dates mirror the hand-written
// versions Rory wrote in roadmap.html before the rebuild — saves losing the
// curated story. Newest first (matches scroll order: top of left column =
// most-recent release, right-column starts on the latest mockup).

const MAJOR_PHASES = [
  { key: 'v0.8', name: 'v0.8 — Unassigned Library & Pixel Waits', date: 'Aug 2026' },
  { key: 'v0.7', name: 'v0.7 — Autocorrect',                     date: 'Aug 2026' },
  { key: 'v0.6', name: 'v0.6 — Macro Recorder & Polish',         date: 'Jun 2026' },
  { key: 'v0.5', name: 'v0.5 — Encryption', date: 'Jun 2026' },
  { key: 'v0.4', name: 'v0.4 — Hardening & Trust',               date: 'May — Jun 2026' },
  { key: 'v0.3', name: 'v0.3 — Radial Menu',                     date: 'May 2026' },
  { key: 'v0.2', name: 'v0.2 — Quick Search Overlay',            date: 'Apr 2026' },
  { key: 'v0.1', name: 'v0.1 — The Foundation',                  date: 'Apr 2026' },
  { key: 'v0',   name: 'v0 — The Prototype',                     date: 'Feb — Mar 2026' },
];

// Map a release title like "v0.6.1" to its major phase key "v0.6". Special
// case: "v0.x.0" (or non-numeric subversion) maps to "v0". Returns null when
// the title doesn't look like a Keyfire version.
function majorOf(title) {
  if (!title) return null;
  const m = String(title).match(/^v(\d+)\.(\d+)(?:\.(\d+))?/i);
  if (!m) return null;
  const [, major, minor] = m;
  if (major === '0' && (minor === undefined || minor === '0')) return 'v0';
  return `v${major}.${minor}`;
}

// ── Read API key ─────────────────────────────────────────────────────────────
function readApiKey() {
  const env = readFileSync(join(repoRoot, '.env.local'), 'utf8');
  const m = env.split(/\r?\n/).find(l => /^FB_API_KEY=/.test(l));
  if (!m) throw new Error('FB_API_KEY not found in .env.local');
  return m.split('=', 2)[1].trim();
}

// ── Fetch all changelog entries ──────────────────────────────────────────────
async function fetchAllEntries(key) {
  const entries = [];
  let cursor = null;
  while (true) {
    const url = new URL('https://do.featurebase.app/v2/changelogs');
    url.searchParams.set('limit', '100');
    if (cursor) url.searchParams.set('cursor', cursor);
    const res = await fetch(url, { headers: { Authorization: `Bearer ${key}` } });
    if (!res.ok) throw new Error(`FB API ${res.status}: ${await res.text()}`);
    const body = await res.json();
    const page = body.data || [];
    entries.push(...page);
    if (!body.nextCursor || page.length === 0) break;
    cursor = body.nextCursor;
  }
  entries.sort((a, b) => new Date(b.date || b.createdAt) - new Date(a.date || a.createdAt));
  return entries;
}

// ── Markdown → HTML for FB content ───────────────────────────────────────────
function htmlEscape(s) {
  return String(s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function renderInline(s) {
  let r = htmlEscape(s);
  r = r.replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, (_, t, u) => `<a href="${u}" target="_blank" rel="noopener">${t}</a>`);
  r = r.replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`);
  r = r.replace(/\*\*([^*]+)\*\*/g, (_, b) => `<strong>${b}</strong>`);
  return r;
}

// FB changelog markdown is structured: `### New` / `### Improved` / `### Fixed`
// headings with `- bullet` lists under each. Anything else falls back to
// plain paragraphs. We render each category as a coloured pill heading
// followed by a tight bullet list, matching the changelog page styling.
function mdToCategoryBlocks(md) {
  if (!md) return '';
  const lines = md.split(/\r?\n/);
  const blocks = [];
  let currentCat = null;
  let currentBullets = [];
  let currentParas = [];
  const flush = () => {
    if (!currentCat && currentBullets.length === 0 && currentParas.length === 0) return;
    const catSlug = currentCat ? currentCat.toLowerCase().replace(/[^a-z0-9]+/g, '-') : 'note';
    const catLabel = currentCat || '';
    let html = '';
    if (catLabel) html += `<h5 class="re-cat re-cat-${catSlug}">${htmlEscape(catLabel)}</h5>`;
    if (currentBullets.length) {
      html += '<ul class="re-bullets">';
      for (const b of currentBullets) html += `<li>${renderInline(b)}</li>`;
      html += '</ul>';
    }
    for (const p of currentParas) html += `<p class="re-para">${renderInline(p)}</p>`;
    blocks.push(html);
    currentBullets = [];
    currentParas = [];
  };
  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;
    const h3 = line.match(/^#{2,3}\s+(.+)$/);
    if (h3) {
      flush();
      currentCat = h3[1].trim();
      continue;
    }
    const bullet = line.match(/^-\s+(.+)$/);
    if (bullet) {
      currentBullets.push(bullet[1].trim());
      continue;
    }
    currentParas.push(line);
  }
  flush();
  return blocks.join('');
}

// ── Date formatter ──────────────────────────────────────────────────────────
function fmtDate(d) {
  try {
    const date = new Date(d);
    const day = date.getDate();
    const month = date.toLocaleString('en-GB', { month: 'long' });
    const year = date.getFullYear();
    return `${day} ${month} ${year}`;
  } catch { return ''; }
}

// ── Build the phase block HTML ───────────────────────────────────────────────
function buildPhaseBlock(phase, entries) {
  // entries is the array of FB entries belonging to this major version,
  // newest first. If empty we still emit the phase header so the IO observer
  // has the data-phase target — keeps the right-column mockup tracking
  // intact even for phases that pre-date the Featurebase changelog.
  const releaseHtml = entries.map(e => {
    const versionLabel = (e.title || '').replace(/^Keyfire\s+/i, '');
    const dateLabel = fmtDate(e.date || e.createdAt);
    return `        <article class="release-entry">
          <header class="release-head">
            <span class="release-version">${htmlEscape(versionLabel || phase.key)}</span>
            ${dateLabel ? `<time class="release-date">${dateLabel}</time>` : ''}
          </header>
          <div class="release-body">
${mdToCategoryBlocks(e.markdownContent || e.content || '').split('\n').map(l => '            ' + l).join('\n')}
          </div>
        </article>`;
  }).join('\n');

  const emptyNote = entries.length === 0
    ? '        <p class="release-empty">No detailed changelog published for this phase. Older releases pre-date the Featurebase changelog. See the public GitHub release notes for raw commit-level history.</p>'
    : '';

  const releaseCount = entries.length;
  const releaseCountSuffix = releaseCount > 0 ? ` · ${releaseCount} release${releaseCount === 1 ? '' : 's'} published` : '';

  // Split the phase name "v0.6 — Macro Recorder & Polish" into two parts so
  // we can render the gold pill only around the version label (left) and
  // leave the subtitle as plain weighted text (right). The em-dash separator
  // is conventional in the metadata above; fall back to "no subtitle" if a
  // name doesn't follow that shape.
  const dashIdx = phase.name.indexOf(' — ');
  const versionLabel = dashIdx >= 0 ? phase.name.slice(0, dashIdx) : phase.name;
  const subtitleLabel = dashIdx >= 0 ? phase.name.slice(dashIdx + 3) : '';

  return `      <div class="phase" data-phase="${phase.key}">
        <div class="phase-dot shipped"></div>
        <div class="phase-header">
          <span class="phase-name">
            <span class="phase-version">${htmlEscape(versionLabel)}</span>${subtitleLabel ? `<span class="phase-subtitle">${htmlEscape(subtitleLabel)}</span>` : ''}
          </span>
          <span class="phase-date">${htmlEscape(phase.date)}${releaseCountSuffix}</span>
        </div>
${releaseHtml}${emptyNote}
      </div>`;
}

// ── Main ─────────────────────────────────────────────────────────────────────
(async () => {
  const key = readApiKey();
  console.log('[whats-new] fetching entries from Featurebase…');
  const entries = await fetchAllEntries(key);
  console.log(`[whats-new] fetched ${entries.length} entries`);

  // Group by major version key. Entries that don't parse to a known phase are
  // skipped with a warning — usually means a custom-titled FB entry.
  const byPhase = new Map();
  for (const phase of MAJOR_PHASES) byPhase.set(phase.key, []);
  let skipped = 0;
  for (const e of entries) {
    const m = majorOf(e.title);
    if (!m || !byPhase.has(m)) {
      console.warn(`[whats-new] skip: title="${e.title}" → major=${m}`);
      skipped++;
      continue;
    }
    byPhase.get(m).push(e);
  }
  console.log(`[whats-new] grouped: ${MAJOR_PHASES.map(p => `${p.key}=${byPhase.get(p.key).length}`).join(', ')}${skipped ? ` (${skipped} skipped)` : ''}`);

  const html = MAJOR_PHASES.map(p => buildPhaseBlock(p, byPhase.get(p.key))).join('\n\n');

  // Inject between sentinels in docs/roadmap.html. Sentinels MUST already
  // exist in the file (run once by hand to wire them up — see the patch in
  // the commit that introduced this script).
  const roadmapPath = join(repoRoot, 'docs', 'roadmap.html');
  const original = readFileSync(roadmapPath, 'utf8');
  const startMark = '<!-- WHATS-NEW:START -->';
  const endMark   = '<!-- WHATS-NEW:END -->';
  const startIdx = original.indexOf(startMark);
  const endIdx   = original.indexOf(endMark);
  if (startIdx === -1 || endIdx === -1 || endIdx < startIdx) {
    throw new Error(`Sentinels missing or malformed in ${roadmapPath}. Expected ${startMark} … ${endMark}.`);
  }
  const before = original.slice(0, startIdx + startMark.length);
  const after  = original.slice(endIdx);
  const next   = `${before}\n${html}\n      ${after}`;
  writeFileSync(roadmapPath, next, 'utf8');
  console.log(`[whats-new] wrote ${roadmapPath}`);
})();
