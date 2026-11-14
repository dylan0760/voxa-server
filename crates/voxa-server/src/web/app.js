// Voxa web admin page: the server as the Voxa client shows it, in a browser.
// Everything the user or other people typed is inserted as text, never as HTML.
'use strict';
(function () {
  const C = globalThis.VoxaCrypto;

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
    return pad(d.getHours()) + ':' + pad(d.getMinutes());
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
  const THEMES = [['dark', 'Dark'], ['light', 'Light'], ['sakura', 'Sakura'], ['layette', 'Layette'], ['crimson', 'Crimson'], ['midnight', 'Midnight'], ['celadon', 'Celadon']];
  function applyTheme(t, fromServer) {
    if (!THEMES.some(([k]) => k === t)) t = 'dark';
    document.documentElement.dataset.theme = t;
    try { localStorage.setItem('voxa-theme', t); } catch (_) { /* private mode */ }
    if (!fromServer && typeof savePrefs === 'function') savePrefs({ theme: t });
  }
  applyTheme(localStorage.getItem('voxa-theme') || (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'), true);
  function themeMenu(x, y) {
    const cur = document.documentElement.dataset.theme;
    menu(x, y, [{ head: 'Theme' }, ...THEMES.map(([k, label]) => ({ label: (k === cur ? '✔ ' : '\u2003 ') + label, onClick: () => applyTheme(k) }))]);
  }

  // ---------------------------------------------------------------- permissions
  const P = {
    SERVER_EDIT: 0, CHANNEL_CREATE_PERMANENT: 1, CHANNEL_CREATE_SEMI_PERMANENT: 2, CHANNEL_CREATE_TEMPORARY: 3,
    CHANNEL_EDIT: 4, CHANNEL_DELETE: 5, CHANNEL_JOIN_IGNORE_PASSWORD: 6, CHANNEL_JOIN_IGNORE_MAXCLIENTS: 7,
    CLIENT_MOVE: 8, CLIENT_KICK_CHANNEL: 9, CLIENT_KICK_SERVER: 10, CLIENT_BAN: 11, BAN_LIST: 12, CLIENT_POKE: 13,
    PRIVATE_MESSAGE: 14, SERVER_CHAT: 15, CHANNEL_CHAT: 16, CHAT_DELETE: 17, GROUP_MANAGE: 18, GROUP_ASSIGN_SERVER: 19,
    GROUP_ASSIGN_CHANNEL: 20, TALK_POWER_GRANT: 21, PRIVILEGE_KEY_CREATE: 22, FILE_BROWSE: 23, FILE_UPLOAD: 24,
    FILE_DOWNLOAD: 25, FILE_DELETE: 26, SCREEN_SHARE: 27, CHANNEL_COMMANDER: 28, CHAT_HISTORY: 29,
    CLIENT_DESCRIPTION_EDIT: 30, IGNORE_SERVER_MAXCLIENTS: 31, CHANNEL_JOIN: 32, CHANNEL_JOIN_IGNORE_POWER: 33,
    CHAT_CLEAR: 34, CHAT_PIN: 35, CHAT_EDIT_OWN: 36,
  };
  const PERM_NAMES = [
    [P.SERVER_EDIT, 'Edit virtual server settings'], [P.CHANNEL_CREATE_PERMANENT, 'Create permanent channels'],
    [P.CHANNEL_CREATE_SEMI_PERMANENT, 'Create semi-permanent channels'], [P.CHANNEL_CREATE_TEMPORARY, 'Create temporary channels'],
    [P.CHANNEL_EDIT, 'Edit channels'], [P.CHANNEL_DELETE, 'Delete channels'], [P.CHANNEL_JOIN, 'Join / switch channels'],
    [P.CHANNEL_JOIN_IGNORE_POWER, 'Join channels regardless of join power'], [P.CHANNEL_JOIN_IGNORE_PASSWORD, 'Join channels without password'],
    [P.CHANNEL_JOIN_IGNORE_MAXCLIENTS, 'Join full channels'], [P.CLIENT_MOVE, 'Move clients'], [P.CLIENT_KICK_CHANNEL, 'Kick clients from channel'],
    [P.CLIENT_KICK_SERVER, 'Kick clients from server'], [P.CLIENT_BAN, 'Ban clients'], [P.BAN_LIST, 'View and edit ban list'],
    [P.CLIENT_POKE, 'Poke clients'], [P.PRIVATE_MESSAGE, 'Send private messages'], [P.SERVER_CHAT, 'Send server chat messages'],
    [P.CHANNEL_CHAT, 'Send channel chat messages'], [P.CHAT_HISTORY, 'Read chat history'], [P.CHAT_DELETE, "Delete anyone's chat messages (incl. history)"],
    [P.CHAT_CLEAR, 'Clear entire chat history'], [P.CHAT_PIN, 'Pin messages'], [P.CHAT_EDIT_OWN, 'Edit own chat messages'], [P.GROUP_MANAGE, 'Create, edit and delete groups'],
    [P.GROUP_ASSIGN_SERVER, 'Assign server groups'], [P.GROUP_ASSIGN_CHANNEL, 'Assign channel groups'], [P.TALK_POWER_GRANT, 'Grant / revoke talk power'],
    [P.PRIVILEGE_KEY_CREATE, 'Create privilege keys'], [P.FILE_BROWSE, 'Browse channel files'], [P.FILE_UPLOAD, 'Upload files'],
    [P.FILE_DOWNLOAD, 'Download files'], [P.FILE_DELETE, 'Delete files'], [P.SCREEN_SHARE, 'Share screen / applications'],
    [P.CHANNEL_COMMANDER, 'Use channel commander'], [P.CLIENT_DESCRIPTION_EDIT, 'Edit client descriptions'], [P.IGNORE_SERVER_MAXCLIENTS, 'Join when server is full'],
  ];
  const bit = (n) => 1n << BigInt(n);
  function myPerms() {
    const me = S.clients.get(S.me);
    if (!me) return 0n;
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
  const CODEC_NAMES = { voice: 'Opus Voice', music: 'Opus Music', lossless: 'Lossless' };
  const KIND_NAMES = { Permanent: 'Permanent', SemiPermanent: 'Semi-Permanent', Temporary: 'Temporary' };
  const keyOf = (t) => (t === 'Server' ? 'server' : t.Channel !== undefined ? 'ch:' + t.Channel : null);
  const IMAGE_RE = /^📷 ([\s\S]*)\n\[voxa-image:(\d+):(\d+)x(\d+)\]$/;
  function leaveText(r) {
    if (r === 'TimedOut') return 'timed out';
    if (r.Disconnected !== undefined) return r.Disconnected ? 'left (' + r.Disconnected + ')' : 'left';
    if (r.Kicked) return 'was kicked by ' + r.Kicked.by + (r.Kicked.reason ? ': ' + r.Kicked.reason : '');
    if (r.Banned) return 'was banned by ' + r.Banned.by + (r.Banned.reason ? ': ' + r.Banned.reason : '');
    return 'left';
  }

  // ---------------------------------------------------------------- state
  const S = {
    me: 0, uid: '', server: null, channels: new Map(), clients: new Map(), groups: new Map(), extra: 0,
    logs: new Map(), tab: 'channel', dms: [], selected: { kind: 'server' }, right: 'info', showRight: true,
    people: [], peopleAsked: 0, collapsed: new Set(), pins: null, showPins: false, wait: {}, dragging: false,
    connected: false, msgDirty: true,
  };
  let es = null;

  function log(key) {
    let l = S.logs.get(key);
    if (!l) S.logs.set(key, (l = { lines: [], unread: 0, loaded: false, complete: false }));
    return l;
  }
  const myChannel = () => S.clients.get(S.me)?.channel;
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
      let msg = 'Request failed (' + r.status + ')';
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
      put(document.getElementById('root'), el('div', { class: 'login' }, el('div', { class: 'login-card' }, el('h1', null, 'Unsupported browser'), el('p', { class: 'sub' }, 'This browser computed the sign-in cryptography wrongly. Please use a current Chrome, Edge, Firefox or Safari.'))));
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
    if (!priv || priv.length !== 32) throw new Error('That is not a Voxa identity key (it should be 44 characters ending in "=").');
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
    status('Waiting for Voxa… Confirm the sign-in in the Voxa window.');
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
        else if (Date.now() - started > 120000) { done(); localStorage.removeItem(PENDING); reject(new Error('Voxa did not answer. Is it installed and running? You can paste your identity key instead.')); }
      }, 2000);
    });
  }

  async function finishClientSignIn(data) {
    let d;
    try { d = JSON.parse(new TextDecoder().decode(C.b64decode(data.replace(/-/g, '+').replace(/_/g, '/')))); } catch (_) { throw new Error('The sign-in from Voxa could not be read.'); }
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
          el('img', { class: 'logo', src: '/icon.svg', alt: '' }), el('h1', null, 'Signed in'), el('p', { class: 'sub' }, 'You are signed in in your other tab. You can close this one.'))));
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
    const key = el('textarea', { placeholder: 'Paste your exported identity key', spellcheck: 'false', autocomplete: 'off' });
    const nick = el('input', { type: 'text', value: localStorage.getItem('voxa-web-nick') || '', placeholder: 'Last used, or "Web admin"', maxlength: '30' });
    const pw = el('input', { type: 'password', placeholder: 'Server password', autocomplete: 'current-password' });
    // The channel to join: a list when the server shares it, otherwise a path.
    const chSel = channels.length
      ? el('select', null,
        el('option', { value: '\u0001', selected: lastChannel == null }, 'The one I used last time'),
        el('option', { value: '', selected: lastChannel === '' }, 'The default channel'),
        channels.map((c) => el('option', { value: c.path, selected: c.path === lastChannel }, c.path + (c.password ? '  🔒' : ''))))
      : el('input', { type: 'text', placeholder: 'e.g. Lobby/Games (empty: the one used last time)', value: lastChannel || '' });
    const chPw = el('input', { type: 'password', placeholder: 'Channel password', autocomplete: 'off' });
    const chPwRow = el('div', { style: 'display:flex;flex-direction:column;gap:6px' }, el('label', null, 'Channel password'), chPw);
    const updPw = () => { const c = channels.find((x) => x.path === chSel.value); chPwRow.classList.toggle('hidden', !(c && c.password) && channels.length > 0); };
    chSel.addEventListener('change', updPw);
    updPw();
    const remember = el('input', { type: 'checkbox', id: 'remember' });
    const err = el('div', { class: 'error' }, error || '');
    const btn = el('button', { class: 'btn primary', type: 'submit' }, 'Sign in with key');
    const voxaBtn = el('button', { class: 'btn primary', type: 'button' }, '🎧 Sign in with Voxa');
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
      el('div', { class: 'help', style: 'margin:2px 0 6px;text-align:center' }, 'Uses the identity Voxa on this PC has for this server. Voxa asks you to confirm.'),
      el('div', { class: 'or' }, 'or paste an identity key'),
      key,
      el('div', { class: 'row' }, remember, el('label', { for: 'remember', style: 'margin:0;color:var(--text);font-size:13px' }, 'Remember the key in this browser')),
      btn,
      el('div', { class: 'or' }, 'options'),
      el('label', null, 'Nickname'), nick,
      el('label', null, 'Join channel'), chSel,
      chPwRow,
      info.has_password ? [el('label', null, 'Server password'), pw] : null,
      err);
    put(document.getElementById('root'),
      el('div', { class: 'login' },
        el('button', { class: 'icon-btn login-theme', title: 'Theme', onclick: (e) => themeMenu(e.clientX - 160, e.clientY + 10) }, '🎨'),
        el('div', { class: 'login-card' },
          el('img', { class: 'logo', src: '/icon.svg', alt: '' }),
          el('h1', null, info.name || 'Voxa Server'),
          el('div', { class: 'sub' }, 'Server administration · Voxa Server ' + (info.version || '')),
          form,
          el('div', { class: 'help' }, 'You get the same rights here as your identity has in Voxa. To get a key, in Voxa click your name at the bottom left ▸ Identities and export the one you use on this server. Keys stay in this browser: only a proof that you own one is sent.'),
          info.fingerprint ? el('div', { class: 'fp' }, 'Server fingerprint: ' + info.fingerprint) : null)));
  }

  async function signOut() {
    localStorage.removeItem('voxa-web-key');
    sessionStorage.removeItem('voxa-web-pw');
    try { await post('/api/logout'); } catch (_) { /* already gone */ }
    showLogin('Signed out.');
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
    es = new EventSource('/api/events', { withCredentials: true });
    es.onmessage = (e) => {
      let m;
      try { m = JSON.parse(e.data); } catch (_) { return; }
      const [kind, v] = typeof m === 'string' ? [m, null] : Object.entries(m)[0];
      handle(kind, v);
    };
    es.onerror = async () => {
      if (!S.connected) {
        if ((await signedIn()) === false) showLogin('Your sign-in expired. Please sign in again.');
        return;
      }
      S.connected = false;
      banner('Connection lost, reconnecting…');
      if ((await signedIn()) === false) showLogin('Your sign-in expired. Please sign in again.');
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
      { label: 'Sign out', onClick: signOut },
      { label: 'Reconnect', primary: true, onClick: () => connect() },
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
        S.me = v.your_id;
        S.uid = v.your_uid;
        S.server = v.server;
        S.channels = new Map(v.channels.map((c) => [c.id, c]));
        S.clients = new Map(v.clients.map((c) => [c.id, c]));
        S.groups = new Map(v.groups.map((g) => [g.id, g]));
        S.logs = new Map();
        S.editing = null;
        avatars.clear();
        S.connected = true;
        S.msgDirty = true;
        banner('');
        closeDialogs(true);
        buildApp();
        loadPrefs();
        activateTab(S.tab.startsWith('dm:') ? S.tab : S.tab);
        break;
      }
      case 'Refused':
        stopped('Could not join the server', v);
        break;
      case 'Kicked':
        stopped('Disconnected', 'You ' + leaveText(v) + '.');
        break;
      case 'Error':
        toast(v, 'bad');
        break;
      case 'Notice':
        toast(v);
        break;
      case 'ServerUpdated':
        S.server = v;
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
        break;
      case 'ClientLeft': {
        const c = S.clients.get(v.id);
        if (c) pushEvent('server', c.nickname + ' ' + leaveText(v.reason), v.reason.Kicked || v.reason.Banned ? 'bad' : '');
        S.clients.delete(v.id);
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
          } else if (was === myChannel() || v.channel === myChannel()) {
            const ch = S.channels.get(v.channel);
            pushEvent('ch:' + myChannel(), c.nickname + (v.channel === myChannel() ? ' joined the channel' : ' went to ' + (ch ? ch.name : 'another channel')) + (v.by ? ' (moved by ' + v.by + ')' : ''));
          }
        }
        break;
      }
      case 'ClientUpdated':
        S.clients.set(v.id, v);
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
        pushEvent(key, 'The chat history was cleared by ' + v.by, 'warn');
        S.msgDirty = true;
        break;
      }
      case 'Poked':
        toast(v.from_name + ' poked you' + (v.message ? ': ' + v.message : ''), 'warn');
        break;
      case 'GroupsUpdated':
        S.groups = new Map(v.map((g) => [g.id, g]));
        break;
      case 'YourPerms':
        S.extra = v;
        break;
      case 'Pins':
        if (keyOf(v.target) === tabKey()) S.pins = v.pins;
        break;
      case 'PinChanged':
        if (keyOf(v.target) === tabKey() && S.showPins) send({ GetPins: v.target });
        pushEvent(keyOf(v.target), v.by + (v.pinned ? ' pinned a message' : ' unpinned a message'));
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
        toast(v.length + ' direct message(s) arrived while you were away');
        break;
      case 'OfflineQueued':
        pushEvent('dm:' + v.to_uid, personName(v.to_uid) + ' is offline: the message will be delivered when they come online');
        break;
      default:
        break;
    }
    const w = S.wait[kind];
    if (w) w(v);
    schedule();
  }

  // ---------------------------------------------------------------- rendering
  let pending = false;
  function schedule() {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      if (!S.connected || !$('.app')) return;
      render();
    });
  }

  let R = {}; // persistent elements of the layout
  function buildApp() {
    const search = el('input', { type: 'text', class: 'search', placeholder: '🔍 Search people', oninput: () => schedule() });
    const input = el('textarea', { rows: '1', placeholder: 'Message', onkeydown: onComposerKey, oninput: autoGrow });
    R = {
      app: el('div', { class: 'app' }),
      rail: el('nav', { class: 'rail' }),
      sideHead: el('div', { class: 'side-head', title: 'Server menu', onclick: (e) => serverMenu(e.clientX, e.clientY), oncontextmenu: (e) => { e.preventDefault(); serverMenu(e.clientX, e.clientY); } }),
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
    R.composer = el('div', { class: 'composer' }, input, el('button', { class: 'icon-btn send', title: 'Send (Enter)', onclick: sendComposer }, '➤'));
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
      el('div', { class: 'srv', title: name, onclick: () => { S.selected = { kind: 'server' }; S.right = 'info'; S.showRight = true; schedule(); } }, initials(name)),
      el('div', { class: 'sep' }),
      el('div', { class: 'grow' }),
      el('button', { class: 'icon-btn', title: 'Theme', onclick: (e) => themeMenu(e.clientX + 10, e.clientY - 250) }, '🎨'),
      el('button', { class: 'icon-btn', title: 'Sign out', onclick: () => confirmDialog('Sign out', 'Sign out of the web admin page?', 'Sign out').then((ok) => ok && signOut()) }, '⏻'));
  }

  function renderSide() {
    put(R.sideHead, el('span', { class: 'name' }, S.server ? S.server.name : ''), el('span', { class: 'dim' }, '▾'));
    if (!S.dragging) renderTree();
    const online = S.clients.size;
    put(R.status, 
      el('span', { style: 'color:var(--green);font-size:16px' }, '🌐'),
      el('div', { class: 'grow' }, el('div', { class: 't' }, 'Web admin connected'), el('div', { class: 'dim small' }, online + ' online · ' + location.host)));
    const me = S.clients.get(S.me);
    if (!me) return;
    const away = me.away != null;
    put(R.meCard, 
      avatar(me.nickname, me.uid, 36, away ? 'var(--yellow)' : 'var(--green)'),
      el('div', { class: 'who' }, el('div', { class: 'n' }, me.nickname), el('div', { class: 'small', style: 'color:' + (away ? 'var(--yellow)' : 'var(--green)') }, away ? 'Away' + (me.away ? ': ' + me.away : '') : 'Online')),
      el('button', { class: 'icon-btn' + (away ? ' on' : ''), title: away ? 'Set online' : 'Set away', onclick: toggleAway }, '🌙'),
      el('button', { class: 'icon-btn', title: 'Change nickname', onclick: changeNickname }, '✏️'));
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
        ch.is_default ? el('span', { title: 'Default channel' }, '🏠') : null,
        ch.kind === 'Temporary' ? el('span', { title: 'Temporary channel' }, '⏳') : null,
        ch.max_clients != null ? el('span', { title: 'Max clients' }, n + '/' + ch.max_clients) : n && S.collapsed.has(ch.id) ? el('span', null, String(n)) : null));
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
        tags.map((t) => { const col = nameColor(t); return el('span', { class: 'gtag', style: `color:${col};background:${col}40` }, t); })),
      el('span', { class: 'badges' },
        c.platform === 'Web' ? el('span', { class: 'webtag', title: 'On the web admin page' }, 'WEB') : null,
        c.streaming ? el('span', { class: 'live', title: c.streaming.title + ' (' + c.streaming.width + 'x' + c.streaming.height + ')' }, 'LIVE') : null,
        c.output_muted ? el('span', { title: 'Speakers muted', style: 'color:var(--red)' }, '🔇') : null,
        c.input_muted ? el('span', { title: 'Microphone muted', style: 'color:var(--red)' }, '🎤') : null,
        c.talk_request != null ? el('span', { title: 'Requests talk power: ' + (c.talk_request || '') }, '✋') : null,
        c.commander ? el('span', { title: 'Channel commander', style: 'color:var(--yellow)' }, '★') : null,
        c.away != null ? el('span', { title: c.away ? 'Away: ' + c.away : 'Away', style: 'color:var(--yellow)' }, '🌙') : null));
    return row;
  }

  function select(sel) {
    S.selected = sel;
    S.right = 'info';
    schedule();
  }

  // ---------------------------------------------------------------- chat
  function activateTab(tab) {
    S.tab = tab;
    S.pins = null;
    const key = tabKey();
    const l = log(key);
    l.unread = 0;
    S.msgDirty = true;
    const target = tabTarget();
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
    if (S.tab === 'server') { title = '🖥  ' + (S.server ? S.server.name : 'Server'); sub = 'Server chat: everyone on the server'; }
    else if (S.tab === 'channel') { const ch = S.channels.get(myChannel()); title = '#  ' + (ch ? ch.name : ''); sub = ch && ch.topic ? ch.topic : 'Channel chat'; }
    else { const uid = S.tab.slice(3); title = '@  ' + personName(uid); sub = onlineByUid(uid) ? 'Direct message' : 'Offline · gets your messages later'; }
    const isDm = S.tab.startsWith('dm:');
    const peopleOn = S.showRight && S.right === 'people';
    put(R.head, 
      el('div', { class: 't' }, el('div', { class: 'title' }, title), el('div', { class: 'subtitle' }, sub)),
      !isDm ? el('button', { class: 'icon-btn' + (S.showPins ? ' on' : ''), title: 'Pinned messages', onclick: togglePins }, '📌') : null,
      !isDm && can(P.CHAT_CLEAR) ? el('button', { class: 'icon-btn', title: 'Clear the entire chat history', onclick: clearChat }, '🗑') : null,
      isDm ? el('button', { class: 'icon-btn', title: 'Close this conversation', onclick: () => closeDm(S.tab.slice(3)) }, '✕') : null,
      el('button', { class: 'icon-btn' + (S.showRight ? ' on' : ''), title: 'Show / hide info panel', onclick: () => { S.showRight = !S.showRight; schedule(); } }, 'ℹ'),
      el('button', { class: 'pill' + (peopleOn ? ' on' : ''), title: 'Everyone on this server, online and offline', onclick: () => { S.showRight = !peopleOn; S.right = 'people'; schedule(); } }, '👥 People'));
  }

  function renderTabs() {
    const tab = (id, label, unread, close) => el('button', { class: 'tab' + (S.tab === id ? ' on' : ''), onclick: () => activateTab(id) },
      label, unread ? el('span', { class: 'unread' }, unread) : null,
      close ? el('span', { class: 'x', title: 'Close', onclick: (e) => { e.stopPropagation(); closeDm(close); } }, '✕') : null);
    const ch = S.channels.get(myChannel());
    put(R.tabs, 
      tab('server', '🖥 Server', S.tab === 'server' ? 0 : log('server').unread),
      tab('channel', '# ' + (ch ? ch.name : 'Channel'), S.tab === 'channel' ? 0 : log('ch:' + myChannel()).unread),
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
    if (!S.pins) { put(R.pins, el('span', { class: 'dim' }, 'Loading pinned messages…')); return; }
    if (!S.pins.length) { put(R.pins, el('span', { class: 'dim' }, '📌 No pinned messages in this chat.')); return; }
    put(R.pins, ...S.pins.map((p) => el('div', { class: 'pin' },
      el('span', null, '📌'),
      el('div', { style: 'flex:1;min-width:0' }, el('div', null, el('span', { class: 'strong' }, p.message.from_name), el('span', { class: 'dim small' }, '  ' + fmtDate(p.message.ts) + ' · pinned by ' + p.pinned_by)), el('div', { class: 'desc' }, messageText(p.message.text))),
      can(P.CHAT_PIN) ? el('button', { class: 'btn small', onclick: () => send({ PinMessage: { target: tabTarget(), id: p.message.id, pin: false } }) }, 'Unpin') : null)));
  }

  function messageText(text) {
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
    if (target && l.loaded && !l.complete && l.lines.some((x) => x.msg)) {
      out.push(el('div', { class: 'older' }, el('button', { class: 'btn small', onclick: () => {
        const oldest = l.lines.find((x) => x.msg);
        send({ FetchHistory: { target, before: oldest ? oldest.msg.id : null, limit: 50 } });
      } }, 'Load older messages')));
    }
    if (!l.lines.length) {
      out.push(el('div', { class: 'event', style: 'padding-left:6px' }, isDm ? 'This is the start of your conversation. On the web page direct messages are not kept after you close it.' : 'No messages yet.'));
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
        canEdit && !editing ? el('button', { class: 'icon-btn', title: 'Edit message', onclick: () => startEdit(key, m) }, '✏️') : null,
        el('button', { class: 'icon-btn', title: 'Copy text', onclick: () => copy(m.text) }, '📋'),
        !isDm && can(P.CHAT_PIN) && m.id ? el('button', { class: 'icon-btn', title: 'Pin', onclick: () => send({ PinMessage: { target, id: m.id, pin: true } }) }, '📌') : null,
        canDel ? el('button', { class: 'icon-btn', title: isDm ? 'Remove from this page' : 'Delete message', style: 'color:var(--red)', onclick: () => deleteMessage(m) }, '🗑') : null);
      out.push(el('div', { class: 'msg' + (cont ? ' cont' : '') },
        avatar(m.from_name, m.from_uid, 36),
        el('div', { class: 'body' },
          cont ? null : el('div', { class: 'head' }, el('span', { class: 'from' }, m.from_name), el('span', { class: 'time' }, fmtTime(m.ts))),
          editing ? [S.editing.box, el('div', { class: 'edit-hint' }, 'Enter to save · Esc to cancel · Shift+Enter for a new line')]
            : el('div', { class: 'text', title: fmtDate(m.ts) }, messageText(m.text), m.edited ? el('span', { class: 'edited', title: 'Edited ' + fmtDate(m.edited) }, '(edited)') : null)),
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
    confirmDialog('Delete message', 'Delete this message for everyone? It is also removed from the stored history.', 'Delete', true)
      .then((ok) => ok && send({ DeleteMessage: { target: tabTarget(), id: m.id } }));
  }
  function clearChat() {
    const target = tabTarget();
    confirmDialog('Clear chat history', 'This removes all messages for everyone and permanently deletes them from the server\'s chat history. It cannot be undone.', 'Clear history', true)
      .then((ok) => ok && send({ ClearChat: target }));
  }

  function renderComposer() {
    let placeholder, allowed = true, note;
    if (S.tab === 'server') { placeholder = 'Message the server'; allowed = can(P.SERVER_CHAT); note = '🔒 Messages are encrypted in transit and stored encrypted on the server'; }
    else if (S.tab === 'channel') { const ch = S.channels.get(myChannel()); placeholder = 'Message #' + (ch ? ch.name : ''); allowed = can(P.CHANNEL_CHAT); note = '🔒 Messages are encrypted in transit and stored encrypted on the server'; }
    else { placeholder = 'Message @' + personName(S.tab.slice(3)); allowed = can(P.PRIVATE_MESSAGE); note = '🔒 Encrypted in transit · direct messages are not kept by the web page'; }
    if (!allowed) placeholder = 'You do not have permission to write here';
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
    if (S.tab === 'server') send({ SendText: { target: 'Server', text } });
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

  // ---------------------------------------------------------------- info panel
  function renderInfo() {
    if (!S.showRight) return;
    put(R.infoTabs, 
      el('button', { class: 'pill' + (S.right === 'info' ? ' on' : ''), onclick: () => { S.right = 'info'; schedule(); } }, 'ℹ Info'),
      el('button', { class: 'pill' + (S.right === 'people' ? ' on' : ''), onclick: () => { S.right = 'people'; schedule(); } }, '👥 People'));
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
      el('div', { class: 'section' }, 'Server'),
      kv([['Version', 'Voxa Server ' + s.version], ['Platform', s.platform], ['Online', S.clients.size + ' / ' + s.max_clients], ['Channels', String(S.channels.size)], ['Uptime', fmtDuration(now() - s.started)], ['Created', fmtDate(s.created)]]),
      el('div', { class: 'section' }, 'Security'),
      kv([['Fingerprint', s.fingerprint, true], ['Password', s.has_password ? 'Yes' : 'No'], ['Chat history', s.chat_retention_days ? 'Encrypted on server, kept ' + s.chat_retention_days + ' days' : 'Encrypted on server'], ['Voice', 'Encrypted']]),
      s.welcome_message ? [el('div', { class: 'section' }, 'Welcome message'), el('div', { class: 'desc' }, s.welcome_message)] : null,
      s.host_message ? [el('div', { class: 'section' }, 'Host message'), el('div', { class: 'desc' }, s.host_message)] : null,
      can(P.SERVER_EDIT) ? el('div', { class: 'actions-row' }, el('button', { class: 'btn', onclick: editServer }, '⚙ Edit server')) : null,
    ].flat().filter(Boolean);
  }

  function channelInfo(ch) {
    const sp = spacer(ch.name);
    const n = clientsIn(ch.id).length;
    const codec = codecOf(ch.codec_quality);
    return [
      el('div', { class: 'info-hero' }, el('span', { class: 'avatar sq', style: 'width:52px;height:52px;font-size:24px;background:var(--card);color:var(--dim)' }, sp ? '―' : ch.has_password ? '🔒' : '💬'),
        el('div', null, el('div', { class: 'n' }, sp ? 'Separator' : ch.name), el('div', { class: 'dim small' }, sp ? ch.name : ch.topic || ''))),
      el('div', { class: 'section' }, 'Channel'),
      kv(sp ? [['Type', 'Separator (nobody can join it)']] : [
        ['Type', KIND_NAMES[ch.kind] + (ch.is_default ? ' · default channel' : '')],
        ['Codec', CODEC_NAMES[codec] + (codec === 'lossless' ? '' : ' · quality ' + qualityOf(ch.codec_quality))],
        ['Bitrate', '~' + bitrate(ch.codec_quality) + ' kbit/s' + (codec === 'voice' ? '' : ' stereo')],
        ['Clients', n + (ch.max_clients != null ? ' / ' + ch.max_clients : '')],
        ['Password', ch.has_password ? 'Yes' : 'No'],
        ['Talk power', ch.needed_talk_power ? 'needs ' + ch.needed_talk_power : 'anyone'],
        ['Join power', ch.needed_join_power ? 'needs ' + ch.needed_join_power : 'anyone'],
        ['Voice', ch.voice_encrypted ? 'Encrypted' : 'Not encrypted'],
      ]),
      ch.description ? [el('div', { class: 'section' }, 'Description'), el('div', { class: 'desc' }, ch.description)] : null,
      el('div', { class: 'actions-row' },
        !sp && ch.id !== myChannel() ? el('button', { class: 'btn', onclick: () => joinChannel(ch) }, '➜ Join') : null,
        can(P.CHANNEL_EDIT) ? el('button', { class: 'btn', onclick: () => channelDialog(ch) }, '✏️ Edit') : null,
        can(P.CHANNEL_DELETE) ? el('button', { class: 'btn danger', onclick: () => deleteChannel(ch) }, '🗑 Delete') : null),
    ].flat().filter(Boolean);
  }

  function clientInfo(c) {
    const ch = S.channels.get(c.channel);
    const groups = c.server_groups.map((g) => S.groups.get(g)?.name).filter(Boolean).join(', ');
    const away = c.away != null;
    return [
      el('div', { class: 'info-hero' }, avatar(c.nickname, c.uid, 52, away ? 'var(--yellow)' : 'var(--green)'),
        el('div', null, el('div', { class: 'n' }, c.nickname), el('div', { class: 'small', style: 'color:' + (away ? 'var(--yellow)' : 'var(--green)') }, away ? 'Away' + (c.away ? ': ' + c.away : '') : 'Online'))),
      el('div', { class: 'section' }, 'Client'),
      kv([
        ['Channel', ch ? ch.name : ''],
        ['Server groups', groups || '—'],
        ['Channel group', S.groups.get(c.channel_group)?.name || '—'],
        ['Platform', c.platform === 'Web' ? 'Web admin page' : c.platform],
        c.platform === 'Web' ? null : ['Version', 'Voxa ' + c.version],
        ['Connected', fmtDuration(now() - c.connected_at)],
        ['Microphone', c.input_muted ? 'Muted' : 'On'],
        ['Speakers', c.output_muted ? 'Muted' : 'On'],
        c.streaming ? ['Sharing', c.streaming.title + ' (' + c.streaming.width + '×' + c.streaming.height + ')'] : null,
        ['Unique ID', c.uid, true],
      ]),
      c.description ? [el('div', { class: 'section' }, 'Description'), el('div', { class: 'desc' }, c.description)] : null,
      c.id !== S.me ? el('div', { class: 'actions-row' },
        el('button', { class: 'btn', onclick: () => { openDm(c.uid, c.nickname); activateTab('dm:' + c.uid); } }, '💬 Message'),
        can(P.CLIENT_POKE) ? el('button', { class: 'btn', onclick: () => pokeClient(c) }, '👉 Poke') : null,
        el('button', { class: 'btn', onclick: (e) => clientMenu(c, e.clientX, e.clientY) }, '⋯ More')) : null,
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
    const row = (uid, name, sub, isOnline) => el('div', { class: 'person' + (isOnline ? '' : ' off'), title: isOnline ? 'Send a direct message' : 'Send a message: it is delivered when they come online', onclick: () => { openDm(uid, name); activateTab('dm:' + uid); } },
      avatar(name, uid, 30, isOnline ? 'var(--green)' : null),
      el('div', { style: 'min-width:0' }, el('div', { class: 'n' }, name), el('div', { class: 'dim small' }, sub)));
    return [
      el('div', { class: 'section' }, 'Online — ' + online.length),
      online.map((c) => row(c.uid, c.nickname, 'in ' + (S.channels.get(c.channel)?.name || '') + (c.platform === 'Web' ? ' · web' : ''), true)),
      offline.length ? el('div', { class: 'section' }, 'Offline — ' + offline.length) : null,
      offline.map((p) => row(p.uid, p.nickname, p.last_seen ? 'last seen ' + fmtDuration(now() - p.last_seen) + ' ago' : 'offline', false)),
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
      { head: S.server ? S.server.name : 'Server' },
      can(P.SERVER_EDIT) ? { label: '⚙  Edit Virtual Server…', onClick: editServer } : null,
      can(P.GROUP_MANAGE) ? { label: '👥  Groups & Permissions…', onClick: groupsDialog } : null,
      can(P.BAN_LIST) ? { label: '⛔  Ban List…', onClick: banListDialog } : null,
      can(P.PRIVILEGE_KEY_CREATE) ? { label: '🔑  Create Privilege Key…', onClick: privilegeKeyDialog } : null,
      { label: '🎟  Use Privilege Key…', onClick: usePrivilegeKey },
      { sep: true },
      canCreate() ? { label: '➕  Create Channel…', onClick: () => channelDialog(null, null) } : null,
      can(P.CHAT_CLEAR) ? { label: '🗑  Clear Server Chat…', danger: true, onClick: () => confirmDialog('Clear server chat', 'Delete every server chat message for everyone, including the stored history?', 'Clear history', true).then((ok) => ok && send({ ClearChat: 'Server' })) } : null,
      { label: '📋  Copy Fingerprint', onClick: () => copy(S.server.fingerprint) },
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
      { head: sp ? 'Separator' : ch.name },
      !sp && ch.id !== myChannel() ? { label: '➜  Join Channel', onClick: () => joinChannel(ch) } : null,
      { label: 'ℹ  Channel Info', onClick: () => { select({ kind: 'channel', id: ch.id }); S.showRight = true; } },
      { sep: true },
      canCreate() ? { label: '➕  Create Channel…', onClick: () => channelDialog(null, ch.parent ?? null) } : null,
      canCreate() && !sp ? { label: '➕  Create Sub-Channel…', onClick: () => channelDialog(null, ch.id) } : null,
      can(P.CHANNEL_EDIT) ? { label: '✏  Edit Channel…', onClick: () => channelDialog(ch) } : null,
      canCreate() && !sp ? { label: '⧉  Clone Channel…', onClick: () => cloneChannel(ch) } : null,
      can(P.CHANNEL_EDIT) ? { label: '▲  Move Up', disabled: i <= 0, onClick: () => moveChannel(ch, -1) } : null,
      can(P.CHANNEL_EDIT) ? { label: '▼  Move Down', disabled: i >= sib.length - 1, onClick: () => moveChannel(ch, 1) } : null,
      { label: '📋  Copy Channel Name', onClick: () => copy(ch.name) },
      can(P.CHANNEL_DELETE) ? { sep: true } : null,
      can(P.CHANNEL_DELETE) ? { label: '🗑  Delete Channel…', danger: true, onClick: () => deleteChannel(ch) } : null,
    ]);
  }

  function clientMenu(c, x, y) {
    if (c.id === S.me) {
      menu(x, y, [
        { head: c.nickname },
        { label: '✏  Change Nickname…', onClick: changeNickname },
        { label: '🌙  Set Away / Online', onClick: toggleAway },
        can(P.CHANNEL_COMMANDER) ? { label: '⭐  Toggle Channel Commander', onClick: () => send({ UpdateSelf: { commander: !c.commander } }) } : null,
        { label: '📝  Edit Description…', onClick: () => editDescription(c) },
        { label: '📋  Copy Unique ID', onClick: () => copy(c.uid) },
      ]);
      return;
    }
    menu(x, y, [
      { head: c.nickname },
      { label: '💬  Send Message', onClick: () => { openDm(c.uid, c.nickname); activateTab('dm:' + c.uid); } },
      can(P.CLIENT_POKE) ? { label: '👉  Poke…', onClick: () => pokeClient(c) } : null,
      can(P.CLIENT_MOVE) ? { label: '➜  Move to Channel…', onClick: () => moveClientDialog(c) } : null,
      can(P.CLIENT_MOVE) && c.channel !== myChannel() ? { label: '⤵  Move to My Channel', onClick: () => send({ MoveClient: { target: c.id, channel: myChannel() } }) } : null,
      can(P.TALK_POWER_GRANT) ? { label: c.is_talker ? '🗣  Revoke Talk Power' : '🗣  Grant Talk Power', onClick: () => send({ SetTalker: { target: c.id, talker: !c.is_talker } }) } : null,
      { sep: true },
      can(P.GROUP_ASSIGN_SERVER) ? { label: '👥  Server Groups…', onClick: () => serverGroupsDialog(c) } : null,
      can(P.GROUP_ASSIGN_CHANNEL) ? { label: '👥  Channel Group…', onClick: () => channelGroupDialog(c) } : null,
      can(P.GROUP_MANAGE) ? { label: '🔐  Permissions…', onClick: () => clientPermsDialog(c) } : null,
      can(P.CLIENT_DESCRIPTION_EDIT) ? { label: '📝  Edit Description…', onClick: () => editDescription(c) } : null,
      can(P.CLIENT_DESCRIPTION_EDIT) && avatars.get(c.uid)?.image ? { label: '🖼  Remove Avatar Picture…', danger: true, onClick: () => confirmDialog('Remove avatar picture', 'Remove the avatar picture of ' + c.nickname + '? They cannot use the same picture again on this server.', 'Remove', true).then((ok) => ok && send({ ClearAvatar: c.uid })) } : null,
      { label: '📋  Copy Unique ID', onClick: () => copy(c.uid) },
      { sep: true },
      can(P.CLIENT_KICK_CHANNEL) ? { label: '👢  Kick from Channel…', danger: true, onClick: () => kickClient(c, false) } : null,
      can(P.CLIENT_KICK_SERVER) ? { label: '👢  Kick from Server…', danger: true, onClick: () => kickClient(c, true) } : null,
      can(P.CLIENT_BAN) ? { label: '⛔  Ban…', danger: true, onClick: () => banClient(c) } : null,
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
      el('div', { class: 'dh' }, title, opts.noClose ? null : el('button', { class: 'icon-btn', title: 'Close', onclick: close }, '✕')),
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
  function confirmDialog(title, text, ok, danger) {
    return new Promise((res) => {
      let done = false;
      dialog(title, el('p', { style: 'margin:0' }, text), [
        { label: 'Cancel', onClick: () => { done = true; res(false); } },
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
        { label: 'Cancel', onClick: () => { done = true; res(null); } },
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

  function joinChannel(ch) {
    if (spacer(ch.name)) return;
    if (ch.has_password && !can(P.CHANNEL_JOIN_IGNORE_PASSWORD)) {
      promptDialog('Join ' + ch.name, 'Password', '', 'Join', { password: true }).then((pw) => pw != null && send({ SwitchChannel: { channel: ch.id, password: pw } }));
    } else send({ SwitchChannel: { channel: ch.id, password: null } });
  }

  function changeNickname() {
    const me = S.clients.get(S.me);
    promptDialog('Change nickname', 'Nickname', me ? me.nickname : '', 'Change', { max: 30 }).then((n) => {
      if (n && n.trim()) {
        send({ UpdateSelf: { nickname: n.trim() } });
        localStorage.setItem('voxa-web-nick', n.trim());
        savePrefs({ nickname: n.trim() });
      }
    });
  }
  function toggleAway() {
    const me = S.clients.get(S.me);
    if (!me) return;
    if (me.away != null) send({ UpdateSelf: { away: null } });
    else promptDialog('Set away', 'Message', '', 'Set away', { max: 80, note: 'Optional, shown next to your name.' }).then((m) => m != null && send({ UpdateSelf: { away: m.trim() } }));
  }
  function editDescription(c) {
    promptDialog('Edit description', 'Description', c.description, 'Save', { multiline: true, max: 500 }).then((d) => {
      if (d == null) return;
      if (c.id === S.me) send({ UpdateSelf: { description: d } });
      else send({ EditClientDescription: { target: c.id, description: d } });
    });
  }
  function pokeClient(c) {
    promptDialog('Poke ' + c.nickname, 'Message', '', 'Poke', { max: 100 }).then((m) => m != null && send({ Poke: { target: c.id, message: m } }));
  }
  function kickClient(c, fromServer) {
    promptDialog((fromServer ? 'Kick from server: ' : 'Kick from channel: ') + c.nickname, 'Reason', '', 'Kick', { max: 80 })
      .then((r) => r != null && send({ Kick: { target: c.id, from_server: fromServer, reason: r } }));
  }
  const DURATIONS = [[3600, '1 hour'], [86400, '1 day'], [7 * 86400, '7 days'], [30 * 86400, '30 days'], [0, 'Permanent']];
  function durationSelect() {
    return el('select', null, DURATIONS.map(([v, l]) => el('option', { value: String(v), selected: v === 86400 }, l)));
  }
  function banClient(c) {
    const dur = durationSelect();
    const reason = el('input', { type: 'text', maxlength: '80' });
    dialog('Ban ' + c.nickname, el('div', { class: 'form' }, field('Duration', dur), field('Reason', reason), el('div', { class: 'note' }, 'Bans this identity and its current address.')), [
      { label: 'Cancel' },
      { label: 'Ban', danger: true, onClick: () => send({ Ban: { target: { Client: c.id }, duration_secs: Number(dur.value) || null, reason: reason.value } }) },
    ]);
  }
  function moveClientDialog(c) {
    const sel = el('select', null, [...S.channels.values()].filter((ch) => !spacer(ch.name)).sort((a, b) => a.name.localeCompare(b.name)).map((ch) => el('option', { value: String(ch.id), selected: ch.id === c.channel }, ch.name)));
    dialog('Move ' + c.nickname, el('div', { class: 'form' }, field('Channel', sel)), [
      { label: 'Cancel' },
      { label: 'Move', primary: true, onClick: () => send({ MoveClient: { target: c.id, channel: Number(sel.value) } }) },
    ]);
  }
  function deleteChannel(ch) {
    const n = subtree(ch.id).reduce((a, id) => a + clientsIn(id).length, 0);
    const subs = subtree(ch.id).length - 1;
    const text = 'Delete "' + (spacer(ch.name) ? 'this separator' : ch.name) + '"' + (subs ? ' and its ' + subs + ' sub-channel(s)' : '') + '?' + (n ? ' ' + n + ' client(s) inside are moved to the default channel.' : '') + ' Its files and chat history are deleted too.';
    confirmDialog('Delete channel', text, 'Delete', true).then((ok) => ok && send({ DeleteChannel: { id: ch.id, force: true } }));
  }
  function cloneChannel(ch) {
    const name = el('input', { type: 'text', value: ch.name + ' (copy)', maxlength: '40' });
    const kids = el('input', { type: 'checkbox', checked: children(ch.id).length > 0 });
    dialog('Clone channel', el('div', { class: 'form' }, field('Name', name), [el('span'), el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, kids, 'Include sub-channels')],
      el('div', { class: 'note' }, 'Copies every setting, the password and everyone\'s channel group.')), [
      { label: 'Cancel' },
      { label: 'Clone', primary: true, onClick: () => send({ CloneChannel: { source: ch.id, name: name.value.trim(), with_children: kids.checked } }) },
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
    const parentSel = el('select', null, el('option', { value: '' }, '(top level)'),
      [...S.channels.values()].filter((c) => !spacer(c.name) && !(editing && subtree(ch.id).includes(c.id))).sort((a, b) => a.name.localeCompare(b.name))
        .map((c) => el('option', { value: String(c.id), selected: c.id === e.parent }, c.name)));
    let kind = e.kind, align = sp ? sp.align : 'c';
    const qLabel = el('span', { class: 'dim' });
    const qRange = el('input', { type: 'range', min: '0', max: '10', value: String(quality), oninput: () => { quality = Number(qRange.value); updQ(); } });
    const updQ = () => { qLabel.textContent = codec === 'lossless' ? 'up to ~1.5 Mbit/s per listener, bit exact' : '~' + bitrate(packCodec(codec, quality)) + ' kbit/s' + (codec === 'music' ? ' stereo' : ''); qRange.disabled = codec === 'lossless'; };
    updQ();
    const draw = () => {
      put(body, el('div', { class: 'form' },
        field('Type', radios('t' + id, [[false, 'Channel'], [true, 'Separator (organises the list, nobody can join it)']], isSep, (v) => { isSep = v; draw(); })),
        isSep ? [
          field('Style', radios('a' + id, [['c', 'Text in the middle'], ['l', 'Text on the left'], ['r', 'Text on the right'], ['*', 'Line (the text repeated)']], align, (v) => { align = v; })),
          field('Text', name, 'e.g. Games, or for a line: ─ or - (empty = a thin line)'),
          field('Parent', parentSel),
        ] : [
          field('Name', name),
          field('Parent', parentSel),
          field('Topic', topic),
          field('Description', desc),
          field('Password', el('div', { style: 'display:flex;gap:10px;align-items:center' }, pw, editing && ch.has_password ? el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, pwClear, 'Remove') : null)),
          field('Lifetime', radios('k' + id, [['Permanent', 'Permanent'], ['SemiPermanent', 'Semi-Permanent'], ['Temporary', 'Temporary']], kind, (v) => { kind = v; })),
          field('Max clients', el('div', { style: 'display:flex;gap:10px;align-items:center' }, el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, unlimited, 'Unlimited'), maxc)),
          field('Default channel', el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, isDefault, 'New clients join this channel')),
          field('Codec', radios('c' + id, [['voice', 'Opus Voice'], ['music', 'Opus Music'], ['lossless', 'Lossless']], codec, (v) => { codec = v; updQ(); })),
          field('Quality', el('div', { style: 'display:flex;gap:10px;align-items:center' }, qRange, qLabel)),
          field('Needed talk power', talk),
          field('Needed join power', join),
          field('Voice encryption', el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, enc, 'Encrypt voice in this channel')),
        ]));
    };
    draw();
    dialog(editing ? 'Edit channel' : 'Create channel', body, [
      { label: 'Cancel' },
      { label: editing ? 'Save' : 'Create', primary: true, onClick: () => {
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
        if (editing) send({ EditChannel: { id: ch.id, edit: out } });
        else send({ CreateChannel: out });
      } },
    ]);
  }

  function groupSelect(kind, value) {
    return el('select', null, [...S.groups.values()].filter((g) => g.kind === kind).sort((a, b) => a.sort - b.sort).map((g) => el('option', { value: String(g.id), selected: g.id === value }, g.name)));
  }

  function editServer() {
    const s = S.server;
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
    const upload = el('input', { type: 'number', min: '0', placeholder: 'loading…' });
    const quota = el('input', { type: 'number', min: '0', placeholder: 'loading…' });
    let limits = null;
    S.wait.FileLimits = (v) => { limits = v; upload.value = String(v.max_upload_mb); quota.value = String(v.channel_quota_mb); };
    send('GetFileLimits');
    dialog('Edit virtual server', el('div', { class: 'form' },
      field('Server name', name),
      field('Welcome message', welcome),
      field('Host message', host),
      field('Max clients', maxc),
      field('Password', el('div', { style: 'display:flex;gap:10px;align-items:center' }, pw, s.has_password ? el('label', { style: 'display:flex;gap:4px;white-space:nowrap;color:var(--text)' }, pwClear, 'Remove') : null)),
      field('Default server group', dsg),
      field('Default channel group', dcg),
      field('Channel admin group', cag),
      field('Keep chat history', retention, 'Days; 0 keeps messages forever.'),
      el('div', { class: 'section full' }, 'File browser'),
      field('Largest upload (MB)', upload, '0 = unlimited'),
      field('Storage per channel (MB)', quota, '0 = unlimited')), [
      { label: 'Cancel' },
      { label: 'Save', primary: true, onClick: () => {
        send({ EditServer: {
          name: name.value.trim() || s.name, welcome_message: welcome.value, host_message: host.value,
          max_clients: Math.max(1, Number(maxc.value) || s.max_clients),
          password: pwClear.checked ? 'Clear' : pw.value ? { Set: pw.value } : 'Keep',
          default_server_group: Number(dsg.value), default_channel_group: Number(dcg.value), channel_admin_group: Number(cag.value),
          chat_retention_days: Math.max(0, Number(retention.value) || 0),
        } });
        if (limits && upload.value !== '' && quota.value !== '') {
          const nl = { max_upload_mb: Math.max(0, Number(upload.value) || 0), channel_quota_mb: Math.max(0, Number(quota.value) || 0) };
          if (nl.max_upload_mb !== limits.max_upload_mb || nl.channel_quota_mb !== limits.channel_quota_mb) send({ SetFileLimits: nl });
        }
      } },
    ], { wide: true, onClose: () => { delete S.wait.FileLimits; } });
  }

  function permChecks(perms, onChange) {
    let p = BigInt(perms);
    return el('div', { class: 'checks' }, PERM_NAMES.map(([b, label]) => el('label', null,
      el('input', { type: 'checkbox', checked: (p & bit(b)) !== 0n, onchange: (e) => { p = e.target.checked ? p | bit(b) : p & ~bit(b); onChange(p); } }), label)));
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
        el('div', { class: 'lh' }, 'Server groups'), groups.filter((g) => g.kind === 'Server').map(item),
        el('div', { class: 'lh' }, 'Channel groups'), groups.filter((g) => g.kind === 'Channel').map(item),
        el('div', { style: 'display:flex;gap:4px;padding:8px 4px;flex-wrap:wrap' },
          el('button', { class: 'btn small', onclick: () => newGroup('Server') }, '+ Server group'),
          el('button', { class: 'btn small', onclick: () => newGroup('Channel') }, '+ Channel group')));
      const g = S.groups.get(selected);
      if (!g) { put(editor, el('span', { class: 'dim' }, 'No group selected')); return; }
      const name = el('input', { type: 'text', value: g.name, maxlength: '40' });
      const talk = el('input', { type: 'number', value: String(g.talk_power) });
      const join = el('input', { type: 'number', value: String(g.join_power) });
      const sort = el('input', { type: 'number', value: String(g.sort) });
      const show = el('input', { type: 'checkbox', checked: g.show_name });
      let perms = BigInt(g.perms);
      const members = el('div', { class: 'list', style: 'max-height:160px;margin-top:6px' }, el('span', { class: 'dim small' }, 'Loading…'));
      S.wait.GroupMembers = (v) => {
        if (v.group !== g.id) return;
        put(members, ...(v.members.length ? v.members.map((m) => el('div', { class: 'li', style: 'cursor:default' },
          el('span', { style: 'flex:1' }, m.name || m.uid, m.online ? el('span', { style: 'color:var(--green)' }, ' ●') : null),
          g.kind === 'Server' && can(P.GROUP_ASSIGN_SERVER) ? el('button', { class: 'btn small danger', onclick: () => { send({ SetServerGroup: { uid: m.uid, group: g.id, add: false } }); setTimeout(() => send({ GroupMembers: g.id }), 300); } }, 'Remove') : null)) : [el('span', { class: 'dim small' }, 'No members')]));
      };
      send({ GroupMembers: g.id });
      put(editor, 
        el('div', { class: 'form' },
          field('Name', name), field('Talk power', talk), field('Join power', join, 'Compared with a channel\'s needed join power.'), field('Sort order', sort),
          field('Show name', el('label', { style: 'display:flex;gap:6px;color:var(--text)' }, show, 'Show the group name next to members'))),
        el('div', { class: 'section' }, 'Permissions'),
        permChecks(perms, (p) => { perms = p; }),
        el('div', { class: 'actions-row' },
          el('button', { class: 'btn primary', onclick: () => send({ EditGroup: { id: g.id, kind: g.kind, name: name.value.trim() || g.name, perms: Number(perms), talk_power: Number(talk.value) || 0, join_power: Number(join.value) || 0, sort: Number(sort.value) || 0, builtin: g.builtin, show_name: show.checked } }) }, 'Save group'),
          !g.builtin ? el('button', { class: 'btn danger', onclick: () => confirmDialog('Delete group', 'Delete the group "' + g.name + '"? Its members lose it.', 'Delete', true).then((ok) => ok && send({ DeleteGroup: g.id })) }, 'Delete group') : null),
        el('div', { class: 'section' }, 'Members'), members);
    };
    const newGroup = (kind) => {
      promptDialog('New ' + kind.toLowerCase() + ' group', 'Name', '', 'Create', { max: 40, note: selected && S.groups.get(selected)?.kind === kind ? 'Starts as a copy of "' + S.groups.get(selected).name + '".' : null })
        .then((n) => n && n.trim() && send({ CreateGroup: { kind, name: n.trim(), copy_from: selected && S.groups.get(selected)?.kind === kind ? selected : null } }));
    };
    draw();
    S.wait.GroupsUpdated = () => draw();
    dialog('Groups & permissions', el('div', { class: 'split' }, list, editor), [{ label: 'Close' }], { wide: true, onClose: () => { delete S.wait.GroupsUpdated; delete S.wait.GroupMembers; } });
  }

  function banListDialog() {
    const table = el('div', null, el('span', { class: 'dim' }, 'Loading…'));
    S.wait.BanList = (bans) => {
      put(table, bans.length ? el('table', { class: 'grid' },
        el('tr', null, ['Banned', 'Reason', 'By', 'Since', 'Expires', ''].map((h) => el('th', null, h))),
        bans.map((b) => el('tr', null,
          el('td', null, b.name || '', b.uid ? el('div', { class: 'dim small', style: 'font-family:Consolas,monospace' }, b.uid) : null, b.ip ? el('div', { class: 'dim small' }, b.ip) : null),
          el('td', null, b.reason), el('td', null, b.by), el('td', null, fmtDate(b.created)),
          el('td', null, b.expires ? fmtDate(b.expires) : 'never'),
          el('td', null, el('button', { class: 'btn small', onclick: () => { send({ Unban: b.id }); setTimeout(() => send('BanList'), 300); } }, 'Unban'))))) : el('span', { class: 'dim' }, 'Nobody is banned.'));
    };
    send('BanList');
    const kind = el('select', null, el('option', { value: 'Uid' }, 'Unique ID'), el('option', { value: 'Ip' }, 'IP address'));
    const value = el('input', { type: 'text', placeholder: 'Unique ID or address' });
    const dur = durationSelect();
    const reason = el('input', { type: 'text', placeholder: 'Reason', maxlength: '80' });
    const add = el('div', { style: 'display:flex;gap:6px;flex-wrap:wrap;margin-top:14px' }, kind, value, dur, reason,
      el('button', { class: 'btn', onclick: () => {
        if (!value.value.trim()) return;
        send({ Ban: { target: { [kind.value]: value.value.trim() }, duration_secs: Number(dur.value) || null, reason: reason.value } });
        value.value = ''; reason.value = '';
        setTimeout(() => send('BanList'), 300);
      } }, 'Add ban'));
    dialog('Ban list', el('div', null, table, el('div', { class: 'section' }, 'Add a ban'), add), [{ label: 'Close' }], { wide: true, onClose: () => { delete S.wait.BanList; } });
  }

  function privilegeKeyDialog() {
    const group = el('select', null, [...S.groups.values()].sort((a, b) => (a.kind > b.kind ? -1 : a.kind < b.kind ? 1 : a.sort - b.sort)).map((g) => el('option', { value: String(g.id) }, (g.kind === 'Server' ? 'Server group: ' : 'Channel group: ') + g.name)));
    const chSel = el('select', null, [...S.channels.values()].filter((c) => !spacer(c.name)).sort((a, b) => a.name.localeCompare(b.name)).map((c) => el('option', { value: String(c.id), selected: c.id === myChannel() }, c.name)));
    const chRow = el('div', { style: 'display:contents' }, field('Channel', chSel));
    const upd = () => { chRow.style.display = S.groups.get(Number(group.value))?.kind === 'Channel' ? 'contents' : 'none'; };
    group.addEventListener('change', upd);
    upd();
    const desc = el('input', { type: 'text', maxlength: '80' });
    const out = el('div');
    S.wait.PrivilegeKeyCreated = (key) => {
      put(out, el('div', { class: 'section' }, 'Privilege key'), el('div', { class: 'key' }, key), el('div', { class: 'actions-row' }, el('button', { class: 'btn', onclick: () => copy(key) }, '📋 Copy')), el('p', { class: 'dim small' }, 'Give this key to the person; they use it once (server menu ▸ Use Privilege Key) to join the group.'));
    };
    dialog('Create privilege key', el('div', null, el('div', { class: 'form' }, field('Group', group), chRow, field('Description', desc)), out), [
      { label: 'Close' },
      { label: 'Create key', primary: true, onClick: () => {
        const g = S.groups.get(Number(group.value));
        send({ CreatePrivilegeKey: { group: g.id, channel: g.kind === 'Channel' ? Number(chSel.value) : null, description: desc.value } });
        return false;
      } },
    ], { onClose: () => { delete S.wait.PrivilegeKeyCreated; } });
  }
  function usePrivilegeKey() {
    promptDialog('Use privilege key', 'Key', '', 'Use key').then((k) => k && k.trim() && send({ UsePrivilegeKey: k.trim() }));
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
    dialog('Server groups of ' + c.nickname, box, [{ label: 'Close' }], { onClose: () => { delete S.wait.ClientUpdated; } });
  }
  function channelGroupDialog(c) {
    const ch = S.channels.get(c.channel);
    const sel = groupSelect('Channel', c.channel_group);
    dialog('Channel group of ' + c.nickname, el('div', { class: 'form' }, field('In channel', el('span', null, ch ? ch.name : '')), field('Channel group', sel)), [
      { label: 'Cancel' },
      { label: 'Save', primary: true, onClick: () => send({ SetChannelGroup: { uid: c.uid, channel: c.channel, group: Number(sel.value) } }) },
    ]);
  }
  function clientPermsDialog(c) {
    let perms = null;
    const box = el('div', null, el('span', { class: 'dim' }, 'Loading…'));
    S.wait.ClientPerms = (v) => {
      if (v.uid !== c.uid) return;
      perms = BigInt(v.perms);
      put(box, el('p', { class: 'dim small', style: 'margin-top:0' }, 'Granted to ' + c.nickname + ' on top of their groups.'), permChecks(perms, (p) => { perms = p; }));
    };
    send({ GetClientPerms: c.uid });
    dialog('Permissions of ' + c.nickname, box, [
      { label: 'Cancel' },
      { label: 'Save', primary: true, onClick: () => { if (perms != null) send({ SetClientPerms: { uid: c.uid, perms: Number(perms) } }); } },
    ], { wide: true, onClose: () => { delete S.wait.ClientPerms; } });
  }

  // Uptime and "connected for" keep counting.
  setInterval(() => { if (S.connected && S.showRight && S.right === 'info') schedule(); }, 10000);

  boot();
})();
