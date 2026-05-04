import React, { useState, useMemo, useRef, useEffect, createElement } from 'react';
import * as AllLucide from 'lucide-react';
import './IconPicker.css';

// ── Build icon map from all Lucide exports ──────────────────────────────────
// Icon components are PascalCase functions; utility exports are lowercase.
const ICON_MAP = {};
const ALL_ICON_NAMES = [];
for (const [name, component] of Object.entries(AllLucide)) {
  if (typeof component === 'object' && component.$$typeof && /^[A-Z]/.test(name)) {
    ICON_MAP[name] = component;
    ALL_ICON_NAMES.push(name);
  }
}
ALL_ICON_NAMES.sort();

// ── Curated categories for quick browsing ───────────────────────────────────
const CATEGORIES = [
  { id: 'all', label: 'All' },
  { id: 'system', label: 'System', icons: ['Monitor','Laptop','Smartphone','Globe','Terminal','Settings','Power','Wifi','Bluetooth','Volume2','VolumeX','Sun','Moon','Eye','EyeOff','Lock','Unlock','Shield','Key','Cpu','Server','HardDrive','Database','Usb','Router','Printer','ScanLine','Fingerprint','QrCode','Nfc'] },
  { id: 'files', label: 'Files', icons: ['File','FileText','FilePlus','FileCode','FileImage','FileVideo','FileAudio','FileArchive','FileSpreadsheet','Folder','FolderOpen','FolderPlus','FolderArchive','Save','Download','Upload','Archive','Clipboard','ClipboardCopy','ClipboardPaste','Trash2','FileSearch','FileCheck','FileLock'] },
  { id: 'edit', label: 'Edit', icons: ['Type','Pencil','PencilLine','Scissors','Copy','Undo2','Redo2','AlignLeft','AlignCenter','AlignRight','AlignJustify','Bold','Italic','Underline','Strikethrough','Code','Hash','List','ListOrdered','ListChecks','Table','WrapText','Eraser','Highlighter','SpellCheck','CaseSensitive'] },
  { id: 'media', label: 'Media', icons: ['Play','Pause','Square','SkipForward','SkipBack','FastForward','Rewind','Mic','MicOff','Camera','CameraOff','Image','Film','Music','Music2','Music3','Music4','Headphones','Radio','Volume1','Volume2','VolumeX','MonitorSpeaker','Tv','Projector','Clapperboard','Youtube','Podcast'] },
  { id: 'comms', label: 'Comms', icons: ['Mail','MailOpen','MailPlus','MessageSquare','MessageCircle','MessagesSquare','Send','SendHorizontal','Phone','PhoneCall','PhoneOff','PhoneIncoming','PhoneOutgoing','Bell','BellOff','BellRing','AtSign','Link','Link2','ExternalLink','Share2','Rss','Radio','Megaphone','Contact','UserPlus','Users'] },
  { id: 'nav', label: 'Navigation', icons: ['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','ArrowUpRight','ArrowDownLeft','ChevronUp','ChevronDown','ChevronLeft','ChevronRight','ChevronsUp','ChevronsDown','Maximize2','Minimize2','Move','MoveHorizontal','MoveVertical','ZoomIn','ZoomOut','Search','Home','Navigation','Compass','Map','MapPin','LocateFixed','Route','Signpost','CornerDownRight','CornerUpLeft'] },
  { id: 'tools', label: 'Tools', icons: ['Zap','ZapOff','Target','Crosshair','Wand2','Wrench','Hammer','Cog','Settings2','Sliders','SlidersHorizontal','ToggleLeft','ToggleRight','RefreshCw','RefreshCcw','RotateCw','RotateCcw','Clock','Timer','TimerOff','Calendar','CalendarDays','Bookmark','BookmarkPlus','Star','Heart','Flag','Tag','Tags','Award','Trophy','Sparkles','Rocket','Bug','Plug','PlugZap','Pipette','Ruler','Gauge','Thermometer'] },
  { id: 'shapes', label: 'Shapes', icons: ['Circle','CircleDot','Square','RectangleHorizontal','Triangle','Hexagon','Octagon','Pentagon','Diamond','Plus','PlusCircle','Minus','MinusCircle','X','XCircle','Check','CheckCircle','CheckSquare','AlertTriangle','AlertCircle','Info','HelpCircle','Ban','ShieldAlert','ShieldCheck'] },
  { id: 'ui', label: 'Interface', icons: ['Grid','Grid3x3','Layout','LayoutDashboard','LayoutGrid','LayoutList','Layers','Layers2','Layers3','Box','Boxes','Package','Sidebar','SidebarOpen','SidebarClose','PanelLeft','PanelRight','PanelTop','PanelBottom','Menu','MoreHorizontal','MoreVertical','GripVertical','GripHorizontal','Maximize','Minimize','Expand','Shrink','Focus','SplitSquareVertical'] },
  { id: 'arrows', label: 'Arrows', icons: ['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','ArrowUpRight','ArrowUpLeft','ArrowDownRight','ArrowDownLeft','MoveUp','MoveDown','MoveLeft','MoveRight','ArrowBigUp','ArrowBigDown','ArrowBigLeft','ArrowBigRight','ArrowUpCircle','ArrowDownCircle','ArrowLeftCircle','ArrowRightCircle','Undo','Redo','CornerDownLeft','CornerDownRight','CornerUpLeft','CornerUpRight','Repeat','Repeat2','Shuffle','ArrowUpDown','ArrowLeftRight'] },
  { id: 'people', label: 'People', icons: ['User','UserPlus','UserMinus','UserCheck','UserX','UserCog','Users','Contact','PersonStanding','Accessibility','Baby','HandMetal','Hand','Handshake','HeartHandshake','ThumbsUp','ThumbsDown','Smile','Frown','Meh','Laugh','Angry'] },
  { id: 'weather', label: 'Weather', icons: ['Sun','Moon','Cloud','CloudRain','CloudSnow','CloudLightning','CloudDrizzle','CloudFog','CloudSun','CloudMoon','CloudOff','Snowflake','Wind','Tornado','Umbrella','Sunrise','Sunset','Rainbow','Thermometer','ThermometerSun','Droplets','Waves'] },
  { id: 'finance', label: 'Finance', icons: ['DollarSign','Euro','PoundSterling','Coins','Wallet','CreditCard','Banknote','Receipt','BadgeDollarSign','BadgePercent','PiggyBank','Landmark','Building2','TrendingUp','TrendingDown','BarChart','BarChart2','BarChart3','LineChart','PieChart','ArrowUpRight','ArrowDownRight','Calculator','Percent'] },
  { id: 'science', label: 'Science', icons: ['Atom','Dna','Microscope','FlaskConical','FlaskRound','TestTube','TestTubes','Beaker','Magnet','Orbit','Binary','Braces','BrainCircuit','BrainCog','CircuitBoard','Cpu','Variable','Sigma','Pi','Infinity','Radical','SquareRoot'] },
  { id: 'transport', label: 'Transport', icons: ['Car','CarFront','Bus','Truck','Bike','Ship','Plane','PlaneTakeoff','PlaneLanding','TrainFront','Sailboat','Rocket','Fuel','ParkingCircle','MapPin','Navigation','Route','Milestone','Footprints','Ambulance'] },
];

export function renderLucideIcon(name, size = 16, color = 'currentColor', duotone = false) {
  const Icon = ICON_MAP[name];
  if (!Icon) return null;
  const strokeWidth = size >= 20 ? 2.2 : 2;
  if (duotone) {
    // Duotone: filled shapes at low opacity behind, crisp strokes on top
    return createElement('div', { style: { position: 'relative', width: size, height: size } },
      createElement(Icon, { size, color, strokeWidth: 0, fill: color, opacity: 0.18, style: { position: 'absolute', top: 0, left: 0 } }),
      createElement(Icon, { size, color, strokeWidth, fill: 'none', style: { position: 'relative' } }),
    );
  }
  return createElement(Icon, { size, color, strokeWidth });
}

export function isLucideIcon(iconStr) {
  return iconStr && iconStr.startsWith('lucide:');
}

export function getLucideIconName(iconStr) {
  return iconStr?.replace('lucide:', '') || '';
}

export default function IconPicker({ onSelect, onClose, currentIcon }) {
  const [query, setQuery] = useState('');
  const [activeCategory, setActiveCategory] = useState('all');
  const inputRef = useRef(null);

  useEffect(() => { inputRef.current?.focus(); }, []);

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    let pool;
    if (activeCategory === 'all') {
      pool = ALL_ICON_NAMES;
    } else {
      const cat = CATEGORIES.find(c => c.id === activeCategory);
      pool = (cat?.icons || []).filter(n => ICON_MAP[n]);
    }
    if (!q) return pool;
    return pool.filter(name => name.toLowerCase().includes(q));
  }, [query, activeCategory]);

  // When searching, always search all icons regardless of category
  const searchResults = useMemo(() => {
    if (!query) return null;
    const q = query.toLowerCase();
    return ALL_ICON_NAMES.filter(name => name.toLowerCase().includes(q));
  }, [query]);

  const displayIcons = searchResults || filtered;

  return (
    <div className="icon-picker">
      <div className="icon-picker-header">
        <input
          ref={inputRef}
          className="icon-picker-search"
          placeholder={`Search ${ALL_ICON_NAMES.length} icons...`}
          value={query}
          onChange={e => setQuery(e.target.value)}
          onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') onClose?.(); }}
        />
        <button className="icon-picker-close" type="button" onClick={onClose}>&#10005;</button>
      </div>
      {!query && (
        <div className="icon-picker-cats">
          {CATEGORIES.map(c => (
            <button
              key={c.id}
              className={`icon-picker-cat${activeCategory === c.id ? ' active' : ''}`}
              type="button"
              onClick={() => setActiveCategory(c.id)}
            >
              {c.label}
            </button>
          ))}
        </div>
      )}
      <div className="icon-picker-grid">
        {displayIcons.length === 0 && <div className="icon-picker-empty">No icons found</div>}
        {displayIcons.slice(0, 200).map(name => {
          const Icon = ICON_MAP[name];
          if (!Icon) return null;
          const isActive = currentIcon === `lucide:${name}`;
          return (
            <button
              key={name}
              className={`icon-picker-item${isActive ? ' active' : ''}`}
              type="button"
              title={name}
              onClick={() => onSelect?.(`lucide:${name}`)}
            >
              {createElement(Icon, { size: 18, strokeWidth: 1.8 })}
            </button>
          );
        })}
        {displayIcons.length > 200 && (
          <div className="icon-picker-empty">Showing first 200 of {displayIcons.length} — search to narrow down</div>
        )}
      </div>
    </div>
  );
}
