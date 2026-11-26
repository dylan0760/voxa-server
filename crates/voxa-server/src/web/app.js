// Voxa web admin page: the server as the Voxa client shows it, in a browser.
// Everything the user or other people typed is inserted as text, never as HTML.
'use strict';
(async function () {
  const C = globalThis.VoxaCrypto;

  // ---------------------------------------------------------------- languages
  // Servers 1.30.0+: the page in the chosen language (Voxa's translations, from the
  // server). T('English text') is the text in that language, else the English.
  const LANG_KEY = 'voxa-web-lang';
  let LANGS = [];
  const TR = {};
  function langFromLocale(loc) {
    loc = String(loc || '').toLowerCase().replace('_', '-');
    if (loc.startsWith('zh')) return /hant|-tw|-hk|-mo/.test(loc) ? 'zh-TW' : 'zh-CN';
    if (loc.startsWith('pt')) return loc.endsWith('-pt') ? 'pt-PT' : 'pt-BR';
    let base = loc.split('-')[0];
    if (base === 'no' || base === 'nn') base = 'nb';
    return LANGS.some((l) => l.code === base) ? base : 'en';
  }
  function chosenLang() {
    let saved = null;
    try { saved = localStorage.getItem(LANG_KEY); } catch (_) { /* private mode */ }
    if (saved && LANGS.some((l) => l.code === saved)) return saved;
    for (const l of navigator.languages || [navigator.language]) {
      const c = langFromLocale(l);
      if (c !== 'en') return c;
    }
    return 'en';
  }
  try {
    LANGS = await (await fetch('/i18n/langs.json', { cache: 'no-cache' })).json();
    const code = chosenLang();
    document.documentElement.lang = code;
    if (code !== 'en') {
      const t = await (await fetch('/i18n/' + code + '.json', { cache: 'no-cache' })).json();
      // Only translations that keep the text's {} (filled in by the page).
      for (const [k, v] of Object.entries(t)) {
        if (typeof v === 'string' && v.trim() && k.split('{}').length === v.split('{}').length) TR[k] = v;
      }
    }
  } catch (_) { /* an older server, or offline: English */ }
  const T = (s) => (Object.prototype.hasOwnProperty.call(TR, s) ? TR[s] : s);
  function flagSvg(shapes) {
    const NS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', 'flag');
    svg.setAttribute('viewBox', '0 0 30 20');
    svg.setAttribute('aria-hidden', 'true');
    const hex = (c) => (typeof c === 'string' && /^#[0-9a-f]{6}$/i.test(c) ? c : '#888888');
    for (const s of Array.isArray(shapes) ? shapes : []) {
      if (!Array.isArray(s)) continue;
      if (s[0] === 'r' && s.length === 6) {
        const e = document.createElementNS(NS, 'rect');
        e.setAttribute('x', String(Number(s[1]) * 30));
        e.setAttribute('y', String(Number(s[2]) * 20));
        e.setAttribute('width', String(Number(s[3]) * 30));
        e.setAttribute('height', String(Number(s[4]) * 20));
        e.setAttribute('fill', hex(s[5]));
        svg.append(e);
      } else if (s[0] === 'c' && s.length === 5) {
        const e = document.createElementNS(NS, 'circle');
        e.setAttribute('cx', String(Number(s[1]) * 30));
        e.setAttribute('cy', String(Number(s[2]) * 20));
        e.setAttribute('r', String(Number(s[3]) * 20));
        e.setAttribute('fill', hex(s[4]));
        svg.append(e);
      } else if (s[0] === 'p' && s.length === 3 && Array.isArray(s[1])) {
        const pts = [];
        for (let i = 0; i + 1 < s[1].length; i += 2) pts.push((Number(s[1][i]) * 30).toFixed(2) + ',' + (Number(s[1][i + 1]) * 20).toFixed(2));
        const e = document.createElementNS(NS, 'polygon');
        e.setAttribute('points', pts.join(' '));
        e.setAttribute('fill', hex(s[2]));
        svg.append(e);
      }
    }
    return svg;
  }
  function browserLang() {
    for (const l of navigator.languages || [navigator.language]) {
      const c = langFromLocale(l);
      if (c !== 'en') return c;
    }
    return 'en';
  }
  function languageDialog() {
    let saved = null;
    try { saved = localStorage.getItem(LANG_KEY); } catch (_) { /* */ }
    let picked = saved || '';
    const sys = LANGS.find((l) => l.code === browserLang()) || LANGS[0];
    const rows = [];
    const applyPick = (val) => {
      try { if (val) localStorage.setItem(LANG_KEY, val); else localStorage.removeItem(LANG_KEY); } catch (_) { /* */ }
      location.reload();
    };
    const row = (val, flag, name, sub) => {
      const b = el('button', {
        type: 'button',
        class: 'lang-row' + (picked === val ? ' on' : ''),
        onclick: () => { picked = val; for (const x of rows) x.classList.toggle('on', x === b); },
        ondblclick: () => applyPick(val),
      }, flagSvg(flag), el('span', { class: 'lang-name' }, name), sub && sub !== name ? el('span', { class: 'lang-sub' }, sub) : null);
      rows.push(b);
      return b;
    };
    const list = el('div', { class: 'lang-list' },
      row('', sys ? sys.flag : [], T('System language') + (sys ? ' (' + sys.name + ')' : ''), ''),
      LANGS.map((l) => row(l.code, l.flag, l.name, l.english)));
    dialog(T('Language'), list, [
      { label: T('Cancel') },
      { label: T('OK'), primary: true, onClick: () => applyPick(picked) },
    ]);
  }

  // ---------------------------------------------------------------- helpers
  function el(tag, props, ...kids) {
    const e = document.createElement(tag);
    if (props) {
      for (const [k, v] of Object.entries(props)) {
        if (v == null || v === false) continue;
        if (k === 'class') e.className = v;
        // Through the CSSOM: the page's security policy blocks style attributes.
        else if (k === 'style') e.style.cssText = v;
        else if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v);
        else if (k === 'value' || k === 'checked' || k === 'disabled' || k === 'selected') e[k] = v;
        else e.setAttribute(k, v === true ? '' : String(v));
      }
    }
    for (const kid of kids.flat(Infinity)) {
      if (kid == null || kid === false) continue;
      e.append(kid instanceof Node ? kid : String(kid));
    }
    return e;
  }
  const $ = (s, root = document) => root.querySelector(s);
  // replaceChildren, skipping null / false (optional parts), which it would show as text.
  const put = (node, ...kids) => node.replaceChildren(...kids.flat(Infinity).filter((k) => k != null && k !== false));
  const now = () => Math.floor(Date.now() / 1000);
  const pad = (n) => String(n).padStart(2, '0');
  function fmtTime(ts) {
    const d = new Date(ts * 1000);
    const h=d.getHours();
    return localStorage.getItem('voxa-clock')==='12' ? (h%12||12)+':'+pad(d.getMinutes())+(h<12?' AM':' PM') : pad(h)+':'+pad(d.getMinutes());
  }
  function fmtDate(ts) {
    const d = new Date(ts * 1000);
    return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' + fmtTime(ts);
  }
  function fmtDuration(secs) {
    secs = Math.max(0, Math.floor(secs));
    const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600), m = Math.floor((secs % 3600) / 60);
    if (d > 0) return d + 'd ' + h + 'h';
    if (h > 0) return h + 'h ' + m + 'm';
    return m + 'm ' + (secs % 60) + 's';
  }
  const awayText = (a) => (!a ? T('Away') : /^away\b/i.test(a) ? a : T('Away: ') + a);
  function initials(name) {
    const w = (name || '?').trim().split(/\s+/).filter(Boolean);
    return ((w[0] || '?')[0] + (w.length > 1 ? w[1][0] : (w[0] || '').slice(1, 2))).toUpperCase();
  }
  // The client's colours for avatars and group tags (same hash over the UTF-8 bytes).
  const NAME_COLORS = ['#2580C3', '#E07B39', '#3BA55D', '#9B59B6', '#D84C5C', '#1AA39C', '#C99A22'];
  function nameColor(s) {
    let h = 0;
    for (const b of new TextEncoder().encode(s || '')) h = (Math.imul(h, 31) + b) >>> 0;
    return NAME_COLORS[h % NAME_COLORS.length];
  }
  // Pictures and colours people chose (servers 1.15.0+): uid -> { color, image }.
  const avatars = new Map();
  const hexColor = (c) => '#' + (c >>> 0).toString(16).padStart(6, '0').slice(-6);
  function avatar(name, uid, size, dot, square) {
    const meta = uid ? avatars.get(uid) : null;
    const bg = meta && meta.color != null ? hexColor(meta.color) : nameColor(uid || name);
    const a = el('span', { class: 'avatar' + (square ? ' sq' : ''), style: `width:${size}px;height:${size}px;font-size:${Math.round(size * 0.4)}px;background:${bg}` },
      meta && meta.image ? el('img', { src: '/api/avatar?uid=' + encodeURIComponent(uid) + '&v=' + meta.image, alt: '', loading: 'lazy' }) : initials(name));
    if (dot) a.append(el('span', { class: 'dot', style: 'background:' + dot }));
    return a;
  }
  function toast(text, kind) {
    const t = el('div', { class: 'toast ' + (kind || '') }, text);
    $('#toasts').append(t);
    setTimeout(() => t.remove(), kind === 'bad' ? 7000 : 4500);
  }
  function copy(text) {
    if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(text).then(() => toast('Copied', 'good'));
    else {
      const t = el('textarea', { value: text, style: 'position:fixed;opacity:0' });
      document.body.append(t);
      t.select();
      document.execCommand('copy');
      t.remove();
      toast('Copied', 'good');
    }
  }

  // ---------------------------------------------------------------- themes
  // The client's themes: solid ones, then two-tone ones (a colour blending into another).
  const THEMES = [['dark', 'Dark'], ['light', 'Light'], ['sakura', 'Sakura'], ['layette', 'Layette'], ['crimson', 'Crimson'], ['midnight', 'Midnight'], ['celadon', 'Celadon'],
    ['mustard', 'Mustard'], ['slate', 'Slate'], ['obsidian', 'Obsidian'],
    ['citrus', 'Citrus', 1], ['candyfloss', 'Candyfloss', 1], ['spearmint', 'Spearmint', 1], ['lofi', 'Lo-fi', 1], ['dune', 'Dune', 1],
    ['raincloud', 'Raincloud', 1], ['afterglow', 'Afterglow', 1], ['bloodmoon', T('Blood Moon'), 1], ['nightshade', 'Nightshade', 1], ['twilight', 'Twilight', 1],
    ['synthwave', 'Synthwave', 1], ['pinklemonade', T('Pink Lemonade'), 1], ['aurora', 'Aurora', 1], ['woodland', 'Woodland', 1], ['vicecity', T('Vice City'), 1], ['ownpicture', T('Your picture'), 2]];
  function applyTheme(t, fromServer) {
    if (!THEMES.some(([k]) => k === t)) t = 'dark';
    document.documentElement.dataset.theme = t;
    try { localStorage.setItem('voxa-theme', t); } catch (_) { /* private mode */ }
    if (!fromServer && typeof savePrefs === 'function') savePrefs({ theme: t });
  }
  // "Your picture": a picture kept in this browser only (shrunk and re-encoded as JPEG).
  const OWN_BG = 'voxa-web-background', OWN_DIM = 'voxa-web-background-dim';
  const safeDataUrl = (u) => (typeof u === 'string' && /^data:image\/jpeg;base64,[A-Za-z0-9+/=]+$/.test(u) ? u : null);
  function applyBackground() {
    let u = null, dim = 0.35;
    try { u = safeDataUrl(localStorage.getItem(OWN_BG)); dim = Math.min(0.8, Math.max(0, Number(localStorage.getItem(OWN_DIM) ?? 0.35) || 0)); } catch (_) { /* private mode */ }
    const s = document.documentElement.style;
    if (u) s.setProperty('--own-bg', 'url("' + u + '")'); else s.removeProperty('--own-bg');
    s.setProperty('--own-dim', String(dim));
  }
  applyBackground();
  applyTheme(localStorage.getItem('voxa-theme') || (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'), true);
  // The theme picker, like the client's Options ▸ Application ▸ Themes: a miniature of each
  // theme (the tile carries data-theme, so it gets that theme's colours). Clicking applies it.
  function themeDialog() {
    const tiles = [];
    const tile = ([k, label]) => {
      const t = el('div', { class: 'theme-tile', title: label, onclick: () => { applyTheme(k); for (const x of tiles) x.classList.toggle('on', x === t); } },
        el('div', { class: 'tt-pic', 'data-theme': k },
          el('div', { class: 'tt-rail' }, el('span', { class: 'tt-dot' })),
          el('div', { class: 'tt-side' }, el('i'), el('i'), el('i')),
          el('div', { class: 'tt-main' }, el('div', { class: 'tt-msg' }, el('i')), el('span', { class: 'tt-btn' }))),
        el('div', { class: 'tt-name' }, label));
      if (k === document.documentElement.dataset.theme) t.classList.add('on');
      tiles.push(t);
      return t;
    };
    dialog(T('Theme'), el('div', null,
      el('div', { class: 'section', style: 'margin-top:0' }, T('Your picture')),
      el('div', { class: 'dim small' }, T('Any picture of yours behind see-through panels. It is kept in this browser only.')),
      el('div', { class: 'own-bg-row' }, THEMES.filter((t) => t[2] === 2).map(tile), ownPictureControls(() => { const t = tiles.find((x) => x.title === 'Your picture'); applyTheme('ownpicture'); for (const x of tiles) x.classList.toggle('on', x === t); })),
      el('div', { class: 'section' }, T('Solid')),
      el('div', { class: 'dim small' }, T('One colour family per theme.')),
      el('div', { class: 'theme-grid' }, THEMES.filter((t) => !t[2]).map(tile)),
      el('div', { class: 'section' }, T('Two-tone')),
      el('div', { class: 'dim small' }, T('A colour that blends into another, behind see-through panels.')),
      el('div', { class: 'theme-grid' }, THEMES.filter((t) => t[2] === 1).map(tile))),
      [{ label: T('Done'), primary: true }], { wide: true });
  }
  // Choose / remove the picture and how dark it is.
  function ownPictureControls(chosen) {
    const msg = el('div', { class: 'dim small' });
    const file = el('input', { type: 'file', accept: 'image/*', style: 'display:none', onchange: () => {
      const f = file.files && file.files[0];
      if (!f) return;
      if (f.size > 40 * 1024 * 1024) { msg.textContent = T('That picture is larger than 40 MB.'); return; }
      const img = new Image();
      const url = URL.createObjectURL(f);
      img.onload = () => {
        URL.revokeObjectURL(url);
        const k = Math.min(1, 1920 / Math.max(img.width, img.height));
        const cv = document.createElement('canvas');
        cv.width = Math.max(1, Math.round(img.width * k)); cv.height = Math.max(1, Math.round(img.height * k));
        cv.getContext('2d').drawImage(img, 0, 0, cv.width, cv.height);
        try { localStorage.setItem(OWN_BG, cv.toDataURL('image/jpeg', 0.85)); msg.textContent = ''; } catch (_) { msg.textContent = T('The browser has no room to keep that picture.'); return; }
        applyBackground();
        chosen();
      };
      img.onerror = () => { URL.revokeObjectURL(url); msg.textContent = T('That file is not a picture the browser can show.'); };
      img.src = url;
    } });
    let dim = 0.35;
    try { dim = Number(localStorage.getItem(OWN_DIM) ?? 0.35) || 0; } catch (_) { /* private mode */ }
    const range = el('input', { type: 'range', min: '0', max: '0.8', step: '0.05', value: String(dim), oninput: () => { try { localStorage.setItem(OWN_DIM, range.value); } catch (_) { /* */ } applyBackground(); } });
    return el('div', { class: 'own-bg-controls' },
      el('button', { class: 'btn', onclick: () => file.click() }, T('🖼 Choose picture…')),
      el('button', { class: 'btn', onclick: () => { try { localStorage.removeItem(OWN_BG); } catch (_) { /* */ } applyBackground(); } }, T('Remove picture')),
      el('label', { class: 'small', style: 'color:var(--text);display:flex;gap:6px;align-items:center' }, T('Darken'), range), file, msg);
  }
  const themeMenu = () => themeDialog();

  // ---------------------------------------------------------------- permissions
  const P = {
    SERVER_EDIT: 0, CHANNEL_CREATE_PERMANENT: 1, CHANNEL_CREATE_SEMI_PERMANENT: 2, CHANNEL_CREATE_TEMPORARY: 3,
    CHANNEL_EDIT: 4, CHANNEL_DELETE: 5, CHANNEL_JOIN_IGNORE_PASSWORD: 6, CHANNEL_JOIN_IGNORE_MAXCLIENTS: 7,
    CLIENT_MOVE: 8, CLIENT_KICK_CHANNEL: 9, CLIENT_KICK_SERVER: 10, CLIENT_BAN: 11, BAN_LIST: 12, CLIENT_POKE: 13,
    PRIVATE_MESSAGE: 14, SERVER_CHAT: 15, CHANNEL_CHAT: 16, CHAT_DELETE: 17, GROUP_MANAGE: 18, GROUP_ASSIGN_SERVER: 19,
    GROUP_ASSIGN_CHANNEL: 20, TALK_POWER_GRANT: 21, PRIVILEGE_KEY_CREATE: 22, FILE_BROWSE: 23, FILE_UPLOAD: 24,
    FILE_DOWNLOAD: 25, FILE_DELETE: 26, SCREEN_SHARE: 27, CHANNEL_COMMANDER: 28, CHAT_HISTORY: 29,
    CLIENT_DESCRIPTION_EDIT: 30, IGNORE_SERVER_MAXCLIENTS: 31, CHANNEL_JOIN: 32, CHANNEL_JOIN_IGNORE_POWER: 33,
    CHAT_CLEAR: 34, CHAT_PIN: 35, CHAT_EDIT_OWN: 36, SERVER_EVENTS: 37, CALENDAR_VIEW: 38, CALENDAR_RSVP: 39, CALENDAR_EVENTS: 40, CALENDAR_MANAGE: 41, CHANNEL_VIEW: 42, GIF_MANAGE: 43,
  };
  const PERM_NAMES = [
    [P.SERVER_EDIT, T('Edit virtual server settings')], [P.CHANNEL_CREATE_PERMANENT, T('Create permanent channels')],
    [P.CHANNEL_CREATE_SEMI_PERMANENT, T('Create semi-permanent channels')], [P.CHANNEL_CREATE_TEMPORARY, T('Create temporary channels')],
    [P.CHANNEL_EDIT, T('Edit channels')], [P.CHANNEL_DELETE, T('Delete channels')], [P.CHANNEL_VIEW, T('See channels')], [P.GIF_MANAGE, T('Manage server GIFs')], [P.CHANNEL_JOIN, T('Join / switch channels')],
    [P.CHANNEL_JOIN_IGNORE_POWER, T('Join channels regardless of join power')], [P.CHANNEL_JOIN_IGNORE_PASSWORD, T('Join channels without password')],
    [P.CHANNEL_JOIN_IGNORE_MAXCLIENTS, T('Join full channels')], [P.CLIENT_MOVE, T('Move clients')], [P.CLIENT_KICK_CHANNEL, T('Kick clients from channel')],
    [P.CLIENT_KICK_SERVER, T('Kick clients from server')], [P.CLIENT_BAN, T('Ban clients')], [P.BAN_LIST, T('View and edit ban list')],
    [P.CLIENT_POKE, T('Poke clients')], [P.PRIVATE_MESSAGE, T('Send private messages')], [P.SERVER_EVENTS, T('See server events (the Server tab)')], [P.CALENDAR_VIEW, T('See the channel calendar')], [P.CALENDAR_RSVP, T('Answer calendar invitations')], [P.CALENDAR_EVENTS, T('Create and edit calendar events')], [P.CALENDAR_MANAGE, T('Create, rename and delete calendars')], [P.SERVER_CHAT, T('Send server chat messages (no longer used)')],
    [P.CHANNEL_CHAT, T('Send channel chat messages')], [P.CHAT_HISTORY, T('Read chat history')], [P.CHAT_DELETE, "Delete anyone's chat messages (incl. history)"],
    [P.CHAT_CLEAR, T('Clear entire chat history')], [P.CHAT_PIN, T('Pin messages')], [P.CHAT_EDIT_OWN, T('Edit own chat messages')], [P.GROUP_MANAGE, T('Create, edit and delete groups')],
    [P.GROUP_ASSIGN_SERVER, T('Assign server groups')], [P.GROUP_ASSIGN_CHANNEL, T('Assign channel groups')], [P.TALK_POWER_GRANT, T('Grant / revoke talk power')],
    [P.PRIVILEGE_KEY_CREATE, T('Create privilege keys')], [P.FILE_BROWSE, T('Browse channel files')], [P.FILE_UPLOAD, T('Upload files')],
    [P.FILE_DOWNLOAD, T('Download files')], [P.FILE_DELETE, T('Delete files')], [P.SCREEN_SHARE, T('Share screen / applications')],
    [P.CHANNEL_COMMANDER, T('Use channel commander')], [P.CLIENT_DESCRIPTION_EDIT, T('Edit client descriptions')], [P.IGNORE_SERVER_MAXCLIENTS, T('Join when server is full')],
  ];
  const bit = (n) => 1n << BigInt(n);
  const GUEST_GROUP = 3;
  function myPerms() {
    const me = S.clients.get(S.me);
    if (!me) return 0n;
    // Until signing in to the account (when the server wants it): only Guest's.
    // Until the account state arrives (right after Welcome), Guest's: nothing is asked for
    // that the server would refuse while a remembered browser is still signing in.
    if (!S.account || S.account.limited) return BigInt(S.groups.get(GUEST_GROUP)?.perms ?? 0);
    let p = BigInt(S.extra || 0);
    for (const g of me.server_groups) p |= BigInt(S.groups.get(g)?.perms ?? 0);
    p |= BigInt(S.groups.get(me.channel_group)?.perms ?? 0);
    return p;
  }
  const can = (n) => (myPerms() & bit(n)) !== 0n;
  const canCreate = () => can(P.CHANNEL_CREATE_PERMANENT) || can(P.CHANNEL_CREATE_SEMI_PERMANENT) || can(P.CHANNEL_CREATE_TEMPORARY);

  // ---------------------------------------------------------------- protocol
  const spacer = (name) => {
    const m = /^\[([clr*]?)spacer(\d{0,6})\]([\s\S]*)$/.exec(name || '');
    return m ? { align: m[1] || 'l', number: m[2], text: m[3] } : null;
  };
  const spacerName = (sp) => '[' + (sp.align === 'l' ? '' : sp.align) + 'spacer' + (sp.number || '') + ']' + sp.text;
  const CODEC_MUSIC = 0x80, CODEC_LOSSLESS = 0x40;
  const codecOf = (q) => (q & CODEC_LOSSLESS ? 'lossless' : q & CODEC_MUSIC ? 'music' : 'voice');
  const qualityOf = (q) => Math.min(10, q & ~(CODEC_MUSIC | CODEC_LOSSLESS));
  const packCodec = (codec, q) => Math.min(10, q) | (codec === 'music' ? CODEC_MUSIC : codec === 'lossless' ? CODEC_LOSSLESS : 0);
  function bitrate(cq) {
    const c = codecOf(cq), q = qualityOf(cq);
    if (c === 'music') return 32 + q * 16;
    if (c === 'lossless') return 1536;
    return Math.round((8000 + q * 8800) / 1000);
  }
  const CODEC_NAMES = { voice: T('Opus Voice'), music: T('Opus Music'), lossless: T('Lossless') };
  const KIND_NAMES = { Permanent: T('Permanent'), SemiPermanent: T('Semi-Permanent'), Temporary: T('Temporary') };
  const keyOf = (t) => (t === 'Server' ? 'server' : t.Channel !== undefined ? 'ch:' + t.Channel : null);
  const IMAGE_RE = /^📷 ([\s\S]*)\n\[voxa-image:(\d+):(\d+)x(\d+)\]$/;
  function leaveText(r) {
    if (r === 'TimedOut') return T('timed out');
    if (r.Disconnected !== undefined) return r.Disconnected ? 'left (' + r.Disconnected + ')' : 'left';
    if (r.Kicked) return T('was kicked by ') + r.Kicked.by + (r.Kicked.reason ? ': ' + r.Kicked.reason : '');
    if (r.Banned) return T('was banned by ') + r.Banned.by + (r.Banned.reason ? ': ' + r.Banned.reason : '');
    return 'left';
  }

  // ---------------------------------------------------------------- state
  const S = {
    me: 0, uid: '', server: null, channels: new Map(), clients: new Map(), groups: new Map(), extra: 0,
    logs: new Map(), tab: 'channel', dms: [], selected: { kind: 'server' }, right: 'info', showRight: true,
    people: [], peopleAsked: 0, collapsed: new Set(), pins: null, showPins: false, wait: {}, dragging: false,
    connected: false, msgDirty: true, presences: new Map(),
  };
  let es = null;
  S.blocks = new Set();
  S.games = new Map();
  // Server 1.31.0 kept browser camera and manual game settings; the admin page no longer uses them.
  try { for (const k of ['voxa-camera-rotation', 'voxa-camera-bg', 'voxa-game-on', 'voxa-game-name']) localStorage.removeItem(k); } catch (_) { /* private mode */ }
  let personalLimits={upload_kib:0,download_kib:0,uploads:3,downloads:3};
  try { Object.assign(personalLimits,JSON.parse(localStorage.getItem('voxa-transfer-limits')||'{}')); } catch (_) {}
  function transferFields(limits) {
    const fields = [], inputs = {};
    for (const [key, label] of [['upload_kib', 'Upload speed'], ['download_kib', 'Download speed'], ['uploads', 'Concurrent uploads'], ['downloads', 'Concurrent downloads']]) {
      const rate = key.endsWith('_kib');
      const input = inputs[key] = el('input', {type:'number', min:rate ? 1 : 0, max:rate ? 1000000 : 1024, value:String(limits[key] || (rate ? 128 : 0))});
      if (rate) {
        input.disabled = !limits[key];
        const unlimited = el('input', {type:'checkbox', checked:input.disabled, onchange:() => { input.disabled = unlimited.checked; }});
        fields.push(field(T(label)+' (KiB/s)', el('div',null,input,el('label',null,unlimited,' '+T('Unlimited')))));
      } else {
        fields.push(field(T(label),input,'0 = '+T('Unlimited')));
      }
    }
    const read = () => Object.fromEntries(Object.entries(inputs).map(([key,i]) => [
      key, i.disabled ? 0 : Math.min(Number(i.max),Math.max(Number(i.min),Math.trunc(Number(i.value) || 0))),
    ]));
    return {fields, read};
  }
  function personalOptions() {
    const transfer = transferFields(personalLimits);
    const clock = el('select', null, ['24', '12'].map((n) => el('option', { value: n, selected: n === (localStorage.getItem('voxa-clock') || '24') }, n + ' hour')));
    dialog(T('Settings'), el('div', { class: 'form' }, field(T('Clock'), clock),
      el('div', { class: 'section full' }, 'Personal transfer limits'), transfer.fields,
      el('div', { class: 'full small' }, 'Voxa Non-Commercial License 1.0 · ', el('a', { href: 'https://github.com/dylan0760/voxa-client', target: '_blank', rel: 'noopener noreferrer' }, 'Original client'), ' · ',
        el('a', { href: 'https://github.com/dylan0760/voxa-server', target: '_blank', rel: 'noopener noreferrer' }, 'Original server'))), [
      { label: T('Cancel') },
      { label: T('Save'), primary: true, onClick: () => {
        localStorage.setItem('voxa-clock', clock.value);
        personalLimits = transfer.read();
        localStorage.setItem('voxa-transfer-limits', JSON.stringify(personalLimits));
        send({ ClientTransferLimits: personalLimits });
        S.msgDirty = true;
        schedule();
      } }]);
  }
  const playing = (id) => S.games.get(id);

  function log(key) {
    let l = S.logs.get(key);
    if (!l) S.logs.set(key, (l = { lines: [], unread: 0, loaded: false, complete: false }));
    return l;
  }
  const myChannel = () => S.clients.get(S.me)?.channel;
  // Channel 0: on the server, but in no channel (like Discord).
  const inChannel = () => { const c = myChannel(); return c != null && c !== 0; };
  const PRESENCES = [
    ['Online', 'var(--green)', 'Online', T('Everyone sees you online')],
    ['Away', 'var(--yellow)', 'Away', T('Shown as away')],
    ['Busy', 'var(--red)', T('Do not disturb'), T('No sounds for joins and leaves')],
    ['Invisible', 'var(--dim)', 'Invisible', T('Appear offline while you are not in a channel (administrators still see you)')],
  ];
  function myPresence() {
    const me = S.clients.get(S.me);
    if (me && me.away != null) return 'Away';
    return S.presences.get(S.me) || 'Online';
  }
  function setPresence(p) {
    localStorage.setItem('voxa-web-presence', p);
    send({ SetPresence: p });
  }
  function tabKey(tab = S.tab) {
    if (tab === 'server') return 'server';
    if (tab === 'channel') return 'ch:' + myChannel();
    return tab; // 'dm:<uid>'
  }
  function tabTarget(tab = S.tab) {
    if (tab === 'server') return 'Server';
    if (tab === 'channel') return { Channel: myChannel() };
    return null;
  }
  function onlineByUid(uid) {
    const all = [...S.clients.values()].filter((c) => c.uid === uid && c.id !== S.me);
    return all.find((c) => c.platform !== 'Web') || all[0] || null;
  }
  function personName(uid) {
    return onlineByUid(uid)?.nickname || S.dms.find((d) => d.uid === uid)?.name || S.people.find((p) => p.uid === uid)?.nickname || 'Someone';
  }

  async function post(path, body) {
    const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Voxa': '1' }, body: JSON.stringify(body ?? {}), credentials: 'same-origin' });
    if (!r.ok && r.status !== 204) {
      let msg = T('Request failed (') + r.status + ')';
      try { msg = (await r.json()).error || msg; } catch (_) { /* not JSON */ }
      throw new Error(msg);
    }
    return r.status === 204 ? null : r.json();
  }
  function send(msg) {
    post('/api/send', msg).catch((e) => toast(e.message, 'bad'));
  }

  // ---------------------------------------------------------------- sign-in
  // The proof is bound to this page's origin, so a look-alike site cannot reuse it.
  const PROOF_CONTEXT = 'voxa-web-login-v2';
  const PENDING = 'voxa-web-pending';
  const channelBus = 'BroadcastChannel' in window ? new BroadcastChannel('voxa-web') : null;

  async function boot() {
    if (!C.selfTest()) {
      put(document.getElementById('root'), el('div', { class: 'login' }, el('div', { class: 'login-card' }, el('h1', null, T('Unsupported browser')), el('p', { class: 'sub' }, T('This browser computed the sign-in cryptography wrongly. Please use a current Chrome, Edge, Firefox or Safari.')))));
      return;
    }
    // A sign-in through Voxa that never finished must not leave a password behind.
    try {
      const p = JSON.parse(localStorage.getItem(PENDING) || 'null');
      if (p && !(Date.now() - p.ts < 180000)) localStorage.removeItem(PENDING);
    } catch (_) { localStorage.removeItem(PENDING); }
    // Opened by the Voxa client with a proof for this page.
    if (location.hash.startsWith('#voxa-login=')) {
      const data = location.hash.slice('#voxa-login='.length);
      history.replaceState(null, '', location.pathname);
      try {
        await finishClientSignIn(data);
        return;
      } catch (ex) {
        return showLogin(ex.message);
      }
    }
    if ((await signedIn()) === true) return connect();
    const remembered = localStorage.getItem('voxa-web-key');
    if (remembered) {
      try {
        await signInWithKey(remembered, pendingFromForm(localStorage.getItem('voxa-web-nick') || '', null, '', sessionStorage.getItem('voxa-web-pw') || ''));
        return connect();
      } catch (_) { /* show the form */ }
    }
    showLogin('');
  }

  /// true / false, or null when the server cannot be reached.
  async function signedIn() {
    try {
      const r = await fetch('/api/session', { credentials: 'same-origin', cache: 'no-store' });
      return r.ok ? !!(await r.json()).signed_in : null;
    } catch (_) {
      return null;
    }
  }

  async function challenge() {
    const ch = await (await fetch('/api/challenge', { cache: 'no-store', credentials: 'same-origin' })).json();
    if (ch.error) throw new Error(ch.error);
    return ch;
  }

  function pendingFromForm(nickname, channel, channelPassword, password) {
    return { nickname: nickname || '', channel, channel_password: channelPassword || null, password: password || null };
  }

  function loginBody(id, pub, proof, p) {
    return { id, public: pub, proof, origin: location.origin, nickname: p.nickname, password: p.password, channel: p.channel, channel_password: p.channel_password };
  }

  async function signInWithKey(key, p) {
    let priv;
    try { priv = C.b64decode(key.trim()); } catch (_) { priv = null; }
    if (!priv || priv.length !== 32) throw new Error(T('That is not a Voxa identity key (it should be 44 characters ending in "=").'));
    const ch = await challenge();
    const serverKey = C.b64decode(ch.server_key);
    const pub = C.publicKey(priv);
    const shared = C.sharedKey(priv, serverKey);
    const proof = C.hmacSha256(shared, C.concat(C.utf8(PROOF_CONTEXT), serverKey, C.b64decode(ch.nonce), pub, C.utf8(location.origin)));
    return post('/api/login', loginBody(ch.id, C.b64encode(pub), C.b64encode(proof), p));
  }

  /// Asks the installed Voxa client to sign this page in: it computes the proof with the
  /// identity it uses for this server (after the user agrees) and opens this page again
  /// with it. The page that asked then picks up the sign-in.
  async function signInWithClient(p, status) {
    const ch = await challenge();
    localStorage.setItem(PENDING, JSON.stringify({ ...p, ts: Date.now() }));
    const q = new URLSearchParams({ o: location.origin, i: ch.id, k: ch.server_key, n: ch.nonce, f: ch.fingerprint || '', s: ch.name || '' });
    location.href = 'voxa://web-login?' + q.toString();
    status(T('Waiting for Voxa… Confirm the sign-in in the Voxa window.'));
    // Picked up when the tab Voxa opens has signed in (same browser, so the same cookie).
    const started = Date.now();
    return new Promise((resolve, reject) => {
      const done = () => { clearInterval(timer); if (channelBus) channelBus.removeEventListener('message', onMsg); };
      const onMsg = (e) => {
        if (e.data && e.data.type === 'signed-in') {
          channelBus.postMessage({ type: 'taken' });
          done();
          resolve();
        }
      };
      if (channelBus) channelBus.addEventListener('message', onMsg);
      const timer = setInterval(async () => {
        if ((await signedIn()) === true) { done(); resolve(); }
        else if (Date.now() - started > 120000) { done(); localStorage.removeItem(PENDING); reject(new Error(T('Voxa did not answer. Is it installed and running? You can paste your identity key instead.'))); }
      }, 2000);
    });
  }

  async function finishClientSignIn(data) {
    let d;
    try { d = JSON.parse(new TextDecoder().decode(C.b64decode(data.replace(/-/g, '+').replace(/_/g, '/')))); } catch (_) { throw new Error(T('The sign-in from Voxa could not be read.')); }
    let p = {};
    try { p = JSON.parse(localStorage.getItem(PENDING) || '{}'); } catch (_) { /* none */ }
    localStorage.removeItem(PENDING);
    if (!p.ts || Date.now() - p.ts > 180000) p = pendingFromForm('', null, '', '');
    await post('/api/login', loginBody(d.i, d.p, d.proof, p));
    // If the page that asked is still open, it takes over and this tab closes.
    if (channelBus) {
      const taken = await new Promise((resolve) => {
        const t = setTimeout(() => resolve(false), 600);
        channelBus.addEventListener('message', (e) => { if (e.data && e.data.type === 'taken') { clearTimeout(t); resolve(true); } }, { once: true });
        channelBus.postMessage({ type: 'signed-in' });
      });
      if (taken) {
        put(document.getElementById('root'), el('div', { class: 'login' }, el('div', { class: 'login-card' },
          el('img', { class: 'logo', src: '/icon.svg', alt: '' }), el('h1', null, T('Signed in')), el('p', { class: 'sub' }, T('You are signed in in your other tab. You can close this one.')))));
        window.close();
        return;
      }
    }
    connect();
  }

  async function showLogin(error) {
    if (es) { es.close(); es = null; }
    S.connected = false;
    let info = {};
    try { info = await (await fetch('/api/challenge', { cache: 'no-store' })).json(); } catch (_) { /* offline */ }
    let channels = [];
    try { channels = (await (await fetch('/api/channels', { cache: 'no-store' })).json()).channels || []; } catch (_) { /* none */ }
    const lastChannel = localStorage.getItem('voxa-web-channel');
    const key = el('textarea', { placeholder: T('Paste your exported identity key'), spellcheck: 'false', autocomplete: 'off' });
    const nick = el('input', { type: 'text', value: localStorage.getItem('voxa-web-nick') || '', placeholder: T('Last used, or "Web admin"'), maxlength: '30' });
    const pw = el('input', { type: 'password', placeholder: T('Server password'), autocomplete: 'current-password' });
    // The channel to join: a list when the server shares it, otherwise a path.
    const chSel = channels.length
      ? el('select', null,
        el('option', { value: '\u0001', selected: lastChannel == null }, T('The one I used last time')),
        el('option', { value: '', selected: lastChannel === '' }, T('The default channel')),
        el('option', { value: '\u0001nochannel\u0001', selected: lastChannel === '\u0001nochannel\u0001' }, T('No channel (just be on the server)')),
        channels.map((c) => el('option', { value: c.path, selected: c.path === lastChannel }, c.path + (c.password ? '  🔒' : ''))))
      : el('input', { type: 'text', placeholder: 'e.g. Lobby/Games (empty: the one used last time)', value: lastChannel || '' });
    const chPw = el('input', { type: 'password', placeholder: T('Channel password'), autocomplete: 'off' });
    const chPwRow = el('div', { style: 'display:flex;flex-direction:column;gap:6px' }, el('label', null, T('Channel password')), chPw);
    const updPw = () => { const c = channels.find((x) => x.path === chSel.value); chPwRow.classList.toggle('hidden', !(c && c.password) && channels.length > 0); };
    chSel.addEventListener('change', updPw);
    updPw();
    const remember = el('input', { type: 'checkbox', id: 'remember' });
    const err = el('div', { class: 'error' }, error || '');
    const btn = el('button', { class: 'btn primary', type: 'submit' }, T('Sign in with key'));
    const voxaBtn = el('button', { class: 'btn primary', type: 'button' }, T('🎧 Sign in with Voxa'));
    const pending = () => {
      let channel = chSel.value;
      if (channel === '\u0001') channel = null;
      else if (!channels.length && !channel.trim()) channel = null;
      localStorage.setItem('voxa-web-nick', nick.value.trim());
      if (channel == null) localStorage.removeItem('voxa-web-channel'); else localStorage.setItem('voxa-web-channel', channel);
      return pendingFromForm(nick.value.trim(), channel, chPw.value, pw.value);
    };
    const busy = (on) => { btn.disabled = on; voxaBtn.disabled = on; };
    voxaBtn.addEventListener('click', async () => {
      err.textContent = '';
      busy(true);
      try {
        await signInWithClient(pending(), (t) => { err.style.color = 'var(--dim)'; err.textContent = t; });
        connect();
      } catch (ex) {
        err.style.color = '';
        err.textContent = ex.message;
        busy(false);
      }
    });
    const form = el('form', {
      onsubmit: async (e) => {
        e.preventDefault();
        err.textContent = '';
        busy(true);
        try {
          await signInWithKey(key.value, pending());
          if (remember.checked) {
            localStorage.setItem('voxa-web-key', key.value.trim());
            if (pw.value) sessionStorage.setItem('voxa-web-pw', pw.value);
          }
          key.value = '';
          connect();
        } catch (ex) {
          err.textContent = ex.message;
          busy(false);
        }
      },
    },
      voxaBtn,
      el('div', { class: 'help', style: 'margin:2px 0 6px;text-align:center' }, T('Uses the identity Voxa on this PC has for this server. Voxa asks you to confirm.')),
      el('div', { class: 'or' }, T('or paste an identity key')),
      key,
      el('div', { class: 'row' }, remember, el('label', { for: 'remember', style: 'margin:0;color:var(--text);font-size:13px' }, T('Remember the key in this browser'))),
      btn,
      el('div', { class: 'or' }, 'options'),
      el('label', null, T('Nickname')), nick,
      el('label', null, T('Join channel')), chSel,
      chPwRow,
      info.has_password ? [el('label', null, T('Server password')), pw] : null,
      err);
    put(document.getElementById('root'),
      el('div', { class: 'login' },
        el('button', { class: 'icon-btn login-lang', title: T('Language'), onclick: languageDialog }, '🌐'),
        el('button', { class: 'icon-btn login-theme', title: T('Theme'), onclick: (e) => themeMenu(e.clientX - 160, e.clientY + 10) }, '🎨'),
        el('div', { class: 'login-card' },
          el('img', { class: 'logo', src: '/icon.svg', alt: '' }),
          el('h1', null, info.name || T('Voxa Server')),
          el('div', { class: 'sub' }, T('Server administration · Voxa Server ') + (info.version || '')),
          form,
          el('div', { class: 'help' }, T('You get the same rights here as your identity has in Voxa. To get a key, in Voxa click your name at the bottom left ▸ Identities and export the one you use on this server. Keys stay in this browser: only a proof that you own one is sent.')),
          info.fingerprint ? el('div', { class: 'fp' }, T('Server fingerprint: ') + info.fingerprint) : null)));
  }

  async function signOut() {
    localStorage.removeItem('voxa-web-key');
    sessionStorage.removeItem('voxa-web-pw');
    try { await post('/api/logout'); } catch (_) { /* already gone */ }
    showLogin(T('Signed out.'));
  }

  /// Theme, nickname and channel are remembered per identity on the server, so they
  /// follow you to other browsers and addresses.
  async function loadPrefs() {
    try {
      const r = await fetch('/api/prefs', { credentials: 'same-origin', cache: 'no-store' });
      if (!r.ok) return;
      const p = await r.json();
      if (p.theme) applyTheme(p.theme, true);
      if (p.nickname) localStorage.setItem('voxa-web-nick', p.nickname);
      localStorage.setItem('voxa-web-channel', p.channel || '');
    } catch (_) { /* keep the local ones */ }
  }
  function savePrefs(p) {
    if (S.connected) post('/api/prefs', p).catch(() => {});
  }

  // ---------------------------------------------------------------- live connection
  function connect() {
    if (es) es.close();
    es = new EventSource('/api/events' + (localStorage.getItem('voxa-web-presence') === 'Invisible' ? '?invisible=1' : ''), { withCredentials: true });
    es.onmessage = (e) => {
      let m;
      try { m = JSON.parse(e.data); } catch (_) { return; }
      const [kind, v] = typeof m === 'string' ? [m, null] : Object.entries(m)[0];
      handle(kind, v);
    };
    es.onerror = async () => {
      if (!S.connected) {
        if ((await signedIn()) === false) showLogin(T('Your sign-in expired. Please sign in again.'));
        return;
      }
      S.connected = false;
      banner(T('Connection lost, reconnecting…'));
      if ((await signedIn()) === false) showLogin(T('Your sign-in expired. Please sign in again.'));
    };
  }
  function banner(text) {
    let b = $('.banner');
    if (!text) { if (b) b.remove(); return; }
    if (!b) document.body.append((b = el('div', { class: 'banner' })));
    b.textContent = text;
  }
  function stopped(title, text) {
    if (es) { es.close(); es = null; }
    S.connected = false;
    dialog(title, el('p', null, text), [
      { label: T('Sign out'), onClick: signOut },
      { label: T('Reconnect'), primary: true, onClick: () => connect() },
    ], { noClose: true });
  }

  function pushEvent(key, text, kind) {
    log(key).lines.push({ event: text, kind: kind || '', ts: now() });
    if (key === tabKey()) S.msgDirty = true;
  }
  function pushMsg(key, m) {
    const l = log(key);
    if (l.lines.some((x) => x.msg && x.msg.id === m.id && m.id)) return;
    l.lines.push({ msg: m });
    if (l.lines.length > 1000) l.lines.splice(0, l.lines.length - 1000);
    if (key === tabKey()) S.msgDirty = true;
    else l.unread++;
  }
  function openDm(uid, name) {
    // Your own identity only when it is also connected elsewhere (e.g. the desktop client).
    if (uid === S.uid && !onlineByUid(uid)) return;
    let d = S.dms.find((x) => x.uid === uid);
    if (!d) S.dms.push((d = { uid, name: name || personName(uid) }));
    else if (name) d.name = name;
    log('dm:' + uid).loaded = true;
    return d;
  }

  function handle(kind, v) {
    switch (kind) {
      case 'Welcome': {
        S.games.clear();
        S.blocks.clear();
        S.serverIcon = null;
        S.transferLimits = null;
        S.me = v.your_id;
        S.uid = v.your_uid;
        S.server = v.server;
        S.channels = new Map(v.channels.map((c) => [c.id, c]));
        S.clients = new Map(v.clients.map((c) => [c.id, c]));
        S.groups = new Map(v.groups.map((g) => [g.id, g]));
        S.logs = new Map();
        S.presences = new Map();
        S.editing = null;
        S.account = null; S.accountPolicy = null; S.hidden = new Map(); S.accountResumed = false; S.accountPrompted = false;
        // A remembered browser signs in before anything else is asked for.
        { const tok = localStorage.getItem(accountKey()); if (tok) { S.accountResumed = true; send({ ResumeAccount: tok }); } }
        avatars.clear();
        S.connected = true;
        S.msgDirty = true;
        banner('');
        closeDialogs(true);
        buildApp();
        send({ ClientTransferLimits: personalLimits });
        loadPrefs();
        activateTab(S.tab);
        // Your status from last time (invisible was already asked for when joining).
        { const p = localStorage.getItem('voxa-web-presence'); if (p === 'Busy' || p === 'Away') setPresence(p); }
        break;
      }
      case 'ClientPresence':
        if (v.presence === 'Online') S.presences.delete(v.id); else S.presences.set(v.id, v.presence);
        break;
      case 'TransferLimits': S.transferLimits=v; break;
      case 'ServerIcon': S.serverIcon=v; break;
      case 'Playing': if(v.game)S.games.set(v.client,v.game);else S.games.delete(v.client);break;
      case 'DmBlocks': S.blocks=new Set(v);break;
      case 'Refused':
        stopped(T('Could not join the server'), v);
        break;
      case 'Kicked':
        stopped(T('Disconnected'), T('You ') + leaveText(v) + '.');
        break;
      case 'Error':
        if (/sign-in has expired/.test(v)) { localStorage.removeItem(accountKey()); if (S.account && S.account.limited && !S.accountDlg) accountDialog(); }
        if (S.accountDlg) S.accountDlg.error(T(v));
        toast(T(v), 'bad');
        break;
      case 'Notice':
        toast(T(v));
        break;
      case 'ServerUpdated':
        S.server = v;
        S.msgDirty = true;
        break;
      case 'ChannelCreated':
      case 'ChannelEdited':
        S.channels.set(v.id, v);
        break;
      case 'ChannelDeleted':
        S.channels.delete(v);
        if (S.selected.kind === 'channel' && S.selected.id === v) S.selected = { kind: 'server' };
        break;
      case 'ClientJoined':
        S.clients.set(v.id, v);
        if (v.id !== S.me) pushEvent('server', v.nickname + ' connected' + (v.platform === 'Web' ? ' (web)' : ''), 'good');
        if (v.id !== S.me && inChannel() && v.channel === myChannel()) tones(JOIN_TONES);
        break;
      case 'ServerOptions':
        S.options = v;
        break;
      case 'BannedHardware':
        S.hwBans = new Set(v);
        break;
      case 'ClientLeft': {
        const c = S.clients.get(v.id);
        if (c && v.id !== S.me && inChannel() && c.channel === myChannel()) tones(LEAVE_TONES);
        if (c) pushEvent('server', c.nickname + ' ' + leaveText(v.reason), v.reason.Kicked || v.reason.Banned ? 'bad' : '');
        S.clients.delete(v.id);
        S.games.delete(v.id);
        S.presences.delete(v.id);
        if (S.selected.kind === 'client' && S.selected.id === v.id) S.selected = { kind: 'server' };
        break;
      }
      case 'ClientMoved': {
        const c = S.clients.get(v.id);
        if (c) {
          const was = c.channel;
          c.channel = v.channel;
          if (v.id === S.me) {
            S.msgDirty = true;
            if (S.tab === 'channel') activateTab('channel');
            if (v.by) toast('You were moved by ' + v.by);
          } else if (inChannel() && (was === myChannel() || v.channel === myChannel())) {
            tones(v.channel === myChannel() ? JOIN_TONES : LEAVE_TONES);
            const ch = S.channels.get(v.channel);
            pushEvent('ch:' + myChannel(), c.nickname + (v.channel === myChannel() ? T(' joined the channel') : T(' went to ') + (ch ? ch.name : T('another channel'))) + (v.by ? ' (moved by ' + v.by + ')' : ''));
          }
        }
        break;
      }
      case 'ClientUpdated':
        S.clients.set(v.id, v);
        if (v.id === S.me) S.msgDirty = true;
        break;
      case 'Text': {
        const t = v.target;
        if (t.Private !== undefined) {
          const fromMe = v.from === S.me;
          const other = fromMe ? S.clients.get(t.Private) : null;
          const uid = fromMe ? other && other.uid : v.from_uid;
          if (!uid) break;
          openDm(uid, fromMe ? other.nickname : v.from_name);
          pushMsg('dm:' + uid, v);
        } else {
          const key = keyOf(t);
          if (key) pushMsg(key, v);
        }
        break;
      }
      case 'History': {
        const key = keyOf(v.target);
        if (!key) break;
        const l = log(key);
        const have = new Set(l.lines.filter((x) => x.msg).map((x) => x.msg.id));
        const add = v.messages.filter((m) => !have.has(m.id)).map((m) => ({ msg: m }));
        l.lines = add.concat(l.lines).sort((a, b) => (a.msg && b.msg ? a.msg.id - b.msg.id : (a.msg ? a.msg.ts : a.ts) - (b.msg ? b.msg.ts : b.ts)));
        l.complete = v.complete;
        l.loaded = true;
        if (key === tabKey()) S.msgDirty = true;
        break;
      }
      case 'MessageDeleted': {
        const key = keyOf(v.target);
        if (!key) break;
        const l = log(key);
        l.lines = l.lines.filter((x) => !(x.msg && x.msg.id === v.id));
        if (key === tabKey()) S.msgDirty = true;
        if (S.pins) S.pins = S.pins.filter((p) => p.message.id !== v.id);
        break;
      }
      case 'ChatCleared': {
        const key = keyOf(v.target);
        if (!key) break;
        log(key).lines = [];
        pushEvent(key, T('The chat history was cleared by ') + v.by, 'warn');
        S.msgDirty = true;
        break;
      }
      case 'Poked':
        toast(v.from_name + T(' poked you') + (v.message ? ': ' + v.message : ''), 'warn');
        break;
      case 'GroupsUpdated':
        S.groups = new Map(v.map((g) => [g.id, g]));
        S.msgDirty = true;
        break;
      case 'YourPerms':
        S.extra = v;
        S.msgDirty = true;
        break;
      case 'Pins':
        if (keyOf(v.target) === tabKey()) S.pins = v.pins;
        break;
      case 'PinChanged':
        if (keyOf(v.target) === tabKey() && S.showPins) send({ GetPins: v.target });
        pushEvent(keyOf(v.target), v.by + (v.pinned ? T(' pinned a message') : T(' unpinned a message')));
        break;
      case 'People':
        S.people = v;
        break;
      case 'AvatarInfo':
        for (const m of v) avatars.set(m.uid, m);
        S.msgDirty = true;
        break;
      case 'MessageEdited': {
        const key = keyOf(v.target);
        const line = key && log(key).lines.find((x) => x.msg && x.msg.id === v.id);
        if (line) { line.msg.text = v.text; line.msg.edited = v.edited_at; }
        if (S.pins) for (const p of S.pins) if (p.message.id === v.id) p.message.text = v.text;
        if (key === tabKey()) S.msgDirty = true;
        break;
      }
      case 'EditedMessages': {
        const key = keyOf(v.target);
        if (!key) break;
        const at = new Map(v.edits);
        for (const x of log(key).lines) if (x.msg && at.has(x.msg.id)) x.msg.edited = at.get(x.msg.id);
        if (key === tabKey()) S.msgDirty = true;
        break;
      }
      case 'OfflineMessages':
        for (const m of v) {
          openDm(m.from_uid, m.from_name);
          pushMsg('dm:' + m.from_uid, { id: 0, target: { Private: 0 }, from: null, from_uid: m.from_uid, from_name: m.from_name, ts: m.ts, text: m.text });
        }
        toast(v.length + T(' direct message(s) arrived while you were away'));
        break;
      case 'AccountPolicy':
        S.accountPolicy = v;
        break;
      case 'AccountState': {
        // Turning MFA on signs remembered devices out (they must sign in with it).
        if (S.account && !S.account.mfa && v.mfa) localStorage.removeItem(accountKey());
        S.account = v;
        const tok = localStorage.getItem(accountKey());
        // A remembered browser signs in by itself (once per connection).
        if (!v.signed_in && v.username && !S.accountResumed && tok) { S.accountResumed = true; send({ ResumeAccount: tok }); }
        else if (v.limited && !S.accountPrompted) { S.accountPrompted = true; accountDialog(); }
        if (v.signed_in) S.accountPrompted = true;
        // Signed in with an administrator's temporary password: choose a new one.
        if (v.signed_in && v.must_change && !S.accountDlg) accountDialog();
        if (S.accountDlg) S.accountDlg.redraw();
        S.msgDirty = true;
        break;
      }
      case 'AccountToken':
        localStorage.setItem(accountKey(), v);
        break;
      case 'AccountReset':
        dialog(T('Temporary password'), el('div', null,
          el('p', null, T('The password of the account "') + v.username + '" was reset. Give them this temporary password (it is not shown again):'),
          el('p', null, el('code', { style: 'font-size:18px;user-select:all' }, v.password)),
          el('p', { class: 'dim small' }, T('They sign in with their username and this password, then choose a new password.'))),
          [{ label: T('Copy'), onClick: () => { if (navigator.clipboard) navigator.clipboard.writeText(v.password); return false; } }, { label: T('Done'), primary: true }]);
        break;
      case 'MfaSetup':
        S.mfaSetup = v;
        if (S.accountDlg) S.accountDlg.redraw(); else accountDialog();
        break;
      case 'RecoveryCodes':
        S.mfaSetup = null;
        S.recoveryCodes = v;
        if (S.accountDlg) S.accountDlg.redraw(); else accountDialog();
        break;
      case 'ChannelsVisible':
        for (const id of v.hidden) {
          S.channels.delete(id);
          if (S.selected.kind === 'channel' && S.selected.id === id) S.selected = { kind: 'server' };
        }
        for (const c of v.shown) S.channels.set(c.id, c);
        break;
      case 'HiddenChannels':
        S.hidden = new Map(v);
        break;
      case 'OfflineQueued':
        pushEvent('dm:' + v.to_uid, personName(v.to_uid) + T(' is offline: the message will be delivered when they come online'));
        break;
      default:
        break;
    }
    const w = S.wait[kind];
    if (w) w(v);
    schedule();
  }

  // ---------------------------------------------------------------- sounds
  // Like the client: a rising pair when someone joins your channel, falling when they leave.
  const JOIN_TONES = [[523, 70], [784, 90]];
  const LEAVE_TONES = [[784, 70], [523, 90]];
  let audio = null;
  function tones(list) {
    if (localStorage.getItem('voxa-web-sounds') === 'off' || myPresence() === 'Busy') return;
    try {
      audio = audio || new AudioContext();
      let t = audio.currentTime + 0.01;
      for (const [f, ms] of list) {
        const o = audio.createOscillator(), g = audio.createGain();
        o.type = 'sine';
        o.frequency.value = f;
        g.gain.setValueAtTime(0.0001, t);
        g.gain.exponentialRampToValueAtTime(0.12, t + 0.01);
        g.gain.exponentialRampToValueAtTime(0.0001, t + ms / 1000);
        o.connect(g).connect(audio.destination);
        o.start(t);
        o.stop(t + ms / 1000 + 0.02);
        t += ms / 1000;
      }
    } catch (_) { /* no audio */ }
  }

  // ---------------------------------------------------------------- auto away
  // After the server's idle time without any input on the page you are set away, and
  // back when you return (only if Voxa set it).
  const AUTO_AWAY = T('Away (idle)');
  let lastInput = Date.now();
  for (const ev of ['mousemove', 'mousedown', 'keydown', 'wheel', 'touchstart']) {
    document.addEventListener(ev, () => {
      lastInput = Date.now();
      const me = S.clients.get(S.me);
      if (S.autoAway && me && me.away === AUTO_AWAY) send({ UpdateSelf: { away: null } });
      S.autoAway = false;
    }, { passive: true, capture: true });
  }
  setInterval(() => {
    const me = S.clients.get(S.me);
    const minutes = S.options ? S.options.auto_away_minutes : 0;
    if (!S.connected || !me || !minutes || me.away != null || S.autoAway) return;
    if (Date.now() - lastInput >= minutes * 60000) {
      S.autoAway = true;
      send({ UpdateSelf: { away: AUTO_AWAY } });
    }
  }, 15000);

  // ---------------------------------------------------------------- rendering
  let pending = false;
  function schedule() {
    if (pending) return;
    pending = true;
    // Normally on the next animation frame; hidden or covered windows get none, so a
    // timer makes sure the page still keeps up (whichever comes first).
    let done = false;
    const run = () => {
      if (done) return;
      done = true;
      pending = false;
      if (!S.connected || !$('.app')) return;
      render();
    };
    requestAnimationFrame(run);
    setTimeout(run, document.hidden ? 500 : 100);
  }

  let R = {}; // persistent elements of the layout
  function buildApp() {
    const search = el('input', { type: 'text', class: 'search', placeholder: T('🔍 Search people'), oninput: () => schedule() });
    const input = el('textarea', { rows: '1', placeholder: T('Message'), onkeydown: onComposerKey, oninput: autoGrow });
    R = {
      app: el('div', { class: 'app' }),
      rail: el('nav', { class: 'rail' }),
      sideHead: el('div', { class: 'side-head', title: T('Server menu'), onclick: (e) => serverMenu(e.clientX, e.clientY), oncontextmenu: (e) => { e.preventDefault(); serverMenu(e.clientX, e.clientY); } }),
      tree: el('div', { class: 'tree' }),
      status: el('div', { class: 'status-card' }),
      meCard: el('div', { class: 'me-card' }),
      head: el('div', { class: 'chat-head' }),
      tabs: el('div', { class: 'tabs' }),
      pins: el('div', { class: 'pins hidden' }),
      messages: el('div', { class: 'messages' }),
      input,
      enc: el('div', { class: 'enc' }),
      infoTabs: el('div', { class: 'info-tabs' }),
      search,
      infoBody: el('div', { class: 'info-body' }),
    };
    R.composer = el('div', { class: 'composer' }, input, el('button', { class: 'icon-btn send', title: T('Send (Enter)'), onclick: sendComposer }, '➤'));
    R.app.append(
      R.rail,
      el('aside', { class: 'side' }, R.sideHead, R.tree, R.status, R.meCard),
      el('main', { class: 'main' }, R.head, R.tabs, R.pins, R.messages, R.composer, R.enc),
      el('aside', { class: 'info' }, R.infoTabs, el('div', { style: 'padding:0 14px' }, search), R.infoBody));
    put(document.getElementById('root'), R.app);
    render();
  }

  function render() {
    R.app.classList.toggle('no-right', !S.showRight);
    renderRail();
    renderSide();
    renderChatHead();
    renderTabs();
    renderPins();
    if (S.msgDirty) renderMessages();
    renderComposer();
    renderInfo();
  }

  function renderRail() {
    const name = S.server ? S.server.name : '';
    put(R.rail, 
      el('div', { class: 'srv', title: name, onclick: () => { S.selected = { kind: 'server' }; S.right = 'info'; S.showRight = true; schedule(); } },
        S.serverIcon ? el('img',{src:'/api/server-icon?v='+encodeURIComponent(S.serverIcon),alt:name,onerror:e=>e.target.replaceWith(initials(name))}) : initials(name)),
      el('div', { class: 'sep' }),
      el('div', { class: 'grow' }),
      el('button', { class: 'icon-btn', title: T('Join / leave sounds (click to turn ') + (localStorage.getItem('voxa-web-sounds') === 'off' ? 'on' : 'off') + ')', onclick: () => { localStorage.setItem('voxa-web-sounds', localStorage.getItem('voxa-web-sounds') === 'off' ? 'on' : 'off'); schedule(); } }, localStorage.getItem('voxa-web-sounds') === 'off' ? '🔕' : '🔔'),
      el('button', { class: 'icon-btn', title: T('Language'), onclick: languageDialog }, '🌐'),
      el('button', {class:'icon-btn',title:T('Settings'),onclick:personalOptions},'⚙'),
      el('button', { class: 'icon-btn', title: T('Theme'), onclick: (e) => themeMenu(e.clientX + 10, e.clientY - 250) }, '🎨'),
      el('button', { class: 'icon-btn', title: T('Sign out'), onclick: () => confirmDialog(T('Sign out'), T('Sign out of the web admin page?'), T('Sign out')).then((ok) => ok && signOut()) }, '⏻'));
  }

  function renderSide() {
    put(R.sideHead, el('span', { class: 'name' }, S.server ? S.server.name : ''), el('span', { class: 'dim' }, '▾'));
    if (!S.dragging) renderTree();
    const online = S.clients.size;
    put(R.status, 
      el('span', { style: 'color:var(--green);font-size:16px' }, '🌐'),
      el('div', { class: 'grow' }, el('div', { class: 't' }, T('Web admin connected')), el('div', { class: 'dim small' }, online + ' online · ' + location.host)));
    const me = S.clients.get(S.me);
    if (!me) return;
    const away = me.away != null;
    const pr = PRESENCES.find((p) => p[0] === myPresence()) || PRESENCES[0];
    const status = away ? awayText(me.away) : pr[2];
    put(R.meCard, 
      avatar(me.nickname, me.uid, 36, pr[1]),
      el('div', { class: 'who', style: 'cursor:pointer', title: T('Set your status'), onclick: (e) => presenceMenu(e.clientX, e.clientY - 220) },
        el('div', { class: 'n' }, me.nickname),
        el('div', { class: 'small', style: 'color:' + pr[1] }, '● ' + status + (inChannel() ? '' : ' · not in a channel'))),
      el('button', { class: 'icon-btn' + (away ? ' on' : ''), title: away ? T('Set online') : T('Set away'), onclick: toggleAway }, '🌙'),
      el('button', { class: 'icon-btn', title: T('Change nickname'), onclick: changeNickname }, '✏️'));
  }

  function children(parent) {
    return [...S.channels.values()].filter((c) => (c.parent ?? null) === parent).sort((a, b) => a.sort - b.sort || a.id - b.id);
  }
  function clientsIn(ch) {
    return [...S.clients.values()].filter((c) => c.channel === ch).sort((a, b) => (b.commander - a.commander) || a.nickname.localeCompare(b.nickname));
  }
  function subtree(id) {
    const out = [id];
    for (const c of children(id)) out.push(...subtree(c.id));
    return out;
  }

  function renderTree() {
    const out = [];
    const walk = (parent, depth) => {
      for (const ch of children(parent)) {
        out.push(channelRow(ch, depth));
        if (S.collapsed.has(ch.id)) continue;
        for (const c of clientsIn(ch.id)) out.push(clientRow(c, depth + 1));
        walk(ch.id, depth + 1);
      }
    };
    // People in no channel are listed under People, not in the tree.
    walk(null, 0);
    put(R.tree, ...out);
  }

  function dropTarget(row, ch) {
    row.addEventListener('dragover', (e) => {
      if (!e.dataTransfer.types.includes('text/voxa-client')) return;
      e.preventDefault();
      row.classList.add('drop');
    });
    row.addEventListener('dragleave', () => row.classList.remove('drop'));
    row.addEventListener('drop', (e) => {
      e.preventDefault();
      row.classList.remove('drop');
      const id = Number(e.dataTransfer.getData('text/voxa-client'));
      if (!id) return;
      if (id === S.me) send({ SwitchChannel: { channel: ch.id, password: null } });
      else send({ MoveClient: { target: id, channel: ch.id } });
    });
  }

  function channelRow(ch, depth) {
    const sp = spacer(ch.name);
    const indent = 'padding-left:' + (6 + depth * 14) + 'px';
    const sel = S.selected.kind === 'channel' && S.selected.id === ch.id;
    const common = {
      onclick: () => select({ kind: 'channel', id: ch.id }),
      oncontextmenu: (e) => { e.preventDefault(); select({ kind: 'channel', id: ch.id }); channelMenu(ch, e.clientX, e.clientY); },
    };
    if (sp) {
      let content;
      if (sp.align === '*') {
        const unit = sp.text || '';
        content = unit ? el('span', { style: 'flex:1;overflow:hidden' }, unit.repeat(Math.ceil(200 / Math.max(1, unit.length)))) : el('span', { class: 'line' });
      } else {
        const t = el('span', null, sp.text);
        content = sp.align === 'c' ? [el('span', { class: 'line' }), t, el('span', { class: 'line' })] : sp.align === 'r' ? [el('span', { class: 'line' }), t] : [t, el('span', { class: 'line' })];
      }
      return el('div', { class: 'spacer-row' + (sp.align === '*' ? ' repeat' : '') + (sel ? ' sel' : ''), style: indent, title: ch.name, ...common }, content);
    }
    const kids = children(ch.id).length + clientsIn(ch.id).length;
    const n = clientsIn(ch.id).length;
    const full = ch.max_clients != null && n >= ch.max_clients;
    const mine = ch.id === myChannel();
    const row = el('div', {
      class: 'row channel' + (sel ? ' sel' : '') + (mine ? ' mine' : ''), style: indent,
      ondblclick: () => { if (!mine) joinChannel(ch); },
      title: ch.topic || '',
      ...common,
    },
      el('span', { class: 'arrow', onclick: (e) => { e.stopPropagation(); S.collapsed.has(ch.id) ? S.collapsed.delete(ch.id) : S.collapsed.add(ch.id); schedule(); } }, kids ? (S.collapsed.has(ch.id) ? '›' : '⌄') : ''),
      el('span', { class: 'ico', style: full ? 'color:var(--red)' : mine ? 'color:var(--green)' : '' }, ch.has_password ? '🔒' : '🔊'),
      el('span', { class: 'label' }, ch.name),
      el('span', { class: 'badges' },
        ch.is_default ? el('span', { title: T('Default channel') }, '🏠') : null,
        ch.kind === 'Temporary' ? el('span', { title: T('Temporary channel') }, '⏳') : null,
        ch.max_clients != null ? el('span', { title: T('Max clients') }, n + '/' + ch.max_clients) : n && S.collapsed.has(ch.id) ? el('span', null, String(n)) : null));
    dropTarget(row, ch);
    return row;
  }

  function clientRow(c, depth) {
    const me = c.id === S.me;
    const sel = S.selected.kind === 'client' && S.selected.id === c.id;
    const tags = c.server_groups.concat([c.channel_group]).map((g) => S.groups.get(g)).filter((g) => g && g.show_name).map((g) => g.name);
    const row = el('div', {
      class: 'row client' + (sel ? ' sel' : '') + (me ? ' me' : '') + (c.away != null ? ' away' : ''), style: 'padding-left:' + (8 + depth * 14) + 'px',
      draggable: me || can(P.CLIENT_MOVE) ? 'true' : null,
      onclick: () => select({ kind: 'client', id: c.id }),
      ondblclick: () => { if (!me) { openDm(c.uid, c.nickname); activateTab('dm:' + c.uid); } },
      oncontextmenu: (e) => { e.preventDefault(); select({ kind: 'client', id: c.id }); clientMenu(c, e.clientX, e.clientY); },
      ondragstart: (e) => { S.dragging = true; e.dataTransfer.setData('text/voxa-client', String(c.id)); e.dataTransfer.effectAllowed = 'move'; },
      ondragend: () => { S.dragging = false; schedule(); },
    },
      avatar(c.nickname, c.uid, 22),
      el('span', { class: 'label' }, el('span', { class: 'nick' }, c.nickname),
        playing(c.id) ? el('span', { class: 'dim small', title: 'Playing ' + playing(c.id) }, '🎮 ' + playing(c.id)) : null,
        tags.map((t) => { const col = nameColor(t); return el('span', { class: 'gtag', style: `color:${col};background:${col}40` }, t); })),
      el('span', { class: 'badges' },
        c.platform === 'Web' ? el('span', { class: 'webtag', title: T('On the web admin page') }, T('WEB')) : null,
        S.presences.get(c.id) === 'Busy' ? el('span', { title: T('Do not disturb'), style: 'color:var(--red)' }, '⛔') : null,
        S.presences.get(c.id) === 'Invisible' ? el('span', { title: 'Invisible: others see them as offline', style: 'color:var(--dim)' }, '👻') : null,
        c.streaming ? el('span', { class: 'live', title: c.streaming.title + ' (' + c.streaming.width + 'x' + c.streaming.height + ')' }, T('LIVE')) : null,
        c.output_muted ? el('span', { title: T('Speakers muted'), style: 'color:var(--red)' }, '🔇') : null,
        c.input_muted ? el('span', { title: T('Microphone muted'), style: 'color:var(--red)' }, '🎤') : null,
        c.talk_request != null ? el('span', { title: T('Requests talk power: ') + (c.talk_request || '') }, '✋') : null,
        c.commander ? el('span', { title: T('Channel commander'), style: 'color:var(--yellow)' }, '★') : null,
        c.away != null ? el('span', { title: c.away ? T('Away: ') + c.away : T('Away'), style: 'color:var(--yellow)' }, '🌙') : null));
    return row;
  }

  function select(sel) {
    S.selected = sel;
    S.right = 'info';
    schedule();
  }

  // ---------------------------------------------------------------- chat
  function activateTab(tab) {
    // Not in a channel: there is no channel chat.
    if (tab === 'channel' && !inChannel()) tab = 'server';
    S.tab = tab;
    S.pins = null;
    const key = tabKey();
    const l = log(key);
    l.unread = 0;
    S.msgDirty = true;
    // The Server tab's history only for people who may see server events (asked for
    // again when the tab is opened later, e.g. once the permission arrived).
    const target = (tabTarget() === 'Server' && !can(P.SERVER_EVENTS)) ? null : tabTarget();
    if (target && !l.loaded) {
      l.loaded = true;
      if (can(P.CHAT_HISTORY)) send({ FetchHistory: { target, before: null, limit: 50 } });
      else l.complete = true;
    }
    if (S.showPins && target) send({ GetPins: target });
    schedule();
    setTimeout(() => R.input && R.input.focus(), 0);
  }

  function renderChatHead() {
    let title, sub;
    if (S.tab === 'server') { title = '🖥  ' + (S.server ? S.server.name : T('Server')); sub = can(P.SERVER_EVENTS) ? T('Server events: who connects and leaves') : T('Welcome'); }
    else if (S.tab === 'channel') { const ch = S.channels.get(myChannel()); title = '#  ' + (ch ? ch.name : ''); sub = ch && ch.topic ? ch.topic : T('Channel chat'); }
    else { const uid = S.tab.slice(3); title = '@  ' + personName(uid); sub = onlineByUid(uid) ? T('Direct message') : T('Offline · gets your messages later'); }
    const isDm = S.tab.startsWith('dm:');
    const peopleOn = S.showRight && S.right === 'people';
    put(R.head, 
      el('div', { class: 't' }, el('div', { class: 'title' }, title), el('div', { class: 'subtitle' }, sub)),
      !isDm && !(S.tab === 'server' && !can(P.SERVER_EVENTS)) ? el('button', { class: 'icon-btn' + (S.showPins ? ' on' : ''), title: T('Pinned messages'), onclick: togglePins }, '📌') : null,
      !isDm && can(P.CHAT_CLEAR) ? el('button', { class: 'icon-btn', title: T('Clear the entire chat history'), onclick: clearChat }, '🗑') : null,
      isDm ? el('button', { class: 'icon-btn', title: T('Close this conversation'), onclick: () => closeDm(S.tab.slice(3)) }, '✕') : null,
      isDm ? blockButton(S.tab.slice(3)) : null,
      el('button', { class: 'icon-btn' + (S.showRight ? ' on' : ''), title: T('Show / hide info panel'), onclick: () => { S.showRight = !S.showRight; schedule(); } }, 'ℹ'),
      el('button', { class: 'pill' + (peopleOn ? ' on' : ''), title: T('Everyone on this server, online and offline'), onclick: () => { S.showRight = !peopleOn; S.right = 'people'; schedule(); } }, T('👥 People')));
  }

  function renderTabs() {
    const tab = (id, label, unread, close) => el('button', { class: 'tab' + (S.tab === id ? ' on' : ''), onclick: () => activateTab(id) },
      label, unread ? el('span', { class: 'unread' }, unread) : null,
      close ? el('span', { class: 'x', title: T('Close'), onclick: (e) => { e.stopPropagation(); closeDm(close); } }, '✕') : null);
    const ch = S.channels.get(myChannel());
    put(R.tabs, 
      tab('server', T('🖥 Server'), S.tab === 'server' ? 0 : log('server').unread),
      inChannel() ? tab('channel', '# ' + (ch ? ch.name : T('Channel')), S.tab === 'channel' ? 0 : log('ch:' + myChannel()).unread) : null,
      S.dms.map((d) => tab('dm:' + d.uid, '@ ' + personName(d.uid) + (onlineByUid(d.uid) ? '' : ' (offline)'), S.tab === 'dm:' + d.uid ? 0 : log('dm:' + d.uid).unread, d.uid)));
  }

  function closeDm(uid) {
    S.dms = S.dms.filter((d) => d.uid !== uid);
    if (S.tab === 'dm:' + uid) activateTab('channel');
    schedule();
  }

  function togglePins() {
    S.showPins = !S.showPins;
    S.pins = null;
    const t = tabTarget();
    if (S.showPins && t) send({ GetPins: t });
    schedule();
  }
  function renderPins() {
    const show = S.showPins && !S.tab.startsWith('dm:');
    R.pins.classList.toggle('hidden', !show);
    if (!show) return;
    if (!S.pins) { put(R.pins, el('span', { class: 'dim' }, T('Loading pinned messages…'))); return; }
    if (!S.pins.length) { put(R.pins, el('span', { class: 'dim' }, T('📌 No pinned messages in this chat.'))); return; }
    put(R.pins, ...S.pins.map((p) => el('div', { class: 'pin' },
      el('span', null, '📌'),
      el('div', { style: 'flex:1;min-width:0' }, el('div', null, el('span', { class: 'strong' }, p.message.from_name), el('span', { class: 'dim small' }, '  ' + fmtDate(p.message.ts) + ' · pinned by ' + p.pinned_by)), el('div', { class: 'desc' }, messageText(p.message.text))),
      can(P.CHAT_PIN) ? el('button', { class: 'btn small', onclick: () => send({ PinMessage: { target: tabTarget(), id: p.message.id, pin: false } }) }, T('Unpin')) : null)));
  }

  function messageText(text) {
    if (/^🎥 Video message\n\[voxa-clip:\d+\]$/.test(text)) return el('span', { class: 'dim' }, '🎥 ' + T('Video message') + ' · play it in the Voxa app');
    const img = IMAGE_RE.exec(text);
    if (img) {
      const src = '/api/image/' + img[2];
      return el('span', null, '📷 ' + img[1], el('img', { src, alt: img[1], loading: 'lazy', width: img[3], height: img[4], style: 'width:auto;height:auto', onclick: () => lightbox(src) }));
    }
    const parts = [];
    let last = 0;
    const re = /https?:\/\/[^\s<>"]+/g;
    let m;
    while ((m = re.exec(text))) {
      parts.push(text.slice(last, m.index));
      parts.push(el('a', { href: m[0], target: '_blank', rel: 'noopener noreferrer' }, m[0]));
      last = m.index + m[0].length;
    }
    parts.push(text.slice(last));
    return parts;
  }
  function lightbox(src) {
    const b = el('div', { class: 'lightbox', onclick: () => b.remove() }, el('img', { src }));
    document.body.append(b);
  }

  function renderMessages() {
    S.msgDirty = false;
    const box = R.messages;
    const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
    const key = tabKey();
    const l = log(key);
    const target = tabTarget();
    const isDm = S.tab.startsWith('dm:');
    const out = [];
    // Without "See server events" the Server tab shows the welcome message.
    if (S.tab === 'server' && !can(P.SERVER_EVENTS)) {
      const welcome = (S.server && S.server.welcome_message) || '';
      put(box, el('div', { class: 'welcome-tab' },
        el('h2', null, T('Welcome to ') + (S.server ? S.server.name : T('the server'))),
        el('div', { class: 'welcome-text' }, welcome.trim() ? welcome : T('The server has no welcome message.')),
        el('div', { class: 'dim small' }, T('Chat in a channel (double-click one to join it), or with direct messages.'))));
      return;
    }
    // The permission arrived while the Server tab was open: load its history now.
    if (S.tab === 'server' && target && !l.loaded) {
      l.loaded = true;
      if (can(P.CHAT_HISTORY)) send({ FetchHistory: { target, before: null, limit: 50 } });
      else l.complete = true;
    }
    if (target && l.loaded && !l.complete && l.lines.some((x) => x.msg)) {
      out.push(el('div', { class: 'older' }, el('button', { class: 'btn small', onclick: () => {
        const oldest = l.lines.find((x) => x.msg);
        send({ FetchHistory: { target, before: oldest ? oldest.msg.id : null, limit: 50 } });
      } }, T('Load older messages'))));
    }
    if (!l.lines.length) {
      out.push(el('div', { class: 'event', style: 'padding-left:6px' }, isDm ? T('This is the start of your conversation. On the web page direct messages are not kept after you close it.') : T('No messages yet.')));
    }
    let prev = null, prevDay = '';
    for (const line of l.lines) {
      const ts = line.msg ? line.msg.ts : line.ts;
      const day = new Date(ts * 1000).toDateString();
      if (day !== prevDay) {
        out.push(el('div', { class: 'day' }, new Date(ts * 1000).toLocaleDateString(undefined, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' })));
        prevDay = day;
        prev = null;
      }
      if (line.event) {
        out.push(el('div', { class: 'event ' + line.kind }, fmtTime(line.ts) + '  ' + line.event));
        prev = null;
        continue;
      }
      const m = line.msg;
      const cont = prev && prev.from_uid === m.from_uid && m.ts - prev.ts < 300;
      const mine = m.from_uid === S.uid;
      const canDel = isDm || mine || can(P.CHAT_DELETE);
      const canEdit = !isDm && mine && m.id && !IMAGE_RE.test(m.text) && can(P.CHAT_EDIT_OWN);
      const editing = S.editing && S.editing.key === key && S.editing.id === m.id;
      const actions = el('div', { class: 'actions' },
        canEdit && !editing ? el('button', { class: 'icon-btn', title: T('Edit message'), onclick: () => startEdit(key, m) }, '✏️') : null,
        el('button', { class: 'icon-btn', title: T('Copy text'), onclick: () => copy(m.text) }, '📋'),
        !isDm && can(P.CHAT_PIN) && m.id ? el('button', { class: 'icon-btn', title: T('Pin'), onclick: () => send({ PinMessage: { target, id: m.id, pin: true } }) }, '📌') : null,
        canDel ? el('button', { class: 'icon-btn', title: isDm ? T('Remove from this page') : T('Delete message'), style: 'color:var(--red)', onclick: () => deleteMessage(m) }, '🗑') : null);
      out.push(el('div', { class: 'msg' + (cont ? ' cont' : '') },
        avatar(m.from_name, m.from_uid, 36),
        el('div', { class: 'body' },
          cont ? null : el('div', { class: 'head' }, el('span', { class: 'from' }, m.from_name), el('span', { class: 'time' }, fmtTime(m.ts))),
          editing ? [S.editing.box, el('div', { class: 'edit-hint' }, T('Enter to save · Esc to cancel · Shift+Enter for a new line'))]
            : el('div', { class: 'text', title: fmtDate(m.ts) }, messageText(m.text), m.edited ? el('span', { class: 'edited', title: T('Edited ') + fmtDate(m.edited) }, '(edited)') : null)),
        actions));
      prev = m;
    }
    const editFocused = S.editing && document.activeElement === S.editing.box;
    put(box, ...out);
    if (editFocused) S.editing.box.focus();
    if (atBottom || box.dataset.key !== key) box.scrollTop = box.scrollHeight;
    box.dataset.key = key;
  }

  /// Edits one of your own messages in place (servers 1.15.0+).
  function startEdit(key, m) {
    const box = el('textarea', { class: 'edit-box', rows: String(Math.min(8, m.text.split('\n').length + 1)), value: m.text, maxlength: '8000' });
    const target = tabTarget();
    const stop = () => { S.editing = null; S.msgDirty = true; schedule(); };
    box.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') { e.preventDefault(); stop(); }
      else if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        const text = box.value.trim();
        if (!text) { toast('A message cannot be empty: delete it instead', 'warn'); return; }
        if (text !== m.text) send({ EditMessage: { target, id: m.id, text } });
        stop();
      }
    });
    S.editing = { key, id: m.id, box };
    S.msgDirty = true;
    schedule();
    setTimeout(() => { box.focus(); box.setSelectionRange(box.value.length, box.value.length); }, 0);
  }

  function deleteMessage(m) {
    if (S.tab.startsWith('dm:')) {
      const l = log(tabKey());
      l.lines = l.lines.filter((x) => x.msg !== m);
      S.msgDirty = true;
      schedule();
      return;
    }
    confirmDialog(T('Delete message'), T('Delete this message for everyone? It is also removed from the stored history.'), 'Delete', true)
      .then((ok) => ok && send({ DeleteMessage: { target: tabTarget(), id: m.id } }));
  }
  function clearChat() {
    const target = tabTarget();
    confirmDialog(T('Clear chat history'), T('This removes all messages for everyone and permanently deletes them from the server\'s chat history. It cannot be undone.'), T('Clear history'), true)
      .then((ok) => ok && send({ ClearChat: target }));
  }

  function renderComposer() {
    let placeholder, allowed = true, note;
    // The Server tab shows what happens on the server (connects, leaves): no messages there.
    R.composer.classList.toggle('hidden', S.tab === 'server');
    if (S.tab === 'server') { placeholder = ''; allowed = false; note = can(P.SERVER_EVENTS) ? T('ℹ This tab shows what happens on the server. Chat in a channel, or with direct messages.') : ''; }
    else if (S.tab === 'channel') { const ch = S.channels.get(myChannel()); placeholder = T('Message #') + (ch ? ch.name : ''); allowed = can(P.CHANNEL_CHAT); note = T('🔒 Messages are encrypted in transit and stored encrypted on the server'); }
    else { placeholder = T('Message @') + personName(S.tab.slice(3)); allowed = can(P.PRIVATE_MESSAGE) && !S.blocks.has(S.tab.slice(3)); note = T('🔒 Encrypted in transit · direct messages are not kept by the web page'); }
    if (!allowed) placeholder = T('You do not have permission to write here');
    R.input.placeholder = placeholder + (allowed ? '  ·  Shift+Enter for a new line' : '');
    R.input.disabled = !allowed;
    R.enc.textContent = note;
  }
  function autoGrow() {
    R.input.style.height = 'auto';
    R.input.style.height = Math.min(160, R.input.scrollHeight) + 'px';
  }
  function onComposerKey(e) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      sendComposer();
    }
  }
  function sendComposer() {
    const text = R.input.value.trim();
    if (!text) return;
    R.input.value = '';
    autoGrow();
    if (S.tab === 'server') return;
    else if (S.tab === 'channel') send({ SendText: { target: { Channel: myChannel() }, text } });
    else {
      const uid = S.tab.slice(3);
      const c = onlineByUid(uid);
      if (c) send({ SendText: { target: { Private: c.id }, text } });
      else {
        const me = S.clients.get(S.me);
        pushMsg('dm:' + uid, { id: 0, target: { Private: 0 }, from: S.me, from_uid: S.uid, from_name: me ? me.nickname : '', ts: now(), text });
        send({ SendOffline: { to_uid: uid, text } });
        schedule();
      }
    }
  }

  function blockDm(uid) { send({ BlockDm: { uid, block: !S.blocks.has(uid) } }); }
  function blockButton(uid) {
    const blocked = S.blocks.has(uid);
    return el('button', { class: 'icon-btn', title: blocked ? T('Unblock DMs') : T('Block DMs'), onclick: () => blockDm(uid) }, blocked ? '🔓' : '🚫');
  }

  // ---------------------------------------------------------------- info panel
  function renderInfo() {
    if (!S.showRight) return;
    put(R.infoTabs, 
      el('button', { class: 'pill' + (S.right === 'info' ? ' on' : ''), onclick: () => { S.right = 'info'; schedule(); } }, T('ℹ Info')),
      el('button', { class: 'pill' + (S.right === 'people' ? ' on' : ''), onclick: () => { S.right = 'people'; schedule(); } }, T('👥 People')));
    R.search.classList.toggle('hidden', S.right !== 'people');
    put(R.infoBody, ...(S.right === 'people' ? peopleView() : infoView()));
  }

  const kv = (rows) => el('div', { class: 'kv' }, rows.filter(Boolean).map(([k, v, mono]) => [el('div', { class: 'k' }, k), el('div', { class: 'v' + (mono ? ' mono' : '') }, v)]));

  function infoView() {
    const sel = S.selected;
    if (sel.kind === 'channel' && S.channels.has(sel.id)) return channelInfo(S.channels.get(sel.id));
    if (sel.kind === 'client' && S.clients.has(sel.id)) return clientInfo(S.clients.get(sel.id));
    const s = S.server;
    if (!s) return [];
    return [
      el('div', { class: 'info-hero' }, avatar(s.name, s.fingerprint, 52, null, true), el('div', null, el('div', { class: 'n' }, s.name), el('div', { class: 'dim small' }, location.host))),
      el('div', { class: 'section' }, T('Server')),
      kv([['Version', T('Voxa Server ') + s.version], ['Platform', s.platform], ['Online', S.clients.size + ' / ' + s.max_clients], ['Channels', String(S.channels.size)], ['Uptime', fmtDuration(now() - s.started)], ['Created', fmtDate(s.created)]]),
      el('div', { class: 'section' }, T('Security')),
      kv([['Fingerprint', s.fingerprint, true], ['Password', s.has_password ? T('Yes') : T('No')], ['Chat history', s.chat_retention_days ? T('Encrypted on server, kept ') + s.chat_retention_days + ' days' : T('Encrypted on server')], ['Voice', 'Encrypted']]),
      s.welcome_message ? [el('div', { class: 'section' }, T('Welcome message')), el('div', { class: 'desc' }, s.welcome_message)] : null,
      s.host_message ? [el('div', { class: 'section' }, T('Host message')), el('div', { class: 'desc' }, s.host_message)] : null,
      can(P.SERVER_EDIT) ? statsView() : null,
      can(P.SERVER_EDIT) ? el('div', { class: 'actions-row' }, el('button', { class: 'btn', onclick: editServer }, T('⚙ Edit server'))) : null,
    ].flat().filter(Boolean);
  }

  // Server statistics (administrators): traffic since the server started, refreshed every 5 s.
  let stats = null, statsAt = 0;
  function fmtSize(n) {
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i ? n.toFixed(1) : String(n)) + ' ' + u[i];
  }
  function statsView() {
    if (Date.now() - statsAt > 5000) {
      statsAt = Date.now();
      fetch('/api/stats', { credentials: 'same-origin', cache: 'no-store' }).then((r) => (r.ok ? r.json() : null)).then((j) => { if (j) { stats = j; schedule(); } }).catch(() => {});
    }
    if (!stats) return null;
    const t = stats.traffic;
    const row = (k, a, b) => [el('div', { class: 'k' }, k), el('div', { class: 'v' }, fmtSize(a)), el('div', { class: 'v' }, fmtSize(b))];
    const totalIn = t.voice_in + t.files_in + t.screen_in + t.chat_in, totalOut = t.voice_out + t.files_out + t.screen_out + t.chat_out;
    return [
      el('div', { class: 'section' }, T('Traffic since the server started')),
      el('div', { class: 'stats' },
        el('div'), el('div', { class: 'k' }, T('⬇ Received')), el('div', { class: 'k' }, T('⬆ Sent')),
        row(T('Voice'), t.voice_in, t.voice_out), row(T('File transfers'), t.files_in, t.files_out), row(T('Screen sharing & video'), t.screen_in, t.screen_out), row(T('Chat & other'), t.chat_in, t.chat_out),
        [el('div', { class: 'k strong' }, T('Total')), el('div', { class: 'v strong' }, fmtSize(totalIn)), el('div', { class: 'v strong' }, fmtSize(totalOut))]),
      el('div', { class: 'dim small' }, T('All traffic combined: ') + fmtSize(totalIn + totalOut)),
    ];
  }

  function channelInfo(ch) {
    const sp = spacer(ch.name);
    const n = clientsIn(ch.id).length;
    const codec = codecOf(ch.codec_quality);
    return [
      el('div', { class: 'info-hero' }, el('span', { class: 'avatar sq', style: 'width:52px;height:52px;font-size:24px;background:var(--card);color:var(--dim)' }, sp ? '―' : ch.has_password ? '🔒' : '💬'),
        el('div', null, el('div', { class: 'n' }, sp ? T('Separator') : ch.name), el('div', { class: 'dim small' }, sp ? ch.name : ch.topic || ''))),
      el('div', { class: 'section' }, T('Channel')),
      kv(sp ? [['Type', T('Separator (nobody can join it)')]] : [
        ['Type', KIND_NAMES[ch.kind] + (ch.is_default ? ' · default channel' : '')],
        ['Codec', CODEC_NAMES[codec] + (codec === 'lossless' ? '' : ' · quality ' + qualityOf(ch.codec_quality))],
        ['Bitrate', '~' + bitrate(ch.codec_quality) + ' kbit/s' + (codec === 'voice' ? '' : ' stereo')],
        ['Clients', n + (ch.max_clients != null ? ' / ' + ch.max_clients : '')],
        ['Password', ch.has_password ? T('Yes') : T('No')],
        ['Talk power', ch.needed_talk_power ? 'needs ' + ch.needed_talk_power : 'anyone'],
        ['Join power', ch.needed_join_power ? 'needs ' + ch.needed_join_power : 'anyone'],
        ['Voice', ch.voice_encrypted ? T('Encrypted') : T('Not encrypted')],
      ]),
      ch.description ? [el('div', { class: 'section' }, T('Description')), el('div', { class: 'desc' }, ch.description)] : null,
      el('div', { class: 'actions-row' },
        !sp && ch.id !== myChannel() ? el('button', { class: 'btn', onclick: () => joinChannel(ch) }, T('➜ Join')) : null,
        ch.id === myChannel() ? el('button', { class: 'btn', title: T('Stay connected to the server without being in a channel'), onclick: leaveChannel }, T('🚪 Leave')) : null,
        can(P.CHANNEL_EDIT) ? el('button', { class: 'btn', onclick: () => channelDialog(ch) }, T('✏️ Edit')) : null,
        can(P.CHANNEL_DELETE) ? el('button', { class: 'btn danger', onclick: () => deleteChannel(ch) }, T('🗑 Delete')) : null),
    ].flat().filter(Boolean);
  }

  function clientInfo(c) {
    const ch = S.channels.get(c.channel);
    const groups = c.server_groups.map((g) => S.groups.get(g)?.name).filter(Boolean).join(', ');
    const away = c.away != null;
    return [
      el('div', { class: 'info-hero' }, avatar(c.nickname, c.uid, 52, away ? 'var(--yellow)' : 'var(--green)'),
        el('div', null, el('div', { class: 'n' }, c.nickname), el('div', { class: 'small', style: 'color:' + (away ? 'var(--yellow)' : 'var(--green)') }, away ? awayText(c.away) : T('Online')))),
      el('div', { class: 'section' }, T('Client')),
      kv([
        ['Channel', ch ? ch.name : ''],
        ['Server groups', groups || '—'],
        ['Channel group', S.groups.get(c.channel_group)?.name || '—'],
        ['Platform', c.platform === 'Web' ? T('Web admin page') : c.platform],
        c.platform === 'Web' ? null : ['Version', T('Voxa ') + c.version],
        ['Connected', fmtDuration(now() - c.connected_at)],
        ['Microphone', c.input_muted ? T('Muted') : T('On')],
        ['Speakers', c.output_muted ? T('Muted') : T('On')],
        c.streaming ? ['Sharing', c.streaming.title + ' (' + c.streaming.width + '×' + c.streaming.height + ')'] : null,
        playing(c.id) ? ['Playing', playing(c.id)] : null,
        ['Unique ID', c.uid, true],
      ]),
      c.description ? [el('div', { class: 'section' }, T('Description')), el('div', { class: 'desc' }, c.description)] : null,
      c.id !== S.me ? el('div', { class: 'actions-row' },
        el('button', { class: 'btn', onclick: () => { openDm(c.uid, c.nickname); activateTab('dm:' + c.uid); } }, T('💬 Message')),
        can(P.CLIENT_POKE) ? el('button', { class: 'btn', onclick: () => pokeClient(c) }, T('👉 Poke')) : null,
        el('button', { class: 'btn', onclick: (e) => clientMenu(c, e.clientX, e.clientY) }, T('⋯ More'))) : null,
    ].flat().filter(Boolean);
  }

  function peopleView() {
    if (now() - S.peopleAsked > 30) {
      S.peopleAsked = now();
      send('GetPeople');
    }
    const q = R.search.value.trim().toLowerCase();
    const match = (n) => !q || (n || '').toLowerCase().includes(q);
    const seen = new Set();
    const online = [...S.clients.values()].filter((c) => c.uid !== S.uid && match(c.nickname)).sort((a, b) => a.nickname.localeCompare(b.nickname)).filter((c) => (seen.has(c.uid) ? false : seen.add(c.uid)));
    const onlineUids = new Set([...S.clients.values()].map((c) => c.uid));
    const offline = S.people.filter((p) => !onlineUids.has(p.uid) && p.uid !== S.uid && match(p.nickname)).sort((a, b) => b.last_seen - a.last_seen);
    // Right-click: message, or (offline, with "Create, edit and delete groups") forget them.
    const personMenu = (uid, name, online, x, y) => menu(x, y, [
      { head: name },
      { label: T('💬  Send Message'), onClick: () => { openDm(uid, name); activateTab('dm:' + uid); } },
      {label:S.blocks.has(uid)?T('Unblock DMs'):T('Block DMs'),onClick:()=>blockDm(uid)},
      !online && can(P.GROUP_MANAGE) ? { label: T('🗑  Remove from Server…'), onClick: () => forgetPerson(uid, name) } : null,
      S.accountPolicy && can(P.SERVER_EDIT) && uid !== S.uid ? { label: T('🔑  Reset Account Password…'), onClick: () => resetAccount(uid, name) } : null,
    ]);
    const row = (uid, name, sub, dot) => el('div', { class: 'person' + (dot ? '' : ' off'), title: dot ? T('Send a direct message') : T('Send a message: it is delivered when they come online (right-click for more)'), onclick: () => { openDm(uid, name); activateTab('dm:' + uid); }, oncontextmenu: (e) => { e.preventDefault(); personMenu(uid, name, !!dot, e.clientX, e.clientY); } },
      avatar(name, uid, 30, dot),
      el('div', { style: 'min-width:0' }, el('div', { class: 'n' }, name), el('div', { class: 'dim small' }, sub)));
    // Where they are (a channel, or just on the server like Discord) and their status.
    const status = (c) => {
      const ch = c.channel !== 0 && S.channels.get(c.channel);
      const place = ch ? 'in ' + ch.name : T('on the server');
      const p = S.presences.get(c.id);
      const [dot, st] = c.away != null ? ['var(--yellow)', awayText(c.away)] : p === 'Busy' ? ['var(--red)', T('Do not disturb')] : p === 'Invisible' ? ['var(--dim)', 'Invisible'] : ['var(--green)', ''];
      // One row per identity: their desktop session may be the one that is playing.
      const game = [...S.clients.values()].filter((x) => x.uid === c.uid).map((x) => playing(x.id)).find(Boolean);
      return [dot, (game ? '🎮 Playing ' + game + ' · ' : '') + (st ? st + ' · ' : '') + place + (c.platform === 'Web' ? ' · web' : '')];
    };
    return [
      el('div', { class: 'section' }, T('Online — ') + online.length),
      online.map((c) => { const [dot, sub] = status(c); return row(c.uid, c.nickname, sub, dot); }),
      offline.length ? el('div', { class: 'section' }, T('Offline — ') + offline.length) : null,
      offline.map((p) => row(p.uid, p.nickname, p.last_seen ? T('last seen ') + fmtDuration(now() - p.last_seen) + ' ago' : 'offline', false)),
    ].flat().filter(Boolean);
  }

  // ---------------------------------------------------------------- menus
  function menu(x, y, items) {
    const m = $('#menu');
    put(m, ...items.filter(Boolean).map((it) => {
      if (it.sep) return el('div', { class: 'sep' });
      if (it.head) return el('div', { class: 'head' }, it.head);
      return el('div', { class: 'item' + (it.danger ? ' danger' : '') + (it.disabled ? ' disabled' : ''), onclick: () => { hideMenu(); it.onClick(); } }, it.label);
    }));
    m.classList.remove('hidden');
    const r = m.getBoundingClientRect();
    m.style.left = Math.max(4, Math.min(x, innerWidth - r.width - 4)) + 'px';
    m.style.top = Math.max(4, Math.min(y, innerHeight - r.height - 4)) + 'px';
  }
  function hideMenu() { $('#menu').classList.add('hidden'); }
  document.addEventListener('mousedown', (e) => { if (!e.target.closest('#menu')) hideMenu(); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') hideMenu(); });
  window.addEventListener('blur', hideMenu);

  function serverMenu(x, y) {
    menu(x, y, [
      { head: S.server ? S.server.name : T('Server') },
      S.account ? { label: T('🔐  Account & Security…'), onClick: accountDialog } : null,
      can(P.SERVER_EDIT) ? { label: T('⚙  Edit Virtual Server…'), onClick: editServer } : null,
      can(P.GROUP_MANAGE) ? { label: T('👥  Groups & Permissions…'), onClick: groupsDialog } : null,
      can(P.BAN_LIST) ? { label: T('⛔  Ban List…'), onClick: banListDialog } : null,
      can(P.PRIVILEGE_KEY_CREATE) ? { label: T('🔑  Create Privilege Key…'), onClick: privilegeKeyDialog } : null,
      { label: T('🎟  Use Privilege Key…'), onClick: usePrivilegeKey },
      { sep: true },
      canCreate() ? { label: T('➕  Create Channel…'), onClick: () => channelDialog(null, null) } : null,
      can(P.CHAT_CLEAR) ? { label: T('🗑  Clear Server Chat…'), danger: true, onClick: () => confirmDialog(T('Clear server chat'), T('Delete every server chat message for everyone, including the stored history?'), T('Clear history'), true).then((ok) => ok && send({ ClearChat: 'Server' })) } : null,
      { label: T('📋  Copy Fingerprint'), onClick: () => copy(S.server.fingerprint) },
    ]);
  }

  function siblings(ch) { return children(ch.parent ?? null); }
  function moveChannel(ch, dir) {
    const sib = siblings(ch);
    const i = sib.findIndex((c) => c.id === ch.id);
    const other = sib[i + dir];
    if (!other) return;
    let a = other.sort, b = ch.sort;
    if (a === b) a = b + dir;
    send({ EditChannel: { id: ch.id, edit: editOf(ch, { sort: a }) } });
    send({ EditChannel: { id: other.id, edit: editOf(other, { sort: b }) } });
  }
  function editOf(ch, changes) {
    return Object.assign({
      parent: ch.parent ?? null, sort: ch.sort, name: ch.name, topic: ch.topic, description: ch.description, max_clients: ch.max_clients ?? null,
      kind: ch.kind, is_default: ch.is_default, codec_quality: ch.codec_quality, needed_talk_power: ch.needed_talk_power,
      needed_join_power: ch.needed_join_power, voice_encrypted: ch.voice_encrypted, password: 'Keep',
    }, changes || {});
  }

  function channelMenu(ch, x, y) {
    const sp = spacer(ch.name);
    const sib = siblings(ch);
    const i = sib.findIndex((c) => c.id === ch.id);
    menu(x, y, [
      { head: sp ? T('Separator') : ch.name },
      !sp && can(P.FILE_BROWSE)?{label:'📁 Channel files',onClick:()=>channelFiles(ch)}:null,
      !sp && ch.id !== myChannel() ? { label: T('➜  Join Channel'), onClick: () => joinChannel(ch) } : null,
      ch.id === myChannel() ? { label: T('🚪  Leave Channel'), onClick: leaveChannel } : null,
      { label: T('ℹ  Channel Info'), onClick: () => { select({ kind: 'channel', id: ch.id }); S.showRight = true; } },
      { sep: true },
      canCreate() ? { label: T('➕  Create Channel…'), onClick: () => channelDialog(null, ch.parent ?? null) } : null,
      canCreate() && !sp ? { label: T('➕  Create Sub-Channel…'), onClick: () => channelDialog(null, ch.id) } : null,
      can(P.CHANNEL_EDIT) ? { label: T('✏  Edit Channel…'), onClick: () => channelDialog(ch) } : null,
      canCreate() && !sp ? { label: T('⧉  Clone Channel…'), onClick: () => cloneChannel(ch) } : null,
      can(P.CHANNEL_EDIT) ? { label: T('▲  Move Up'), disabled: i <= 0, onClick: () => moveChannel(ch, -1) } : null,
      can(P.CHANNEL_EDIT) ? { label: T('▼  Move Down'), disabled: i >= sib.length - 1, onClick: () => moveChannel(ch, 1) } : null,
      { label: T('📋  Copy Channel Name'), onClick: () => copy(ch.name) },
      can(P.CHANNEL_DELETE) ? { sep: true } : null,
      can(P.CHANNEL_DELETE) ? { label: T('🗑  Delete Channel…'), danger: true, onClick: () => deleteChannel(ch) } : null,
    ]);
  }

  function channelFiles(ch) {
    let dir='',closed=false,serial=0;
    const running=new Set();
    const list=el('div'), notice=el('div',{class:'dim small'}), where=el('strong');
    const endpoint=(route,path)=>route+'?'+new URLSearchParams({channel:String(ch.id),path});
    const refresh=async()=>{
      const gen=++serial;where.textContent='/'+dir;
      try {
        const r=await fetch(endpoint('/api/files',dir),{headers:{'X-Voxa':'1'}});
        if(!r.ok)throw new Error(await r.text());
        const entries=await r.json();
        if(closed||serial!==gen)return;
        put(list,...entries.map(f=>{
          const path=dir?dir+'/'+f.name:f.name;
          return f.is_dir ? el('button',{class:'btn',onclick:()=>{dir=path;refresh();}},'📁 '+f.name) :
            el('a',{class:'btn',href:endpoint('/api/files/download',path),download:f.name},'⬇ '+f.name);
        }));
      }catch(e){if(!closed)notice.textContent=e.message;}
    };
    const file=el('input',{type:'file',multiple:true});
    const upload=async()=>{
      const files=Array.from(file.files),prefix=dir;
      if(!files.length)return;
      start.disabled=true;
      const worker=async()=>{
        while(files.length && !closed){
          const f=files.shift(),ctrl=new AbortController();running.add(ctrl);
          try{
            notice.textContent='Uploading '+f.name;
            const r=await fetch(endpoint('/api/files/upload',prefix?prefix+'/'+f.name:f.name),{method:'POST',headers:{'X-Voxa':'1'},body:f,signal:ctrl.signal});
            if(!r.ok)throw new Error(await r.text());
          }catch(e){if(!closed)toast(f.name+': '+e.message,'bad');}
          finally{running.delete(ctrl);}
        }
      };
      await Promise.all(Array.from({length:Math.min(files.length,personalLimits.uploads||8,S.transferLimits?.uploads||8,8)},worker));
      if(!closed){start.disabled=false;notice.textContent='';file.value='';refresh();}
    };
    const start=el('button',{class:'btn',onclick:upload},'Upload');
    dialog(ch.name+' · Channel files',el('div',null,where,el('button',{class:'btn',onclick:()=>{dir=dir.split('/').slice(0,-1).join('/');refresh();}},'⬆ Up'),list,
      can(P.FILE_UPLOAD)?[file,start]:null,notice),[{label:T('Close')}],{onClose:()=>{closed=true;for(const c of running)c.abort();}});
    refresh();
  }

  function clientMenu(c, x, y) {
    if (c.id === S.me) {
      menu(x, y, [
        { head: c.nickname },
        { label: T('✏  Change Nickname…'), onClick: changeNickname },
        { label: T('🟢  Set Status…'), onClick: () => presenceMenu(x, y) },
        can(P.CHANNEL_COMMANDER) ? { label: T('⭐  Toggle Channel Commander'), onClick: () => send({ UpdateSelf: { commander: !c.commander } }) } : null,
        { label: T('📝  Edit Description…'), onClick: () => editDescription(c) },
        { label: T('📋  Copy Unique ID'), onClick: () => copy(c.uid) },
      ]);
      return;
    }
    menu(x, y, [
      { head: c.nickname },
      { label: T('💬  Send Message'), onClick: () => { openDm(c.uid, c.nickname); activateTab('dm:' + c.uid); } },
      { label:S.blocks.has(c.uid)?T('Unblock DMs'):T('Block DMs'),onClick:()=>blockDm(c.uid) },
      can(P.CLIENT_POKE) ? { label: T('👉  Poke…'), onClick: () => pokeClient(c) } : null,
      can(P.CLIENT_MOVE) ? { label: T('➜  Move to Channel…'), onClick: () => moveClientDialog(c) } : null,
      can(P.CLIENT_MOVE) && inChannel() && c.channel !== myChannel() ? { label: T('⤵  Move to My Channel'), onClick: () => send({ MoveClient: { target: c.id, channel: myChannel() } }) } : null,
      can(P.TALK_POWER_GRANT) ? { label: c.is_talker ? T('🗣  Revoke Talk Power') : T('🗣  Grant Talk Power'), onClick: () => send({ SetTalker: { target: c.id, talker: !c.is_talker } }) } : null,
      { sep: true },
      can(P.GROUP_ASSIGN_SERVER) ? { label: T('👥  Server Groups…'), onClick: () => serverGroupsDialog(c) } : null,
      can(P.GROUP_ASSIGN_CHANNEL) ? { label: T('👥  Channel Group…'), onClick: () => channelGroupDialog(c) } : null,
      can(P.GROUP_MANAGE) ? { label: T('🔐  Permissions…'), onClick: () => clientPermsDialog(c) } : null,
      S.accountPolicy && can(P.SERVER_EDIT) && c.id !== S.me ? { label: T('🔑  Reset Account Password…'), onClick: () => resetAccount(c.uid, c.nickname) } : null,
      can(P.CLIENT_DESCRIPTION_EDIT) ? { label: T('📝  Edit Description…'), onClick: () => editDescription(c) } : null,
      can(P.CLIENT_DESCRIPTION_EDIT) && avatars.get(c.uid)?.image ? { label: T('🖼  Remove Avatar Picture…'), danger: true, onClick: () => confirmDialog(T('Remove avatar picture'), T('Remove the avatar picture of ') + c.nickname + '? They cannot use the same picture again on this server.', 'Remove', true).then((ok) => ok && send({ ClearAvatar: c.uid })) } : null,
      { label: T('📋  Copy Unique ID'), onClick: () => copy(c.uid) },
      { sep: true },
      can(P.CLIENT_KICK_CHANNEL) ? { label: T('👢  Kick from Channel…'), danger: true, onClick: () => kickClient(c, false) } : null,
      can(P.CLIENT_KICK_SERVER) ? { label: T('👢  Kick from Server…'), danger: true, onClick: () => kickClient(c, true) } : null,
      can(P.CLIENT_BAN) ? { label: T('⛔  Ban…'), danger: true, onClick: () => banClient(c) } : null,
    ]);
  }

  // ---------------------------------------------------------------- dialogs
  function dialog(title, body, buttons, opts) {
    opts = opts || {};
    const ov = el('div', { class: 'overlay' });
    const close = () => { ov.remove(); document.removeEventListener('keydown', onKey); if (opts.onClose) opts.onClose(); };
    const onKey = (e) => { if (e.key === 'Escape' && !opts.noClose) close(); };
    document.addEventListener('keydown', onKey);
    const foot = (buttons || []).map((b) => el('button', {
      class: 'btn' + (b.primary ? ' primary' : '') + (b.danger ? ' danger' : ''),
      onclick: async () => { const r = b.onClick ? await b.onClick() : undefined; if (r !== false) close(); },
    }, b.label));
    ov.append(el('div', { class: 'dialog' + (opts.wide ? ' wide' : '') },
      el('div', { class: 'dh' }, title, opts.noClose ? null : el('button', { class: 'icon-btn', title: T('Close'), onclick: close }, '✕')),
      el('div', { class: 'db' }, body),
      foot.length ? el('div', { class: 'df' }, foot) : null));
    ov.addEventListener('mousedown', (e) => { if (e.target === ov && !opts.noClose) close(); });
    ov.__close = close;
    $('#modals').append(ov);
    const first = ov.querySelector('input:not([type=checkbox]):not([type=radio]), textarea, select');
    if (first) setTimeout(() => first.focus(), 0);
    return { close, root: ov };
  }
  function closeDialogs(all) {
    for (const d of [...$('#modals').children]) if (all) d.__close ? d.__close() : d.remove();
  }
  // Forgets someone who is offline (servers 1.23.0+): the server answers with the People list.
  // Resets someone's account password (servers 1.25.1+): the server answers with AccountReset.
  function resetAccount(uid, name) {
    const mfa = el('input', { type: 'checkbox' });
    dialog(T('Reset account password: ') + name, el('div', null,
      el('p', { style: 'margin-top:0' }, T('The server makes a temporary password for their account and shows it to you once. Give it to them: they sign in with it and must then choose a new password. Their remembered devices are signed out.')),
      el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, mfa, T('Also turn off their authenticator app (they lost their phone or key)')),
      el('p', { class: 'dim small' }, T('Make sure it really is them asking: whoever has the temporary password gets the account.'))), [
      { label: T('Cancel') },
      { label: T('Reset password'), danger: true, onClick: () => send({ ResetAccount: { uid, remove_mfa: mfa.checked } }) },
    ]);
  }
  async function forgetPerson(uid, name) {
    const text = T('The server forgets ') + name + ': their server groups, personal permissions, channel groups, picture and messages waiting for them. ' +
      T('Their chat messages stay. If they connect again they start fresh, like someone new (to keep them out, ban them instead).');
    if (await confirmDialog(T('Remove from server'), text, 'Remove', true)) send({ ForgetClient: uid });
  }

  function confirmDialog(title, text, ok, danger) {
    return new Promise((res) => {
      let done = false;
      dialog(title, el('p', { style: 'margin:0' }, text), [
        { label: T('Cancel'), onClick: () => { done = true; res(false); } },
        { label: ok || 'OK', primary: !danger, danger, onClick: () => { done = true; res(true); } },
      ], { onClose: () => { if (!done) res(false); } });
    });
  }
  function promptDialog(title, label, initial, ok, opts) {
    opts = opts || {};
    return new Promise((res) => {
      let done = false;
      const input = opts.multiline ? el('textarea', { rows: '4', value: initial || '', maxlength: opts.max || null }) : el('input', { type: opts.password ? 'password' : 'text', value: initial || '', maxlength: opts.max || null });
      const submit = () => { done = true; res(input.value); };
      if (!opts.multiline) input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); submit(); d.close(); } });
      const d = dialog(title, el('div', { class: 'form', style: 'grid-template-columns:120px 1fr' }, el('label', null, label), input, opts.note ? el('div', { class: 'note' }, opts.note) : null), [
        { label: T('Cancel'), onClick: () => { done = true; res(null); } },
        { label: ok || 'OK', primary: true, onClick: submit },
      ], { onClose: () => { if (!done) res(null); } });
    });
  }
  const field = (label, input, note) => [el('label', null, label), input, note ? el('div', { class: 'note' }, note) : null];
  function radios(name, options, value, onChange) {
    return el('div', { class: 'radios' }, options.map(([v, label]) => el('label', null,
      el('input', { type: 'radio', name, checked: v === value, onchange: () => onChange(v) }), label)));
  }
  let uniq = 0;

  // Stay on the server without being in a channel.
  function leaveChannel() {
    send({ SwitchChannel: { channel: 0, password: null } });
  }
  function joinChannel(ch) {
    if (spacer(ch.name)) return;
    if (ch.has_password && !can(P.CHANNEL_JOIN_IGNORE_PASSWORD)) {
      promptDialog(T('Join ') + ch.name, 'Password', '', 'Join', { password: true }).then((pw) => pw != null && send({ SwitchChannel: { channel: ch.id, password: pw } }));
    } else send({ SwitchChannel: { channel: ch.id, password: null } });
  }

  function changeNickname() {
    const me = S.clients.get(S.me);
    promptDialog(T('Change nickname'), 'Nickname', me ? me.nickname : '', 'Change', { max: 30 }).then((n) => {
      if (n && n.trim()) {
        send({ UpdateSelf: { nickname: n.trim() } });
        localStorage.setItem('voxa-web-nick', n.trim());
        savePrefs({ nickname: n.trim() });
      }
    });
  }
  function presenceMenu(x, y) {
    const cur = myPresence();
    menu(x, y, [
      { head: T('Status') },
      ...PRESENCES.map(([p, , label]) => ({ label: (cur === p ? '◉  ' : '○  ') + label, onClick: () => setPresence(p) })),
      { label: T('🌙  Away with a message…'), onClick: toggleAwayMessage },
      inChannel() ? { sep: true } : null,
      inChannel() ? { label: T('🚪  Leave Channel'), onClick: leaveChannel } : null,
    ]);
  }
  function toggleAwayMessage() {
    promptDialog(T('Set away'), 'Message', '', T('Set away'), { max: 80, note: T('Optional, shown next to your name.') }).then((m) => m != null && send({ UpdateSelf: { away: m.trim() } }));
  }
  function toggleAway() {
    const me = S.clients.get(S.me);
    if (!me) return;
    if (me.away != null) setPresence('Online');
    else toggleAwayMessage();
  }
  function editDescription(c) {
    promptDialog(T('Edit description'), 'Description', c.description, 'Save', { multiline: true, max: 500 }).then((d) => {
      if (d == null) return;
      if (c.id === S.me) send({ UpdateSelf: { description: d } });
      else send({ EditClientDescription: { target: c.id, description: d } });
    });
  }
  function pokeClient(c) {
    promptDialog(T('Poke ') + c.nickname, 'Message', '', 'Poke', { max: 100 }).then((m) => m != null && send({ Poke: { target: c.id, message: m } }));
  }
  function kickClient(c, fromServer) {
    promptDialog((fromServer ? T('Kick from server: ') : T('Kick from channel: ')) + c.nickname, 'Reason', '', 'Kick', { max: 80 })
      .then((r) => r != null && send({ Kick: { target: c.id, from_server: fromServer, reason: r } }));
  }
  const DURATIONS = [[3600, '1 hour'], [86400, '1 day'], [7 * 86400, '7 days'], [30 * 86400, '30 days'], [0, 'Permanent']];
  function durationSelect() {
    return el('select', null, DURATIONS.map(([v, l]) => el('option', { value: String(v), selected: v === 86400 }, l)));
  }
  function banClient(c) {
    const dur = durationSelect();
    const reason = el('input', { type: 'text', maxlength: '80' });
    const check = (label, on, note, disabled) => { const i = el('input', { type: 'checkbox', checked: on && !disabled, disabled }); return [i, el('label', { style: 'display:flex;gap:6px;color:var(--text)' + (disabled ? ';opacity:.5' : ''), title: note }, i, label)]; };
    const [byUid, uidRow] = check(T('Unique ID (this identity)'), true, '');
    const [byIp, ipRow] = check(T('IP address (') + T('their current address)'), true, '');
    const noHw = c.platform === 'Web';
    const [byHw, hwRow] = check(T('Hardware (this PC: network adapters and Windows machine ID)'), false, '', noHw);
    dialog(T('Ban ') + c.nickname, el('div', { class: 'form' },
      field(T('Ban by'), el('div', { style: 'display:flex;flex-direction:column;gap:4px' }, uidRow, ipRow, hwRow)),
      noHw ? el('div', { class: 'note' }, T('The web page has no hardware to ban.')) : el('div', { class: 'note' }, T('Hardware bans stop new identities from the same PC.')),
      field(T('Duration'), dur), field(T('Reason'), reason)), [
      { label: T('Cancel') },
      { label: T('Ban'), danger: true, onClick: () => {
        if (!byUid.checked && !byIp.checked && !byHw.checked) { toast('Choose at least one thing to ban', 'warn'); return false; }
        send({ BanWith: { target: c.id, uid: byUid.checked, ip: byIp.checked, hardware: byHw.checked, duration_secs: Number(dur.value) || null, reason: reason.value } });
      } },
    ]);
  }
  function moveClientDialog(c) {
    const sel = el('select', null, [...S.channels.values()].filter((ch) => !spacer(ch.name)).sort((a, b) => a.name.localeCompare(b.name)).map((ch) => el('option', { value: String(ch.id), selected: ch.id === c.channel }, ch.name)));
    dialog(T('Move ') + c.nickname, el('div', { class: 'form' }, field(T('Channel'), sel)), [
      { label: T('Cancel') },
      { label: T('Move'), primary: true, onClick: () => send({ MoveClient: { target: c.id, channel: Number(sel.value) } }) },
    ]);
  }
  function deleteChannel(ch) {
    const n = subtree(ch.id).reduce((a, id) => a + clientsIn(id).length, 0);
    const subs = subtree(ch.id).length - 1;
    const text = T('Delete "') + (spacer(ch.name) ? T('this separator') : ch.name) + '"' + (subs ? T(' and its ') + subs + ' sub-channel(s)' : '') + '?' + (n ? ' ' + n + ' client(s) inside are moved to the default channel.' : '') + T(' Its files and chat history are deleted too.');
    confirmDialog(T('Delete channel'), text, 'Delete', true).then((ok) => ok && send({ DeleteChannel: { id: ch.id, force: true } }));
  }
  function cloneChannel(ch) {
    const name = el('input', { type: 'text', value: ch.name + ' (copy)', maxlength: '40' });
    const kids = el('input', { type: 'checkbox', checked: children(ch.id).length > 0 });
    dialog(T('Clone channel'), el('div', { class: 'form' }, field(T('Name'), name), [el('span'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, kids, T('Include sub-channels'))],
      el('div', { class: 'note' }, T('Copies every setting, the password and everyone\'s channel group.'))), [
      { label: T('Cancel') },
      { label: T('Clone'), primary: true, onClick: () => send({ CloneChannel: { source: ch.id, name: name.value.trim(), with_children: kids.checked } }) },
    ]);
  }

  function channelDialog(ch, parent) {
    const editing = !!ch;
    const e = editing ? editOf(ch) : editOf({ parent: parent ?? null, sort: 0, name: '', topic: '', description: '', max_clients: null, kind: 'Permanent', is_default: false, codec_quality: 6, needed_talk_power: 0, needed_join_power: 0, voice_encrypted: true });
    if (!editing && parent != null) {
      const sib = children(parent);
      e.sort = sib.length ? Math.max(...sib.map((c) => c.sort)) + 1 : 0;
    }
    let sp = spacer(e.name);
    let isSep = !!sp;
    let codec = codecOf(e.codec_quality), quality = qualityOf(e.codec_quality);
    const id = ++uniq;
    const body = el('div');
    const name = el('input', { type: 'text', value: sp ? sp.text : e.name, maxlength: '40' });
    const topic = el('input', { type: 'text', value: e.topic, maxlength: '120' });
    const desc = el('textarea', { rows: '3', value: e.description });
    const pw = el('input', { type: 'password', placeholder: editing && ch.has_password ? 'unchanged' : 'none', autocomplete: 'new-password' });
    const pwClear = el('input', { type: 'checkbox' });
    const unlimited = el('input', { type: 'checkbox', checked: e.max_clients == null });
    const maxc = el('input', { type: 'number', min: '1', max: '9999', value: String(e.max_clients ?? 10), disabled: e.max_clients == null });
    unlimited.addEventListener('change', () => { maxc.disabled = unlimited.checked; });
    const isDefault = el('input', { type: 'checkbox', checked: e.is_default });
    const talk = el('input', { type: 'number', value: String(e.needed_talk_power) });
    const join = el('input', { type: 'number', value: String(e.needed_join_power) });
    const enc = el('input', { type: 'checkbox', checked: e.voice_encrypted });
    // Servers 1.25.0+: server groups that do not see this channel.
    let hidden = editing && S.accountPolicy && can(P.CHANNEL_EDIT) ? [...(S.hidden.get(ch.id) || [])] : null;
    const myGroups = (S.clients.get(S.me) || {}).server_groups || [];
    const hiddenBox = hidden && el('div', { style: 'display:flex;flex-direction:column;gap:4px' }, [...S.groups.values()].filter((g) => g.kind === 'Server').sort((a, b) => a.sort - b.sort || a.id - b.id).map((g) => {
      const own = myGroups.includes(g.id);
      const box = el('input', { type: 'checkbox', checked: hidden.includes(g.id), disabled: own && !hidden.includes(g.id), onchange: () => { hidden = hidden.filter((x) => x !== g.id); if (box.checked) hidden.push(g.id); } });
      return el('label', { style: 'display:flex;gap:6px;color:var(--text)', title: own ? T('You are in this group') : '' }, box, g.name);
    }));
    const parentSel = el('select', null, el('option', { value: '' }, '(top level)'),
      [...S.channels.values()].filter((c) => !spacer(c.name) && !(editing && subtree(ch.id).includes(c.id))).sort((a, b) => a.name.localeCompare(b.name))
        .map((c) => el('option', { value: String(c.id), selected: c.id === e.parent }, c.name)));
    let kind = e.kind, align = sp ? sp.align : 'c';
    const qLabel = el('span', { class: 'dim' });
    const qRange = el('input', { type: 'range', min: '0', max: '10', value: String(quality), oninput: () => { quality = Number(qRange.value); updQ(); } });
    const updQ = () => { qLabel.textContent = codec === 'lossless' ? T('up to ~1.5 Mbit/s per listener, bit exact') : '~' + bitrate(packCodec(codec, quality)) + ' kbit/s' + (codec === 'music' ? ' stereo' : ''); qRange.disabled = codec === 'lossless'; };
    updQ();
    const draw = () => {
      put(body, el('div', { class: 'form' },
        field(T('Type'), radios('t' + id, [[false, 'Channel'], [true, T('Separator (organises the list, nobody can join it)')]], isSep, (v) => { isSep = v; draw(); })),
        isSep ? [
          field(T('Style'), radios('a' + id, [['c', T('Text in the middle')], ['l', T('Text on the left')], ['r', T('Text on the right')], ['*', T('Line (the text repeated)')]], align, (v) => { align = v; })),
          field(T('Text'), name, 'e.g. Games, or for a line: ─ or - (empty = a thin line)'),
          field(T('Parent'), parentSel),
        ] : [
          field(T('Name'), name),
          field(T('Parent'), parentSel),
          field(T('Topic'), topic),
          field(T('Description'), desc),
          field(T('Password'), el('div', { style: 'display:flex;gap:10px;align-items:center' }, pw, editing && ch.has_password ? el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, pwClear, 'Remove') : null)),
          field(T('Lifetime'), radios('k' + id, [['Permanent', 'Permanent'], ['SemiPermanent', 'Semi-Permanent'], ['Temporary', 'Temporary']], kind, (v) => { kind = v; })),
          field(T('Max clients'), el('div', { style: 'display:flex;gap:10px;align-items:center' }, el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, unlimited, 'Unlimited'), maxc)),
          field(T('Default channel'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, isDefault, T('New clients join this channel'))),
          field(T('Codec'), radios('c' + id, [['voice', T('Opus Voice')], ['music', T('Opus Music')], ['lossless', 'Lossless']], codec, (v) => { codec = v; updQ(); })),
          field(T('Quality'), el('div', { style: 'display:flex;gap:10px;align-items:center' }, qRange, qLabel)),
          field(T('Needed talk power'), talk),
          field(T('Needed join power'), join),
          field(T('Voice encryption'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, enc, T('Encrypt voice in this channel'))),
          hiddenBox ? field(T('Hidden from'), hiddenBox, T('People in these groups do not see this channel, its sub-channels, people or chat. Administrators always see it.')) : null,
        ]));
    };
    draw();
    dialog(editing ? T('Edit channel') : T('Create channel'), body, [
      { label: T('Cancel') },
      { label: editing ? T('Save') : T('Create'), primary: true, onClick: () => {
        const out = Object.assign({}, e);
        out.parent = parentSel.value ? Number(parentSel.value) : null;
        if (isSep) {
          let s = { align, number: sp ? sp.number : '', text: name.value };
          const taken = (nm) => children(out.parent).some((c) => (!editing || c.id !== ch.id) && c.name === nm);
          if (taken(spacerName(s))) { let n = 1; while (taken(spacerName({ ...s, number: String(n) }))) n++; s.number = String(n); }
          out.name = spacerName(s);
          out.topic = ''; out.description = '';
        } else {
          out.name = name.value.trim();
          if (!out.name) { toast('Enter a name', 'bad'); return false; }
          out.topic = topic.value;
          out.description = desc.value;
          out.password = pwClear.checked ? 'Clear' : pw.value ? { Set: pw.value } : 'Keep';
          out.kind = kind;
          out.max_clients = unlimited.checked ? null : Math.max(1, Number(maxc.value) || 1);
          out.is_default = isDefault.checked;
          out.codec_quality = packCodec(codec, quality);
          out.needed_talk_power = Number(talk.value) || 0;
          out.needed_join_power = Number(join.value) || 0;
          out.voice_encrypted = enc.checked;
        }
        if (editing && hidden) {
          const a = [...hidden].sort((x, y) => x - y), b = [...(S.hidden.get(ch.id) || [])].sort((x, y) => x - y);
          if (a.join() !== b.join()) send({ SetChannelHidden: { channel: ch.id, groups: a } });
        }
        if (editing) send({ EditChannel: { id: ch.id, edit: out } });
        else send({ CreateChannel: out });
      } },
    ]);
  }

  // ---------------------------------------------------------------- accounts (servers 1.25.0+)
  // This page's origin is the server, so the remembered sign-in is per identity.
  const accountKey = () => 'voxa-web-account-' + (S.uid || '');
  function accountDialog() {
    if (S.accountDlg || !S.account) return;
    let page = 'main';
    let err = '';
    const body = el('div');
    const inp = (type, ph) => el('input', { type, placeholder: ph || '', autocomplete: type === 'password' ? 'current-password' : 'off' });
    const user = inp('text'), pw = inp('password'), pw2 = inp('password'), npw = inp('password'), code = inp('text', '123456');
    const remember = el('input', { type: 'checkbox', checked: true });
    const btn = (label, onClick, kind) => el('button', { class: 'btn' + (kind ? ' ' + kind : ''), onclick: onClick }, label);
    const go = (p) => { page = p; err = ''; draw(); };
    const clear = () => { for (const i of [pw, pw2, npw, code]) i.value = ''; };
    const goodPw = (p) => p.length >= 10 && p.length <= 200;
    const fail = (t) => { err = t; draw(); };
    const draw = () => {
      const a = S.account, pol = S.accountPolicy || { mode: 'Off' };
      if (!a) return;
      const has = !!a.username;
      let p = a.signed_in && a.must_change ? 'password' : page === 'main' && !a.signed_in && has ? 'signin' : page;
      const rows = [];
      if (a.limited && a.reason) rows.push(el('div', { class: 'note full', style: 'color:var(--warn,#e0b040)' }, '⚠ ' + a.reason + T(' Until then you only have Guest permissions here.')));
      if (err) rows.push(el('div', { class: 'note full', style: 'color:var(--bad,#e05050)' }, err));
      const actions = (...b) => el('div', { class: 'actions-row full', style: 'display:flex;gap:8px;flex-wrap:wrap' }, b);
      if (S.recoveryCodes) {
        rows.push(el('div', { class: 'full' }, el('p', null, T('Your recovery codes. Each works once, instead of your authenticator app or to reset your password. Keep them somewhere safe: they are not shown again.')),
          el('pre', { style: 'font-size:15px;user-select:all' }, S.recoveryCodes.join('\n'))),
          actions(btn(T('Copy'), () => navigator.clipboard && navigator.clipboard.writeText(S.recoveryCodes.join('\n'))), btn(T('I have saved them'), () => { S.recoveryCodes = null; draw(); }, 'primary')));
      } else if (S.mfaSetup) {
        rows.push(el('div', { class: 'full' }, el('p', null, T('Add this account to an authenticator app (Microsoft or Google Authenticator, Aegis, 2FAS, Yubico Authenticator…): on a phone tap the link, otherwise type the key into the app. Then enter the 6-digit code it shows.')),
          el('p', null, T('Key: '), el('code', { style: 'user-select:all' }, S.mfaSetup.secret)),
          el('p', null, el('a', { href: S.mfaSetup.uri }, T('Open in an authenticator app')))),
          ...field(T('Code'), code),
          actions(btn(T('Turn on'), () => { if (code.value.trim()) send({ ConfirmMfa: code.value.trim() }); code.value = ''; }, 'primary'), btn(T('Cancel'), () => { S.mfaSetup = null; draw(); })));
      } else if (p === 'main' && !has) {
        if (pol.mode === 'Off') rows.push(el('p', { class: 'full' }, T('This server does not use accounts.')));
        else {
          rows.push(...field(T('Username'), user, '3-32 letters, digits, . - _'), ...field(T('Password'), pw, T('At least 10 characters.')), ...field(T('Again'), pw2),
            el('div', { class: 'note full' }, T('Forgot it later? Use a recovery code (you get them with an authenticator app), or ask a server administrator to reset it.')),
            ...field('', el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, remember, T('Remember this browser for 30 days'))),
            actions(btn(T('Make account'), () => {
              const u = user.value.trim();
              if (!/^[\p{L}\p{N}._-]{3,32}$/u.test(u)) return fail(T('A username has 3 to 32 letters, digits, dots, dashes or underscores.'));
              if (!goodPw(pw.value)) return fail(T('A password needs at least 10 characters.'));
              if (pw.value !== pw2.value) return fail(T('The two passwords are different.'));
              send({ CreateAccount: { username: u, password: pw.value } });
              if (remember.checked) S.afterCreate = { username: u, password: pw.value };
              clear();
            }, 'primary'), btn(T('I already have an account'), () => go('signin'))));
        }
      } else if (p === 'signin') {
        if (!user.value && a.username) user.value = a.username;
        rows.push(...field(T('Username'), user), ...field(T('Password'), pw), ...field(T('Code'), code, T('From your authenticator app (or a recovery code), if you use one.')),
          ...field('', el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, remember, T('Remember this browser for 30 days'))),
          actions(btn(T('Sign in'), () => {
            if (!pw.value) return;
            send({ SignIn: { username: user.value.trim(), password: pw.value, code: code.value.trim() || null, remember: remember.checked } });
            clear(); err = '';
          }, 'primary'), btn(T('Forgot password?'), () => go('forgot')), has ? null : btn(T('Back'), () => go('main'))));
      } else if (p === 'forgot') {
        rows.push(...field(T('Username'), user));
        rows.push(...field(T('Recovery code'), code), ...field(T('New password'), npw, T('At least 10 characters.')),
          actions(btn(T('Reset password'), () => {
            if (!goodPw(npw.value)) return fail(T('A password needs at least 10 characters.'));
            if (!code.value.trim()) return;
            send({ ResetPassword: { username: user.value.trim(), code: code.value.trim(), password: npw.value } });
            clear(); go('signin');
          }, 'primary'), btn(T('Back'), () => go('signin'))),
          el('div', { class: 'note full' }, T('No recovery code? Ask a server administrator to reset your password: they give you a temporary one, and you choose a new password after signing in.')));
      } else if (p === 'password') {
        const forced = a.must_change;
        if (forced) rows.push(el('div', { class: 'full', style: 'color:var(--warn,#e0b040);font-weight:600' }, T('An administrator reset your password: choose a new one.')));
        rows.push(...field(forced ? T('Temporary password') : T('Current password'), pw), ...field(T('New password'), npw, T('At least 10 characters.')), ...field(T('Again'), pw2),
          actions(btn(T('Change'), () => {
            if (!goodPw(npw.value)) return fail(T('A password needs at least 10 characters.'));
            if (npw.value !== pw2.value) return fail(T('The two new passwords are different.'));
            send({ ChangePassword: { old: pw.value, new: npw.value } });
            clear(); go('main');
          }, 'primary'), forced ? null : btn(T('Back'), () => go('main'))));
      } else if (p === 'nomfa') {
        rows.push(...field(T('Your password'), pw), ...field(T('Code'), code, T('From the app, or a recovery code.')),
          actions(btn(T('Turn off'), () => { if (pw.value) { send({ DisableMfa: { password: pw.value, code: code.value.trim() } }); clear(); go('main'); } }, 'danger'), btn(T('Back'), () => go('main'))));
      } else if (p === 'codes') {
        rows.push(el('p', { class: 'full' }, T('The old codes stop working.')), ...field(T('Your password'), pw),
          actions(btn(T('Make new codes'), () => { if (pw.value) { send({ NewRecoveryCodes: { password: pw.value } }); clear(); go('main'); } }, 'primary'), btn(T('Back'), () => go('main'))));
      } else {
        const tok = localStorage.getItem(accountKey());
        rows.push(el('div', { class: 'full', style: 'color:var(--good,#40c070);font-weight:600' }, T('✔ Signed in as ') + a.username),
          ...field(T('Authenticator app'), el('span', null, a.mfa ? T('On') : T('Off'))),
          ...field(T('Recovery codes left'), el('span', null, a.mfa ? String(a.recovery_left) : '-')),
          ...field(T('This browser'), el('span', null, tok ? 'remembered' : T('not remembered'))),
          actions(
            a.mfa ? btn(T('Turn off the authenticator app…'), () => go('nomfa')) : btn(T('🔑 Add an authenticator app (MFA)'), () => send('StartMfa'), 'primary'),
            a.mfa ? btn(T('New recovery codes…'), () => go('codes')) : null,
            btn(T('Change password…'), () => go('password')),
            btn(T('Sign out'), () => { send({ SignOut: tok }); localStorage.removeItem(accountKey()); })));
      }
      put(body, el('div', null, el('p', { class: 'dim small', style: 'margin-top:0' }, T('Each server has its own accounts. The password protects your identity here: someone with a copy of your identity key still needs it.')),
        el('div', { class: 'form' }, rows)));
      // Made an account and asked to be remembered: sign in once to get this browser's token.
      if (S.afterCreate && a.signed_in) {
        send({ SignIn: { username: S.afterCreate.username, password: S.afterCreate.password, code: null, remember: true } });
        S.afterCreate = null;
      }
      if (a.signed_in && page === 'signin') page = 'main';
    };
    S.accountDlg = { redraw: draw, error: fail };
    draw();
    dialog(T('🔐 Account & security'), body, [{ label: T('Close') }], { wide: true, onClose: () => { S.accountDlg = null; S.afterCreate = null; } });
  }

  function groupSelect(kind, value) {
    return el('select', null, [...S.groups.values()].filter((g) => g.kind === kind).sort((a, b) => a.sort - b.sort).map((g) => el('option', { value: String(g.id), selected: g.id === value }, g.name)));
  }

  function editServer() {
    const s = S.server;
    let removeIcon = false;
    const icon = el('input',{type:'file',accept:'image/png,image/jpeg'});
    async function chosenIcon() {
      const file = icon.files[0];
      if (!file) return removeIcon ? null : undefined;
      if (file.size > 16 * 1024 * 1024) throw new Error('Choose a picture under 16 MiB');
      const img = await createImageBitmap(file);
      try {
        const side = Math.min(img.width,img.height);
        const canvas = el('canvas',{width:128,height:128});
        canvas.getContext('2d').drawImage(img,(img.width-side)/2,(img.height-side)/2,side,side,0,0,128,128);
        const blob = await new Promise(ok => canvas.toBlob(ok,'image/png'));
        return Array.from(new Uint8Array(await blob.arrayBuffer()));
      } finally { img.close(); }
    }
    const caps=S.transferLimits||{};
    const transferForm=transferFields(caps);
    const name = el('input', { type: 'text', value: s.name, maxlength: '60' });
    const welcome = el('textarea', { rows: '3', value: s.welcome_message });
    const host = el('textarea', { rows: '2', value: s.host_message });
    const maxc = el('input', { type: 'number', min: '1', value: String(s.max_clients) });
    const pw = el('input', { type: 'password', placeholder: s.has_password ? 'unchanged' : 'none', autocomplete: 'new-password' });
    const pwClear = el('input', { type: 'checkbox' });
    const dsg = groupSelect('Server', s.default_server_group);
    const dcg = groupSelect('Channel', s.default_channel_group);
    const cag = groupSelect('Channel', s.channel_admin_group);
    const retention = el('input', { type: 'number', min: '0', value: String(s.chat_retention_days) });
    const away = el('input', { type: 'number', min: '0', max: '30', value: String(S.options ? S.options.auto_away_minutes : 10) });
    const upload = el('input', { type: 'number', min: '0', placeholder: 'loading…' });
    const quota = el('input', { type: 'number', min: '0', placeholder: 'loading…' });
    const pol = S.accountPolicy || { mode: 'Off', mfa_for_staff: false };
    const accMode = el('select', null, [['Off', 'Off'], ['Optional', 'Optional'], ['RequiredForStaff', T('Required for staff')], ['Required', T('Required for everyone')]].map(([v, l]) => el('option', { value: v, selected: v === pol.mode }, l)));
    const accMfa = el('input', { type: 'checkbox', checked: pol.mfa_for_staff });
    let limits = null;
    S.wait.FileLimits = (v) => { limits = v; upload.value = String(v.max_upload_mb); quota.value = String(v.channel_quota_mb); };
    send('GetFileLimits');
    dialog(T('Edit virtual server'), el('div', { class: 'form' },
      field(T('Server name'), name),
      field(T('Server icon'),el('div',null,icon,el('button',{type:'button',class:'btn',onclick:()=>{icon.value='';removeIcon=true;}},T('Use initials')))),
      field(T('Welcome message'), welcome),
      field(T('Host message'), host),
      field(T('Max clients'), maxc),
      field(T('Password'), el('div', { style: 'display:flex;gap:10px;align-items:center' }, pw, s.has_password ? el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, pwClear, 'Remove') : null)),
      field(T('Default server group'), dsg),
      field(T('Default channel group'), dcg),
      field(T('Channel admin group'), cag),
      field(T('Keep chat history'), retention, T('Days; 0 keeps messages forever.')),
      field(T('Auto away after'), away, T('Minutes without input before people are set away (0 = never, at most 30).')),
      S.accountPolicy ? [el('div', { class: 'section full' }, T('Accounts & security')),
        field(T('Accounts'), accMode, T('Accounts add a username and password (and optionally an authenticator app) on top of each identity. People who must sign in only get Guest permissions until they do.')),
        field(T('MFA'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, accMfa, T('Staff must also use an authenticator app')),
          T('Forgotten passwords: a recovery code, or right-click the person ▸ Reset Account Password.'))] : null,
      el('div', { class: 'section full' }, T('File browser')),
      field(T('Largest upload (MB)'), upload, '0 = unlimited'),
      transferForm.fields,
      field(T('Storage per channel (MB)'), quota, '0 = unlimited')), [
      { label: T('Cancel') },
      { label: T('Save'), primary: true, onClick: async () => {
        let iconChange;
        try { iconChange = await chosenIcon(); } catch (e) { toast(e.message,'bad'); return false; }
        send({ EditServer: {
          name: name.value.trim() || s.name, welcome_message: welcome.value, host_message: host.value,
          max_clients: Math.max(1, Number(maxc.value) || s.max_clients),
          password: pwClear.checked ? 'Clear' : pw.value ? { Set: pw.value } : 'Keep',
          default_server_group: Number(dsg.value), default_channel_group: Number(dcg.value), channel_admin_group: Number(cag.value),
          chat_retention_days: Math.max(0, Number(retention.value) || 0),
        } });
        if (S.accountPolicy && (accMode.value !== pol.mode || accMfa.checked !== pol.mfa_for_staff)) send({ SetAccountPolicy: { mode: accMode.value, mfa_for_staff: accMfa.checked } });
        if(iconChange!==undefined)send({SetServerIcon:iconChange});
        const transfer=transferForm.read();
        if(Object.keys(transfer).some(k=>transfer[k]!==caps[k]))send({SetTransferLimits:transfer});
        const mins = Math.min(30, Math.max(0, Math.round(Number(away.value) || 0)));
        if (!S.options || mins !== S.options.auto_away_minutes) send({ SetServerOptions: { auto_away_minutes: mins } });
        if (limits && upload.value !== '' && quota.value !== '') {
          const nl = { max_upload_mb: Math.max(0, Number(upload.value) || 0), channel_quota_mb: Math.max(0, Number(quota.value) || 0) };
          if (nl.max_upload_mb !== limits.max_upload_mb || nl.channel_quota_mb !== limits.channel_quota_mb) send({ SetFileLimits: nl });
        }
      } },
    ], { wide: true, onClose: () => { delete S.wait.FileLimits; } });
  }

  // The permission search is kept while switching groups (the list is long).
  let permQuery = '';
  const permMatches = (q, label) => q.toLowerCase().split(/\s+/).filter(Boolean).every((w) => label.toLowerCase().includes(w));
  function permChecks(perms, onChange) {
    let p = BigInt(perms);
    const rows = PERM_NAMES.map(([b, label]) => [label, el('label', null,
      el('input', { type: 'checkbox', checked: (p & bit(b)) !== 0n, onchange: (e) => { p = e.target.checked ? p | bit(b) : p & ~bit(b); onChange(p); } }), label)]);
    const count = el('span', { class: 'dim small' });
    const none = el('div', { class: 'dim small' }, T('No permission matches.'));
    const apply = () => {
      let shown = 0;
      for (const [label, row] of rows) {
        const on = permMatches(permQuery, label);
        row.style.display = on ? '' : 'none';
        if (on) shown++;
      }
      count.textContent = permQuery.trim() ? shown + ' of ' + rows.length : '';
      none.style.display = shown ? 'none' : '';
    };
    const search = el('input', { type: 'search', class: 'perm-search', placeholder: T('🔍 Search permissions'), value: permQuery, oninput: (e) => { permQuery = e.target.value; apply(); } });
    apply();
    return el('div', null, el('div', { class: 'perm-search-row' }, search, count), el('div', { class: 'checks' }, rows.map(([, row]) => row), none));
  }

  function groupsDialog() {
    let selected = null;
    const list = el('div', { class: 'list' });
    const editor = el('div');
    const draw = () => {
      const groups = [...S.groups.values()].sort((a, b) => a.sort - b.sort || a.id - b.id);
      if (!selected || !S.groups.has(selected)) selected = groups[0] ? groups[0].id : null;
      const item = (g) => el('div', { class: 'li' + (g.id === selected ? ' on' : ''), onclick: () => { selected = g.id; draw(); } }, g.name, g.builtin ? el('span', { class: 'small', style: 'opacity:.7' }, '(built-in)') : null);
      put(list, 
        el('div', { class: 'lh' }, T('Server groups')), groups.filter((g) => g.kind === 'Server').map(item),
        el('div', { class: 'lh' }, T('Channel groups')), groups.filter((g) => g.kind === 'Channel').map(item),
        el('div', { style: 'display:flex;gap:4px;padding:8px 4px;flex-wrap:wrap' },
          el('button', { class: 'btn small', onclick: () => newGroup('Server') }, '+ Server group'),
          el('button', { class: 'btn small', onclick: () => newGroup('Channel') }, '+ Channel group')));
      const g = S.groups.get(selected);
      if (!g) { put(editor, el('span', { class: 'dim' }, T('No group selected'))); return; }
      const name = el('input', { type: 'text', value: g.name, maxlength: '40' });
      const talk = el('input', { type: 'number', value: String(g.talk_power) });
      const join = el('input', { type: 'number', value: String(g.join_power) });
      const sort = el('input', { type: 'number', value: String(g.sort) });
      const show = el('input', { type: 'checkbox', checked: g.show_name });
      let perms = BigInt(g.perms);
      const members = el('div', { class: 'list', style: 'max-height:160px;margin-top:6px' }, el('span', { class: 'dim small' }, T('Loading…')));
      S.wait.GroupMembers = (v) => {
        if (v.group !== g.id) return;
        put(members, ...(v.members.length ? v.members.map((m) => el('div', { class: 'li', style: 'cursor:default' },
          el('span', { style: 'flex:1' }, m.name || m.uid, m.online ? el('span', { style: 'color:var(--green)' }, ' ●') : null),
          g.kind === 'Server' && can(P.GROUP_ASSIGN_SERVER) ? el('button', { class: 'btn small danger', onclick: () => { send({ SetServerGroup: { uid: m.uid, group: g.id, add: false } }); setTimeout(() => send({ GroupMembers: g.id }), 300); } }, T('Remove')) : null)) : [el('span', { class: 'dim small' }, T('No members'))]));
      };
      send({ GroupMembers: g.id });
      put(editor, 
        el('div', { class: 'form' },
          field(T('Name'), name), field(T('Talk power'), talk), field(T('Join power'), join, T('Compared with a channel\'s needed join power.')), field(T('Sort order'), sort),
          field(T('Show name'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, show, T('Show the group name next to members')))),
        el('div', { class: 'section' }, T('Permissions')),
        permChecks(perms, (p) => { perms = p; }),
        el('div', { class: 'actions-row' },
          el('button', { class: 'btn primary', onclick: () => send({ EditGroup: { id: g.id, kind: g.kind, name: name.value.trim() || g.name, perms: Number(perms), talk_power: Number(talk.value) || 0, join_power: Number(join.value) || 0, sort: Number(sort.value) || 0, builtin: g.builtin, show_name: show.checked } }) }, T('Save group')),
          !g.builtin ? el('button', { class: 'btn danger', onclick: () => confirmDialog(T('Delete group'), T('Delete the group "') + g.name + '"? Its members lose it.', 'Delete', true).then((ok) => ok && send({ DeleteGroup: g.id })) }, T('Delete group')) : null),
        el('div', { class: 'section' }, T('Members')), members);
    };
    const newGroup = (kind) => {
      promptDialog(T('New ') + kind.toLowerCase() + ' group', 'Name', '', 'Create', { max: 40, note: selected && S.groups.get(selected)?.kind === kind ? T('Starts as a copy of "') + S.groups.get(selected).name + '".' : null })
        .then((n) => n && n.trim() && send({ CreateGroup: { kind, name: n.trim(), copy_from: selected && S.groups.get(selected)?.kind === kind ? selected : null } }));
    };
    draw();
    S.wait.GroupsUpdated = () => draw();
    dialog(T('Groups & permissions'), el('div', { class: 'split' }, list, editor), [{ label: T('Close') }], { wide: true, onClose: () => { delete S.wait.GroupsUpdated; delete S.wait.GroupMembers; } });
  }

  function banListDialog() {
    const table = el('div', null, el('span', { class: 'dim' }, T('Loading…')));
    let last = [];
    S.wait.BannedHardware = () => S.wait.BanList(last);
    S.wait.BanList = (bans) => {
      last = bans;
      put(table, bans.length ? el('table', { class: 'grid' },
        el('tr', null, ['Banned', 'Reason', 'By', 'Since', 'Expires', ''].map((h) => el('th', null, h))),
        bans.map((b) => el('tr', null,
          el('td', null, b.name || '', b.uid ? el('div', { class: 'dim small', style: 'font-family:Consolas,monospace' }, b.uid) : null, b.ip ? el('div', { class: 'dim small' }, b.ip) : null, S.hwBans && S.hwBans.has(b.id) ? el('div', { class: 'dim small' }, T('🖥 hardware')) : null),
          el('td', null, b.reason), el('td', null, b.by), el('td', null, fmtDate(b.created)),
          el('td', null, b.expires ? fmtDate(b.expires) : 'never'),
          el('td', null, el('button', { class: 'btn small', onclick: () => { send({ Unban: b.id }); setTimeout(() => send('BanList'), 300); } }, T('Unban')))))) : el('span', { class: 'dim' }, T('Nobody is banned.')));
    };
    send('BanList');
    const kind = el('select', null, el('option', { value: 'Uid' }, T('Unique ID')), el('option', { value: 'Ip' }, T('IP address')));
    const value = el('input', { type: 'text', placeholder: T('Unique ID or address') });
    const dur = durationSelect();
    const reason = el('input', { type: 'text', placeholder: T('Reason'), maxlength: '80' });
    const add = el('div', { style: 'display:flex;gap:6px;flex-wrap:wrap;margin-top:14px' }, kind, value, dur, reason,
      el('button', { class: 'btn', onclick: () => {
        if (!value.value.trim()) return;
        send({ Ban: { target: { [kind.value]: value.value.trim() }, duration_secs: Number(dur.value) || null, reason: reason.value } });
        value.value = ''; reason.value = '';
        setTimeout(() => send('BanList'), 300);
      } }, T('Add ban')));
    dialog(T('Ban list'), el('div', null, table, el('div', { class: 'section' }, T('Add a ban')), add), [{ label: T('Close') }], { wide: true, onClose: () => { delete S.wait.BanList; delete S.wait.BannedHardware; } });
  }

  function privilegeKeyDialog() {
    const group = el('select', null, [...S.groups.values()].sort((a, b) => (a.kind > b.kind ? -1 : a.kind < b.kind ? 1 : a.sort - b.sort)).map((g) => el('option', { value: String(g.id) }, (g.kind === 'Server' ? T('Server group: ') : T('Channel group: ')) + g.name)));
    const chSel = el('select', null, [...S.channels.values()].filter((c) => !spacer(c.name)).sort((a, b) => a.name.localeCompare(b.name)).map((c) => el('option', { value: String(c.id), selected: c.id === myChannel() }, c.name)));
    const chRow = el('div', { style: 'display:contents' }, field(T('Channel'), chSel));
    const upd = () => { chRow.style.display = S.groups.get(Number(group.value))?.kind === 'Channel' ? 'contents' : 'none'; };
    group.addEventListener('change', upd);
    upd();
    const desc = el('input', { type: 'text', maxlength: '80' });
    const out = el('div');
    S.wait.PrivilegeKeyCreated = (key) => {
      put(out, el('div', { class: 'section' }, T('Privilege key')), el('div', { class: 'key' }, key), el('div', { class: 'actions-row' }, el('button', { class: 'btn', onclick: () => copy(key) }, T('📋 Copy'))), el('p', { class: 'dim small' }, T('Give this key to the person; they use it once (server menu ▸ Use Privilege Key) to join the group.')));
    };
    dialog(T('Create privilege key'), el('div', null, el('div', { class: 'form' }, field(T('Group'), group), chRow, field(T('Description'), desc)), out), [
      { label: T('Close') },
      { label: T('Create key'), primary: true, onClick: () => {
        const g = S.groups.get(Number(group.value));
        send({ CreatePrivilegeKey: { group: g.id, channel: g.kind === 'Channel' ? Number(chSel.value) : null, description: desc.value } });
        return false;
      } },
    ], { onClose: () => { delete S.wait.PrivilegeKeyCreated; } });
  }
  function usePrivilegeKey() {
    promptDialog(T('Use privilege key'), 'Key', '', T('Use key')).then((k) => k && k.trim() && send({ UsePrivilegeKey: k.trim() }));
  }

  function serverGroupsDialog(c) {
    const box = el('div', { class: 'checks', style: 'grid-template-columns:1fr' });
    const draw = () => {
      const cur = S.clients.get(c.id) || c;
      put(box, ...[...S.groups.values()].filter((g) => g.kind === 'Server').sort((a, b) => a.sort - b.sort).map((g) => el('label', null,
        el('input', { type: 'checkbox', checked: cur.server_groups.includes(g.id), onchange: (e) => send({ SetServerGroup: { uid: c.uid, group: g.id, add: e.target.checked } }) }), g.name)));
    };
    draw();
    S.wait.ClientUpdated = (v) => { if (v.id === c.id) draw(); };
    dialog(T('Server groups of ') + c.nickname, box, [{ label: T('Close') }], { onClose: () => { delete S.wait.ClientUpdated; } });
  }
  function channelGroupDialog(c) {
    const ch = S.channels.get(c.channel);
    const sel = groupSelect('Channel', c.channel_group);
    dialog(T('Channel group of ') + c.nickname, el('div', { class: 'form' }, field(T('In channel'), el('span', null, ch ? ch.name : '')), field(T('Channel group'), sel)), [
      { label: T('Cancel') },
      { label: T('Save'), primary: true, onClick: () => send({ SetChannelGroup: { uid: c.uid, channel: c.channel, group: Number(sel.value) } }) },
    ]);
  }
  function clientPermsDialog(c) {
    let perms = null;
    const box = el('div', null, el('span', { class: 'dim' }, T('Loading…')));
    S.wait.ClientPerms = (v) => {
      if (v.uid !== c.uid) return;
      perms = BigInt(v.perms);
      put(box, el('p', { class: 'dim small', style: 'margin-top:0' }, T('Granted to ') + c.nickname + T(' on top of their groups.')), permChecks(perms, (p) => { perms = p; }));
    };
    send({ GetClientPerms: c.uid });
    dialog(T('Permissions of ') + c.nickname, box, [
      { label: T('Cancel') },
      { label: T('Save'), primary: true, onClick: () => { if (perms != null) send({ SetClientPerms: { uid: c.uid, perms: Number(perms) } }); } },
    ], { wide: true, onClose: () => { delete S.wait.ClientPerms; } });
  }

  // Uptime and "connected for" keep counting.
  setInterval(() => { if (S.connected && S.showRight && S.right === 'info') schedule(); }, 10000);

  boot();
})();
