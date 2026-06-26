#!/usr/bin/env node
// Build docs/changelog.html from Featurebase's changelog API.
//
// Why build-time vs client-side: FB's read endpoint requires Bearer auth so
// we can't ship the API key to the browser. This script runs locally (or in
// CI later) with the key from .env.local, fetches all changelog entries,
// renders them into a static Keyfire-branded HTML page committed to docs/.
//
// Run:  npm run build:changelog
// Re-run on every release (after /release publishes the FB entry). Output
// is checked in so Netlify serves a pre-rendered page — full SEO benefit,
// no API call from the visitor's browser.

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, '..');

// ── Read API key from .env.local (gitignored) ────────────────────────────────
function readApiKey() {
  const env = readFileSync(join(repoRoot, '.env.local'), 'utf8');
  const m = env.split(/\r?\n/).find(l => /^FB_API_KEY=/.test(l));
  if (!m) {
    throw new Error('FB_API_KEY not found in .env.local');
  }
  return m.split('=', 2)[1].trim();
}

// ── Fetch all changelog entries ──────────────────────────────────────────────
async function fetchAllEntries(key) {
  // Page through nextCursor until we have everything. FB caps each page at
  // some limit; we ask for 100 and follow cursors. Practically there'll only
  // ever be a few dozen entries.
  const entries = [];
  let cursor = null;
  while (true) {
    const url = new URL('https://do.featurebase.app/v2/changelogs');
    url.searchParams.set('limit', '100');
    if (cursor) url.searchParams.set('cursor', cursor);
    const res = await fetch(url, {
      headers: { Authorization: `Bearer ${key}` },
    });
    if (!res.ok) {
      throw new Error(`FB API ${res.status}: ${await res.text()}`);
    }
    const body = await res.json();
    const page = body.data || [];
    entries.push(...page);
    if (!body.nextCursor || page.length === 0) break;
    cursor = body.nextCursor;
  }
  // Sort newest first by date (FB returns sorted but defensive).
  entries.sort((a, b) => new Date(b.date || b.createdAt) - new Date(a.date || a.createdAt));
  return entries;
}

// ── Minimal markdown → HTML for FB changelog body ────────────────────────────
// FB content uses ### headers and - bullet lists. No tables, no code blocks,
// no inline emphasis we need to preserve. Anything more exotic goes through
// htmlEscape and renders as plain text — safer than running a full parser
// on unsanitised content.
function htmlEscape(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function mdToHtml(md) {
  if (!md) return '';
  const lines = md.split(/\r?\n/);
  const out = [];
  let inList = false;
  const closeList = () => { if (inList) { out.push('</ul>'); inList = false; } };

  for (const raw of lines) {
    const line = raw.trim();
    if (!line) { closeList(); continue; }

    // ### Category header
    const h3 = line.match(/^###\s+(.+)$/);
    if (h3) {
      closeList();
      const cat = h3[1].trim();
      const slug = cat.toLowerCase().replace(/[^a-z0-9]+/g, '-');
      out.push(`<h3 class="cl-cat cl-cat-${slug}">${htmlEscape(cat)}</h3>`);
      continue;
    }

    // - bullet
    const bullet = line.match(/^-\s+(.+)$/);
    if (bullet) {
      if (!inList) { out.push('<ul class="cl-bullets">'); inList = true; }
      out.push(`<li>${renderInline(bullet[1].trim())}</li>`);
      continue;
    }

    // Anything else → paragraph
    closeList();
    out.push(`<p class="cl-para">${renderInline(line)}</p>`);
  }
  closeList();
  return out.join('\n');
}

// Inline markdown: **bold**, `code`, and link [text](url). Light touch on
// purpose — FB bullets are mostly plain sentences with the occasional
// **emphasis** for feature names. Anything else stays as plain text.
function renderInline(s) {
  let r = htmlEscape(s);
  // [text](url)
  r = r.replace(/\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g, (_, text, url) =>
    `<a href="${url}" target="_blank" rel="noopener">${text}</a>`
  );
  // `code`
  r = r.replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`);
  // **bold**
  r = r.replace(/\*\*([^*]+)\*\*/g, (_, b) => `<strong>${b}</strong>`);
  return r;
}

// ── Date formatter — UK convention since the brand is UK-based ──────────────
function fmtDate(d) {
  try {
    const date = new Date(d);
    const day = date.getDate();
    const month = date.toLocaleString('en-GB', { month: 'long' });
    const year = date.getFullYear();
    return `${day} ${month} ${year}`;
  } catch { return ''; }
}

// ── Build the HTML page ──────────────────────────────────────────────────────
function buildPage(entries) {
  const latest = entries[0];
  const latestTitle = latest?.title || 'v0.6.1';

  const entriesHtml = entries.map((e, i) => {
    const date = fmtDate(e.date || e.createdAt);
    const isLatest = i === 0;
    const id = (e.slug || e.title || '').toLowerCase().replace(/[^a-z0-9]+/g, '-');
    return `      <article class="cl-entry${isLatest ? ' cl-entry-latest' : ''}" id="${htmlEscape(id)}">
        <div class="cl-entry-head">
          <h2 class="cl-version">${htmlEscape(e.title || 'Release')}</h2>
          ${date ? `<time class="cl-date">${date}</time>` : ''}
          ${isLatest ? '<span class="cl-latest-pill">Latest</span>' : ''}
        </div>
        <div class="cl-body">
${mdToHtml(e.markdownContent || e.content || '')}
        </div>
      </article>`;
  }).join('\n\n');

  // Date for the page meta — use the latest entry's date so search engines
  // see this as a fresh page on every regeneration.
  const lastUpdatedIso = new Date(latest?.date || latest?.createdAt || Date.now()).toISOString();

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Keyfire — Changelog</title>
<link rel="icon" href="favicon.ico" sizes="any">
<link rel="icon" type="image/svg+xml" href="favicon.svg">
<link rel="icon" type="image/png" sizes="32x32" href="favicon-32.png">
<link rel="icon" type="image/png" sizes="192x192" href="favicon-192.png">
<link rel="apple-touch-icon" href="apple-touch-icon.png">
<meta name="description" content="What is new in Keyfire. Every release shipped to date — features, improvements and fixes for the Windows hotkey, macro and text expansion tool.">

<link rel="canonical" href="https://keyfire.app/changelog.html">

<meta property="og:type" content="website">
<meta property="og:url" content="https://keyfire.app/changelog.html">
<meta property="og:title" content="Keyfire — Changelog">
<meta property="og:description" content="What is new in Keyfire. Every release shipped to date.">
<meta property="og:image" content="https://keyfire.app/og-image.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:site_name" content="Keyfire">

<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="Keyfire — Changelog">
<meta name="twitter:description" content="What is new in Keyfire. Every release shipped to date.">
<meta name="twitter:image" content="https://keyfire.app/og-image.png">

<!-- JSON-LD: SoftwareApplication with the latest version -->
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "name": "Keyfire",
  "applicationCategory": "BusinessApplication",
  "operatingSystem": "Windows 10, Windows 11",
  "softwareVersion": "${htmlEscape(latestTitle.replace(/^v/i, ''))}",
  "datePublished": "${lastUpdatedIso}",
  "offers": {
    "@type": "Offer",
    "price": "0",
    "priceCurrency": "GBP"
  },
  "url": "https://keyfire.app/changelog.html"
}
</script>

<script>
(function(){
  var saved = localStorage.getItem('keyfire-theme');
  document.documentElement.setAttribute('data-theme', saved || 'light');
})();
</script>
<link rel="preload" href="/fonts/SpaceGrotesk-latin.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="/fonts.css">
<style>
*, *::before, *::after { box-sizing:border-box; margin:0; padding:0; }
:root {
  --gold:#e8a020; --gold-light:#f0b942; --gold-dark:#c8860a;
  --gold-glow:rgba(232,160,32,0.15); --gold-line:rgba(232,160,32,0.22);
  --bg:#080B14; --bg-2:#0D1120; --bg-3:#111827;
  --bg-card:#0E1320; --bg-card-hi:#131829;
  --hairline:rgba(255,255,255,0.06); --hairline-strong:rgba(255,255,255,0.1); --hairline-gold:rgba(232,160,32,0.18);
  --text:#F1F5F9; --text-muted:#94A3B8; --text-dim:#475569;
  --font-display:'Space Grotesk',sans-serif;
  --font-mono:'Rajdhani',sans-serif;
  --font-body:'DM Sans',sans-serif;
  --font-label:'Chakra Petch',sans-serif;
  --green:#3ec47a; --green-glow:rgba(62,196,122,0.15);
  --blue:#5b9cff; --blue-glow:rgba(91,156,255,0.15);
  --red:#ff7a7a; --red-glow:rgba(255,122,122,0.12);
}
[data-theme="light"] {
  --bg:#F5F5FA; --bg-2:#FFFFFF; --bg-3:#EEEEF2;
  --bg-card:#FFFFFF; --bg-card-hi:#FAFAFE;
  --hairline:rgba(0,0,0,0.07); --hairline-strong:rgba(0,0,0,0.12); --hairline-gold:rgba(232,160,32,0.25);
  --text:#1A1A2E; --text-muted:#4A4A6A; --text-dim:#9090B0;
  --green:#1f8a4a; --green-glow:rgba(31,138,74,0.1);
  --blue:#3a7fc4; --blue-glow:rgba(58,127,196,0.1);
  --red:#c64545; --red-glow:rgba(198,69,69,0.08);
}
html { scroll-behavior:smooth; }
body { font-family:var(--font-body); background:var(--bg); color:var(--text); line-height:1.6; overflow-x:hidden; -webkit-font-smoothing:antialiased; }
body::before { content:''; position:fixed; inset:0; background-image:url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noise'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noise)' opacity='0.03'/%3E%3C/svg%3E"); pointer-events:none; z-index:0; opacity:0.4; }
[data-theme="light"] body::before { opacity:0.08; }

/* Nav */
nav { position:fixed; top:0; left:0; right:0; z-index:100; padding:0 32px; height:64px; display:flex; align-items:center; justify-content:space-between; background:rgba(8,11,20,0.7); backdrop-filter:blur(20px) saturate(140%); -webkit-backdrop-filter:blur(20px) saturate(140%); border-bottom:1px solid var(--hairline); }
[data-theme="light"] nav { background:rgba(245,245,250,0.75); }
.nav-logo { display:flex; align-items:center; gap:10px; text-decoration:none; }
.logo-mark { width:30px; height:30px; border-radius:7px; }
.logo-mark > svg { width:100%; height:100%; display:block; }
.nav-wordmark { font-family:var(--font-label); font-weight:600; font-size:15px; color:var(--text); }
.nav-links { display:flex; align-items:center; gap:28px; }
.nav-links a { font-size:13px; font-weight:500; color:var(--text-muted); text-decoration:none; transition:color 0.2s; letter-spacing:0.01em; }
.nav-links a:hover { color:var(--text); }
.nav-links a.active { color:var(--gold-light); }
[data-theme="light"] .nav-links a.active { color:var(--gold-dark); }
.nav-cta { background:linear-gradient(180deg,var(--gold-light),var(--gold)); color:#0d0d11; padding:8px 18px; border-radius:8px; font-weight:600!important; border:1px solid var(--gold); box-shadow:0 1px 6px rgba(232,160,32,0.35), inset 0 1px 0 rgba(255,255,255,0.18); transition:transform 0.15s, box-shadow 0.2s, background 0.2s; }
.nav-cta:hover { background:linear-gradient(180deg,#ffbb44,var(--gold-light)); color:#0d0d11!important; transform:translateY(-1px); box-shadow:0 4px 14px rgba(232,160,32,0.5), inset 0 1px 0 rgba(255,255,255,0.22); }
[data-theme="light"] .nav-cta { color:#fff; }
[data-theme="light"] .nav-cta:hover { color:#fff!important; }
.theme-toggle { width:34px; height:34px; border-radius:8px; border:1px solid var(--hairline); background:transparent; color:var(--text-muted); font-size:14px; cursor:pointer; display:flex; align-items:center; justify-content:center; transition:all 0.2s; }
.theme-toggle:hover { color:var(--gold); border-color:var(--hairline-gold); background:var(--gold-glow); }

/* Hero */
.hero { position:relative; padding:140px 32px 60px; text-align:center; overflow:hidden; }
.hero-bg { position:absolute; inset:0; pointer-events:none; }
.hero-bg::before { content:''; position:absolute; top:-180px; left:50%; transform:translateX(-50%); width:1100px; height:600px; background:radial-gradient(ellipse at center,rgba(232,160,32,0.13) 0%,rgba(232,160,32,0.04) 35%,transparent 70%); }
[data-theme="light"] .hero-bg::before { background:radial-gradient(ellipse at center,rgba(232,160,32,0.09) 0%,rgba(232,160,32,0.02) 35%,transparent 70%); }
.hero-inner { position:relative; max-width:780px; margin:0 auto; }
.hero-eyebrow { font-family:var(--font-label); font-size:12px; font-weight:600; letter-spacing:0.18em; text-transform:uppercase; color:var(--text-muted); margin-bottom:18px; }
.hero h1 { font-family:var(--font-display); font-weight:700; font-size:clamp(36px,5vw,60px); line-height:1.05; letter-spacing:-0.03em; color:var(--text); margin-bottom:24px; }
.hero h1 em { font-style:normal; background:linear-gradient(135deg,#f5cb6a 0%,#e8a020 50%,#c8860a 100%); -webkit-background-clip:text; -webkit-text-fill-color:transparent; background-clip:text; }
.hero p { font-size:17px; color:var(--text-muted); max-width:560px; margin:0 auto; font-weight:400; line-height:1.6; }

/* Changelog list */
.changelog { position:relative; padding:60px 32px 100px; }
.changelog-inner { max-width:820px; margin:0 auto; position:relative; }

.cl-entry { position:relative; padding:32px 0; border-bottom:1px solid var(--hairline); }
.cl-entry:last-child { border-bottom:none; }
.cl-entry-head { display:flex; align-items:center; gap:14px; margin-bottom:20px; flex-wrap:wrap; }
.cl-version { font-family:var(--font-display); font-weight:700; font-size:clamp(24px,3vw,32px); line-height:1.1; letter-spacing:-0.02em; color:var(--text); }
.cl-entry-latest .cl-version em, .cl-entry-latest .cl-version { background:linear-gradient(135deg,#f5cb6a 0%,#e8a020 60%,#c8860a 100%); -webkit-background-clip:text; -webkit-text-fill-color:transparent; background-clip:text; }
.cl-date { font-family:var(--font-mono); font-size:13px; font-weight:600; letter-spacing:0.04em; color:var(--text-muted); }
.cl-latest-pill { font-family:var(--font-mono); font-size:10px; font-weight:700; letter-spacing:0.1em; text-transform:uppercase; padding:3px 9px; border:1px solid var(--hairline-gold); border-radius:5px; background:var(--gold-glow); color:var(--gold-light); }
[data-theme="light"] .cl-latest-pill { color:var(--gold-dark); }

.cl-body { display:flex; flex-direction:column; gap:18px; }
.cl-body > * { margin:0; }

.cl-cat { font-family:var(--font-label); font-size:11px; font-weight:700; letter-spacing:0.16em; text-transform:uppercase; padding:4px 11px; border-radius:5px; display:inline-block; align-self:flex-start; margin-top:6px; }
.cl-cat-new      { background:var(--green-glow); color:var(--green); border:1px solid color-mix(in srgb, var(--green) 35%, transparent); }
.cl-cat-improved { background:var(--blue-glow);  color:var(--blue);  border:1px solid color-mix(in srgb, var(--blue) 35%, transparent); }
.cl-cat-fixed    { background:var(--red-glow);   color:var(--red);   border:1px solid color-mix(in srgb, var(--red) 35%, transparent); }

.cl-bullets { list-style:none; padding:0; display:flex; flex-direction:column; gap:12px; }
.cl-bullets li { position:relative; padding-left:22px; font-size:15px; line-height:1.65; color:var(--text); }
.cl-bullets li::before { content:''; position:absolute; left:4px; top:11px; width:6px; height:6px; border-radius:50%; background:var(--gold); opacity:0.6; }
.cl-bullets li strong { color:var(--text); font-weight:600; }
.cl-bullets li code { font-family:var(--font-mono); font-size:0.92em; background:rgba(232,160,32,0.1); border:1px solid var(--hairline-gold); border-radius:4px; padding:1px 6px; color:var(--gold-light); }
[data-theme="light"] .cl-bullets li code { color:var(--gold-dark); }
.cl-bullets li a { color:var(--gold-light); text-decoration:underline; text-decoration-color:var(--hairline-gold); text-underline-offset:2px; }
[data-theme="light"] .cl-bullets li a { color:var(--gold-dark); }
.cl-bullets li a:hover { text-decoration-color:var(--gold); }
.cl-para { font-size:15px; line-height:1.65; color:var(--text); }

/* Bottom CTA */
.cl-cta-strip { margin-top:80px; padding:48px 36px; background:linear-gradient(135deg, var(--bg-card) 0%, var(--bg-card-hi) 100%); border:1px solid var(--hairline-gold); border-radius:14px; text-align:center; box-shadow:0 12px 36px rgba(0,0,0,0.18); }
[data-theme="light"] .cl-cta-strip { box-shadow:0 12px 36px rgba(0,0,0,0.06); }
.cl-cta-title { font-family:var(--font-display); font-weight:700; font-size:24px; letter-spacing:-0.02em; color:var(--text); margin-bottom:10px; }
.cl-cta-sub { font-size:14px; color:var(--text-muted); margin-bottom:22px; }
.cl-cta-btn { display:inline-flex; align-items:center; gap:10px; background:linear-gradient(180deg,var(--gold-light),var(--gold)); color:#0d0d11; text-decoration:none; padding:12px 24px; border-radius:9px; font-size:14px; font-weight:600; border:1px solid var(--gold); box-shadow:0 4px 16px rgba(232,160,32,0.32), inset 0 1px 0 rgba(255,255,255,0.18); transition:transform 0.15s, box-shadow 0.2s; }
.cl-cta-btn:hover { transform:translateY(-1px); box-shadow:0 6px 22px rgba(232,160,32,0.46), inset 0 1px 0 rgba(255,255,255,0.22); }
[data-theme="light"] .cl-cta-btn { color:#fff; }
.cl-cta-link { display:inline-block; margin-left:18px; font-size:14px; color:var(--text-muted); text-decoration:none; border-bottom:1px solid var(--hairline-strong); padding-bottom:1px; }
.cl-cta-link:hover { color:var(--text); border-color:var(--text); }

/* Footer */
footer { background:var(--bg-2); border-top:1px solid var(--hairline); padding:60px 32px 32px; position:relative; z-index:1; }
.footer-inner { max-width:1180px; margin:0 auto; display:grid; grid-template-columns:1.4fr 2fr; gap:60px; padding-bottom:40px; border-bottom:1px solid var(--hairline); }
.footer-brand .ft-logo { display:flex; align-items:center; gap:10px; text-decoration:none; margin-bottom:14px; }
.ft-wm { font-family:var(--font-label); font-weight:600; font-size:16px; color:var(--text); }
.footer-tagline { font-size:13px; color:var(--text-muted); max-width:340px; line-height:1.5; }
.footer-links { display:grid; grid-template-columns:repeat(3,1fr); gap:36px; }
.ft-col h4 { font-family:var(--font-label); font-size:11px; font-weight:700; letter-spacing:0.14em; text-transform:uppercase; color:var(--text); margin-bottom:14px; }
.ft-col a { display:block; font-size:13px; color:var(--text-muted); text-decoration:none; margin-bottom:8px; transition:color 0.15s; }
.ft-col a:hover { color:var(--gold-light); }
[data-theme="light"] .ft-col a:hover { color:var(--gold-dark); }
.footer-bottom { max-width:1180px; margin:0 auto; padding-top:24px; display:flex; align-items:center; justify-content:space-between; font-size:12px; color:var(--text-dim); flex-wrap:wrap; gap:12px; }
.ft-legal { display:flex; gap:18px; }
.ft-legal a { color:var(--text-dim); text-decoration:none; transition:color 0.15s; }
.ft-legal a:hover { color:var(--text-muted); }

@media (max-width: 768px) {
  nav { padding:0 16px; }
  .nav-links { gap:14px; }
  .nav-links a:not(.nav-cta) { display:none; }
  .hero { padding:120px 20px 40px; }
  .changelog { padding:40px 20px 80px; }
  .cl-cta-strip { padding:36px 20px; }
  .footer-inner { grid-template-columns:1fr; gap:36px; }
  .footer-links { grid-template-columns:repeat(2,1fr); gap:24px; }
}
</style>
</head>
<body>

<nav>
  <a class="nav-logo" href="index.html">
    <div class="logo-mark"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="lb-nav" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#f0b942"/><stop offset="100%" stop-color="#c8860a"/></linearGradient><linearGradient id="lk-nav" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#ffffff"/><stop offset="100%" stop-color="#e8e5dc"/></linearGradient></defs><rect width="64" height="64" rx="9" fill="url(#lb-nav)"/><rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5" fill="url(#lk-nav)"/><rect x="7.68" y="46.5" width="48.64" height="3.42" rx="1.5" fill="#000" opacity="0.06"/><path d="M 33 14 C 36 18, 41 23, 41 30 C 41 37, 36 41, 32 41 C 26 41, 22 37, 22 32 C 22 28, 25 26, 27 23 C 28 26, 30 27, 30 24 C 30 20, 32 17, 33 14 Z" fill="#c8860a"/></svg></div>
    <span class="nav-wordmark">Keyfire</span>
  </a>
  <div class="nav-links">
    <a href="index.html#features">Features</a>
    <a href="trigr-help.html">Guide</a>
    <a href="roadmap.html">Roadmap</a>
    <a href="changelog.html" class="active">Changelog</a>
    <a href="blog/">Blog</a>
    <a href="pricing.html">Pricing</a>
    <a href="https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Keyfire_x64-setup.exe" class="nav-cta" data-loc="nav" data-arch="x64">Download</a>
  </div>
</nav>

<section class="hero">
  <div class="hero-bg"></div>
  <div class="hero-inner">
    <div class="hero-eyebrow">Changelog</div>
    <h1>What is <em>new</em> in Keyfire.</h1>
    <p>Every release, line by line. New features at the top, fixes at the bottom, all the way back to the start.</p>
  </div>
</section>

<section class="changelog">
  <div class="changelog-inner">

${entriesHtml}

    <div class="cl-cta-strip">
      <div class="cl-cta-title">Try the latest release.</div>
      <div class="cl-cta-sub">Free during Beta. Windows 10 and 11. Around 10 MB.</div>
      <a class="cl-cta-btn" href="https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Keyfire_x64-setup.exe" data-loc="changelog_cta">Download ${htmlEscape(latestTitle)}</a>
      <a class="cl-cta-link" href="roadmap.html">See the roadmap</a>
    </div>

  </div>
</section>

<footer>
  <div class="footer-inner">
    <div class="footer-brand">
      <a href="index.html" class="ft-logo">
        <div class="logo-mark"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="lb-ft" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#f0b942"/><stop offset="100%" stop-color="#c8860a"/></linearGradient><linearGradient id="lk-ft" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#ffffff"/><stop offset="100%" stop-color="#e8e5dc"/></linearGradient></defs><rect width="64" height="64" rx="9" fill="url(#lb-ft)"/><rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5" fill="url(#lk-ft)"/><rect x="7.68" y="46.5" width="48.64" height="3.42" rx="1.5" fill="#000" opacity="0.06"/><path d="M 33 14 C 36 18, 41 23, 41 30 C 41 37, 36 41, 32 41 C 26 41, 22 37, 22 32 C 22 28, 25 26, 27 23 C 28 26, 30 27, 30 24 C 30 20, 32 17, 33 14 Z" fill="#c8860a"/></svg></div>
        <span class="ft-wm">Keyfire</span>
      </a>
      <p class="footer-tagline">Productivity automation for Windows. No scripting. Built solo in London, free during Beta.</p>
    </div>
    <div class="footer-links">
      <div class="ft-col">
        <h4>Product</h4>
        <a href="index.html#features">Features</a>
        <a href="pricing.html">Pricing</a>
        <a href="roadmap.html">Roadmap</a>
        <a href="changelog.html">Changelog</a>
        <a href="https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Keyfire_x64-setup.exe">Download</a>
      </div>
      <div class="ft-col">
        <h4>Resources</h4>
        <a href="trigr-help.html">Guide</a>
        <a href="blog/">Blog</a>
        <a href="https://github.com/Trigr-it/trigr-tauri">GitHub</a>
      </div>
      <div class="ft-col">
        <h4>Get in touch</h4>
        <a href="mailto:admin@keyfire.app">admin@keyfire.app</a>
      </div>
    </div>
  </div>
  <div class="footer-bottom">
    <span class="ft-copy">© 2026 Keyfire</span>
    <div class="ft-legal">
      <a href="terms.html">Terms</a>
      <a href="privacy.html">Privacy</a>
      <a href="refund.html">Refunds</a>
    </div>
  </div>
</footer>

</body>
</html>
`;
}

// ── Main ─────────────────────────────────────────────────────────────────────
(async () => {
  const key = readApiKey();
  console.log('[changelog] fetching entries from Featurebase…');
  const entries = await fetchAllEntries(key);
  if (entries.length === 0) {
    throw new Error('No changelog entries returned from Featurebase');
  }
  console.log(`[changelog] fetched ${entries.length} entries`);
  const html = buildPage(entries);
  const outPath = join(repoRoot, 'docs', 'changelog.html');
  writeFileSync(outPath, html, 'utf8');
  console.log(`[changelog] wrote ${outPath}`);
})();
