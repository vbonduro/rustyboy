/**
 * rustyboy — Game Boy emulator frontend
 * ES module, no bundler required.
 * Loads rustyboy_wasm via wasm-bindgen --target web.
 */

import init, { EmulatorHandle, WasmMenuRenderer } from '/static/rustyboy_web_client.js?v=gemini-title-bg-crisp-csp-20260627';

const WASM_URL = '/static/rustyboy_web_client_bg.wasm?v=gemini-title-bg-crisp-csp-20260627';

class Logger {
  #tag;
  #seq = 0;

  constructor(tag) { this.#tag = tag; }

  /** Fire-and-forget POST to /dev/log so logs surface in `docker logs`. */
  #post(level, msg) {
    fetch('/dev/log', { method: 'POST', body: `${level} ${msg}` }).catch(() => {});
  }

  debug(msg) { console.debug(`[${this.#tag}]`, msg); this.#post('DEBUG', msg); }
  warn(msg)  { console.warn(`[${this.#tag}]`, msg);  this.#post('WARN', msg); }
  error(msg) { console.error(`[${this.#tag}]`, msg); this.#post('ERROR', msg); }

  /** Log a named app-state transition with sequence number, then POST to /dev/log. */
  event(label) {
    const seq = ++this.#seq;
    const activeMenu = state.activeMenu?.isActive() ? state.activeMenu._opts?.title : 'none';
    const msg = `#${seq} ${label} | activeMenu=${activeMenu} | emulator=${!!state.emulator}`;
    console.debug(`[${this.#tag}] ${msg}`);
    this.#post('EVENT', msg);
  }
}

const log = new Logger('rustyboy');

// ── Boot jingle ────────────────────────────────────────────────────────────
// Plays the classic "Vintendo" power-on ding via Web Audio API.
// Approximates the DMG startup: a short falling chime into a warm ding.

function playBootJingle() {
  let ctx;
  try { ctx = new (window.AudioContext || window.webkitAudioContext)(); }
  catch(e) { return; }

  // "Vin-ten-do" approximated as three quick descending tones + one sustained ding
  const notes = [
    { freq: 1320, start: 0.00, dur: 0.08, gain: 0.25 }, // "Vin"
    { freq: 1047, start: 0.09, dur: 0.08, gain: 0.25 }, // "ten"
    { freq:  880, start: 0.18, dur: 0.08, gain: 0.25 }, // "do"
    { freq:  523, start: 0.30, dur: 0.55, gain: 0.40 }, // the ding
  ];

  const master = ctx.createGain();
  master.gain.setValueAtTime(1, ctx.currentTime);
  master.connect(ctx.destination);

  notes.forEach(({ freq, start, dur, gain }) => {
    const osc = ctx.createOscillator();
    const env = ctx.createGain();

    osc.type = 'square';
    osc.frequency.setValueAtTime(freq, ctx.currentTime + start);

    env.gain.setValueAtTime(0, ctx.currentTime + start);
    env.gain.linearRampToValueAtTime(gain, ctx.currentTime + start + 0.01);
    env.gain.setValueAtTime(gain, ctx.currentTime + start + dur * 0.6);
    env.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + start + dur);

    osc.connect(env);
    env.connect(master);
    osc.start(ctx.currentTime + start);
    osc.stop(ctx.currentTime + start + dur + 0.05);
  });

  // Close context after jingle finishes
  setTimeout(() => ctx.close(), 1200);
}

// ── State ──────────────────────────────────────────────────────────────────

const state = {
  wasm:         null,   // wasm module (after init)
  emulator:     null,   // EmulatorHandle | null
  roms:         [],     // string[]
  selectedIdx:  0,
  lastRomName:  localStorage.getItem('lastRom') || null,
  running:      false,
  rafId:        null,
  audioCtx:     null,   // AudioContext | null
  audioNode:    null,   // AudioWorkletNode | null
  debugOverlay: false,  // toggle with D key
  user:         null,   // logged-in user object | null
  activeMenu:   null,   // MenuRenderer | null (canvas-based menu)
  currentRomName: null, // name of the currently loaded ROM
  currentRomId: null,   // SHA-256 hex of the currently loaded ROM bytes
  batterySaveTimer: null, // setInterval id for periodic battery save upload
  paused:       false,  // true when emulation loop is suspended for in-game menu
  menuPending:  false,  // true while showInGameMenu fetch is in-flight; blocks re-entry
  menuGen:      0,      // incremented on every pause/resume; stale async callbacks self-cancel
};

// ── Audio ───────────────────────────────────────────────────────────────────

const AUDIO_SAMPLE_RATE = 48000;

async function initAudio() {
  if (state.audioCtx) return;
  try {
    state.audioCtx = new (window.AudioContext || window.webkitAudioContext)({
      sampleRate: AUDIO_SAMPLE_RATE,
    });
    await state.audioCtx.resume();

    // Ring buffer consumed by ScriptProcessorNode
    state._ring     = new Float32Array(65536 * 2);
    state._ringHead = 0;
    state._ringTail = 0;
    state._ringSize = 0;

    const node = state.audioCtx.createScriptProcessor(4096, 0, 2);
    node.onaudioprocess = (e) => {
      const L = e.outputBuffer.getChannelData(0);
      const R = e.outputBuffer.getChannelData(1);
      for (let i = 0; i < L.length; i++) {
        if (state._ringSize > 0) {
          L[i] = state._ring[state._ringTail * 2];
          R[i] = state._ring[state._ringTail * 2 + 1];
          state._ringTail = (state._ringTail + 1) & 65535;
          state._ringSize--;
        } else {
          L[i] = R[i] = 0;
        }
      }
    };
    node.connect(state.audioCtx.destination);
    state.audioNode = node;
    log.debug(`audio init: ctx=${state.audioCtx.state} rate=${state.audioCtx.sampleRate}`);
  } catch (e) {
    log.warn(`Audio init failed: ${e}`);
  }
}

function pushAudioSamples(samples) {
  if (!state._ring || samples.length === 0) return;
  const pairs = samples.length >> 1;
  for (let i = 0; i < pairs; i++) {
    if (state._ringSize >= 65536) break; // drop if full
    state._ring[state._ringHead * 2]     = samples[i * 2];
    state._ring[state._ringHead * 2 + 1] = samples[i * 2 + 1];
    state._ringHead = (state._ringHead + 1) & 65535;
    state._ringSize++;
  }
}

function stopAudio() {
  if (state.audioNode) { state.audioNode.disconnect(); state.audioNode = null; }
  if (state.audioCtx)  { state.audioCtx.close(); state.audioCtx = null; }
  state._ring = null; state._ringSize = 0;
}

// ── Battery saves ──────────────────────────────────────────────────────────

async function sha256Hex(bytes) {
  // crypto.subtle is only defined in a secure context (HTTPS or http://localhost).
  // Over plain HTTP on a LAN IP it's undefined, so fall back to a pure-JS SHA-256.
  // Both compute the same digest, so ROM IDs (used for save scoping) stay stable
  // regardless of how the page was served.
  if (globalThis.crypto?.subtle) {
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    return [...new Uint8Array(digest)]
      .map(b => b.toString(16).padStart(2, '0'))
      .join('');
  }
  return sha256HexJs(bytes);
}

// Minimal pure-JS SHA-256 (FIPS 180-4) over a Uint8Array → lowercase hex.
// Fallback for insecure contexts where crypto.subtle is unavailable.
function sha256HexJs(bytes) {
  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ]);
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;

  const bitLen = bytes.length * 8;
  const withPad = ((bytes.length + 8) >> 6 << 6) + 64; // padded length, multiple of 64
  const m = new Uint8Array(withPad);
  m.set(bytes);
  m[bytes.length] = 0x80;
  // 64-bit big-endian length in the last 8 bytes (length < 2^32 bits here)
  const dv = new DataView(m.buffer);
  dv.setUint32(withPad - 4, bitLen >>> 0, false);
  dv.setUint32(withPad - 8, Math.floor(bitLen / 0x100000000), false);

  const w = new Uint32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let off = 0; off < withPad; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4, false);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) | 0;
      h = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0;
    h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0;
  }
  return [h0, h1, h2, h3, h4, h5, h6, h7]
    .map(x => (x >>> 0).toString(16).padStart(8, '0'))
    .join('');
}

function romScopedHeaders(romId = state.currentRomId, extra = {}) {
  const headers = { ...extra };
  if (romId) headers['x-rustyboy-rom-id'] = romId;
  return headers;
}

async function loadBatterySave(romName, romId = state.currentRomId) {
  try {
    const res = await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders(romId),
    });
    if (res.ok) {
      const buf = await res.arrayBuffer();
      if (buf.byteLength > 0) {
        state.emulator.set_battery_save(new Uint8Array(buf));
        log.debug(`battery save loaded: ${buf.byteLength} bytes`);
      }
    }
  } catch (e) {
    log.warn(`battery save load failed: ${e}`);
  }
}

async function uploadBatterySave(romName, romId = state.currentRomId) {
  if (!state.emulator) return;
  const data = state.emulator.get_battery_save();
  if (!data || data.length === 0) return;
  try {
    await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      method: 'PUT',
      headers: romScopedHeaders(romId, { 'content-type': 'application/octet-stream' }),
      body: data,
    });
    log.debug(`battery save uploaded: ${data.length} bytes`);
  } catch (e) {
    log.warn(`battery save upload failed: ${e}`);
  }
}

function startBatterySaveTimer(romName) {
  stopBatterySaveTimer();
  state.batterySaveTimer = setInterval(() => uploadBatterySave(romName), 30_000);
}

function stopBatterySaveTimer() {
  if (state.batterySaveTimer) {
    clearInterval(state.batterySaveTimer);
    state.batterySaveTimer = null;
  }
}

// ── DOM refs ───────────────────────────────────────────────────────────────

const canvas      = document.getElementById('gameCanvas');
const ctx         = canvas.getContext('2d');

// Keep the canvas buffer at device-pixel resolution so upscaling is done by
// canvas drawImage (with imageSmoothingEnabled=false) rather than CSS, which
// guarantees crisp nearest-neighbor scaling on all browsers/zoom levels.
new ResizeObserver(entries => {
  for (const entry of entries) {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(entry.contentRect.width * dpr);
    const h = Math.round(entry.contentRect.height * dpr);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width  = w;
      canvas.height = h;
    }
  }
}).observe(canvas);

const menuOverlay = document.getElementById('menuOverlay');
const romList     = document.getElementById('romList');
const powerBtn    = document.getElementById('powerBtn');
const powerLed    = document.getElementById('powerLed');
const resetLed    = document.getElementById('resetLed');
const screenInner = canvas.parentElement;
const screenBezel = screenInner.parentElement;

// Debug overlay toggle — wired in boot() after DOM confirmed ready

// ── Boot ───────────────────────────────────────────────────────────────────

async function boot() {
  try {
    state.wasm = await init(WASM_URL);
    window.RustyBoyWasmMenuRenderer = WasmMenuRenderer;
  } catch (err) {
    showError('WASM LOAD FAILED');
    log.error(err);
    return;
  }

  setLed('menu');

  const authed = await checkAuth();
  bindButtons();
  bindKeyboard();
  if (!authed) {
    showLoginScreen();
    return;
  }

  await loadRomList();
  showMainMenu();
  // Only wire debug overlay if compiled in (debug-overlay feature)
  if (typeof EmulatorHandle.prototype.debug_state === 'function') {
    bindDebugButton();
  }
}

function bindDebugButton() {
  // Inject DBG button only when the debug-overlay feature is compiled in
  const housing = document.querySelector('.screen-housing');
  if (!housing) return;
  const btn = document.createElement('button');
  btn.id = 'debugBtn';
  btn.textContent = 'DBG';
  btn.style.cssText = 'position:absolute;top:4px;right:8px;background:rgba(0,0,0,0.7);color:#9BBC0F;font:8px monospace;border:1px solid #9BBC0F;border-radius:2px;padding:3px 6px;z-index:50;cursor:pointer;touch-action:manipulation;-webkit-tap-highlight-color:transparent;';
  housing.appendChild(btn);
  btn.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    e.stopPropagation();
    state.debugOverlay = !state.debugOverlay;
    btn.style.background = state.debugOverlay ? '#9BBC0F' : 'rgba(0,0,0,0.7)';
    btn.style.color = state.debugOverlay ? '#000' : '#9BBC0F';
  });
}

// ── Auth ───────────────────────────────────────────────────────────────────

async function checkAuth() {
  const params = new URLSearchParams(window.location.search);

  if (params.has('auth_error')) {
    await showLoginError();
    return false;
  }

  // Already have a valid session?
  try {
    const res = await fetch('/api/me');
    if (res.ok) {
      state.user = await res.json();
      if (params.has('logged_in')) {
        history.replaceState({}, '', '/');
      }
      return true;
    }
  } catch (e) {
    // network error — treat as not authed
  }

  // After an explicit logout, skip silent CF attempt and show login screen.
  if (params.has('logged_out')) {
    history.replaceState({}, '', '/');
    return false;
  }

  // Not authed — check available auth methods.
  try {
    const res = await fetch('/api/auth-method');
    if (res.ok) {
      const { methods } = await res.json();
      if (methods.includes('cf')) {
        // Try Cloudflare Access silently — only works when the CF JWT header
        // is present (i.e. accessed via the Cloudflare tunnel).
        // Falls through to login screen on failure (local/direct access).
        await fetch('/auth/cf-access');
        // Confirm a session was actually established (not just a redirect to /?auth_error).
        try {
          const meRes = await fetch('/api/me');
          if (meRes.ok) {
            state.user = await meRes.json();
            window.location.href = '/';
            return false;
          }
        } catch (_) { /* fall through */ }
        // CF failed (no header present) — fall through to login screen.
      }
    }
  } catch (e) {
    // ignore — fall through to login screen
  }

  return false; // show login screen
}

function bindMenuToButtons(menu) {
  // Store reference so keyboard handler can forward to it
  state.activeMenu = menu;

  // Button releases forward to the canvas menu
  // We patch sendButton so that while a canvas menu is active, button releases
  // route to the menu instead of handleMenuInput.
  // The patch is applied by overriding sendButton's menu path via activeMenu.
}

function showLoginScreen() {
  log.event('showLoginScreen');
  const menu = new window.MenuRenderer(canvas);
  menuOverlay.classList.add('hidden');
  state.activeMenu = menu;
  menu.show({
    title: 'RUSTYBOY',
    items: [{ label: 'SIGN IN WITH GOOGLE', value: 'login' }],
    footer: '\u25b2\u25bc MOVE  A SELECT',
    onSelect: () => {
      window.location.href = '/auth/google';
    },
    onBack: () => { showLoginScreen(); }, // B on login → stay on login
  });
}

async function showLoginError() {
  return new Promise(() => {
    const menu = new window.MenuRenderer(canvas);
    menuOverlay.classList.add('hidden');
    state.activeMenu = menu;
    menu.show({
      title: 'AUTH FAILED',
      items: [{ label: 'TRY AGAIN', value: 'retry' }],
      footer: 'A SELECT  B BACK',
      onSelect: () => { window.location.href = '/auth/google'; },
      onBack: () => { showLoginScreen(); },
    });
  });
  // Intentionally never resolves — user must click TRY AGAIN
}

// ── Main menu / ROM list ───────────────────────────────────────────────────

/** Returns the ID of the most recent save state for a ROM, or null if none. */
async function fetchLatestSaveId(romName, romId = state.currentRomId) {
  if (!romName) return null;
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}/latest`, {
      headers: romScopedHeaders(romId),
    });
    if (!res.ok) return null;
    const data = await res.json();
    return data.id || null;
  } catch (_) {
    return null;
  }
}

/** Returns true if any save states exist. Pass a romName to scope to that ROM. */
async function fetchHasSaves(romName, romId = state.currentRomId) {
  try {
    const url = romName
      ? `/api/save-states/${encodeURIComponent(romName)}`
      : '/api/save-states';
    const res = await fetch(url, {
      headers: romName ? romScopedHeaders(romId) : {},
    });
    if (!res.ok) return false;
    const data = await res.json();
    return data.length > 0;
  } catch (_) {
    return false;
  }
}

async function showMainMenu() {
  log.event('showMainMenu');
  menuOverlay.classList.add('hidden');

  const items = [
    { label: 'CONTINUE', value: 'continue' },
    { label: 'GAMES',    value: 'games' },
  ];

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  const rawName = state.user && (state.user.display_name || state.user.email) || '';
  const name = rawName.replace(/@[^@]+$/, '');
  menu.show({
    title: 'RUSTYBOY',
    items,
    footer: name ? ('HELLO, ' + name.toUpperCase()) : '\u25b2\u25bc MOVE  A SELECT',
    onSelect: async (item) => {
      state.activeMenu = null;
      if (item.value === 'continue') {
        await continueLatestSave();
      } else if (item.value === 'games') {
        showRomList();
      }
    },
    onBack: () => { showMainMenu(); }, // B on main menu → stay on main menu
    onSelectBtn: () => {
      fetch('/auth/logout', { method: 'POST' }).finally(() => {
        window.location.href = '/?logged_out=1';
      });
    },
  });
}

/** CONTINUE: find the most recently saved game across all ROMs and resume it. */
async function continueLatestSave() {
  try {
    const res = await fetch('/api/save-states');
    if (!res.ok) { showMainMenu(); return; }
    const roms = await res.json(); // [{rom_name, last_saved}, ...] sorted newest first
    if (roms.length === 0) { showMainMenu(); return; }
    const romName = roms[0].rom_name;
    await launchRom(romName);
  } catch (_) {
    showMainMenu();
  }
}

async function loadRomList() {
  try {
    const res = await fetch('/api/roms');
    if (!res.ok) throw new Error(res.statusText);
    state.roms = await res.json();
  } catch (err) {
    log.error(err);
    state.roms = [];
  }
}

function showRomList() {
  log.event('showRomList');
  if (state.roms.length === 0) {
    showCanvasError('NO ROMS FOUND');
    return;
  }

  const lastIdx = state.roms.indexOf(state.lastRomName);
  state.selectedIdx = lastIdx >= 0 ? lastIdx : 0;

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'SELECT GAME',
    items: state.roms.map(name => ({ label: stripExtension(name), value: name })),
    footer: '\u25b2\u25bc MOVE  A SELECT  B BACK',
    onSelect: (item) => {
      state.activeMenu = null;
      launchRom(item.value);
    },
    onBack: () => {
      state.activeMenu = null;
      showMainMenu();
    },
  });
}

function stripExtension(name) {
  return name.replace(/\.(gb|gbc)$/i, '');
}

function showCanvasError(msg) {
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'ERROR',
    items: [{ label: msg, value: 'error' }],
    footer: 'B BACK',
    onBack: () => {
      state.activeMenu = null;
      showMainMenu();
    },
  });
}

// ── Launch / stop ──────────────────────────────────────────────────────────

async function launchRom(name) {
  await launchRomWithSaveState(name, undefined);
}

async function launchRomWithSaveState(name, saveStateId) {
  log.event(`launch: start name=${name} saveStateId=${saveStateId}`);
  // Fetch ROM bytes
  let bytes;
  try {
    const res = await fetch(`/roms/${encodeURIComponent(name)}`);
    if (!res.ok) throw new Error(res.statusText);
    const buf = await res.arrayBuffer();
    bytes = new Uint8Array(buf);
  } catch (err) {
    showCanvasError('LOAD ERROR');
    log.error(err);
    return;
  }
  log.event(`launch: rom fetched ${bytes.length} bytes`);
  const romId = await sha256Hex(bytes);
  log.event(`launch: romId=${romId.slice(0, 8)}`);

  if (saveStateId === undefined) {
    saveStateId = await fetchLatestSaveId(name, romId);
    log.event(`launch: latest saveStateId=${saveStateId}`);
  }

  // Tear down previous
  await stopEmulation();
  log.event('launch: stopEmulation done');

  // Create emulator
  try {
    state.emulator = new EmulatorHandle(bytes);
  } catch (err) {
    showCanvasError('ROM ERROR');
    log.error(err);
    return;
  }
  log.event('launch: EmulatorHandle created');

  state.lastRomName = name;
  state.currentRomName = name;
  state.currentRomId = romId;
  localStorage.setItem('lastRom', name);
  state.running = true;
  state.paused = false;

  // Load save state if available, otherwise load battery save
  if (saveStateId) {
    try {
      const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(saveStateId)}/data`);
      if (res.ok) {
        const buf = await res.arrayBuffer();
        state.emulator.load_state(new Uint8Array(buf));
        log.debug(`save state loaded: ${buf.byteLength} bytes`);
      }
    } catch (e) {
      log.warn(`save state load failed: ${e}`);
    }
  } else {
    await loadBatterySave(name, romId);
  }
  log.event('launch: saves loaded');
  startBatterySaveTimer(name);

  initAudio();
  log.event('launch: initAudio returned');
  playBootJingle();
  menuOverlay.classList.add('hidden');
  screenInner.classList.add('booting');
  screenInner.classList.add('running');
  screenBezel.classList.add('running');
  setLed('on');

  // Remove boot class after animation
  screenInner.addEventListener('animationend', () => {
    screenInner.classList.remove('booting');
  }, { once: true });

  startLoop();
}

async function stopEmulation() {
  if (state.rafId) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  stopBatterySaveTimer();
  if (state.emulator && state.currentRomName) {
    await uploadBatterySave(state.currentRomName);
  }
  if (state.emulator) {
    state.emulator.free?.();
    state.emulator = null;
  }
  state.currentRomName = null;
  state.currentRomId = null;
  state.running = false;
  state.paused = false;
  state.menuPending = false;
  state.menuGen++; // invalidate any in-flight showInGameMenu calls
  stopAudio();
  screenInner.classList.remove('running', 'booting');
  screenBezel.classList.remove('running');
}

function pauseEmulation() {
  if (!state.running || state.paused) return;
  // Release all buttons before pausing so none stay stuck in the emulator
  if (state.emulator) {
    for (let i = 0; i < 8; i++) state.emulator.set_button(i, false);
  }
  state.paused = true;
  state.menuGen++;
  if (state.rafId) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  // Flush ring buffer so audio stops immediately without a pop
  if (state._ring) { state._ringHead = 0; state._ringTail = 0; state._ringSize = 0; }
}

function resumeEmulation() {
  if (!state.running || !state.paused) return;
  state.paused = false;
  state.menuGen++;
  if (state.activeMenu) {
    state.activeMenu.hide();
    state.activeMenu = null;
  }
  startLoop();
}

async function returnToMenu() {
  await stopEmulation();
  setLed('menu');
  showMainMenu();
}

// ── In-game pause menu ─────────────────────────────────────────────────────

function showPauseMenu(hasSaves, latestSaveId) {
  const items = [
    { label: 'RESUME',     value: 'resume' },
    { label: 'SAVE',       value: 'save' },
  ];
  if (hasSaves) {
    items.push({ label: 'QUICK LOAD', value: 'quickload' });
    items.push({ label: 'LOAD',       value: 'load' });
  }
  items.push({ label: 'RESET', value: 'reset' });
  items.push({ label: 'QUIT',  value: 'quit' });

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: state.currentRomName ? stripExtension(state.currentRomName).toUpperCase() : 'PAUSED',
    items,
    footer: '\u25b2\u25bc MOVE  A SELECT  B RESUME',
    onSelect: async (item) => {
      state.activeMenu = null;
      if (item.value === 'resume') {
        resumeEmulation();
      } else if (item.value === 'save') {
        await saveCurrentState();
        resumeEmulation();
      } else if (item.value === 'quickload') {
        if (latestSaveId) {
          try {
            const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(latestSaveId)}/data`);
            if (res.ok) {
              const buf = await res.arrayBuffer();
              state.emulator.load_state(new Uint8Array(buf));
              log.debug(`quick load: ${buf.byteLength} bytes`);
            }
          } catch (e) {
            log.warn(`quick load failed: ${e}`);
          }
        }
        resumeEmulation();
      } else if (item.value === 'load') {
        // When returning from load screen (e.g. deleted all slots), re-show pause menu without saves
        showSaveStateSlots(state.currentRomName, async () => {
          const [hasSaves, latestSave] = await Promise.all([
            fetchHasSaves(state.currentRomName),
            fetchLatestSaveId(state.currentRomName),
          ]);
          showPauseMenu(hasSaves, latestSave);
        });
      } else if (item.value === 'reset') {
        const romName = state.currentRomName;
        await stopEmulation();
        await launchRomWithSaveState(romName, null); // fresh start
      } else if (item.value === 'quit') {
        await returnToMenu();
      }
    },
    onBack: () => {
      resumeEmulation();
    },
  });
}

async function showInGameMenu() {
  if (!state.running || state.menuPending) return;
  // If already paused (menu visible or fetch in-flight), ignore
  if (state.paused) return;
  pauseEmulation();
  state.menuPending = true;
  log.event('showInGameMenu');
  const gen = state.menuGen; // snapshot before async gap

  const [hasSaves, latestSave] = await Promise.all([
    fetchHasSaves(state.currentRomName),
    fetchLatestSaveId(state.currentRomName),
  ]);

  state.menuPending = false;

  // If state changed while we were fetching (resumed, quit, new game), abort
  if (state.menuGen !== gen || !state.paused || !state.running) return;

  showPauseMenu(hasSaves, latestSave);
}

async function saveCurrentState() {
  if (!state.emulator || !state.currentRomName) return;
  try {
    const blob = state.emulator.save_state();
    await fetch(`/api/save-states/${encodeURIComponent(state.currentRomName)}`, {
      method: 'POST',
      headers: romScopedHeaders(state.currentRomId, { 'content-type': 'application/octet-stream' }),
      body: blob,
    });
    showSavedOverlay();
    log.debug(`save state uploaded: ${blob.length} bytes`);
  } catch (e) {
    log.warn(`save state upload failed: ${e}`);
  }
}

function showSavedOverlay() {
  const c = canvas.getContext('2d');
  c.save();
  const s = canvas.width / 160;
  c.scale(s, s);
  c.fillStyle = 'rgba(15,56,15,0.85)';
  c.fillRect(0, 60, 160, 24);
  c.fillStyle = '#9BBC0F';
  c.font = 'bold 10px monospace';
  c.textAlign = 'center';
  c.textBaseline = 'middle';
  c.fillText('\u2713 SAVED', 80, 72);
  c.restore();
  setTimeout(() => { if (state.running && !state.paused) drawFrame(); }, 1500);
}

async function showSaveStateSlots(romName, onBack) {
  let saves = [];
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders(),
    });
    if (res.ok) saves = await res.json();
  } catch (_) {}

  if (saves.length === 0) {
    if (onBack) onBack();
    return;
  }

  const items = saves.map(s => ({
    label: formatSaveSlotLabel(s.updated_at),
    value: s.id,
  }));

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'LOAD STATE',
    items,
    footer: '\u25b2\u25bc MOVE  A LOAD  SEL DEL  B BACK',
    onSelect: async (item) => {
      state.activeMenu = null;
      try {
        const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(item.value)}/data`);
        if (res.ok) {
          const buf = await res.arrayBuffer();
          state.emulator.load_state(new Uint8Array(buf));
          log.debug(`save state loaded: ${buf.byteLength} bytes`);
        }
      } catch (e) {
        log.warn(`save state load failed: ${e}`);
      }
      resumeEmulation();
    },
    onBack: () => {
      // B = back to pause menu
      if (onBack) onBack();
    },
    onSelectBtn: async (selIdx) => {
      // Select = delete the currently selected slot
      const id = items[selIdx]?.value;
      if (!id) return;
      try {
        await fetch(`/api/save-states/by-id/${encodeURIComponent(id)}`, { method: 'DELETE' });
        log.debug(`save state deleted: ${id}`);
      } catch (e) {
        log.warn(`save state delete failed: ${e}`);
      }
      // Re-open the slot list (minus the deleted slot); if empty, go back
      await showSaveStateSlots(romName, onBack);
    },
  });
}

function formatSaveSlotLabel(unixSecs) {
  const d = new Date(unixSecs * 1000);
  const months = ['JAN','FEB','MAR','APR','MAY','JUN','JUL','AUG','SEP','OCT','NOV','DEC'];
  const mon = months[d.getMonth()];
  const day = String(d.getDate()).padStart(2, '0');
  const h   = String(d.getHours()).padStart(2, '0');
  const m   = String(d.getMinutes()).padStart(2, '0');
  return `${mon} ${day} ${h}:${m}`;
}

// ── Emulation loop ─────────────────────────────────────────────────────────

let imageData = null;
let offscreenCanvas = null;
let offscreenCtx = null;
let loopGeneration = 0; // incremented each time startLoop() is called; stale RAF callbacks self-cancel

function startLoop() {
  offscreenCanvas = document.createElement('canvas');
  offscreenCanvas.width = 160;
  offscreenCanvas.height = 144;
  offscreenCtx = offscreenCanvas.getContext('2d');
  imageData = offscreenCtx.createImageData(160, 144);
  const myGen = ++loopGeneration;
  let frameCount = 0;
  log.event('startLoop: scheduling first frame');

  function frame(now) {
    if (!state.running || !state.emulator || loopGeneration !== myGen) return;

    try {
      if (frameCount < 3) log.event(`frame ${frameCount}: run_frame begin`);
      state.emulator.run_frame();
      if (frameCount < 3) log.event(`frame ${frameCount}: run_frame end`);
    } catch(e) {
      log.error(`run_frame error: ${e}`);
      return;
    }

    if (state.audioCtx) {
      pushAudioSamples(state.emulator.drain_audio_samples());
    }

    try {
      drawFrame();
    } catch(e) {
      log.error(`drawFrame error: ${e}`);
      return;
    }
    if (frameCount < 3) log.event(`frame ${frameCount}: drawFrame done`);
    frameCount++;

    state.rafId = requestAnimationFrame(frame);
  }

  state.rafId = requestAnimationFrame(frame);
}

function drawFrame() {
  const rgba = state.emulator.framebuffer_rgba();
  imageData.data.set(rgba);
  offscreenCtx.putImageData(imageData, 0, 0);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(offscreenCanvas, 0, 0, canvas.width, canvas.height);

  if (state.debugOverlay && typeof state.emulator.debug_state === 'function') {
    const lines = state.emulator.debug_state().split('\n');
    const s = canvas.width / 160;
    const fontSize = 10;
    const lineH = fontSize + 3;
    const pad = 3;
    ctx.save();
    ctx.scale(s, s);
    ctx.font = `bold ${fontSize}px monospace`;
    ctx.textBaseline = 'top';
    let maxW = 0;
    lines.forEach(l => { const m = ctx.measureText(l).width; if (m > maxW) maxW = m; });
    const boxW = Math.min(maxW + pad * 2, 160);
    const boxH = lines.length * lineH + pad * 2;
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, boxW, boxH);
    ctx.fillStyle = '#9BBC0F';
    lines.forEach((line, i) => {
      ctx.fillText(line, pad, pad + lineH * i);
    });
    ctx.restore();
  }
}

// ── Button handling ────────────────────────────────────────────────────────

function sendButton(idx, pressed) {
  log.event(`sendButton idx=${idx} pressed=${pressed}`);
  // While paused, route button releases to the canvas menu (not the emulator)
  if (state.paused) {
    if (!pressed && state.activeMenu && state.activeMenu.isActive()) {
      const keyMap = { 2: 'ArrowUp', 3: 'ArrowDown', 4: 'Enter', 5: 'Escape', 6: 'Select' };
      const key = keyMap[idx];
      log.debug(`sendButton (paused) → menu key=${key}`);
      if (key) { state.activeMenu.handleInput(key); }
    }
    return;
  }
  if (state.emulator) {
    state.emulator.set_button(idx, pressed);
  } else if (!pressed) {
    // If a canvas menu is active, forward to it
    if (state.activeMenu && state.activeMenu.isActive()) {
      const keyMap = { 2: 'ArrowUp', 3: 'ArrowDown', 4: 'Enter', 5: 'Escape', 6: 'Select' };
      const key = keyMap[idx];
      log.debug(`sendButton → menu key=${key}`);
      if (key) { state.activeMenu.handleInput(key); return; }
    }
    // Menu navigation on button release
    handleMenuInput(idx);
  }
}

function handleMenuInput(_idx) {
  // No-op: all menu navigation is handled by MenuRenderer via sendButton → activeMenu
}

function bindButtons() {
  // All game / dpad buttons
  document.querySelectorAll('[data-btn]').forEach(el => {
    const idx = parseInt(el.dataset.btn, 10);
    let held = false;

    el.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      try {
        el.setPointerCapture(e.pointerId);
      } catch (_) {
        // Synthetic PointerEvents in tests may not be active pointers.
      }
      el.classList.add('pressed');
      held = true;
      sendButton(idx, true);
    });

    el.addEventListener('pointerup', (e) => {
      e.preventDefault();
      el.classList.remove('pressed');
      if (held) { held = false; sendButton(idx, false); }
    });

    el.addEventListener('pointercancel', () => {
      el.classList.remove('pressed');
      if (held) { held = false; sendButton(idx, false); }
    });

    // Fires after pointerup/pointercancel AND when capture is dropped mid-slide.
    // The held guard ensures we only send a release if pointerup/cancel hasn't already.
    el.addEventListener('lostpointercapture', () => {
      el.classList.remove('pressed');
      if (held) { held = false; sendButton(idx, false); }
    });
  });

  // Power button — if running, pause and show in-game menu; otherwise go to main menu
  powerBtn.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    powerBtn.classList.add('pressed');
    flashResetLed();
  });
  powerBtn.addEventListener('pointerup', () => {
    powerBtn.classList.remove('pressed');
    if (state.menuPending) return; // fetch in-flight — ignore
    if (state.running && !state.paused) {
      showInGameMenu();
    } else if (state.paused && state.activeMenu) {
      // Power pressed while in-game menu is open → resume (resumeEmulation hides the menu)
      resumeEmulation();
    } else if (!state.running) {
      returnToMenu();
    }
    // state.paused && !state.activeMenu: menu just closed, ignore
  });
  powerBtn.addEventListener('pointerleave',  () => { powerBtn.classList.remove('pressed'); });
  powerBtn.addEventListener('pointercancel', () => { powerBtn.classList.remove('pressed'); });
}

// ── Keyboard support ───────────────────────────────────────────────────────

const KEY_MAP = {
  'ArrowRight': 0, 'ArrowLeft': 1, 'ArrowUp': 2, 'ArrowDown': 3,
  'z': 4, 'Z': 4,   // A
  'x': 5, 'X': 5,   // B
  'Shift': 6,        // Select
  'Enter': 7,        // Start
  'Backspace': -1,   // Power / menu
};

const heldKeys = new Set();

function clearHeldKeys() { heldKeys.clear(); }

function bindKeyboard() {
  document.addEventListener('keydown', (e) => {
    if (heldKeys.has(e.key)) {
      log.debug(`keydown IGNORED (held) key=${e.key} heldKeys=[${[...heldKeys].join(',')}]`);
      return;
    }
    heldKeys.add(e.key);
    const activeMenu = state.activeMenu?.isActive() ? state.activeMenu._opts?.title : 'none';
    log.debug(`keydown key=${e.key} heldKeys=[${[...heldKeys].join(',')}] activeMenu=${activeMenu}`);

    // Forward to canvas menu if active.
    // Navigation keys (arrows, w/s) and Enter/Escape are handled directly.
    // z/x (A/B buttons) are intentionally NOT intercepted here — they route
    // through sendButton on keyup, which maps them to Enter/Escape for the menu.
    const MENU_NAV_KEYS = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'w', 's', 'Enter', 'Escape', 'a', 'b', 'Shift']);
    if (state.activeMenu && state.activeMenu.isActive() && MENU_NAV_KEYS.has(e.key)) {
      e.preventDefault();
      // Shift key = Select button in menu context
      const menuKey = e.key === 'Shift' ? 'Select' : e.key;
      state.activeMenu.handleInput(menuKey);
      return;
    }

    // Toggle debug overlay with backtick/apostrophe (only if compiled in)
    if ((e.key === "'" || e.key === '`') && typeof EmulatorHandle.prototype.debug_state === 'function') {
      state.debugOverlay = !state.debugOverlay;
      return;
    }

    const idx = KEY_MAP[e.key];
    if (idx === undefined) return;
    e.preventDefault();

    if (idx === -1) {
      if (state.menuPending) return;
      if (state.running && !state.paused) {
        showInGameMenu();
      } else if (!state.running) {
        returnToMenu();
      }
    } else {
      sendButton(idx, true);
    }
  });

  document.addEventListener('keyup', (e) => {
    log.debug(`keyup key=${e.key}`);
    heldKeys.delete(e.key);
    const idx = KEY_MAP[e.key];
    if (idx === undefined || idx === -1) return;
    e.preventDefault();
    sendButton(idx, false);
  });
}

// ── LED helper ─────────────────────────────────────────────────────────────

function setLed(mode) {
  powerLed.className = 'power-led ' + (mode || '');
  // Keep reset LED in sync: red when running, off when in menu
  if (resetLed) resetLed.className = 'reset-led' + (mode === 'on' ? ' on' : '');
}

function flashResetLed() {
  if (!resetLed) return;
  resetLed.classList.remove('flash');
  // Force reflow to restart animation
  void resetLed.offsetWidth;
  resetLed.classList.add('flash');
  resetLed.addEventListener('animationend', () => resetLed.classList.remove('flash'), { once: true });
}

// ── Start ──────────────────────────────────────────────────────────────────

window.__appState = state;
boot();
