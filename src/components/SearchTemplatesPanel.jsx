import React, { useState, useRef, useEffect, useLayoutEffect, useMemo } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import './SearchTemplatesPanel.css';
import { MacroSequenceForm, AppForm } from './MacroPanel';
import { SearchBar } from './SearchBar';
import { findPresetIconForUrl } from '../utils/presetIcons';
import { readVoicePhrases, writeVoicePhrases } from '../voicePhrases';

// ── Colour palette (matches TextExpansions) ────────────────────────────────

const CATEGORY_COLOURS = [
  { hex: null,      label: 'None' },
  { hex: '#e84040', label: 'Red' },
  { hex: '#e87040', label: 'Orange' },
  { hex: '#e8a020', label: 'Gold' },
  { hex: '#50c878', label: 'Green' },
  { hex: '#40b0b0', label: 'Teal' },
  { hex: '#4a9eff', label: 'Blue' },
  { hex: '#6a7eff', label: 'Indigo' },
  { hex: '#9a6eff', label: 'Purple' },
  { hex: '#c864ff', label: 'Violet' },
  { hex: '#ff6eb4', label: 'Pink' },
  { hex: '#8a8799', label: 'Grey' },
  { hex: '#c0b090', label: 'Sand' },
];

function ColourPicker({ value, onChange }) {
  return (
    <div className="cat-colour-picker">
      {CATEGORY_COLOURS.map(c => (
        <button
          key={c.label}
          className={`cat-colour-swatch${value === c.hex ? ' selected' : ''}`}
          style={c.hex ? { '--swatch-color': c.hex, background: c.hex } : undefined}
          onClick={() => onChange(c.hex)}
          title={c.label}
          type="button"
        />
      ))}
    </div>
  );
}

// ── Left-click-only sensor (prevents right-click from starting drag) ───────

class LeftClickSensor extends PointerSensor {
  static activators = [
    {
      eventName: 'onPointerDown',
      handler: ({ nativeEvent }) => nativeEvent.button === 0,
    },
  ];
}

// ── Sortable category row wrapper ──────────────────────────────────────────

function SortableCatRow({ id, children }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };
  return (
    <div ref={setNodeRef} style={style} {...attributes} {...listeners}>
      {children}
    </div>
  );
}

// ── Bundled presets (with categories for grouped picker) ────────────────────

const PRESETS = [
  // ── Search ──
  { label: 'Google',               trigger: 'g',     urlTemplate: 'https://www.google.com/search?q={query}',                                       category: 'Search',      icon: 'google.png',          description: 'Web search' },
  { label: 'DuckDuckGo',           trigger: 'ddg',   urlTemplate: 'https://duckduckgo.com/?q={query}',                                              category: 'Search',      icon: 'duckduckgo.png',      description: 'Privacy-focused search' },
  { label: 'Bing',                 trigger: 'bn',    urlTemplate: 'https://www.bing.com/search?q={query}',                                          category: 'Search',      icon: 'bing.png',            description: 'Microsoft search engine' },
  { label: 'Brave Search',         trigger: 'bs',    urlTemplate: 'https://search.brave.com/search?q={query}',                                      category: 'Search',      icon: 'brave.png',           description: 'Independent privacy search' },
  { label: 'Kagi',                 trigger: 'kg',    urlTemplate: 'https://kagi.com/search?q={query}',                                              category: 'Search',      icon: 'kagi.png',            description: 'Premium ad-free search' },
  { label: 'Ecosia',               trigger: 'eco',   urlTemplate: 'https://www.ecosia.org/search?q={query}',                                        category: 'Search',      icon: 'ecosia.png',          description: 'Search that plants trees' },
  { label: 'Yahoo',                trigger: 'yah',   urlTemplate: 'https://search.yahoo.com/search?p={query}',                                      category: 'Search',      icon: 'yahoo.png',           description: 'Yahoo web search' },
  { label: 'Google Translate',     trigger: 'tr',    urlTemplate: 'https://translate.google.com/?sl=auto&tl=en&text={query}&op=translate',          category: 'Search',      icon: 'googletranslate.png', description: 'Translate (auto → English)' },

  // ── AI ──
  { label: 'ChatGPT',              trigger: 'gpt',   urlTemplate: 'https://chatgpt.com/?q={query}',                                                 category: 'AI',          icon: 'chatgpt.png',         description: 'AI chat assistant' },
  { label: 'Perplexity',           trigger: 'pp',    urlTemplate: 'https://www.perplexity.ai/search?q={query}',                                     category: 'AI',          icon: 'perplexity.png',      description: 'AI-powered answers' },
  { label: 'Phind',                trigger: 'phind', urlTemplate: 'https://www.phind.com/search?q={query}',                                         category: 'AI',          icon: null,                  description: 'AI search for developers' },
  { label: 'You.com',              trigger: 'you',   urlTemplate: 'https://you.com/search?q={query}',                                               category: 'AI',          icon: 'you.png',             description: 'AI-assisted search' },
  { label: 'Microsoft Copilot',    trigger: 'cop',   urlTemplate: 'https://copilot.microsoft.com/?q={query}',                                       category: 'AI',          icon: 'copilot.png',         description: 'Microsoft AI assistant' },

  // ── Development ──
  { label: 'GitHub',               trigger: 'gh',    urlTemplate: 'https://github.com/search?q={query}&type=repositories',                          category: 'Development', icon: 'github.png',          description: 'Code & repositories' },
  { label: 'GitLab',               trigger: 'gl',    urlTemplate: 'https://gitlab.com/search?search={query}',                                       category: 'Development', icon: 'gitlab.png',          description: 'Code repositories' },
  { label: 'Stack Overflow',       trigger: 'so',    urlTemplate: 'https://stackoverflow.com/search?q={query}',                                     category: 'Development', icon: 'stackoverflow.png',   description: 'Developer Q&A' },
  { label: 'Server Fault',         trigger: 'sf',    urlTemplate: 'https://serverfault.com/search?q={query}',                                       category: 'Development', icon: 'serverfault.png',     description: 'Sysadmin Q&A' },
  { label: 'Super User',           trigger: 'su',    urlTemplate: 'https://superuser.com/search?q={query}',                                         category: 'Development', icon: 'superuser.png',       description: 'Power-user Q&A' },
  { label: 'MDN',                  trigger: 'mdn',   urlTemplate: 'https://developer.mozilla.org/en-US/search?q={query}',                           category: 'Development', icon: 'mdn.png',             description: 'Web docs & references' },
  { label: 'W3Schools',            trigger: 'w3s',   urlTemplate: 'https://www.w3schools.com/search/search.asp?q={query}',                          category: 'Development', icon: 'w3schools.png',       description: 'Web tutorials' },
  { label: 'Can I Use',            trigger: 'ciu',   urlTemplate: 'https://caniuse.com/?search={query}',                                            category: 'Development', icon: 'caniuse.png',         description: 'Browser feature support' },
  { label: 'DevDocs',              trigger: 'dd',    urlTemplate: 'https://devdocs.io/#q={query}',                                                  category: 'Development', icon: 'devdocs.png',         description: 'Developer docs aggregator' },
  { label: 'CodePen',              trigger: 'cp',    urlTemplate: 'https://codepen.io/search/pens?q={query}',                                       category: 'Development', icon: 'codepen.png',         description: 'Front-end playground' },
  { label: 'regex101',             trigger: 're',    urlTemplate: 'https://regex101.com/?regex={query}',                                            category: 'Development', icon: 'regex101.png',        description: 'Regex tester' },
  { label: 'npm',                  trigger: 'npm',   urlTemplate: 'https://www.npmjs.com/search?q={query}',                                         category: 'Development', icon: 'npm.png',             description: 'JavaScript packages' },
  { label: 'PyPI',                 trigger: 'pip',   urlTemplate: 'https://pypi.org/search/?q={query}',                                             category: 'Development', icon: 'pypi.png',            description: 'Python packages' },
  { label: 'crates.io',            trigger: 'cr',    urlTemplate: 'https://crates.io/search?q={query}',                                             category: 'Development', icon: 'crates.png',          description: 'Rust packages' },
  { label: 'pkg.go.dev',           trigger: 'go',    urlTemplate: 'https://pkg.go.dev/search?q={query}',                                            category: 'Development', icon: 'pkggo.png',           description: 'Go packages' },
  { label: 'NuGet',                trigger: 'nu',    urlTemplate: 'https://www.nuget.org/packages?q={query}',                                       category: 'Development', icon: 'nuget.png',           description: '.NET packages' },
  { label: 'Packagist',            trigger: 'pkst',  urlTemplate: 'https://packagist.org/?query={query}',                                           category: 'Development', icon: 'packagist.png',       description: 'PHP packages' },
  { label: 'Docker Hub',           trigger: 'dh',    urlTemplate: 'https://hub.docker.com/search?q={query}',                                        category: 'Development', icon: 'docker.png',          description: 'Container images' },
  { label: 'Hugging Face',         trigger: 'hf',    urlTemplate: 'https://huggingface.co/search/full-text?q={query}',                              category: 'Development', icon: 'huggingface.png',     description: 'ML models & datasets' },
  { label: 'Hacker News',          trigger: 'hn',    urlTemplate: 'https://hn.algolia.com/?q={query}',                                              category: 'Development', icon: 'hackernews.svg',      description: 'Tech news & discussion' },

  // ── Reference ──
  { label: 'Wolfram Alpha',        trigger: 'wa',    urlTemplate: 'https://www.wolframalpha.com/input?i={query}',                                   category: 'Reference',   icon: 'wolframalpha.png',    description: 'Computational answers' },
  { label: 'Cambridge Dictionary', trigger: 'dict',  urlTemplate: 'https://dictionary.cambridge.org/search/english/?q={query}',                     category: 'Reference',   icon: 'cambridge.png',       description: 'English definitions' },
  { label: 'Merriam-Webster',      trigger: 'mw',    urlTemplate: 'https://www.merriam-webster.com/dictionary/{query}',                             category: 'Reference',   icon: 'merriamwebster.png',  description: 'US English dictionary' },
  { label: 'Thesaurus.com',        trigger: 'thes',  urlTemplate: 'https://www.thesaurus.com/browse/{query}',                                       category: 'Reference',   icon: 'thesaurus.png',       description: 'Synonyms & antonyms' },
  { label: 'Etymonline',           trigger: 'etym',  urlTemplate: 'https://www.etymonline.com/search?q={query}',                                    category: 'Reference',   icon: 'etymonline.png',      description: 'Word etymology' },
  { label: 'Britannica',           trigger: 'brit',  urlTemplate: 'https://www.britannica.com/search?query={query}',                                category: 'Reference',   icon: 'britannica.png',      description: 'Encyclopedia' },
  { label: 'Quora',                trigger: 'quora', urlTemplate: 'https://www.quora.com/search?q={query}',                                         category: 'Reference',   icon: 'quora.png',           description: 'Community Q&A' },
  { label: 'arxiv',                trigger: 'arx',   urlTemplate: 'https://arxiv.org/search/?searchtype=all&query={query}',                         category: 'Reference',   icon: 'arxiv.png',           description: 'Research papers' },
  { label: 'PubMed',               trigger: 'pm',    urlTemplate: 'https://pubmed.ncbi.nlm.nih.gov/?term={query}',                                  category: 'Reference',   icon: 'pubmed.png',          description: 'Medical research' },
  { label: 'IEEE Xplore',          trigger: 'ieee',  urlTemplate: 'https://ieeexplore.ieee.org/search/searchresult.jsp?queryText={query}',          category: 'Reference',   icon: null,                  description: 'Engineering papers' },
  { label: 'Google Scholar',       trigger: 'gs',    urlTemplate: 'https://scholar.google.com/scholar?q={query}',                                   category: 'Reference',   icon: 'scholar.png',         description: 'Academic search' },
  { label: 'Internet Archive',     trigger: 'ia',    urlTemplate: 'https://archive.org/search?query={query}',                                       category: 'Reference',   icon: 'archive.png',         description: 'Books, video, audio archive' },

  // ── Media ──
  { label: 'YouTube',              trigger: 'yt',    urlTemplate: 'https://www.youtube.com/results?search_query={query}',                           category: 'Media',       icon: 'youtube.png',         description: 'Video search' },
  { label: 'Vimeo',                trigger: 'vim',   urlTemplate: 'https://vimeo.com/search?q={query}',                                             category: 'Media',       icon: 'vimeo.png',           description: 'Creator video platform' },
  { label: 'Twitch',               trigger: 'tw',    urlTemplate: 'https://www.twitch.tv/search?term={query}',                                      category: 'Media',       icon: 'twitch.png',          description: 'Live streaming' },
  { label: 'TikTok',               trigger: 'tik',   urlTemplate: 'https://www.tiktok.com/search?q={query}',                                        category: 'Media',       icon: 'tiktok.png',          description: 'Short-form video' },
  { label: 'Reddit',               trigger: 'r',     urlTemplate: 'https://www.reddit.com/search/?q={query}',                                       category: 'Media',       icon: 'reddit.png',          description: 'Communities & posts' },
  { label: 'X (Twitter)',          trigger: 'x',     urlTemplate: 'https://twitter.com/search?q={query}',                                           category: 'Media',       icon: 'twitter.png',         description: 'Posts & profiles' },
  { label: 'LinkedIn',             trigger: 'li',    urlTemplate: 'https://www.linkedin.com/search/results/all/?keywords={query}',                  category: 'Media',       icon: 'linkedin.png',        description: 'Professional network' },
  { label: 'Pinterest',            trigger: 'pin',   urlTemplate: 'https://www.pinterest.com/search/pins/?q={query}',                               category: 'Media',       icon: 'pinterest.png',       description: 'Visual inspiration boards' },
  { label: 'Wikipedia',            trigger: 'wiki',  urlTemplate: 'https://en.wikipedia.org/w/index.php?search={query}',                            category: 'Media',       icon: 'wikipedia.png',       description: 'Encyclopedia articles' },
  { label: 'Spotify',              trigger: 'sp',    urlTemplate: 'https://open.spotify.com/search/{query}',                                        category: 'Media',       icon: 'spotify.png',         description: 'Music & podcasts' },
  { label: 'SoundCloud',           trigger: 'sc',    urlTemplate: 'https://soundcloud.com/search?q={query}',                                        category: 'Media',       icon: 'soundcloud.png',      description: 'Audio & tracks' },
  { label: 'Bandcamp',             trigger: 'bc',    urlTemplate: 'https://bandcamp.com/search?q={query}',                                          category: 'Media',       icon: 'bandcamp.png',        description: 'Indie music marketplace' },
  { label: 'IMDB',                 trigger: 'imdb',  urlTemplate: 'https://www.imdb.com/find?q={query}',                                            category: 'Media',       icon: 'imdb.png',            description: 'Movies & TV' },
  { label: 'Letterboxd',           trigger: 'lbox',  urlTemplate: 'https://letterboxd.com/search/{query}/',                                         category: 'Media',       icon: 'letterboxd.png',      description: 'Film reviews & lists' },
  { label: 'Goodreads',            trigger: 'gr',    urlTemplate: 'https://www.goodreads.com/search?q={query}',                                     category: 'Media',       icon: 'goodreads.png',       description: 'Book reviews' },
  { label: 'Steam',                trigger: 'stm',   urlTemplate: 'https://store.steampowered.com/search/?term={query}',                            category: 'Media',       icon: 'steam.png',           description: 'PC game store' },
  { label: 'Unsplash',             trigger: 'uns',   urlTemplate: 'https://unsplash.com/s/photos/{query}',                                          category: 'Media',       icon: 'unsplash.png',        description: 'Free stock photos' },

  // ── Maps ──
  { label: 'Google Maps',          trigger: 'maps',  urlTemplate: 'https://www.google.com/maps/search/{query}',                                     category: 'Maps',        icon: 'googlemaps.png',      description: 'Places & directions' },
  { label: 'Bing Maps',            trigger: 'bm',    urlTemplate: 'https://www.bing.com/maps?q={query}',                                            category: 'Maps',        icon: 'bing.png',            description: 'Microsoft maps' },
  { label: 'OpenStreetMap',        trigger: 'osm',   urlTemplate: 'https://www.openstreetmap.org/search?query={query}',                             category: 'Maps',        icon: 'openstreetmap.png',   description: 'Open mapping data' },
  { label: 'Ordnance Survey',      trigger: 'os',    urlTemplate: 'https://osdatahub.os.uk/search?q={query}',                                       category: 'Maps',        icon: 'ordnancesurvey.png',  description: 'UK mapping data' },
  { label: 'what3words',           trigger: 'w3w',   urlTemplate: 'https://what3words.com/{query}',                                                 category: 'Maps',        icon: 'what3words.png',      description: '3-word location codes' },

  // ── News ──
  { label: 'BBC',                  trigger: 'bbc',   urlTemplate: 'https://www.bbc.co.uk/search?q={query}',                                         category: 'News',        icon: 'bbc.png',             description: 'BBC news search' },
  { label: 'Guardian',             trigger: 'gua',   urlTemplate: 'https://www.theguardian.com/search?q={query}',                                   category: 'News',        icon: 'guardian.png',        description: 'Guardian news search' },
  { label: 'The Telegraph',        trigger: 'tel',   urlTemplate: 'https://www.telegraph.co.uk/search/?q={query}',                                  category: 'News',        icon: 'telegraph.png',       description: 'Telegraph UK news' },
  { label: 'The Independent',      trigger: 'ind',   urlTemplate: 'https://www.independent.co.uk/search?q={query}',                                 category: 'News',        icon: 'independent.png',     description: 'Independent UK news' },
  { label: 'Sky News',             trigger: 'sky',   urlTemplate: 'https://news.sky.com/search?query={query}',                                      category: 'News',        icon: 'skynews.png',         description: 'Sky News UK' },
  { label: 'Financial Times',      trigger: 'ft',    urlTemplate: 'https://www.ft.com/search?q={query}',                                            category: 'News',        icon: null,                  description: 'Financial Times' },
  { label: 'New York Times',       trigger: 'nyt',   urlTemplate: 'https://www.nytimes.com/search?query={query}',                                   category: 'News',        icon: 'nytimes.png',         description: 'NYT search' },
  { label: 'Reuters',              trigger: 'reu',   urlTemplate: 'https://www.reuters.com/site-search/?query={query}',                              category: 'News',        icon: 'reuters.png',         description: 'Reuters newswire' },
  { label: 'AP News',              trigger: 'ap',    urlTemplate: 'https://apnews.com/search?q={query}',                                            category: 'News',        icon: 'apnews.png',          description: 'Associated Press' },

  // ── Shopping ──
  { label: 'Amazon UK',            trigger: 'amz',   urlTemplate: 'https://www.amazon.co.uk/s?k={query}',                                           category: 'Shopping',    icon: 'amazon.png',          description: 'UK marketplace' },
  { label: 'eBay UK',              trigger: 'eb',    urlTemplate: 'https://www.ebay.co.uk/sch/i.html?_nkw={query}',                                 category: 'Shopping',    icon: 'ebay.png',            description: 'Auctions & marketplace' },
  { label: 'Etsy',                 trigger: 'ets',   urlTemplate: 'https://www.etsy.com/search?q={query}',                                          category: 'Shopping',    icon: 'etsy.png',            description: 'Handmade & vintage' },
  { label: 'IKEA UK',              trigger: 'ikea',  urlTemplate: 'https://www.ikea.com/gb/en/search/?q={query}',                                   category: 'Shopping',    icon: 'ikea.png',            description: 'Furniture & home' },
  { label: 'John Lewis',           trigger: 'jl',    urlTemplate: 'https://www.johnlewis.com/search?search-term={query}',                            category: 'Shopping',    icon: 'johnlewis.png',       description: 'UK department store' },
  { label: 'Argos',                trigger: 'arg',   urlTemplate: 'https://www.argos.co.uk/search/{query}/',                                        category: 'Shopping',    icon: 'argos.png',           description: 'UK general retail' },
  { label: 'ASOS',                 trigger: 'asos',  urlTemplate: 'https://www.asos.com/search/?q={query}',                                         category: 'Shopping',    icon: 'asos.png',            description: 'Online fashion' },
  { label: 'Booking.com',          trigger: 'book',  urlTemplate: 'https://www.booking.com/searchresults.html?ss={query}',                          category: 'Shopping',    icon: 'booking.png',         description: 'Hotels & stays' },
  { label: 'Airbnb',               trigger: 'abnb',  urlTemplate: 'https://www.airbnb.com/s/{query}/homes',                                         category: 'Shopping',    icon: 'airbnb.png',          description: 'Holiday rentals' },

  // ── UK Business ──
  { label: 'Companies House',      trigger: 'ch',    urlTemplate: 'https://find-and-update.company-information.service.gov.uk/search?q={query}',    category: 'UK Business', icon: 'companieshouse.png',  description: 'UK company records' },
  { label: 'gov.uk',               trigger: 'gov',   urlTemplate: 'https://www.gov.uk/search/all?keywords={query}',                                 category: 'UK Business', icon: 'govuk.png',           description: 'UK government services' },
  { label: 'Planning Portal',      trigger: 'plan',  urlTemplate: 'https://www.planningportal.co.uk/planning/search?q={query}',                     category: 'UK Business', icon: 'planningportal.png',  description: 'UK planning rules' },
  { label: 'BSI Knowledge',        trigger: 'bsi',   urlTemplate: 'https://knowledge.bsigroup.com/search?q={query}',                                category: 'UK Business', icon: 'bsi.png',             description: 'UK standards & specs' },
  { label: 'ICE Knowledge Hub',    trigger: 'ice',   urlTemplate: 'https://www.ice.org.uk/search?q={query}',                                        category: 'UK Business', icon: 'ice.png',             description: 'Civil engineering' },
  { label: 'RIBA',                 trigger: 'riba',  urlTemplate: 'https://www.architecture.com/search?q={query}',                                  category: 'UK Business', icon: 'riba.png',            description: 'Architecture institute' },
  { label: 'HSE',                  trigger: 'hse',   urlTemplate: 'https://www.hse.gov.uk/search/search-results.htm?q={query}',                     category: 'UK Business', icon: 'hse.png',             description: 'Health & safety executive' },
  { label: 'ONS',                  trigger: 'ons',   urlTemplate: 'https://www.ons.gov.uk/search?q={query}',                                        category: 'UK Business', icon: 'ons.png',             description: 'UK national statistics' },
  { label: 'TfL',                  trigger: 'tfl',   urlTemplate: 'https://tfl.gov.uk/search?query={query}',                                        category: 'UK Business', icon: 'tfl.png',             description: 'Transport for London' },
  { label: 'Met Office',           trigger: 'met',   urlTemplate: 'https://www.metoffice.gov.uk/search/results?q={query}',                          category: 'UK Business', icon: 'metoffice.png',       description: 'UK weather & climate' },
];

const PRESET_CATEGORIES = [...new Set(PRESETS.map(p => p.category))];

// Map preset domains → bundled icon filename. Exported so the shared lookup util
// (src/utils/presetIcons.js) and other components can map any URL to a brand icon.
// May contain null values for presets with no icon (Phind/FT/IEEE).
export const PRESET_ICONS_BY_DOMAIN = (() => {
  const map = {};
  for (const p of PRESETS) {
    if (!p.icon) continue;
    try {
      const host = new URL(p.urlTemplate).hostname.replace(/^www\./, '');
      if (host && !map[host]) map[host] = p.icon;
    } catch {}
  }
  return map;
})();

// ── Quick Action types (subset of MacroPanel ACTION_TYPES) ─────────────────

const QA_ACTION_TYPES = [
  { id: 'app',    icon: '⬡', label: 'Open App',          desc: 'Launch an application or file',            color: '#50c878' },
  { id: 'url',    icon: '⊕', label: 'Open URL',          desc: 'Open a website in your browser',           color: '#ffc832' },
  { id: 'folder', icon: '⬢', label: 'Open Folder',       desc: 'Open a folder in File Explorer',           color: '#40c8a0' },
  { id: 'macro',  icon: '◈', label: 'Macro Sequence',    desc: 'Run a sequence of actions one after another', color: '#ff783c' },
];

// The three Open sub-types (app/url/folder) collapse into a single "Open"
// button — matching MacroPanel's sub-pill pattern. Underlying type ids are
// unchanged so existing saved Quick Actions still load correctly.
const QA_OPEN_TYPE_IDS = ['app', 'url', 'folder'];

// ── Google Translate target-language list ──────────────────────────────────

// Full set of Google Translate target languages (sorted alphabetically by name).
const TRANSLATE_LANGS = [
  { code: 'af', name: 'Afrikaans' },
  { code: 'sq', name: 'Albanian' },
  { code: 'am', name: 'Amharic' },
  { code: 'ar', name: 'Arabic' },
  { code: 'hy', name: 'Armenian' },
  { code: 'as', name: 'Assamese' },
  { code: 'ay', name: 'Aymara' },
  { code: 'az', name: 'Azerbaijani' },
  { code: 'bm', name: 'Bambara' },
  { code: 'eu', name: 'Basque' },
  { code: 'be', name: 'Belarusian' },
  { code: 'bn', name: 'Bengali' },
  { code: 'bho', name: 'Bhojpuri' },
  { code: 'bs', name: 'Bosnian' },
  { code: 'bg', name: 'Bulgarian' },
  { code: 'ca', name: 'Catalan' },
  { code: 'ceb', name: 'Cebuano' },
  { code: 'zh-CN', name: 'Chinese (Simplified)' },
  { code: 'zh-TW', name: 'Chinese (Traditional)' },
  { code: 'co', name: 'Corsican' },
  { code: 'hr', name: 'Croatian' },
  { code: 'cs', name: 'Czech' },
  { code: 'da', name: 'Danish' },
  { code: 'dv', name: 'Dhivehi' },
  { code: 'doi', name: 'Dogri' },
  { code: 'nl', name: 'Dutch' },
  { code: 'en', name: 'English' },
  { code: 'eo', name: 'Esperanto' },
  { code: 'et', name: 'Estonian' },
  { code: 'ee', name: 'Ewe' },
  { code: 'fil', name: 'Filipino (Tagalog)' },
  { code: 'fi', name: 'Finnish' },
  { code: 'fr', name: 'French' },
  { code: 'fy', name: 'Frisian' },
  { code: 'gl', name: 'Galician' },
  { code: 'ka', name: 'Georgian' },
  { code: 'de', name: 'German' },
  { code: 'el', name: 'Greek' },
  { code: 'gn', name: 'Guarani' },
  { code: 'gu', name: 'Gujarati' },
  { code: 'ht', name: 'Haitian Creole' },
  { code: 'ha', name: 'Hausa' },
  { code: 'haw', name: 'Hawaiian' },
  { code: 'he', name: 'Hebrew' },
  { code: 'hi', name: 'Hindi' },
  { code: 'hmn', name: 'Hmong' },
  { code: 'hu', name: 'Hungarian' },
  { code: 'is', name: 'Icelandic' },
  { code: 'ig', name: 'Igbo' },
  { code: 'ilo', name: 'Ilocano' },
  { code: 'id', name: 'Indonesian' },
  { code: 'ga', name: 'Irish' },
  { code: 'it', name: 'Italian' },
  { code: 'ja', name: 'Japanese' },
  { code: 'jv', name: 'Javanese' },
  { code: 'kn', name: 'Kannada' },
  { code: 'kk', name: 'Kazakh' },
  { code: 'km', name: 'Khmer' },
  { code: 'rw', name: 'Kinyarwanda' },
  { code: 'gom', name: 'Konkani' },
  { code: 'ko', name: 'Korean' },
  { code: 'kri', name: 'Krio' },
  { code: 'ku', name: 'Kurdish' },
  { code: 'ckb', name: 'Kurdish (Sorani)' },
  { code: 'ky', name: 'Kyrgyz' },
  { code: 'lo', name: 'Lao' },
  { code: 'la', name: 'Latin' },
  { code: 'lv', name: 'Latvian' },
  { code: 'ln', name: 'Lingala' },
  { code: 'lt', name: 'Lithuanian' },
  { code: 'lg', name: 'Luganda' },
  { code: 'lb', name: 'Luxembourgish' },
  { code: 'mk', name: 'Macedonian' },
  { code: 'mai', name: 'Maithili' },
  { code: 'mg', name: 'Malagasy' },
  { code: 'ms', name: 'Malay' },
  { code: 'ml', name: 'Malayalam' },
  { code: 'mt', name: 'Maltese' },
  { code: 'mi', name: 'Maori' },
  { code: 'mr', name: 'Marathi' },
  { code: 'mni-Mtei', name: 'Meiteilon (Manipuri)' },
  { code: 'lus', name: 'Mizo' },
  { code: 'mn', name: 'Mongolian' },
  { code: 'my', name: 'Myanmar (Burmese)' },
  { code: 'ne', name: 'Nepali' },
  { code: 'no', name: 'Norwegian' },
  { code: 'ny', name: 'Nyanja (Chichewa)' },
  { code: 'or', name: 'Odia (Oriya)' },
  { code: 'om', name: 'Oromo' },
  { code: 'ps', name: 'Pashto' },
  { code: 'fa', name: 'Persian' },
  { code: 'pl', name: 'Polish' },
  { code: 'pt', name: 'Portuguese' },
  { code: 'pa', name: 'Punjabi' },
  { code: 'qu', name: 'Quechua' },
  { code: 'ro', name: 'Romanian' },
  { code: 'ru', name: 'Russian' },
  { code: 'sm', name: 'Samoan' },
  { code: 'sa', name: 'Sanskrit' },
  { code: 'gd', name: 'Scots Gaelic' },
  { code: 'nso', name: 'Sepedi' },
  { code: 'sr', name: 'Serbian' },
  { code: 'st', name: 'Sesotho' },
  { code: 'sn', name: 'Shona' },
  { code: 'sd', name: 'Sindhi' },
  { code: 'si', name: 'Sinhala' },
  { code: 'sk', name: 'Slovak' },
  { code: 'sl', name: 'Slovenian' },
  { code: 'so', name: 'Somali' },
  { code: 'es', name: 'Spanish' },
  { code: 'su', name: 'Sundanese' },
  { code: 'sw', name: 'Swahili' },
  { code: 'sv', name: 'Swedish' },
  { code: 'tg', name: 'Tajik' },
  { code: 'ta', name: 'Tamil' },
  { code: 'tt', name: 'Tatar' },
  { code: 'te', name: 'Telugu' },
  { code: 'th', name: 'Thai' },
  { code: 'ti', name: 'Tigrinya' },
  { code: 'ts', name: 'Tsonga' },
  { code: 'tr', name: 'Turkish' },
  { code: 'tk', name: 'Turkmen' },
  { code: 'ak', name: 'Twi' },
  { code: 'uk', name: 'Ukrainian' },
  { code: 'ur', name: 'Urdu' },
  { code: 'ug', name: 'Uyghur' },
  { code: 'uz', name: 'Uzbek' },
  { code: 'vi', name: 'Vietnamese' },
  { code: 'cy', name: 'Welsh' },
  { code: 'xh', name: 'Xhosa' },
  { code: 'yi', name: 'Yiddish' },
  { code: 'yo', name: 'Yoruba' },
  { code: 'zu', name: 'Zulu' },
];

function isTranslateUrl(url) {
  return /translate\.google\.com/i.test(url || '');
}

function getTranslateTargetLang(url) {
  const m = (url || '').match(/[?&]tl=([a-zA-Z-]+)/);
  return m ? m[1] : 'en';
}

function setTranslateTargetLang(url, code) {
  if (/[?&]tl=/.test(url)) {
    return url.replace(/([?&])tl=[a-zA-Z-]+/, `$1tl=${code}`);
  }
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}tl=${code}`;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

function buildPreviewUrl(urlTemplate, sampleQuery) {
  if (!urlTemplate || !sampleQuery) return '';
  const encoded = encodeURIComponent(sampleQuery).replace(/%20/g, '+');
  return urlTemplate.replace('{query}', encoded);
}

function truncateUrl(url, maxLen = 60) {
  if (!url || url.length <= maxLen) return url;
  return url.slice(0, maxLen) + '…';
}

function extractHost(urlTemplate) {
  try {
    return new URL(urlTemplate).hostname.replace(/^www\./, '');
  } catch {
    return '';
  }
}

// ── Searchable language combobox ───────────────────────────────────────────

function TranslateLangPicker({ value, onChange }) {
  const [open, setOpen]           = useState(false);
  const [filter, setFilter]       = useState('');
  const [highlight, setHighlight] = useState(0);
  const wrapRef  = useRef(null);
  const inputRef = useRef(null);
  const listRef  = useRef(null);
  const popoverRef = useRef(null);

  const selected = TRANSLATE_LANGS.find(l => l.code === value) || TRANSLATE_LANGS.find(l => l.code === 'en');

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return TRANSLATE_LANGS;
    return TRANSLATE_LANGS.filter(l =>
      l.name.toLowerCase().includes(q) || l.code.toLowerCase().includes(q)
    );
  }, [filter]);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    function onDown(e) {
      if (wrapRef.current && !wrapRef.current.contains(e.target)) setOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  // Reset filter & focus input on open
  useEffect(() => {
    if (!open) return;
    setFilter('');
    const idx = TRANSLATE_LANGS.findIndex(l => l.code === value);
    setHighlight(idx === -1 ? 0 : idx);
    setTimeout(() => inputRef.current?.focus(), 0);
  }, [open, value]);

  // Reset highlight to top when filter changes
  useEffect(() => { setHighlight(0); }, [filter]);

  // Flip the language popover upward when its default below-trigger position
  // would clip the viewport.
  useLayoutEffect(() => {
    if (!open || !popoverRef.current) return;
    const el = popoverRef.current;
    el.style.top = '';
    el.style.bottom = '';
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = 'auto';
      el.style.bottom = 'calc(100% + 4px)';
    }
  }, [open, filtered]);

  // Scroll highlighted row into view
  useEffect(() => {
    if (!open) return;
    const el = listRef.current?.querySelector(`[data-idx="${highlight}"]`);
    if (el) el.scrollIntoView({ block: 'nearest' });
  }, [highlight, open]);

  function commit(code) {
    onChange(code);
    setOpen(false);
  }

  function handleKey(e) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlight(i => Math.min(filtered.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlight(i => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const pick = filtered[highlight];
      if (pick) commit(pick.code);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
    }
  }

  return (
    <div className="stp-lang-picker" ref={wrapRef}>
      <button
        type="button"
        className="stp-input stp-lang-trigger"
        onClick={() => setOpen(v => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span className="stp-lang-trigger-name">{selected?.name || '—'}</span>
        <span className="stp-lang-trigger-code">{selected?.code || ''}</span>
        <span className="stp-lang-trigger-caret">▾</span>
      </button>
      {open && (
        <div className="stp-lang-popover" ref={popoverRef}>
          <SearchBar
            ref={inputRef}
            className="stp-lang-search-bar compact"
            placeholder="Filter languages…"
            value={filter}
            onChange={e => setFilter(e.target.value)}
            onKeyDown={handleKey}
          />
          <div className="stp-lang-list" ref={listRef} role="listbox">
            {filtered.length === 0 ? (
              <div className="stp-lang-empty">No matches</div>
            ) : filtered.map((l, idx) => (
              <button
                key={l.code}
                data-idx={idx}
                type="button"
                role="option"
                aria-selected={l.code === value}
                className={`stp-lang-row${idx === highlight ? ' highlight' : ''}${l.code === value ? ' selected' : ''}`}
                onMouseEnter={() => setHighlight(idx)}
                onClick={() => commit(l.code)}
              >
                <span className="stp-lang-row-name">{l.name}</span>
                <span className="stp-lang-row-code">{l.code}</span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Main component ──────────────────────────────────────────────────────────

export default function SearchTemplatesPanel({
  searchTemplates = [],
  categories = [],
  isPro = false,
  onShowUpgrade,
  onAdd,
  onUpdate,
  onDelete,
  onAddCategory,
  onRenameCategory,
  onDeleteCategory,
  onUpdateCategoryColour,
  onReorderCategories,
  quickActions = [],
  onAddQuickAction,
  onUpdateQuickAction,
  onDeleteQuickAction,
  qaCategories = [],
  onAddQaCategory,
  onRenameQaCategory,
  onDeleteQaCategory,
  onUpdateQaCategoryColour,
  onReorderQaCategories,
  onExportQuickActions,
  onImportQuickActions,
  quickActionImportPrompt,
  onQuickActionImportResolve,
  globalInputMethod,
  onShowNotification,
  // Suppress foreground auto-switch while the user is mid-edit
  onEditingChange,
}) {
  // Panel mode: 'quickactions' | 'templates'
  const [panelMode, setPanelMode]           = useState('quickactions');

  const [selectedId, setSelectedId]         = useState(null);
  const [showPresets, setShowPresets]        = useState(false);
  const [presetFilter, setPresetFilter]     = useState('');

  // Template form state
  const [formLabel, setFormLabel]           = useState('');
  const [formTrigger, setFormTrigger]       = useState('');
  const [formUrl, setFormUrl]               = useState('');
  const [formEncode, setFormEncode]         = useState(true);
  const [formSource, setFormSource]         = useState('custom');
  const [formCategory, setFormCategory]     = useState(null);
  const [formIcon, setFormIcon]             = useState(null);
  const [formDescription, setFormDescription] = useState('');
  const [triggerError, setTriggerError]     = useState('');
  const [isNew, setIsNew]                   = useState(false);

  // Quick action form state
  const [qaSelectedId, setQaSelectedId]     = useState(null);
  const [qaIsNew, setQaIsNew]              = useState(false);
  const [qaLabel, setQaLabel]              = useState('');
  const [qaType, setQaType]                = useState('url');
  // Which Open sub-type (app/url/folder) the merged Open selector targets.
  // Tracks qaType so loading a saved Open Quick Action syncs the sub-pill bar.
  const [lastQaOpenType, setLastQaOpenType] = useState('app');
  useEffect(() => {
    if (QA_OPEN_TYPE_IDS.includes(qaType)) setLastQaOpenType(qaType);
  }, [qaType]);
  const [qaFormValue, setQaFormValue]      = useState({});
  const [qaCategory, setQaCategory]        = useState(null);
  const [qaVoicePhrases, setQaVoicePhrases] = useState([]);
  const [qaConfirmAction, setQaConfirmAction] = useState(null); // null | 'clear-action' | 'delete'
  // Right-click row context menu (Quick Actions): { id, x, y } | null
  const [qaItemContextMenu, setQaItemContextMenu] = useState(null);
  const qaItemContextMenuRef = useRef(null);

  // Test state
  const [testQuery, setTestQuery]           = useState('');
  const [showHelp, setShowHelp]             = useState(false);
  const helpRef = useRef(null);

  // Category sidebar state
  const [activeCategory, setActiveCategory]     = useState('All');
  const [addingCategory, setAddingCategory]     = useState(false);
  const [newCategoryName, setNewCategoryName]   = useState('');
  const [newCategoryColour, setNewCategoryColour] = useState(null);
  // Colour picker popover
  const [catColourPopover, setCatColourPopover] = useState(null); // { forCat, x, y }
  const catColourPopoverRef = useRef(null);
  // Context menu
  const [catContextMenu, setCatContextMenu]     = useState(null); // { catName, x, y }
  const [ctxDeleteConfirm, setCtxDeleteConfirm] = useState(false);
  const catContextMenuRef  = useRef(null);
  const catContextTabRef   = useRef(null);
  // Inline rename
  const [renamingCat, setRenamingCat]   = useState(null);
  const [renameValue, setRenameValue]   = useState('');
  const [renameError, setRenameError]   = useState('');
  const renameInputRef                  = useRef(null);
  const renameCommitting                = useRef(false);
  // Drag reorder
  const catDndSensors = useSensors(useSensor(LeftClickSensor, { activationConstraint: { distance: 8 } }));
  const [catDragId, setCatDragId] = useState(null);

  // ── Active categories depend on mode (must be before effects/functions) ──
  const activeCats = panelMode === 'quickactions' ? qaCategories : categories;
  const activeCatHandlers = panelMode === 'quickactions'
    ? { onAdd: onAddQaCategory, onRename: onRenameQaCategory, onDelete: onDeleteQaCategory, onColour: onUpdateQaCategoryColour, onReorder: onReorderQaCategories }
    : { onAdd: onAddCategory, onRename: onRenameCategory, onDelete: onDeleteCategory, onColour: onUpdateCategoryColour, onReorder: onReorderCategories };

  // Close help popover on outside click
  useEffect(() => {
    if (!showHelp) return;
    function onDown(e) {
      if (helpRef.current && !helpRef.current.contains(e.target)) setShowHelp(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [showHelp]);

  // Close colour picker on outside click
  useEffect(() => {
    if (!catColourPopover) return;
    function onDown(e) {
      if (!catColourPopoverRef.current?.contains(e.target)) setCatColourPopover(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [catColourPopover]);

  // Close context menu on outside click or Escape
  useEffect(() => {
    if (!catContextMenu) return;
    function onDown(e) {
      if (catContextMenuRef.current && !catContextMenuRef.current.contains(e.target)) setCatContextMenu(null);
    }
    function onKey(e) { if (e.key === 'Escape') setCatContextMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey); };
  }, [catContextMenu]);

  // Close QA row context menu on outside click or Escape
  useEffect(() => {
    if (!qaItemContextMenu) return;
    function onDown(e) {
      if (qaItemContextMenuRef.current && !qaItemContextMenuRef.current.contains(e.target)) setQaItemContextMenu(null);
    }
    function onKey(e) { if (e.key === 'Escape') setQaItemContextMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey); };
  }, [qaItemContextMenu]);

  // Flip the category colour popover up / clamp left if its default position
  // (anchored below the trigger tab) would clip the viewport.
  useLayoutEffect(() => {
    if (!catColourPopover || !catColourPopoverRef.current) return;
    const el = catColourPopoverRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
  }, [catColourPopover]);

  // Clamp both right-click context menus inside the viewport — raw clientX /
  // clientY overflow when right-clicking near the edge of the panel.
  useLayoutEffect(() => {
    if (!catContextMenu || !catContextMenuRef.current) return;
    const el = catContextMenuRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
  }, [catContextMenu]);

  useLayoutEffect(() => {
    if (!qaItemContextMenu || !qaItemContextMenuRef.current) return;
    const el = qaItemContextMenuRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
  }, [qaItemContextMenu]);

  // Auto-select rename input text
  useEffect(() => {
    if (renamingCat) renameInputRef.current?.select();
  }, [renamingCat]);

  // If active category deleted externally, fall back to All
  useEffect(() => {
    if (activeCategory !== 'All' && activeCategory !== '__uncategorised__' &&
        !activeCats.some(c => c.name === activeCategory)) {
      setActiveCategory('All');
    }
  }, [activeCats, activeCategory]);

  // ── Trigger validation ──────────────────────────────────────────────────

  function validateTrigger(value, excludeId) {
    if (!value) return 'Trigger is required';
    if (!/^[a-z0-9]+$/.test(value)) return 'Lowercase letters and numbers only';
    if (value.length > 10) return 'Max 10 characters';
    const exists = searchTemplates.some(
      t => t.trigger.toLowerCase() === value.toLowerCase() && t.id !== excludeId
    );
    if (exists) return 'Trigger already in use';
    return '';
  }

  // ── Template CRUD ───────────────────────────────────────────────────────

  function selectTemplate(template) {
    setSelectedId(template.id);
    setFormLabel(template.label);
    setFormTrigger(template.trigger);
    setFormUrl(template.url_template);
    setFormEncode(template.encode_query ?? true);
    setFormSource(template.source || 'custom');
    setFormCategory(template.category || null);
    setFormIcon(template.icon || null);
    setFormDescription(template.description || '');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(false);
  }

  function closePanel() {
    setSelectedId(null);
    setIsNew(false);
  }

  function openNewFromPreset(preset) {
    if (!isPro && searchTemplates.length >= 5) {
      onShowUpgrade?.('More than 5 search templates');
      setShowPresets(false);
      return;
    }
    let trigger = preset.trigger;
    const taken = new Set(searchTemplates.map(t => t.trigger.toLowerCase()));
    if (taken.has(trigger)) {
      for (let i = 2; i <= 99; i++) {
        const candidate = `${preset.trigger}${i}`;
        if (!taken.has(candidate) && candidate.length <= 10) { trigger = candidate; break; }
      }
    }
    setSelectedId(null);
    setFormLabel(preset.label);
    setFormTrigger(trigger);
    setFormUrl(preset.urlTemplate);
    setFormEncode(true);
    setFormSource('preset');
    setFormCategory(null);
    setFormIcon(preset.icon || null);
    setFormDescription(preset.description || '');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  function openNewCustom() {
    if (!isPro && searchTemplates.length >= 5) {
      onShowUpgrade?.('More than 5 search templates');
      setShowPresets(false);
      return;
    }
    setSelectedId(null);
    setFormLabel('');
    setFormTrigger('');
    setFormUrl('');
    setFormEncode(true);
    setFormSource('custom');
    setFormCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setFormIcon(null);
    setFormDescription('');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  function handleSave() {
    const excludeId = isNew ? null : selectedId;
    const err = validateTrigger(formTrigger, excludeId);
    if (err) { setTriggerError(err); return; }
    if (!formLabel.trim()) return;
    if (!formUrl.includes('{query}')) return;

    if (isNew) {
      const newId = crypto.randomUUID();
      onAdd?.({
        id: newId,
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
        category: formCategory || null,
        icon: formIcon || null,
        description: formDescription || null,
      });
      setSelectedId(newId);
      setIsNew(false);
      onShowNotification?.('Template added', 'success');
    } else {
      onUpdate?.(selectedId, {
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
        category: formCategory || null,
        icon: formIcon || null,
        description: formDescription || null,
      });
      onShowNotification?.('Template updated', 'success');
    }
  }

  function handleDelete(id) {
    onDelete?.(id);
    if (selectedId === id) closePanel();
    onShowNotification?.('Template deleted', 'success');
  }

  function handleTest() {
    if (!testQuery.trim() || !formUrl) return;
    const encoded = formEncode
      ? encodeURIComponent(testQuery.trim()).replace(/%20/g, '+')
      : testQuery.trim();
    const finalUrl = formUrl.replace('{query}', encoded);
    window.electronAPI?.openExternal(finalUrl);
  }

  function handleNewClick() {
    // Always allow browsing the preset catalog — Free users see what Pro unlocks.
    // The cap gate fires on selecting a preset or "Custom", not on opening the browser.
    setPresetFilter('');
    setShowPresets(true);
  }

  // ── Quick Action CRUD ──────────────────────────────────────────────────

  function selectQuickAction(qa) {
    setQaSelectedId(qa.id);
    setQaLabel(qa.label || '');
    setQaType(qa.type || 'app');
    setQaFormValue(qa.data || {});
    setQaCategory(qa.data?.category || null);
    setQaVoicePhrases(readVoicePhrases(qa.data));
    setQaIsNew(false);
    setQaConfirmAction(null);
  }

  function openNewQuickAction() {
    setQaSelectedId(null);
    setQaLabel('');
    setQaType('app');
    setQaFormValue({});
    setQaCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setQaVoicePhrases([]);
    setQaIsNew(true);
    setQaConfirmAction(null);
  }

  function closeQaPanel() {
    setQaSelectedId(null);
    setQaIsNew(false);
    setQaConfirmAction(null);
  }

  function handleQaSave() {
    if (!qaLabel.trim()) return;
    const data = { ...qaFormValue, category: qaCategory || null };
    writeVoicePhrases(data, qaVoicePhrases);
    if (qaIsNew) {
      const newId = crypto.randomUUID();
      onAddQuickAction?.({ id: newId, type: qaType, label: qaLabel.trim(), data });
      setQaSelectedId(newId);
      setQaIsNew(false);
      onShowNotification?.('Quick action added', 'success');
    } else {
      onUpdateQuickAction?.(qaSelectedId, { type: qaType, label: qaLabel.trim(), data });
      onShowNotification?.('Quick action updated', 'success');
    }
  }

  function handleQaDelete(id) {
    onDeleteQuickAction?.(id);
    if (qaSelectedId === id) closeQaPanel();
    onShowNotification?.('Quick action deleted', 'success');
  }

  // Reset the editor form to defaults without touching saved state.
  // Mirrors MacroPanel's Clear Action behaviour.
  function handleQaClearAction() {
    setQaLabel('');
    setQaType('app');
    setQaFormValue({});
    setQaCategory(activeCategory === 'All' || activeCategory === '__uncategorised__' ? null : activeCategory);
    setQaVoicePhrases([]);
  }

  // Duplicate a quick action by id. Creates a new entry with " (copy)" suffix
  // (numbered if needed) and opens it in the editor.
  function duplicateQuickAction(id) {
    const original = quickActions.find(a => a.id === id);
    if (!original) return;

    // Find a unique label
    const existingLabels = new Set(quickActions.map(a => a.label));
    let copyLabel = `${original.label} (copy)`;
    let counter = 2;
    while (existingLabels.has(copyLabel)) {
      copyLabel = `${original.label} (copy ${counter++})`;
    }

    const newId = crypto.randomUUID();
    const newData = { ...(original.data || {}) };
    onAddQuickAction?.({ id: newId, type: original.type, label: copyLabel, data: newData });

    // Switch the editor to the new copy immediately.
    setQaSelectedId(newId);
    setQaIsNew(false);
    setQaLabel(copyLabel);
    setQaType(original.type);
    setQaFormValue(newData);
    setQaCategory(newData.category || null);
    setQaVoicePhrases(readVoicePhrases(newData));
    setQaConfirmAction(null);
    onShowNotification?.('Quick action duplicated', 'success');
  }

  function handleQaItemContextMenu(e, id) {
    e.preventDefault();
    e.stopPropagation();
    setQaItemContextMenu({ id, x: e.clientX, y: e.clientY });
  }

  function qaCtxItemDuplicate() {
    if (!qaItemContextMenu) return;
    const id = qaItemContextMenu.id;
    setQaItemContextMenu(null);
    duplicateQuickAction(id);
  }

  function qaCtxItemDelete() {
    if (!qaItemContextMenu) return;
    const id = qaItemContextMenu.id;
    setQaItemContextMenu(null);
    handleQaDelete(id);
  }

  // ── Category CRUD (matches TextExpansions exactly) ────────────────────

  function handleAddCategory(e) {
    e?.preventDefault?.();
    const name = newCategoryName.trim();
    if (name && !activeCats.some(c => c.name === name)) {
      activeCatHandlers.onAdd?.(name, newCategoryColour);
    }
    setNewCategoryName('');
    setNewCategoryColour(null);
    setAddingCategory(false);
  }

  function openCatColourPopover(e, forCat) {
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    setCatColourPopover({ forCat, x: rect.left, y: rect.bottom + 4 });
  }

  function handleCatColourSelect(colour) {
    if (catColourPopover?.forCat === '__new__') {
      setNewCategoryColour(colour);
    } else if (catColourPopover?.forCat) {
      activeCatHandlers.onColour?.(catColourPopover.forCat, colour);
    }
    setCatColourPopover(null);
  }

  function handleCatContextMenu(e, catName) {
    e.preventDefault();
    catContextTabRef.current = e.currentTarget;
    setCtxDeleteConfirm(false);
    setCatContextMenu({ catName, x: e.clientX, y: e.clientY });
  }

  function ctxRename() {
    const name = catContextMenu.catName;
    setCatContextMenu(null);
    setRenamingCat(name);
    setRenameValue(name);
    setRenameError('');
  }

  function ctxChangeColour() {
    const { catName } = catContextMenu;
    const tabRect = catContextTabRef.current?.getBoundingClientRect();
    if (tabRect) {
      const PICKER_WIDTH = 212;
      const left = Math.min(tabRect.left, window.innerWidth - PICKER_WIDTH - 8);
      setCatColourPopover({ forCat: catName, x: left, y: tabRect.bottom + 4 });
    } else {
      setCatColourPopover({ forCat: catName, x: catContextMenu.x, y: catContextMenu.y + 4 });
    }
    setCatContextMenu(null);
  }

  function ctxDelete() {
    if (!ctxDeleteConfirm) {
      setCtxDeleteConfirm(true);
      return;
    }
    activeCatHandlers.onDelete?.(catContextMenu.catName);
    if (activeCategory === catContextMenu.catName) setActiveCategory('All');
    setCatContextMenu(null);
    setCtxDeleteConfirm(false);
  }

  function commitCatRename() {
    const trimmed = renameValue.trim();
    if (!trimmed) { setRenameError('Name cannot be empty'); return; }
    if (trimmed !== renamingCat && activeCats.some(c => c.name === trimmed)) {
      setRenameError('Already exists'); return;
    }
    renameCommitting.current = true;
    if (trimmed !== renamingCat) {
      activeCatHandlers.onRename?.(renamingCat, trimmed);
      if (activeCategory === renamingCat) setActiveCategory(trimmed);
    }
    setRenamingCat(null);
    setRenameValue('');
    setRenameError('');
  }

  function cancelCatRename() {
    if (renameCommitting.current) { renameCommitting.current = false; return; }
    setRenamingCat(null);
    setRenameValue('');
    setRenameError('');
  }

  function handleCatDragEnd(event) {
    setCatDragId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIdx = activeCats.findIndex(c => c.name === active.id);
    const newIdx = activeCats.findIndex(c => c.name === over.id);
    if (oldIdx === -1 || newIdx === -1) return;
    activeCatHandlers.onReorder?.(arrayMove([...activeCats], oldIdx, newIdx));
  }

  // ── Filtered + grouped template list ──────────────────────────────────

  const uncategorisedCount = searchTemplates.filter(t => !t.category).length;

  const filteredTemplates = useMemo(() => {
    if (activeCategory === 'All') return searchTemplates;
    if (activeCategory === '__uncategorised__') return searchTemplates.filter(t => !t.category);
    return searchTemplates.filter(t => t.category === activeCategory);
  }, [searchTemplates, activeCategory]);

  const groupedList = useMemo(() => {
    if (activeCategory !== 'All') {
      return filteredTemplates.map(t => ({ type: 'item', template: t }));
    }
    const result = [];
    const uncat = searchTemplates.filter(t => !t.category);
    if (uncat.length > 0) {
      result.push({ type: 'header', label: 'Uncategorised', count: uncat.length });
      uncat.forEach(t => result.push({ type: 'item', template: t }));
    }
    for (const cat of categories) {
      const items = searchTemplates.filter(t => t.category === cat.name);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat.name, count: items.length, colour: cat.colour });
        items.forEach(t => result.push({ type: 'item', template: t }));
      }
    }
    return result;
  }, [searchTemplates, categories, activeCategory, filteredTemplates]);

  // Grouped presets for preset picker
  const filteredPresets = presetFilter
    ? PRESETS.filter(p => p.label.toLowerCase().includes(presetFilter.toLowerCase()) || p.trigger.includes(presetFilter.toLowerCase()))
    : PRESETS;

  const groupedPresets = useMemo(() => {
    const result = [];
    const catOrder = presetFilter ? [...new Set(filteredPresets.map(p => p.category))] : PRESET_CATEGORIES;
    for (const cat of catOrder) {
      const items = filteredPresets.filter(p => p.category === cat);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat });
        items.forEach(p => result.push({ type: 'preset', preset: p }));
      }
    }
    return result;
  }, [filteredPresets, presetFilter]);

  // Quick action filtered/grouped list
  const qaUncategorisedCount = quickActions.filter(a => !a.data?.category).length;

  const qaFilteredList = useMemo(() => {
    if (activeCategory === 'All') return quickActions;
    if (activeCategory === '__uncategorised__') return quickActions.filter(a => !a.data?.category);
    return quickActions.filter(a => a.data?.category === activeCategory);
  }, [quickActions, activeCategory]);

  const qaGroupedList = useMemo(() => {
    if (activeCategory !== 'All') {
      return qaFilteredList.map(a => ({ type: 'item', action: a }));
    }
    const result = [];
    const uncat = quickActions.filter(a => !a.data?.category);
    if (uncat.length > 0) {
      result.push({ type: 'header', label: 'Uncategorised', count: uncat.length });
      uncat.forEach(a => result.push({ type: 'item', action: a }));
    }
    for (const cat of qaCategories) {
      const items = quickActions.filter(a => a.data?.category === cat.name);
      if (items.length > 0) {
        result.push({ type: 'header', label: cat.name, count: items.length, colour: cat.colour });
        items.forEach(a => result.push({ type: 'item', action: a }));
      }
    }
    return result;
  }, [quickActions, qaCategories, activeCategory, qaFilteredList]);

  const atCap = !isPro && searchTemplates.length >= 5;
  // Free users: only the first 5 templates in array order fire in Quick Search.
  // Anything past index 5 is visibly locked but kept on disk so it returns on upgrade.
  const lockedIds = useMemo(() => {
    if (isPro || searchTemplates.length <= 5) return new Set();
    return new Set(searchTemplates.slice(5).map(t => t.id));
  }, [isPro, searchTemplates]);
  const editOpen = selectedId !== null || isNew;
  const canSave = formLabel.trim() && formTrigger && !triggerError && formUrl.includes('{query}');
  const qaEditOpen = qaSelectedId !== null || qaIsNew;

  // Push editing state to parent — covers both Template and Quick Action forms.
  // Suppresses foreground auto-switch so the user can test mid-build.
  useEffect(() => {
    onEditingChange?.(editOpen || qaEditOpen);
  }, [editOpen, qaEditOpen, onEditingChange]);
  const qaCanSave = !!qaLabel.trim() && (
    (qaType === 'url' && qaFormValue.url?.trim()) ||
    (qaType === 'app' && (qaFormValue.appId || qaFormValue.path?.trim())) ||
    (qaType === 'folder' && qaFormValue.path?.trim()) ||
    (qaType === 'macro' && qaFormValue.steps?.length > 0)
  );

  // ── Render ──────────────────────────────────────────────────────────────

  return (
    <div className="stp-panel">
      {/* Header */}
      <div className="stp-header">
        <div className="stp-mode-tabs">
          <button
            className={`stp-mode-tab${panelMode === 'quickactions' ? ' active' : ''}`}
            onClick={() => { setPanelMode('quickactions'); closePanel(); setActiveCategory('All'); }}
            type="button"
          >⚡ Quick Actions</button>
          <button
            className={`stp-mode-tab${panelMode === 'templates' ? ' active' : ''}`}
            onClick={() => { setPanelMode('templates'); closeQaPanel(); setActiveCategory('All'); }}
            type="button"
          >⌕ Search Templates</button>
        </div>
        <div className="stp-header-right">
          {panelMode === 'templates' ? (
            <>
              {atCap && (
                <span className="stp-cap-nudge" title="Upgrade to Pro for unlimited templates">
                  {searchTemplates.length > 5
                    ? `5 active, ${searchTemplates.length - 5} locked`
                    : '5/5. Pro for unlimited'}
                </span>
              )}
              <button className="stp-add-btn" onClick={handleNewClick} type="button">+ New Template</button>
            </>
          ) : (
            <button className="stp-add-btn" onClick={openNewQuickAction} type="button">+ New Action</button>
          )}
        </div>
      </div>

      {/* Preset picker overlay */}
      {showPresets && (
        <div className="stp-presets-overlay">
          <div className="stp-presets">
            <div className="stp-presets-header">
              <span className="stp-presets-title">Choose a preset or create custom</span>
              <button className="stp-back-btn" onClick={() => setShowPresets(false)} type="button">Cancel</button>
            </div>
            <SearchBar
              className="stp-preset-filter-bar"
              placeholder="Filter presets…"
              value={presetFilter}
              onChange={e => setPresetFilter(e.target.value)}
              autoFocus
            />
            <div className="stp-preset-list">
              {groupedPresets.map((entry) => {
                if (entry.type === 'header') {
                  return <div key={`ph-${entry.label}`} className="stp-preset-group-header">{entry.label}</div>;
                }
                const p = entry.preset;
                return (
                  <button key={p.trigger} className="stp-tile" onClick={() => openNewFromPreset(p)} type="button" title={p.urlTemplate}>
                    <span className="stp-tile-trigger">{p.trigger}</span>
                    {p.icon ? (
                      <img
                        className="stp-tile-icon"
                        src={`/preset-icons/${p.icon}`}
                        alt=""
                        draggable={false}
                        onError={e => { e.currentTarget.style.visibility = 'hidden'; }}
                      />
                    ) : (
                      <span className="stp-tile-icon stp-tile-letter">{(p.label || '?').charAt(0).toUpperCase()}</span>
                    )}
                    <div className="stp-tile-body">
                      <div className="stp-tile-label">{p.label}</div>
                      <div className="stp-tile-desc">{p.description}</div>
                    </div>
                  </button>
                );
              })}
            </div>
            <button className="stp-custom-btn" onClick={openNewCustom} type="button">
              + Create custom template
            </button>
          </div>
        </div>
      )}

      {/* Body: sidebar + list + edit panel */}
      <div className="stp-body">
        {/* Category sidebar — switches between template and quick action categories based on mode */}
        <div className="stp-cat-sidebar">
          <div className="stp-cat-sidebar-list">
            <button
              className={`stp-cat-row${activeCategory === 'All' ? ' stp-cat-row-active' : ''}`}
              onClick={() => setActiveCategory('All')}
              type="button"
            >
              <span className="stp-cat-row-name">All</span>
              <span className="stp-cat-count">{panelMode === 'quickactions' ? quickActions.length : searchTemplates.length}</span>
            </button>

            {((panelMode === 'quickactions' ? qaUncategorisedCount : uncategorisedCount) > 0) && (
              <button
                className={`stp-cat-row stp-cat-row-uncategorised${activeCategory === '__uncategorised__' ? ' stp-cat-row-active' : ''}`}
                onClick={() => setActiveCategory('__uncategorised__')}
                type="button"
              >
                <span className="stp-cat-row-name">Uncategorised</span>
                <span className="stp-cat-count">{panelMode === 'quickactions' ? qaUncategorisedCount : uncategorisedCount}</span>
              </button>
            )}

            <DndContext sensors={catDndSensors} onDragStart={e => setCatDragId(e.active.id)} onDragEnd={handleCatDragEnd}>
              <SortableContext items={activeCats.map(c => c.name)} strategy={verticalListSortingStrategy}>
                {activeCats.map(cat => {
                  const catColour = cat.colour || null;
                  const count = panelMode === 'quickactions'
                    ? quickActions.filter(a => a.data?.category === cat.name).length
                    : searchTemplates.filter(t => t.category === cat.name).length;
                  return (
                    <SortableCatRow key={cat.name} id={cat.name}>
                      <div className="stp-cat-row-group" onContextMenu={e => handleCatContextMenu(e, cat.name)}>
                        {renamingCat === cat.name ? (
                          <div
                            className="stp-cat-row stp-cat-row-active stp-cat-rename-wrap"
                            style={catColour ? { '--cat-color': catColour } : {}}
                          >
                            <input
                              ref={renameInputRef}
                              className="stp-cat-rename-input"
                              value={renameValue}
                              onChange={e => { setRenameValue(e.target.value); setRenameError(''); }}
                              onKeyDown={e => {
                                if (e.key === 'Enter')  { e.preventDefault(); commitCatRename(); }
                                if (e.key === 'Escape') { e.preventDefault(); cancelCatRename(); }
                                e.stopPropagation();
                              }}
                              onBlur={cancelCatRename}
                            />
                            {renameError && <span className="stp-cat-rename-error">{renameError}</span>}
                          </div>
                        ) : (
                          <button
                            className={`stp-cat-row${activeCategory === cat.name ? ' stp-cat-row-active' : ''}`}
                            style={catColour ? { '--cat-color': catColour } : {}}
                            onClick={() => setActiveCategory(cat.name)}
                            type="button"
                          >
                            <span
                              className="stp-cat-dot stp-cat-dot-pick"
                              style={catColour ? { background: catColour } : {}}
                              onClick={e => openCatColourPopover(e, cat.name)}
                              title="Change colour"
                            />
                            <span className="stp-cat-row-name">{cat.name}</span>
                            <span className="stp-cat-count">{count}</span>
                          </button>
                        )}
                      </div>
                    </SortableCatRow>
                  );
                })}
              </SortableContext>
              <DragOverlay>
                {catDragId ? (
                  <div className="stp-cat-row-group stp-cat-row-ghost">
                    <button className="stp-cat-row stp-cat-row-active" type="button">{catDragId}</button>
                  </div>
                ) : null}
              </DragOverlay>
            </DndContext>

            {addingCategory ? (
              <form onSubmit={handleAddCategory} className="stp-cat-add-form">
                <span
                  className="stp-cat-add-colour-dot"
                  style={newCategoryColour ? { background: newCategoryColour } : {}}
                  onMouseDown={e => e.preventDefault()}
                  onClick={e => openCatColourPopover(e, '__new__')}
                  title="Pick a colour (optional)"
                />
                <input
                  autoFocus
                  className="stp-cat-add-input"
                  value={newCategoryName}
                  onChange={e => setNewCategoryName(e.target.value)}
                  placeholder="Category name…"
                  onBlur={handleAddCategory}
                  onKeyDown={e => e.key === 'Escape' && setAddingCategory(false)}
                />
              </form>
            ) : (
              <button className="stp-cat-new-btn" onClick={() => { setAddingCategory(true); setNewCategoryColour(null); }} type="button">
                + Add Category
              </button>
            )}

            {/* Quick Action pack import/export — only in quick actions mode */}
            {panelMode === 'quickactions' && (
              <>
                <button
                  className="stp-cat-new-btn stp-cat-pack-btn"
                  onClick={() => onImportQuickActions?.()}
                  title="Import a quick action pack file (.json)"
                  type="button"
                >
                  ↓ Import Pack
                </button>
                <button
                  className="stp-cat-new-btn stp-cat-pack-btn"
                  onClick={() => onExportQuickActions?.('all')}
                  title="Export all quick actions to a pack file"
                  type="button"
                >
                  ↑ Export All
                </button>
              </>
            )}
          </div>
        </div>

        {/* ═══ TEMPLATES MODE ═══ */}
        {panelMode === 'templates' && (<>
        <div className="stp-list stp-list-tiles">
          {searchTemplates.length === 0 && !isNew ? (
            <div className="stp-empty-state">
              <div className="stp-empty-icon">⌕</div>
              <div className="stp-empty-heading">No search templates yet</div>
              <div className="stp-empty-sub">Add one to search Google, GitHub, or your own URLs from Quick Search.</div>
              <button className="stp-add-btn stp-empty-cta" onClick={handleNewClick} type="button">+ New Template</button>
            </div>
          ) : (
            groupedList.map((entry) => {
              if (entry.type === 'header') {
                return (
                  <div key={`gh-${entry.label}`} className="stp-group-header">
                    {entry.colour && <span className="stp-cat-dot" style={{ background: entry.colour }} />}
                    <span className="stp-group-name">{entry.label.toUpperCase()}</span>
                    <span className="stp-group-count">{entry.count}</span>
                    <span className="stp-group-rule" />
                  </div>
                );
              }
              const t = entry.template;
              const locked = lockedIds.has(t.id);
              const handleLockedClick = () => onShowUpgrade?.('More than 5 search templates');
              return (
                <div
                  key={t.id}
                  className={`stp-tile${selectedId === t.id ? ' active' : ''}${locked ? ' locked' : ''}`}
                  onClick={() => locked ? handleLockedClick() : selectTemplate(t)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); locked ? handleLockedClick() : selectTemplate(t); } }}
                  title={locked ? 'Upgrade to Pro to enable this template' : t.url_template}
                >
                  <span className="stp-tile-trigger">{t.trigger}</span>
                  {locked && <span className="stp-tile-lock-badge">Pro</span>}
                  {t.icon ? (
                    <img
                      className="stp-tile-icon"
                      src={`/preset-icons/${t.icon}`}
                      alt=""
                      draggable={false}
                      onError={e => { e.currentTarget.style.visibility = 'hidden'; }}
                    />
                  ) : (
                    <span className="stp-tile-icon stp-tile-letter">{(t.label || '?').charAt(0).toUpperCase()}</span>
                  )}
                  <div className="stp-tile-body">
                    <div className="stp-tile-label">{t.label}</div>
                    <div className="stp-tile-desc">{t.description || extractHost(t.url_template)}</div>
                  </div>
                  <button
                    className="stp-tile-del"
                    onClick={e => { e.stopPropagation(); handleDelete(t.id); }}
                    title="Delete"
                    type="button"
                  >✕</button>
                </div>
              );
            })
          )}
        </div>
        {editOpen ? (
          <div className="stp-edit-panel">
            <div className="stp-ep-header">
              <span className="stp-ep-title">{isNew ? 'New Template' : 'Edit Template'}</span>
              <button className="stp-ep-close" onClick={closePanel} type="button">✕</button>
            </div>
            <div className="stp-ep-fields">
              <div className="stp-field">
                <label className="stp-label">Label</label>
                <input className="stp-input" type="text" value={formLabel} onChange={e => setFormLabel(e.target.value)} placeholder="e.g. Google" spellCheck={false} />
              </div>
              <div className="stp-field">
                <label className="stp-label">Trigger</label>
                <input className={`stp-input stp-trigger-input${triggerError ? ' error' : ''}`} type="text" value={formTrigger}
                  onChange={e => { const v = e.target.value.toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 10); setFormTrigger(v); setTriggerError(validateTrigger(v, isNew ? null : selectedId)); }}
                  placeholder="e.g. g" spellCheck={false} maxLength={10} />
                {triggerError && <div className="stp-trigger-error">{triggerError}</div>}
                <div className="stp-field-hint">Type this in Quick Search + Space to activate</div>
              </div>
              <div className="stp-field">
                <label className="stp-label">Category</label>
                <select className="stp-input stp-cat-select" value={formCategory || ''} onChange={e => setFormCategory(e.target.value || null)}>
                  <option value="">Uncategorised</option>
                  {categories.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
                </select>
              </div>
              {isTranslateUrl(formUrl) && (
                <div className="stp-field">
                  <label className="stp-label">Translate To</label>
                  <TranslateLangPicker
                    value={getTranslateTargetLang(formUrl)}
                    onChange={code => setFormUrl(setTranslateTargetLang(formUrl, code))}
                  />
                  <div className="stp-field-hint">Source language is auto-detected from your input</div>
                </div>
              )}
              <div className="stp-field">
                <div className="stp-label-row">
                  <label className="stp-label">URL Template</label>
                  <button className="stp-help-btn" onClick={() => setShowHelp(v => !v)} type="button" title="How to find the right URL">?</button>
                </div>
                {showHelp && (
                  <div className="stp-help-popover" ref={helpRef}>
                    <p><strong>How to find the right URL pattern:</strong></p>
                    <p>1. Go to the website and search for a word (e.g. "test")</p>
                    <p>2. Copy the URL from your browser's address bar</p>
                    <p>3. Paste it here and replace "test" with <code>{'{query}'}</code></p>
                    <p className="stp-help-example">Example: https://google.com/search?q=test becomes https://google.com/search?q={'{query}'}</p>
                  </div>
                )}
                <input className="stp-input stp-url-input" type="text" value={formUrl} onChange={e => setFormUrl(e.target.value)} placeholder="https://example.com/search?q={query}" spellCheck={false} />
                {formUrl && !formUrl.includes('{query}') && <div className="stp-trigger-error">URL must contain {'{query}'} placeholder</div>}
                {formUrl && formUrl.includes('{query}') && <div className="stp-preview-line">Example: typing "tauri" would open {truncateUrl(buildPreviewUrl(formUrl, 'tauri'), 80)}</div>}
              </div>
              <div className="stp-field">
                <label className="stp-toggle-label">
                  <input type="checkbox" checked={formEncode} onChange={e => setFormEncode(e.target.checked)} />
                  URL-encode query
                </label>
              </div>
            </div>
            <div className="stp-ep-test">
              <input className="stp-input stp-test-input" type="text" value={testQuery} onChange={e => setTestQuery(e.target.value)} placeholder="Test query…" spellCheck={false} onKeyDown={e => { if (e.key === 'Enter') handleTest(); }} />
              <button className="stp-test-btn" onClick={handleTest} disabled={!testQuery.trim() || !formUrl.includes('{query}')} type="button">Test</button>
            </div>
            <div className="stp-ep-footer">
              <button className="stp-save-btn" onClick={handleSave} disabled={!canSave} type="button">{isNew ? 'Add Template' : 'Save Changes'}</button>
              {!isNew && <button className="stp-delete-btn" onClick={() => handleDelete(selectedId)} type="button">Delete</button>}
            </div>
          </div>
        ) : (
          <div className="stp-edit-panel stp-panel-idle">
            <span className="stp-idle-text">Select a template to edit, or add a new one</span>
          </div>
        )}
        </>)}

        {/* ═══ QUICK ACTIONS MODE ═══ */}
        {panelMode === 'quickactions' && (<>
        <div className="stp-list stp-list-tiles">
          {quickActions.length === 0 && !qaIsNew ? (
            <div className="stp-empty-state">
              <div className="stp-empty-icon">⚡</div>
              <div className="stp-empty-heading">No quick actions yet</div>
              <div className="stp-empty-sub">Add actions accessible via Quick Search without assigning a hotkey. Open folders, URLs, apps, or type text.</div>
              <button className="stp-add-btn stp-empty-cta" onClick={openNewQuickAction} type="button">+ New Action</button>
            </div>
          ) : (
            qaGroupedList.map((entry) => {
              if (entry.type === 'header') {
                return (
                  <div key={`qh-${entry.label}`} className="stp-group-header">
                    {entry.colour && <span className="stp-cat-dot" style={{ background: entry.colour }} />}
                    <span className="stp-group-name">{entry.label.toUpperCase()}</span>
                    <span className="stp-group-count">{entry.count}</span>
                    <span className="stp-group-rule" />
                  </div>
                );
              }
              const a = entry.action;
              const typeIcons = { url: '⊕', app: '⬡', folder: '⬢', text: '✦', hotkey: '⌨', macro: '◈' };
              const typeColors = { url: '#ffc832', app: '#50c878', folder: '#40c8a0', text: '#64b4ff', hotkey: '#c864ff', macro: '#ff783c' };
              const preview = a.type === 'macro'
                ? `Sequence (${a.data?.steps?.length || 0} step${(a.data?.steps?.length || 0) !== 1 ? 's' : ''})`
                : (a.data?.url || a.data?.path || a.data?.folderPath || a.data?.urlName || a.data?.appName || '');
              const matchedIcon = a.type === 'url' ? findPresetIconForUrl(a.data?.url) : null;
              return (
                <div
                  key={a.id}
                  className={`stp-tile${qaSelectedId === a.id ? ' active' : ''}`}
                  onClick={() => selectQuickAction(a)}
                  onContextMenu={e => handleQaItemContextMenu(e, a.id)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); selectQuickAction(a); } }}
                  title={preview}
                >
                  {matchedIcon ? (
                    <img
                      className="stp-tile-icon"
                      src={`/preset-icons/${matchedIcon}`}
                      alt=""
                      draggable={false}
                      onError={e => { e.currentTarget.style.visibility = 'hidden'; }}
                    />
                  ) : (
                    <span
                      className="stp-tile-icon stp-tile-glyph"
                      style={{ '--glyph-color': typeColors[a.type] || '#8a8799' }}
                    >{typeIcons[a.type] || '◈'}</span>
                  )}
                  <div className="stp-tile-body">
                    <div className="stp-tile-label">{a.label}</div>
                    <div className="stp-tile-desc">{truncateUrl(preview, 60)}</div>
                  </div>
                  <button
                    className="stp-tile-export"
                    onClick={e => { e.stopPropagation(); onExportQuickActions?.('single', a.id); }}
                    title="Export quick action"
                    type="button"
                  >↑</button>
                  <button
                    className="stp-tile-del"
                    onClick={e => { e.stopPropagation(); handleQaDelete(a.id); }}
                    title="Delete"
                    type="button"
                  >✕</button>
                </div>
              );
            })
          )}
        </div>
        {qaEditOpen ? (
          <div className="stp-edit-panel stp-qa-edit">
            <div className="stp-ep-header">
              <span className="stp-ep-title">{qaIsNew ? 'New Quick Action' : 'Edit Quick Action'}</span>
              <button className="stp-ep-close" onClick={closeQaPanel} type="button">✕</button>
            </div>
            <div className="stp-qa-body">
              {/* Action type selector — App/URL/Folder share one Open button
                  with the sub-pill pattern mirroring MacroPanel. */}
              <div className="type-selector">
                {QA_ACTION_TYPES.map(t => {
                  if (QA_OPEN_TYPE_IDS.includes(t.id)) {
                    if (t.id !== QA_OPEN_TYPE_IDS[0]) return null;
                    const openType = QA_ACTION_TYPES.find(x => x.id === lastQaOpenType) || t;
                    const isOpenActive = QA_OPEN_TYPE_IDS.includes(qaType);
                    return (
                      <button
                        key="open"
                        className={`type-btn type-btn-half${isOpenActive ? ' active' : ''}`}
                        onClick={() => { setQaType(lastQaOpenType); setQaFormValue({}); }}
                        type="button"
                      >
                        <span className="type-btn-icon">{openType.icon}</span>
                        <span className="type-btn-label">Open</span>
                      </button>
                    );
                  }
                  return (
                    <button
                      key={t.id}
                      className={`type-btn type-btn-half${qaType === t.id ? ' active' : ''}`}
                      onClick={() => { setQaType(t.id); setQaFormValue({}); }}
                      type="button"
                    >
                      <span className="type-btn-icon">{t.icon}</span>
                      <span className="type-btn-label">{t.label}</span>
                    </button>
                  );
                })}
              </div>

              {/* Open sub-pill bar — shown while the merged Open button is active */}
              {QA_OPEN_TYPE_IDS.includes(qaType) && (
                <div className="type-subtype-bar">
                  {QA_OPEN_TYPE_IDS.map(id => {
                    const t = QA_ACTION_TYPES.find(x => x.id === id);
                    return (
                      <button
                        key={id}
                        className={`type-subtype-btn${qaType === id ? ' active' : ''}`}
                        onClick={() => { setQaType(id); setQaFormValue({}); }}
                        type="button"
                      >
                        <span style={{ fontSize: 11 }}>{t.icon}</span>
                        {t.label.replace(/^Open /, '')}
                      </button>
                    );
                  })}
                </div>
              )}

              {/* Type description */}
              <div className="type-desc">
                {QA_ACTION_TYPES.find(t => t.id === qaType)?.desc}
              </div>

              <div className="type-selector-separator" aria-hidden="true" />

              {/* Dynamic form per type */}
              <div className="form-body">
                {qaType === 'url' && (
                  <div className="form-section">
                    <label className="form-label">URL to open</label>
                    <input
                      className="form-input"
                      placeholder="https://example.com"
                      value={qaFormValue.url || ''}
                      onChange={e => setQaFormValue(prev => ({ ...prev, url: e.target.value }))}
                    />
                  </div>
                )}
                {qaType === 'app' && (
                  <AppForm value={qaFormValue} onChange={setQaFormValue} />
                )}
                {qaType === 'folder' && (
                  <div className="form-section">
                    <label className="form-label">Folder path</label>
                    <div className="file-input-row">
                      <input
                        className="form-input"
                        placeholder="C:\Users\Me\Documents"
                        value={qaFormValue.path || ''}
                        readOnly
                      />
                      <button className="browse-btn" type="button" onClick={async () => {
                        const path = await window.electronAPI?.browseForFolder();
                        if (path) setQaFormValue(prev => ({ ...prev, path }));
                      }}>Browse</button>
                    </div>
                  </div>
                )}
                {qaType === 'macro' && (
                  <MacroSequenceForm value={qaFormValue} onChange={setQaFormValue} globalInputMethod={globalInputMethod} />
                )}

                {/* Display label */}
                <div className="form-section" style={{ marginTop: 4 }}>
                  <label className="form-label">Display label</label>
                  <input
                    className="form-input"
                    placeholder="Short label for Quick Search..."
                    value={qaLabel}
                    onChange={e => setQaLabel(e.target.value)}
                  />
                </div>

                {/* Category */}
                <div className="form-section" style={{ marginTop: 4 }}>
                  <label className="form-label">Category</label>
                  <select className="form-select" style={{ width: 'auto', minWidth: 140 }} value={qaCategory || ''} onChange={e => setQaCategory(e.target.value || null)}>
                    <option value="">Uncategorised</option>
                    {qaCategories.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
                  </select>
                </div>

                {/* Voice commands */}
                <div className="form-section" style={{ marginTop: 4 }}>
                  <label className="form-label">Voice commands <span className="experimental-badge">EXPERIMENTAL</span></label>
                  <div className="voice-phrase-list">
                    {qaVoicePhrases.map((p, i) => (
                      <div className="voice-phrase-row" key={i}>
                        <input
                          className="form-input voice-phrase-input"
                          placeholder="e.g. open Revit"
                          value={p}
                          onChange={e => {
                            const next = [...qaVoicePhrases];
                            next[i] = e.target.value;
                            setQaVoicePhrases(next);
                          }}
                          onKeyDown={e => e.stopPropagation()}
                        />
                        <button
                          type="button"
                          className="voice-phrase-remove"
                          title="Remove phrase"
                          onClick={() => setQaVoicePhrases(qaVoicePhrases.filter((_, idx) => idx !== i))}
                        >×</button>
                      </div>
                    ))}
                    <button
                      type="button"
                      className="voice-phrase-add"
                      onClick={() => setQaVoicePhrases([...qaVoicePhrases, ''])}
                    >+ Add voice phrase</button>
                  </div>
                  <span className="form-hint">All aliases fire this quick action when spoken</span>
                </div>
              </div>
            </div>
            <div className="macro-panel-footer">
              {qaConfirmAction ? (
                <div className="footer-assignment-actions footer-confirm-row">
                  <span className="footer-confirm-text">
                    {qaConfirmAction === 'clear-action'
                      ? 'Clear the current action? Editor resets to blank.'
                      : 'Delete this quick action?'}
                  </span>
                  <button
                    className="btn-confirm-yes"
                    type="button"
                    onClick={() => {
                      if (qaConfirmAction === 'clear-action') {
                        handleQaClearAction();
                      } else if (qaConfirmAction === 'delete') {
                        handleQaDelete(qaSelectedId);
                      }
                      setQaConfirmAction(null);
                    }}
                  >Yes</button>
                  <button className="btn-confirm-no" type="button" onClick={() => setQaConfirmAction(null)}>Cancel</button>
                </div>
              ) : (
                <div className="footer-assignment-actions">
                  <button
                    className="btn-clear-action"
                    onClick={() => setQaConfirmAction('clear-action')}
                    type="button"
                    title="Clears the editor and resets it to blank. Saved data is not affected."
                  >Clear Action</button>
                  {!qaIsNew && (
                    <>
                      <button
                        className="btn-duplicate"
                        onClick={() => duplicateQuickAction(qaSelectedId)}
                        type="button"
                        title="Create a copy of this quick action"
                      >Duplicate</button>
                      <button
                        className="btn-delete"
                        onClick={() => setQaConfirmAction('delete')}
                        type="button"
                        title="Delete this quick action"
                      >Delete</button>
                    </>
                  )}
                </div>
              )}
              <button className="btn-save" onClick={handleQaSave} disabled={!qaCanSave} type="button">
                {qaIsNew ? 'Add Action' : 'Save Changes'}
              </button>
            </div>
          </div>
        ) : (
          <div className="stp-edit-panel stp-panel-idle">
            <span className="stp-idle-text">Select an action to edit, or add a new one</span>
          </div>
        )}
        </>)}
      </div>

      {/* Category right-click context menu (portal) */}
      {catContextMenu && ReactDOM.createPortal(
        <div
          ref={catContextMenuRef}
          className="profile-ctx-menu"
          style={{ top: catContextMenu.y, left: catContextMenu.x }}
        >
          <button className="profile-ctx-item" onClick={ctxRename}>Rename</button>
          <button className="profile-ctx-item" onClick={ctxChangeColour}>Change Colour</button>
          {panelMode === 'quickactions' && (
            <button
              className="profile-ctx-item"
              onClick={() => {
                const name = catContextMenu.catName;
                setCatContextMenu(null);
                onExportQuickActions?.('category', name);
              }}
            >
              Export Category
            </button>
          )}
          <div className="profile-ctx-divider" />
          <button className="profile-ctx-item profile-ctx-delete" onClick={ctxDelete}>
            {ctxDeleteConfirm ? 'Confirm Delete?' : 'Delete'}
          </button>
        </div>,
        document.body
      )}

      {/* Quick Action row right-click context menu (portal) */}
      {qaItemContextMenu && ReactDOM.createPortal(
        <div
          ref={qaItemContextMenuRef}
          className="profile-ctx-menu"
          style={{ top: qaItemContextMenu.y, left: qaItemContextMenu.x }}
        >
          <button className="profile-ctx-item" onClick={qaCtxItemDuplicate}>Duplicate</button>
          <button
            className="profile-ctx-item"
            onClick={() => {
              const id = qaItemContextMenu.id;
              setQaItemContextMenu(null);
              onExportQuickActions?.('single', id);
            }}
          >
            Export
          </button>
          <div className="profile-ctx-divider" />
          <button className="profile-ctx-item profile-ctx-delete" onClick={qaCtxItemDelete}>Delete</button>
        </div>,
        document.body
      )}

      {/* Quick action pack import collision dialog */}
      {quickActionImportPrompt && ReactDOM.createPortal(
        <div className="te-delete-overlay">
          <div className="te-delete-dialog te-import-dialog">
            <div className="te-delete-title">Import Quick Action Pack</div>
            <p className="te-delete-body">
              This pack contains <strong>{quickActionImportPrompt.totalCount}</strong>{' '}
              quick action{quickActionImportPrompt.totalCount !== 1 ? 's' : ''}.{' '}
              <strong>{quickActionImportPrompt.collisions.length}</strong> already
              exist{quickActionImportPrompt.collisions.length === 1 ? 's' : ''} with the same label in the same category:
            </p>
            <div className="te-import-collisions">
              {quickActionImportPrompt.collisions.slice(0, 8).map((c, i) => (
                <kbd key={`${c.id || c.label}-${i}`} className="te-trigger-badge">{c.label}</kbd>
              ))}
              {quickActionImportPrompt.collisions.length > 8 && (
                <span className="te-import-collisions-more">
                  + {quickActionImportPrompt.collisions.length - 8} more
                </span>
              )}
            </div>
            <div className="te-delete-actions te-import-actions">
              <button
                className="te-cancel-btn"
                onClick={() => onQuickActionImportResolve?.('cancel')}
                type="button"
              >
                Cancel
              </button>
              <button
                className="te-cancel-btn"
                onClick={() => onQuickActionImportResolve?.('skip')}
                type="button"
                title="Keep your existing quick actions; only import new ones"
              >
                Skip Duplicates
              </button>
              <button
                className="te-delete-confirm-btn"
                onClick={() => onQuickActionImportResolve?.('overwrite')}
                type="button"
                title="Replace your existing quick actions with the ones in this pack"
              >
                Overwrite All
              </button>
            </div>
          </div>
        </div>,
        document.body
      )}

      {/* Category colour picker popover (portal) */}
      {catColourPopover && ReactDOM.createPortal(
        <div
          ref={catColourPopoverRef}
          className="cat-colour-popover"
          style={{ left: catColourPopover.x, top: catColourPopover.y }}
        >
          <ColourPicker
            value={
              catColourPopover.forCat === '__new__'
                ? newCategoryColour
                : activeCats.find(c => c.name === catColourPopover.forCat)?.colour || null
            }
            onChange={handleCatColourSelect}
          />
        </div>,
        document.body
      )}
    </div>
  );
}
