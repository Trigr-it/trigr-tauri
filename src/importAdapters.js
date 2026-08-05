// ── Third-party expansion import adapters ───────────────────────────────────
// Pure functions: file text in, { expansions, warnings } out. Each adapter
// returns entries shaped for applyExpansionImport in App.jsx:
//   { trigger, data: { text, html, triggerMode, displayName } }
// The caller stamps the category ("Imported") and runs the collision flow.
// Warnings are aggregated, user-facing sentences describing anything dropped
// or changed during conversion (no em-dashes in these strings).

// Triggers longer than the engine's keystroke buffer can never fire.
const MAX_TRIGGER_LENGTH = 50;

// Mirrors plainTextToHtml in TextExpansions.jsx — imported entries get both
// text and html bodies so the rich-text editor opens them like native saves.
function plainTextToHtml(text) {
  const escaped = (text || '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return escaped.replace(/\n/g, '<br>');
}

function escapeAttr(s) {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// Render a Keyfire token as the editor's native chip markup so imported
// snippets open with real editable chips instead of raw token text. Markup
// and display strings mirror insertTokenHtml / buildFillInChipDisplay /
// buildFormulaChipDisplay in TextExpansions.jsx (span.rte-token with a
// data-token attribute, trailing zero-width space for caret placement).
// Returns null for token shapes the editor has no chip convention for.
function tokenChipHtml(token) {
  let display = null;
  let m;
  if ((m = token.match(/^\{fillIn:([^:}]+)\}$/))) {
    display = `▭ ${m[1]}`;
  } else if ((m = token.match(/^\{fillIn:([^:}]+):([^:}]+)/))) {
    display = `▭ ${m[1]} · ${m[2]}`;
  } else if ((m = token.match(/^\{=\s*([^{}]*)\}$/))) {
    const expr = m[1].trim();
    display = `ƒ ${expr.length > 24 ? expr.slice(0, 22) + '…' : expr}`;
  } else if ((m = token.match(/^\{(?:date|time):([^{}]+)\}$/))) {
    display = m[1];
  } else if (token === '{date}') {
    display = 'Default';
  }
  if (display === null) return null;
  return `<span class="rte-token" data-token="${escapeAttr(token)}" contenteditable="false">${plainTextToHtml(display)}</span>&#8203;`;
}

// plainTextToHtml, but Keyfire tokens ({fillIn:…}, {=…}, {date…}, {time:…})
// become native editor chips. The fire path reads data.text (raw tokens), so
// chips are purely a display upgrade — text and html round-trip identically.
function tokenAwareHtml(text) {
  const re = /\{(?:fillIn:[^{}]+|=[^{}]*|date(?::[^{}]+)?|time:[^{}]+)\}/g;
  let out = '';
  let last = 0;
  for (const m of (text || '').matchAll(re)) {
    out += plainTextToHtml(text.slice(last, m.index));
    const chip = tokenChipHtml(m[0]);
    out += chip !== null ? chip : plainTextToHtml(m[0]);
    last = m.index + m[0].length;
  }
  out += plainTextToHtml((text || '').slice(last));
  return out;
}

function makeEntry(trigger, text, triggerMode, displayName) {
  return {
    trigger,
    data: {
      text,
      html: tokenAwareHtml(text),
      triggerMode,
      displayName: displayName || null,
    },
  };
}

// Shared tail: lowercase triggers (the engine matches against a lowercased
// buffer, so stored triggers must be lowercase to ever fire), drop triggers
// that can never fire (whitespace clears the keystroke buffer), drop
// over-long triggers, dedupe within the file. Mutates `counts`.
function finalizeEntries(rawEntries, counts) {
  const seen = new Set();
  const out = [];
  for (const e of rawEntries) {
    const lowered = e.trigger.toLowerCase();
    if (lowered.length === 0) continue;
    if (/\s/.test(lowered)) { counts.whitespace++; continue; }
    if (lowered.length > MAX_TRIGGER_LENGTH) { counts.tooLong++; continue; }
    if (lowered !== e.trigger) counts.caseLowered++;
    if (seen.has(lowered)) { counts.duplicates++; continue; }
    seen.add(lowered);
    out.push({ ...e, trigger: lowered });
  }
  return out;
}

function pluralNoun(n, singular, plural) {
  return n === 1 ? singular : plural;
}

function sharedWarnings(counts) {
  const warnings = [];
  if (counts.whitespace > 0) {
    warnings.push(`${counts.whitespace} ${pluralNoun(counts.whitespace, 'entry', 'entries')} skipped (triggers cannot contain spaces).`);
  }
  if (counts.caseLowered > 0) {
    warnings.push(`${counts.caseLowered} ${pluralNoun(counts.caseLowered, 'trigger', 'triggers')} made lowercase (Keyfire triggers are case-insensitive).`);
  }
  if (counts.duplicates > 0) {
    warnings.push(`${counts.duplicates} duplicate ${pluralNoun(counts.duplicates, 'trigger', 'triggers')} skipped.`);
  }
  if (counts.tooLong > 0) {
    warnings.push(`${counts.tooLong} ${pluralNoun(counts.tooLong, 'entry', 'entries')} skipped (trigger longer than ${MAX_TRIGGER_LENGTH} characters).`);
  }
  return warnings;
}

// ── Espanso (.yml match files) ──────────────────────────────────────────────
// Handles the common shapes of an Espanso match file without a YAML library:
//   matches:
//     - trigger: ":btw"
//       replace: "by the way"
//     - triggers: [":hi", ":hello"]
//       replace: |
//         multi
//         line
//       word: true
//       label: Greeting
// Dynamic entries (vars, forms, shell, regex triggers) and image/rich bodies
// are skipped with a warning. `word: true` maps to space mode; the Espanso
// default (fire as soon as the trigger is typed) maps to immediate mode.

function unquoteYamlScalar(raw) {
  let v = raw.trim();
  if (v.startsWith('"') && v.endsWith('"') && v.length >= 2) {
    const ESCAPES = { '"': '"', '\\': '\\', '/': '/', n: '\n', r: '\r', t: '\t', b: '\b', '0': '\0' };
    return v.slice(1, -1).replace(/\\(.)/g, (m, c) => ESCAPES[c] !== undefined ? ESCAPES[c] : m);
  }
  if (v.startsWith("'") && v.endsWith("'") && v.length >= 2) {
    return v.slice(1, -1).replace(/''/g, "'");
  }
  // Plain scalar: a ` #` starts a comment.
  const hash = v.search(/\s#/);
  if (hash >= 0) v = v.slice(0, hash).trimEnd();
  return v;
}

function indentOf(line) {
  const m = line.match(/^(\s*)/);
  return m[1].length;
}

// Collects a block scalar (| / |- / > / >-) starting after `startIdx`.
// Returns { value, nextIdx }.
function collectBlockScalar(lines, startIdx, keyIndent, style) {
  const body = [];
  let i = startIdx;
  let blockIndent = null;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === '') { body.push(''); i++; continue; }
    const ind = indentOf(line);
    if (ind <= keyIndent) break;
    if (blockIndent === null) blockIndent = ind;
    body.push(line.slice(Math.min(blockIndent, ind)));
    i++;
  }
  // Trim trailing blank lines (their newlines are noise inside an expansion).
  while (body.length > 0 && body[body.length - 1] === '') body.pop();
  let value;
  if (style.startsWith('>')) {
    // Folded: single newlines become spaces, blank lines become newlines.
    value = body
      .join('\n')
      .split(/\n{2,}/)
      .map(par => par.replace(/\n/g, ' '))
      .join('\n');
  } else {
    value = body.join('\n');
  }
  return { value, nextIdx: i };
}

export function parseEspansoYaml(text) {
  const lines = (text || '').split(/\r\n|\r|\n/);
  const counts = { dynamic: 0, images: 0, rich: 0, malformed: 0, whitespace: 0, caseLowered: 0, duplicates: 0, tooLong: 0 };
  const rawEntries = [];

  // Locate the top-level matches: block.
  let matchesIndent = -1;
  let i = 0;
  for (; i < lines.length; i++) {
    const m = lines[i].match(/^(\s*)matches:\s*(#.*)?$/);
    if (m) { matchesIndent = m[1].length; i++; break; }
  }
  if (matchesIndent < 0) {
    return { expansions: [], warnings: ['No "matches:" section found. Is this an Espanso match file?'] };
  }

  // Parse list items under matches:.
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === '' || line.trim().startsWith('#')) { i++; continue; }
    const ind = indentOf(line);
    if (ind <= matchesIndent) break; // left the matches block
    const itemMatch = line.match(/^(\s*)-\s*(.*)$/);
    if (!itemMatch) { i++; continue; }

    const itemIndent = itemMatch[1].length;
    const entry = { triggers: [], replace: null, word: false, label: null, dynamic: false, image: false, rich: false };

    // The item's keys: the remainder of the "- " line plus deeper lines.
    const keyLines = [];
    if (itemMatch[2].trim() !== '') {
      keyLines.push({ text: itemMatch[2], indent: itemIndent + 2, idx: i });
    }
    i++;
    while (i < lines.length) {
      const l = lines[i];
      if (l.trim() === '' || l.trim().startsWith('#')) { i++; continue; }
      const lInd = indentOf(l);
      if (lInd <= itemIndent) break; // next item or end of block
      keyLines.push({ text: l.trim(), indent: lInd, idx: i });
      i++;
    }

    for (let k = 0; k < keyLines.length; k++) {
      const kl = keyLines[k];
      const kv = kl.text.match(/^([A-Za-z_][A-Za-z0-9_]*):\s*(.*)$/);
      if (!kv) continue;
      const key = kv[1];
      const rawVal = kv[2].trim();

      if (key === 'trigger') {
        entry.triggers.push(unquoteYamlScalar(rawVal));
      } else if (key === 'triggers') {
        if (rawVal.startsWith('[')) {
          const inner = rawVal.replace(/^\[/, '').replace(/\]\s*(#.*)?$/, '');
          for (const piece of inner.match(/"(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^,]+/g) || []) {
            const t = unquoteYamlScalar(piece);
            if (t) entry.triggers.push(t);
          }
        } else {
          // Block list: consume following "- item" key lines.
          while (k + 1 < keyLines.length && keyLines[k + 1].text.startsWith('- ')) {
            k++;
            entry.triggers.push(unquoteYamlScalar(keyLines[k].text.slice(2)));
          }
        }
      } else if (key === 'replace') {
        const styleMatch = rawVal.match(/^([|>][+-]?)\s*(#.*)?$/);
        if (styleMatch) {
          const { value, nextIdx } = collectBlockScalar(lines, kl.idx + 1, kl.indent, styleMatch[1]);
          entry.replace = value;
          // Skip the keyLines the block consumed.
          while (k + 1 < keyLines.length && keyLines[k + 1].idx < nextIdx) k++;
        } else {
          entry.replace = unquoteYamlScalar(rawVal);
        }
      } else if (key === 'word') {
        entry.word = /^true$/i.test(unquoteYamlScalar(rawVal));
      } else if (key === 'label') {
        entry.label = unquoteYamlScalar(rawVal);
      } else if (key === 'vars' || key === 'form' || key === 'regex') {
        entry.dynamic = true;
      } else if (key === 'image_path') {
        entry.image = true;
      } else if (key === 'markdown' || key === 'html') {
        entry.rich = true;
      }
    }

    if (entry.image) { counts.images++; continue; }
    if (entry.dynamic || (entry.replace && /\{\{.+?\}\}/.test(entry.replace))) { counts.dynamic++; continue; }
    if (entry.rich && entry.replace === null) { counts.rich++; continue; }
    if (entry.triggers.length === 0 || entry.replace === null || entry.replace === '') { counts.malformed++; continue; }

    const triggerMode = entry.word ? 'space' : 'immediate';
    for (const trigger of entry.triggers) {
      rawEntries.push(makeEntry(trigger, entry.replace, triggerMode, entry.label));
    }
  }

  const expansions = finalizeEntries(rawEntries, counts);
  const warnings = [];
  if (counts.dynamic > 0) {
    warnings.push(`${counts.dynamic} dynamic ${pluralNoun(counts.dynamic, 'snippet', 'snippets')} skipped (forms, variables, or scripts).`);
  }
  if (counts.images > 0) {
    warnings.push(`${counts.images} image ${pluralNoun(counts.images, 'snippet', 'snippets')} skipped.`);
  }
  if (counts.rich > 0) {
    warnings.push(`${counts.rich} rich-format ${pluralNoun(counts.rich, 'snippet', 'snippets')} skipped (markdown or HTML bodies).`);
  }
  if (counts.malformed > 0) {
    warnings.push(`${counts.malformed} ${pluralNoun(counts.malformed, 'entry', 'entries')} skipped (missing trigger or replacement).`);
  }
  warnings.push(...sharedWarnings(counts));
  return { expansions, warnings };
}

// ── TextExpander (.csv exports) ─────────────────────────────────────────────
// Modern TextExpander exports a snippet group as headerless CSV:
//   "abbreviation","content","group label"
// Quoted fields, "" escapes, embedded newlines for multi-line snippets.
// Abbreviations fire as soon as they are typed -> immediate mode. The group
// label becomes the display name. Snippets using TextExpander macros
// (%filltext%, %clipboard%, %snippet%, date tokens like %Y) are skipped.

function parseCsvRows(text) {
  const rows = [];
  let row = [];
  let field = '';
  let inQuotes = false;
  let i = 0;
  const src = text || '';
  while (i < src.length) {
    const ch = src[i];
    if (inQuotes) {
      if (ch === '"') {
        if (src[i + 1] === '"') { field += '"'; i += 2; continue; }
        inQuotes = false;
        i++;
        continue;
      }
      field += ch;
      i++;
      continue;
    }
    if (ch === '"') { inQuotes = true; i++; continue; }
    if (ch === ',') { row.push(field); field = ''; i++; continue; }
    if (ch === '\n' || ch === '\r') {
      if (ch === '\r' && src[i + 1] === '\n') i++;
      row.push(field); field = '';
      if (row.length > 1 || row[0] !== '') rows.push(row);
      row = [];
      i++;
      continue;
    }
    field += ch;
    i++;
  }
  row.push(field);
  if (row.length > 1 || row[0] !== '') rows.push(row);
  return rows;
}

// TextExpander macro detection: named function macros, or a % immediately
// followed by a single letter token (strftime-style date fields like %Y).
// Plain percentages ("50% off") don't match either shape.
function hasTextExpanderMacro(content) {
  if (/%(fill\w*|clipboard|key|snippet|sysinfo)\b/i.test(content)) return true;
  return /(?:^|[^%\w])%[a-zA-Z](?![a-zA-Z])/.test(content);
}

export function parseTextExpanderCsv(text) {
  const counts = { dynamic: 0, malformed: 0, whitespace: 0, caseLowered: 0, duplicates: 0, tooLong: 0 };
  const rawEntries = [];

  for (const row of parseCsvRows(text)) {
    const trigger = (row[0] || '').trim();
    const content = row[1] || '';
    const label = (row[2] || '').trim() || null;
    if (!trigger || !content) { counts.malformed++; continue; }
    if (hasTextExpanderMacro(content)) { counts.dynamic++; continue; }
    rawEntries.push(makeEntry(trigger, content, 'immediate', label));
  }

  const expansions = finalizeEntries(rawEntries, counts);
  const warnings = [];
  if (counts.dynamic > 0) {
    warnings.push(`${counts.dynamic} dynamic ${pluralNoun(counts.dynamic, 'snippet', 'snippets')} skipped (fill-ins, dates, or other macros).`);
  }
  if (counts.malformed > 0) {
    warnings.push(`${counts.malformed} ${pluralNoun(counts.malformed, 'row', 'rows')} skipped (missing abbreviation or content).`);
  }
  warnings.push(...sharedWarnings(counts));
  return { expansions, warnings };
}

// ── Text Blaze (.json dashboard exports) ────────────────────────────────────
// Version-7 export shape: { version, folders: [{ name, snippets: [{ name,
// shortcut, type: "text"|"html", text, html }] }] }. Shortcuts fire as they
// are typed -> immediate mode; the snippet name becomes the display name.
// Text Blaze commands use {command: args} syntax ({time}, {formtext}, {=…},
// {cursor}, …) — snippets containing them are skipped with a warning. Styled
// snippets (type "html") import their plain-text body; the export's HTML is
// TinyMCE-flavoured and not safe to drop into the expansion editor as-is.

const TEXTBLAZE_COMMANDS = [
  'time', 'formtext', 'formmenu', 'formtoggle', 'formdate', 'formparagraph',
  'cursor', 'clipboard', 'if', 'elseif', 'else', 'endif', 'note', 'endnote',
  'snippet', 'import', 'key', 'click', 'wait', 'run', 'button', 'error',
  'image', 'link', 'repeat', 'endrepeat', 'site', 'urlload', 'urlsend', 'ping',
];

function hasTextBlazeCommand(content) {
  const re = new RegExp(`\\{\\s*(?:${TEXTBLAZE_COMMANDS.join('|')})\\s*[:};]`, 'i');
  return re.test(content);
}

// Text Blaze {time: FORMAT} tokens whose format has an exact Keyfire date
// token equivalent. Anything else (arbitrary moment formats, shift= args)
// leaves the token in place and the snippet skips.
const TEXTBLAZE_TIME_MAP = {
  'YYYY-MM-DD': '{date:YYYY-MM-DD}',
  'DD/MM/YYYY': '{date:DD/MM/YYYY}',
  'DD/MM/YY': '{date:DD/MM/YY}',
  'MM/DD/YYYY': '{date:MM/DD/YYYY}',
  'D MMMM YYYY': '{date:D MMMM YYYY}',
  'HH:mm': '{time:HH:MM}',
  'HH:MM': '{time:HH:MM}',
  'HH:mm:ss': '{time:HH:MM:SS}',
  'HH:MM:SS': '{time:HH:MM:SS}',
};

// Convert Text Blaze dynamic commands into their Keyfire equivalents:
//   forms    -> {fillIn:…} tokens (same prompt-once semantics both sides)
//   formulas -> {=expr} (Keyfire expressions read fill-in values by label,
//               so {=price * quantity} carries over verbatim; Text Blaze
//               formatting args like `; format=,` are presentation-only
//               and dropped; non-arithmetic formulas are left unconverted)
//   times    -> {date:…}/{time:…} when the format has an exact equivalent
// Returns { text, converted, unconvertible } — anything left unconverted
// makes the caller skip the whole snippet rather than import broken output.
function convertTextBlazeCommands(content) {
  let converted = 0;
  let anonCounter = 0;
  let unconvertible = false;

  let text = content.replace(
    /\{\s*(formtext|formparagraph|formmenu|formdate)\s*:?\s*([^{}]*)\}/gi,
    (whole, cmd, argStr) => {
      // Segments split on ';': `key=value` pairs or bare dropdown options.
      const options = [];
      let name = null;
      let def = null;
      for (const seg of argStr.split(';')) {
        const s = seg.trim();
        if (!s) continue;
        const eq = s.indexOf('=');
        if (eq > 0) {
          const key = s.slice(0, eq).trim().toLowerCase();
          const val = s.slice(eq + 1).trim();
          if (key === 'name') name = val;
          else if (key === 'default') { def = val; options.push(val); }
          else if (key === 'values') options.push(...val.split(',').map(v => v.trim()).filter(Boolean));
          // width/cols/rows/formatter etc. are presentation-only — dropped.
        } else {
          options.push(s);
        }
      }

      // Fill-in labels may not contain ':' or '}' (token grammar limits).
      let label = (name || '').replace(/[:}{]/g, '').trim();
      if (!label) { anonCounter++; label = `Field ${anonCounter}`; }

      const kind = cmd.toLowerCase();
      let token;
      if (kind === 'formmenu') {
        // Dropdown options are comma-separated in the token grammar — an
        // option containing a comma or colon cannot be represented; bail on
        // the whole command so the snippet falls through to the dynamic skip.
        const opts = [...new Set(options)];
        if (opts.length === 0 || opts.some(o => /[:,}{]/.test(o))) { unconvertible = true; return whole; }
        token = `{fillIn:${label}:dropdown:${opts.join(',')}`;
        if (def && !/[:}{]/.test(def)) token += `:default=${def}`;
        token += '}';
      } else {
        const fillKind = kind === 'formparagraph' ? 'multiline'
          : kind === 'formdate' ? 'date'
          : 'text';
        if (def && !/[}{]/.test(def)) {
          token = `{fillIn:${label}:${fillKind}:default=${def}}`;
        } else if (fillKind !== 'text') {
          token = `{fillIn:${label}:${fillKind}}`;
        } else {
          token = `{fillIn:${label}}`;
        }
      }
      converted++;
      return token;
    }
  );

  // Formulas: {=expr} or {=expr; format=,}. Keep pure arithmetic over
  // identifiers (fill-in labels resolve in the expression scope); anything
  // fancier (function calls, strings) stays Text Blaze-specific.
  text = text.replace(/\{=\s*([^{}]*)\}/g, (whole, body) => {
    const expr = body.split(';')[0].trim();
    if (expr && /^[\w\s+\-*/().]+$/.test(expr)) {
      converted++;
      return `{=${expr}}`;
    }
    unconvertible = true;
    return whole;
  });

  // Simple {time: FORMAT} tokens with an exact Keyfire equivalent.
  text = text.replace(/\{\s*time\s*:\s*([^{};]+?)\s*\}/gi, (whole, fmt) => {
    const mapped = TEXTBLAZE_TIME_MAP[fmt.trim()];
    if (mapped) {
      converted++;
      return mapped;
    }
    return whole; // stays a {time:…} command -> snippet skips
  });

  return { text, converted, unconvertible };
}

export function parseTextBlazeJson(text) {
  const counts = { dynamic: 0, malformed: 0, styled: 0, whitespace: 0, caseLowered: 0, duplicates: 0, tooLong: 0 };
  const rawEntries = [];

  let parsed;
  try {
    parsed = JSON.parse(text || '');
  } catch {
    return { expansions: [], warnings: ['Could not read this file as JSON. Is it a Text Blaze export?'] };
  }
  const folders = Array.isArray(parsed?.folders) ? parsed.folders : [];
  if (folders.length === 0) {
    return { expansions: [], warnings: ['No snippet folders found. Is this a Text Blaze export?'] };
  }

  let formsConverted = 0;
  for (const folder of folders) {
    for (const snip of (Array.isArray(folder?.snippets) ? folder.snippets : [])) {
      const trigger = (snip?.shortcut || '').trim();
      const content = snip?.text || '';
      const label = (snip?.name || '').trim() || null;
      if (!trigger || !content) { counts.malformed++; continue; }
      // Forms, arithmetic formulas, and exact-format dates convert to
      // Keyfire tokens; anything dynamic left over skips the whole snippet.
      const { text: convertedText, converted, unconvertible } = convertTextBlazeCommands(content);
      if (unconvertible || hasTextBlazeCommand(convertedText)) { counts.dynamic++; continue; }
      if (converted > 0) formsConverted++;
      if (snip?.type === 'html') counts.styled++;
      rawEntries.push(makeEntry(trigger, convertedText, 'immediate', label));
    }
  }

  const expansions = finalizeEntries(rawEntries, counts);
  const warnings = [];
  if (formsConverted > 0) {
    warnings.push(`${formsConverted} ${pluralNoun(formsConverted, 'snippet', 'snippets')} converted to Keyfire fill-in fields, formulas, or date tokens.`);
  }
  if (counts.dynamic > 0) {
    warnings.push(`${counts.dynamic} dynamic ${pluralNoun(counts.dynamic, 'snippet', 'snippets')} skipped (commands with no Keyfire equivalent).`);
  }
  if (counts.styled > 0) {
    warnings.push(`${counts.styled} styled ${pluralNoun(counts.styled, 'snippet', 'snippets')} imported as plain text.`);
  }
  if (counts.malformed > 0) {
    warnings.push(`${counts.malformed} ${pluralNoun(counts.malformed, 'snippet', 'snippets')} skipped (missing shortcut or content).`);
  }
  warnings.push(...sharedWarnings(counts));
  return { expansions, warnings };
}

// ── AutoHotkey hotstrings (.ahk scripts) ────────────────────────────────────
// Imports `::trigger::expansion` hotstring lines, including basic
// multi-line continuation sections. Everything else in the script (hotkeys,
// functions, directives) is ignored. Option handling:
//   *  fire without an ending character  -> immediate mode
//   ?  fire inside words                 -> immediate matching covers this
//   C  case-sensitive                    -> imported case-insensitive, warned
//   R / T raw or text mode               -> body taken literally
//   X  executes code                     -> skipped
// Backtick escapes are converted; {Enter}/{Tab} become newline/tab; entries
// whose body contains other {key} send-commands are skipped (they are
// keystroke scripts, not text).

const AHK_ESCAPES = { n: '\n', r: '\r', t: '\t', b: '\b', s: ' ', '`': '`' };

function convertAhkBody(raw, isRawMode) {
  // Inline comments: AHK requires whitespace before the semicolon.
  let body = raw.replace(/\s+;.*$/, '');
  // Backtick escapes apply in all modes.
  body = body.replace(/`(.)/g, (m, c) => {
    const lower = c.toLowerCase();
    return AHK_ESCAPES[lower] !== undefined ? AHK_ESCAPES[lower] : c;
  });
  if (isRawMode) return { text: body, hasSendCommands: false };
  // Brace send-syntax: translate the text-like ones, flag the rest.
  body = body
    .replace(/\{enter\}/gi, '\n')
    .replace(/\{tab\}/gi, '\t')
    .replace(/\{\{\}/g, '\x00OPEN\x00')
    .replace(/\{\}\}/g, '\x00CLOSE\x00');
  const hasSendCommands = /\{[^}]+\}/.test(body);
  body = body.replace(/\x00OPEN\x00/g, '{').replace(/\x00CLOSE\x00/g, '}');
  return { text: body, hasSendCommands };
}

export function parseAhkHotstrings(text) {
  const lines = (text || '').split(/\r\n|\r|\n/);
  const counts = { code: 0, sendCommands: 0, continuationUnsupported: 0, caseSensitive: 0, whitespace: 0, caseLowered: 0, duplicates: 0, tooLong: 0 };
  const rawEntries = [];
  let inBlockComment = false;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    if (inBlockComment) {
      if (/^\*\//.test(trimmed)) inBlockComment = false;
      continue;
    }
    if (/^\/\*/.test(trimmed)) { inBlockComment = true; continue; }
    if (trimmed === '' || trimmed.startsWith(';')) continue;

    const m = trimmed.match(/^:([^:]*):(.+?)::(.*)$/);
    if (!m) continue;

    const options = m[1].toUpperCase();
    const trigger = m[2];
    let replacement = m[3].trim();

    if (options.includes('X')) { counts.code++; continue; }
    const isRawMode = options.includes('R') || options.includes('T');
    const immediate = options.includes('*');
    if (options.includes('C')) counts.caseSensitive++;

    if (replacement === '') {
      // Multi-line continuation section, or a hotstring that runs code below.
      let j = i + 1;
      while (j < lines.length && lines[j].trim() === '') j++;
      const contMatch = j < lines.length ? lines[j].trim().match(/^\((.*)$/) : null;
      if (!contMatch) { counts.code++; continue; }
      const contOpts = contMatch[1];
      if (/join/i.test(contOpts)) { counts.continuationUnsupported++; continue; }
      const ltrim = /ltrim/i.test(contOpts);
      const body = [];
      let closed = false;
      j++;
      for (; j < lines.length; j++) {
        if (lines[j].trim() === ')') { closed = true; break; }
        body.push(ltrim ? lines[j].replace(/^\s+/, '') : lines[j]);
      }
      if (!closed) { counts.continuationUnsupported++; continue; }
      i = j;
      replacement = body.join('\n');
    }

    const { text: bodyText, hasSendCommands } = convertAhkBody(replacement, isRawMode);
    if (hasSendCommands) { counts.sendCommands++; continue; }
    if (bodyText === '') { counts.code++; continue; }

    rawEntries.push(makeEntry(trigger, bodyText, immediate ? 'immediate' : 'space', null));
  }

  const expansions = finalizeEntries(rawEntries, counts);
  const warnings = [];
  if (counts.code > 0) {
    warnings.push(`${counts.code} ${pluralNoun(counts.code, 'hotstring', 'hotstrings')} skipped (they run script code, not text).`);
  }
  if (counts.sendCommands > 0) {
    warnings.push(`${counts.sendCommands} ${pluralNoun(counts.sendCommands, 'hotstring', 'hotstrings')} skipped (they send key commands, not text).`);
  }
  if (counts.continuationUnsupported > 0) {
    warnings.push(`${counts.continuationUnsupported} multi-line ${pluralNoun(counts.continuationUnsupported, 'hotstring', 'hotstrings')} skipped (unsupported continuation options).`);
  }
  if (counts.caseSensitive > 0) {
    warnings.push(`${counts.caseSensitive} case-sensitive ${pluralNoun(counts.caseSensitive, 'hotstring', 'hotstrings')} imported as case-insensitive.`);
  }
  warnings.push(...sharedWarnings(counts));
  return { expansions, warnings };
}
