/**
 * rustyboy — Game Boy emulator frontend
 * ES module bundled by esbuild. Loads rustyboy-wasm via wasm-bindgen --target web.
 */

import init, { EmulatorHandle, WasmMenuRenderer } from 'rustyboy-wasm';
import { create as createNipple } from 'nipplejs';
import type { MenuItem, MenuOptions, MenuRendererInstance } from './types.js';

// ── Wasm URL (must match ?v= in index.html) ───────────────────────────────────
const WASM_URL = '/static/rustyboy_web_client_bg.wasm?v=gemini-title-bg-crisp-csp-20260627';

// ── Global augmentations ──────────────────────────────────────────────────────

/** EmulatorHandle with the optional debug-overlay method compiled in via feature flag. */
type EmulatorHandleDebug = EmulatorHandle & { debug_state?(): string };

declare global {
  // Window.MenuRenderer declaration must match menu.ts structurally (merged interface).
  interface Window {
    MenuRenderer: { new(canvas: HTMLCanvasElement): MenuRendererInstance };
    RustyBoyWasmMenuRenderer?: typeof WasmMenuRenderer;
    __appState?: AppState;
  }
}

// ── Logger ────────────────────────────────────────────────────────────────────

class Logger {
  readonly #tag: string;
  #seq = 0;

  constructor(tag: string) { this.#tag = tag; }

  #post(level: string, msg: string): void {
    fetch('/dev/log', { method: 'POST', body: `${level} ${msg}` }).catch(() => {});
  }

  debug(msg: string): void { console.debug(`[${this.#tag}]`, msg); this.#post('DEBUG', msg); }
  warn(msg: string): void  { console.warn(`[${this.#tag}]`, msg);  this.#post('WARN',  msg); }
  error(msg: string | unknown): void {
    console.error(`[${this.#tag}]`, msg);
    this.#post('ERROR', String(msg));
  }

  /** Log a named app-state transition with sequence number. */
  event(label: string): void {
    const seq = ++this.#seq;
    const menuTitle = state.activeMenu?.isActive() ? state.activeMenu.title : 'none';
    const msg = `#${seq} ${label} | activeMenu=${menuTitle} | emulator=${!!state.emulator}`;
    console.debug(`[${this.#tag}] ${msg}`);
    this.#post('EVENT', msg);
  }
}

const log = new Logger('rustyboy');

// ── Boot jingle ───────────────────────────────────────────────────────────────
// Approximates the classic DMG startup chime via Web Audio API oscillators.

interface Note { freq: number; start: number; dur: number; gain: number }

const BOOT_NOTES: Note[] = [
  { freq: 1320, start: 0.00, dur: 0.08, gain: 0.25 }, // "Vin"
  { freq: 1047, start: 0.09, dur: 0.08, gain: 0.25 }, // "ten"
  { freq:  880, start: 0.18, dur: 0.08, gain: 0.25 }, // "do"
  { freq:  523, start: 0.30, dur: 0.55, gain: 0.40 }, // the ding
];

function playBootJingle(): void {
  let ctx: AudioContext;
  try {
    ctx = new (window.AudioContext || (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext)();
  } catch (_) { return; }

  const master = ctx.createGain();
  master.gain.setValueAtTime(1, ctx.currentTime);
  master.connect(ctx.destination);

  for (const { freq, start, dur, gain } of BOOT_NOTES) {
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
  }

  setTimeout(() => ctx.close(), 1200);
}

// ── Audio ring buffer ─────────────────────────────────────────────────────────
// Holds interleaved stereo samples [L,R,L,R,…] in a power-of-two ring so
// onaudioprocess can drain them without stalling on the main thread.

const RING_CAPACITY = 65536; // must be a power of 2
const RING_MASK     = RING_CAPACITY - 1;

class AudioRingBuffer {
  private readonly _data = new Float32Array(RING_CAPACITY * 2);
  private _head = 0;
  private _tail = 0;
  private _size = 0;

  /** Push interleaved stereo pairs; silently drops when full. */
  push(samples: Float32Array): void {
    const pairs = samples.length >> 1;
    for (let i = 0; i < pairs; i++) {
      if (this._size >= RING_CAPACITY) break;
      this._data[this._head * 2]     = samples[i * 2];
      this._data[this._head * 2 + 1] = samples[i * 2 + 1];
      this._head = (this._head + 1) & RING_MASK;
      this._size++;
    }
  }

  /** Fill separate left/right output buffers from the ring (silence on underrun). */
  drain(left: Float32Array, right: Float32Array): void {
    for (let i = 0; i < left.length; i++) {
      if (this._size > 0) {
        left[i]  = this._data[this._tail * 2];
        right[i] = this._data[this._tail * 2 + 1];
        this._tail = (this._tail + 1) & RING_MASK;
        this._size--;
      } else {
        left[i] = right[i] = 0;
      }
    }
  }

  clear(): void {
    this._head = 0;
    this._tail = 0;
    this._size = 0;
  }
}

// ── App state ─────────────────────────────────────────────────────────────────

interface UserInfo {
  display_name?: string;
  email?: string;
}

interface AppState {
  wasm:            unknown;                       // wasm module instance (after init)
  emulator:        EmulatorHandleDebug | null;
  roms:            string[];
  selectedIdx:     number;
  lastRomName:     string | null;
  running:         boolean;
  rafId:           number | null;
  audioCtx:        AudioContext | null;
  audioNode:       ScriptProcessorNode | null;
  audioRing:       AudioRingBuffer | null;
  debugOverlay:    boolean;
  user:            UserInfo | null;
  activeMenu:      MenuRendererInstance | null;
  currentRomName:  string | null;
  currentRomId:    string | null;
  batterySaveTimer: ReturnType<typeof setInterval> | null;
  paused:          boolean;
  menuPending:     boolean;
  menuGen:         number;
}

const state: AppState = {
  wasm:            null,
  emulator:        null,
  roms:            [],
  selectedIdx:     0,
  lastRomName:     localStorage.getItem('lastRom'),
  running:         false,
  rafId:           null,
  audioCtx:        null,
  audioNode:       null,
  audioRing:       null,
  debugOverlay:    false,
  user:            null,
  activeMenu:      null,
  currentRomName:  null,
  currentRomId:    null,
  batterySaveTimer: null,
  paused:          false,
  menuPending:     false,
  menuGen:         0,
};

// ── Audio ─────────────────────────────────────────────────────────────────────

const AUDIO_SAMPLE_RATE = 48000;

async function initAudio(): Promise<void> {
  if (state.audioCtx) return;
  try {
    const AudioCtx = window.AudioContext || (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    state.audioCtx = new AudioCtx({ sampleRate: AUDIO_SAMPLE_RATE });
    await state.audioCtx.resume();

    state.audioRing = new AudioRingBuffer();

    const node = state.audioCtx.createScriptProcessor(4096, 0, 2);
    node.onaudioprocess = (e: AudioProcessingEvent) => {
      const L = e.outputBuffer.getChannelData(0);
      const R = e.outputBuffer.getChannelData(1);
      state.audioRing?.drain(L, R);
    };
    node.connect(state.audioCtx.destination);
    state.audioNode = node;
    log.debug(`audio init: ctx=${state.audioCtx.state} rate=${state.audioCtx.sampleRate}`);
  } catch (e) {
    log.warn(`Audio init failed: ${e}`);
  }
}

function stopAudio(): void {
  state.audioNode?.disconnect();
  state.audioNode = null;
  state.audioCtx?.close();
  state.audioCtx = null;
  state.audioRing = null;
}

// ── SHA-256 (for ROM ID computation) ─────────────────────────────────────────

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  // crypto.subtle is only available in a secure context (HTTPS / localhost).
  // Fall back to a pure-JS implementation on plain HTTP LAN access.
  if (globalThis.crypto?.subtle) {
    // TS 5.7+: Uint8Array<ArrayBufferLike> doesn't satisfy BufferSource; cast is safe.
    const digest = await crypto.subtle.digest('SHA-256', bytes as unknown as BufferSource);
    return [...new Uint8Array(digest)]
      .map(b => b.toString(16).padStart(2, '0'))
      .join('');
  }
  return sha256HexJs(bytes);
}

/** Minimal FIPS 180-4 SHA-256 fallback for insecure contexts. */
function sha256HexJs(bytes: Uint8Array): string {
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

  const bitLen  = bytes.length * 8;
  const withPad = ((bytes.length + 8) >> 6 << 6) + 64;
  const m  = new Uint8Array(withPad);
  m.set(bytes);
  m[bytes.length] = 0x80;
  const dv = new DataView(m.buffer);
  dv.setUint32(withPad - 4, bitLen >>> 0, false);
  dv.setUint32(withPad - 8, Math.floor(bitLen / 0x100000000), false);

  const w    = new Uint32Array(64);
  const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));
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
      const t2  = (S0 + maj) | 0;
      h = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0;
    h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0;
  }
  return [h0, h1, h2, h3, h4, h5, h6, h7]
    .map(x => (x >>> 0).toString(16).padStart(8, '0'))
    .join('');
}

// ── ROM-scoped request headers ─────────────────────────────────────────────────

function romScopedHeaders(romId: string | null = state.currentRomId, extra: Record<string, string> = {}): Record<string, string> {
  const headers: Record<string, string> = { ...extra };
  if (romId) headers['x-rustyboy-rom-id'] = romId;
  return headers;
}

// ── Battery saves ─────────────────────────────────────────────────────────────

async function loadBatterySave(romName: string, romId: string | null = state.currentRomId): Promise<void> {
  try {
    const res = await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders(romId),
    });
    if (res.ok) {
      const buf = await res.arrayBuffer();
      if (buf.byteLength > 0) {
        state.emulator!.set_battery_save(new Uint8Array(buf));
        log.debug(`battery save loaded: ${buf.byteLength} bytes`);
      }
    }
  } catch (e) {
    log.warn(`battery save load failed: ${e}`);
  }
}

async function uploadBatterySave(romName: string, romId: string | null = state.currentRomId): Promise<void> {
  if (!state.emulator) return;
  const data = state.emulator.get_battery_save();
  if (!data || data.length === 0) return;
  try {
    await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      method: 'PUT',
      headers: romScopedHeaders(romId, { 'content-type': 'application/octet-stream' }),
      // TS 5.7+: Uint8Array<ArrayBufferLike> is not assignable to BodyInit; runtime is fine.
      body: data as unknown as BodyInit,
    });
    log.debug(`battery save uploaded: ${data.length} bytes`);
  } catch (e) {
    log.warn(`battery save upload failed: ${e}`);
  }
}

function startBatterySaveTimer(romName: string): void {
  stopBatterySaveTimer();
  state.batterySaveTimer = setInterval(() => uploadBatterySave(romName), 30_000);
}

function stopBatterySaveTimer(): void {
  if (state.batterySaveTimer !== null) {
    clearInterval(state.batterySaveTimer);
    state.batterySaveTimer = null;
  }
}

// ── DOM refs ──────────────────────────────────────────────────────────────────

const canvas      = document.getElementById('gameCanvas') as HTMLCanvasElement;
const ctx         = canvas.getContext('2d')!;
const menuOverlay = document.getElementById('menuOverlay')!;
const powerBtn    = document.getElementById('powerBtn')!;
const powerLed    = document.getElementById('powerLed')!;
const resetLed    = document.getElementById('resetLed');
const screenInner = canvas.parentElement!;
const screenBezel = screenInner.parentElement!;

// Keep the canvas buffer at device-pixel resolution so scaling is crisp
// (nearest-neighbour via drawImage rather than CSS).
new ResizeObserver(entries => {
  for (const entry of entries) {
    const dpr = window.devicePixelRatio || 1;
    const w   = Math.round(entry.contentRect.width  * dpr);
    const h   = Math.round(entry.contentRect.height * dpr);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width  = w;
      canvas.height = h;
    }
  }
}).observe(canvas);

// ── Boot ──────────────────────────────────────────────────────────────────────

async function boot(): Promise<void> {
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
  setupDpad();
  bindKeyboard();
  if (!authed) {
    showLoginScreen();
    return;
  }

  await loadRomList();
  showMainMenu();

  if (typeof (EmulatorHandle.prototype as EmulatorHandleDebug).debug_state === 'function') {
    bindDebugButton();
  }
}

function bindDebugButton(): void {
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
    btn.style.color      = state.debugOverlay ? '#000'    : '#9BBC0F';
  });
}

// ── Auth ──────────────────────────────────────────────────────────────────────

async function checkAuth(): Promise<boolean> {
  const params = new URLSearchParams(window.location.search);

  if (params.has('auth_error')) {
    await showLoginError();
    return false;
  }

  try {
    const res = await fetch('/api/me');
    if (res.ok) {
      state.user = await res.json() as UserInfo;
      if (params.has('logged_in')) history.replaceState({}, '', '/');
      return true;
    }
  } catch (_) { /* network error — treat as not authed */ }

  if (params.has('logged_out')) {
    history.replaceState({}, '', '/');
    return false;
  }

  // Not authed — check available auth methods
  try {
    const res = await fetch('/api/auth-method');
    if (res.ok) {
      const { methods } = await res.json() as { methods: string[] };
      if (methods.includes('cf')) {
        await fetch('/auth/cf-access');
        try {
          const meRes = await fetch('/api/me');
          if (meRes.ok) {
            state.user = await meRes.json() as UserInfo;
            window.location.href = '/';
            return false;
          }
        } catch (_) { /* fall through */ }
      }
    }
  } catch (_) { /* ignore — fall through to login screen */ }

  return false;
}

// ── Login / error screens ─────────────────────────────────────────────────────

function showLoginScreen(): void {
  log.event('showLoginScreen');
  menuOverlay.classList.add('hidden');
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'RUSTYBOY',
    items: [{ label: 'SIGN IN WITH GOOGLE', value: 'login' }],
    footer: '▲▼ MOVE  A SELECT',
    onSelect: () => { window.location.href = '/auth/google'; },
    onBack:   () => { showLoginScreen(); },
  });
}

/** Shows an auth-error screen and never resolves — user must click TRY AGAIN. */
async function showLoginError(): Promise<never> {
  return new Promise<never>(() => {
    menuOverlay.classList.add('hidden');
    const menu = new window.MenuRenderer(canvas);
    state.activeMenu = menu;
    menu.show({
      title: 'AUTH FAILED',
      items: [{ label: 'TRY AGAIN', value: 'retry' }],
      footer: 'A SELECT  B BACK',
      onSelect: () => { window.location.href = '/auth/google'; },
      onBack:   () => { showLoginScreen(); },
    });
  });
}

function showError(msg: string): void {
  // Best-effort: try MenuRenderer (may not be loaded yet), fall back to alert
  try {
    const menu = new window.MenuRenderer(canvas);
    state.activeMenu = menu;
    menu.show({
      title: 'ERROR',
      items: [{ label: msg, value: 'error' }],
      footer: '',
    });
  } catch (_) {
    alert(msg);
  }
}

// ── Main menu / ROM list ──────────────────────────────────────────────────────

/** Returns the ID of the most recent save state for a ROM, or null if none. */
async function fetchLatestSaveId(romName: string | null, romId: string | null = state.currentRomId): Promise<string | null> {
  if (!romName) return null;
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}/latest`, {
      headers: romScopedHeaders(romId),
    });
    if (!res.ok) return null;
    const data = await res.json() as { id?: string };
    return data.id ?? null;
  } catch (_) { return null; }
}

/** Returns true if any save states exist for the given ROM (or globally if romName is null). */
async function fetchHasSaves(romName: string | null, romId: string | null = state.currentRomId): Promise<boolean> {
  try {
    const url = romName
      ? `/api/save-states/${encodeURIComponent(romName)}`
      : '/api/save-states';
    const res = await fetch(url, {
      headers: romName ? romScopedHeaders(romId) : {},
    });
    if (!res.ok) return false;
    const data = await res.json() as unknown[];
    return data.length > 0;
  } catch (_) { return false; }
}

function showMainMenu(): void {
  log.event('showMainMenu');
  menuOverlay.classList.add('hidden');
  const gen = ++state.menuGen;

  // Render synchronously so a menu is active immediately — otherwise B/back
  // (onBack → showMainMenu) would leave a window with no active menu while we
  // await the save-state check. CONTINUE is prepended a moment later once we've
  // confirmed a save exists; with none (the common case) the menu is stable.
  renderMainMenu(false);

  hasSaveState()
    .then((has) => { if (has && state.menuGen === gen) renderMainMenu(true); })
    .catch(() => {});
}

/** (Re)build the main menu. CONTINUE is only present when a save state exists. */
function renderMainMenu(hasSave: boolean): void {
  const items: MenuItem[] = [];
  if (hasSave) items.push({ label: 'CONTINUE', value: 'continue' });
  items.push({ label: 'GAMES',  value: 'games' });
  items.push({ label: 'LOGOUT', value: 'logout' });

  const rawName = state.user?.display_name ?? state.user?.email ?? '';
  const name    = rawName.replace(/@[^@]+$/, '');
  const footer  = name ? `HELLO, ${name.toUpperCase()}` : '▲▼ MOVE  A SELECT';

  state.activeMenu?.hide();
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'RUSTYBOY',
    items,
    footer,
    onSelect: async (item) => {
      state.activeMenu = null;
      if (item.value === 'continue') {
        await continueLatestSave();
      } else if (item.value === 'logout') {
        doLogout();
      } else {
        showRomList();
      }
    },
    onBack: () => { showMainMenu(); },
    onSelectBtn: () => { doLogout(); },
  });
}

/** Log out server-side, then reload to the (now unauthenticated) login screen. */
function doLogout(): void {
  fetch('/auth/logout', { method: 'POST' }).finally(() => {
    window.location.href = '/?logged_out=1';
  });
}

/** Whether the user has at least one save state to resume via CONTINUE. */
async function hasSaveState(): Promise<boolean> {
  try {
    const res = await fetch('/api/save-states');
    if (!res.ok) return false;
    const roms = await res.json() as Array<{ rom_name: string }>;
    return roms.length > 0;
  } catch (_) {
    return false;
  }
}

/** CONTINUE: find the most recently saved game across all ROMs and resume it. */
async function continueLatestSave(): Promise<void> {
  try {
    const res = await fetch('/api/save-states');
    if (!res.ok) { showMainMenu(); return; }
    const roms = await res.json() as Array<{ rom_name: string }>;
    if (roms.length === 0) { showMainMenu(); return; }
    await launchRom(roms[0].rom_name);
  } catch (_) {
    showMainMenu();
  }
}

async function loadRomList(): Promise<void> {
  try {
    const res = await fetch('/api/roms');
    if (!res.ok) throw new Error(res.statusText);
    state.roms = await res.json() as string[];
  } catch (err) {
    log.error(err);
    state.roms = [];
  }
}

function showRomList(): void {
  log.event('showRomList');
  if (state.roms.length === 0) {
    showCanvasError('NO ROMS FOUND');
    return;
  }

  const lastIdx = state.roms.indexOf(state.lastRomName ?? '');
  state.selectedIdx = lastIdx >= 0 ? lastIdx : 0;

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: 'SELECT GAME',
    items: state.roms.map(name => ({ label: stripExtension(name), value: name })),
    footer: '▲▼ MOVE  A SELECT  B BACK',
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

function stripExtension(name: string): string {
  return name.replace(/\.(gb|gbc)$/i, '');
}

function showCanvasError(msg: string): void {
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

// ── Launch / stop ─────────────────────────────────────────────────────────────

async function launchRom(name: string): Promise<void> {
  await launchRomWithSaveState(name, undefined);
}

/** Fetch ROM bytes from the server. Returns null on failure (error shown). */
async function fetchRomBytes(name: string): Promise<Uint8Array | null> {
  try {
    const res = await fetch(`/roms/${encodeURIComponent(name)}`);
    if (!res.ok) throw new Error(res.statusText);
    return new Uint8Array(await res.arrayBuffer());
  } catch (err) {
    showCanvasError('LOAD ERROR');
    log.error(err);
    return null;
  }
}

/** Load a save state by ID, or fall back to battery save. */
async function loadSaveOrBattery(
  romName: string,
  romId: string,
  saveStateId: string | null,
): Promise<void> {
  if (saveStateId) {
    try {
      const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(saveStateId)}/data`);
      if (res.ok) {
        const buf = await res.arrayBuffer();
        state.emulator!.load_state(new Uint8Array(buf));
        log.debug(`save state loaded: ${buf.byteLength} bytes`);
      }
    } catch (e) {
      log.warn(`save state load failed: ${e}`);
    }
  } else {
    await loadBatterySave(romName, romId);
  }
}

async function launchRomWithSaveState(name: string, saveStateId: string | null | undefined): Promise<void> {
  log.event(`launch: start name=${name} saveStateId=${saveStateId}`);

  const bytes = await fetchRomBytes(name);
  if (!bytes) return;
  log.event(`launch: rom fetched ${bytes.length} bytes`);

  const romId = await sha256Hex(bytes);
  log.event(`launch: romId=${romId.slice(0, 8)}`);

  if (saveStateId === undefined) {
    saveStateId = await fetchLatestSaveId(name, romId);
    log.event(`launch: latest saveStateId=${saveStateId}`);
  }

  await stopEmulation();
  log.event('launch: stopEmulation done');

  try {
    state.emulator = new EmulatorHandle(bytes) as EmulatorHandleDebug;
  } catch (err) {
    showCanvasError('ROM ERROR');
    log.error(err);
    return;
  }
  log.event('launch: EmulatorHandle created');

  state.lastRomName    = name;
  state.currentRomName = name;
  state.currentRomId   = romId;
  localStorage.setItem('lastRom', name);
  state.running = true;
  state.paused  = false;

  await loadSaveOrBattery(name, romId, saveStateId ?? null);
  log.event('launch: saves loaded');

  startBatterySaveTimer(name);
  initAudio();
  log.event('launch: initAudio returned');
  playBootJingle();

  menuOverlay.classList.add('hidden');
  screenInner.classList.add('booting', 'running');
  screenBezel.classList.add('running');
  setLed('on');

  screenInner.addEventListener('animationend', () => screenInner.classList.remove('booting'), { once: true });
  startLoop();
}

async function stopEmulation(): Promise<void> {
  if (state.rafId !== null) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  stopBatterySaveTimer();
  if (state.emulator && state.currentRomName) {
    await uploadBatterySave(state.currentRomName);
  }
  state.emulator?.free?.();
  state.emulator = null;
  state.currentRomName = null;
  state.currentRomId   = null;
  state.running     = false;
  state.paused      = false;
  state.menuPending = false;
  state.menuGen++;
  stopAudio();
  screenInner.classList.remove('running', 'booting');
  screenBezel.classList.remove('running');
}

function pauseEmulation(): void {
  if (!state.running || state.paused) return;
  // Release all buttons so none stay stuck in the emulator while paused
  if (state.emulator) {
    for (let i = 0; i < 8; i++) state.emulator.set_button(i, false);
  }
  state.paused = true;
  state.menuGen++;
  if (state.rafId !== null) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  // Flush ring buffer so audio stops immediately without a pop
  state.audioRing?.clear();
}

function resumeEmulation(): void {
  if (!state.running || !state.paused) return;
  state.paused = false;
  state.menuGen++;
  state.activeMenu?.hide();
  state.activeMenu = null;
  startLoop();
}

async function returnToMenu(): Promise<void> {
  await stopEmulation();
  setLed('menu');
  showMainMenu();
}

// ── In-game pause menu ────────────────────────────────────────────────────────

function buildPauseMenuItems(hasSaves: boolean): MenuItem[] {
  const items: MenuItem[] = [
    { label: 'RESUME',     value: 'resume' },
    { label: 'SAVE',       value: 'save' },
  ];
  if (hasSaves) {
    items.push({ label: 'QUICK LOAD', value: 'quickload' });
    items.push({ label: 'LOAD',       value: 'load' });
  }
  items.push({ label: 'RESET', value: 'reset' });
  items.push({ label: 'QUIT',  value: 'quit' });
  return items;
}

async function handlePauseMenuSelect(
  item: MenuItem,
  latestSaveId: string | null,
): Promise<void> {
  state.activeMenu = null;

  switch (item.value) {
    case 'resume':
      resumeEmulation();
      break;

    case 'save':
      await saveCurrentState();
      resumeEmulation();
      break;

    case 'quickload':
      if (latestSaveId) {
        try {
          const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(latestSaveId)}/data`);
          if (res.ok) {
            const buf = await res.arrayBuffer();
            state.emulator!.load_state(new Uint8Array(buf));
            log.debug(`quick load: ${buf.byteLength} bytes`);
          }
        } catch (e) { log.warn(`quick load failed: ${e}`); }
      }
      resumeEmulation();
      break;

    case 'load':
      showSaveStateSlots(state.currentRomName!, async () => {
        const [hasSaves, latestSave] = await Promise.all([
          fetchHasSaves(state.currentRomName),
          fetchLatestSaveId(state.currentRomName),
        ]);
        showPauseMenu(hasSaves, latestSave);
      });
      break;

    case 'reset': {
      const romName = state.currentRomName!;
      await stopEmulation();
      await launchRomWithSaveState(romName, null);
      break;
    }

    case 'quit':
      await returnToMenu();
      break;
  }
}

function showPauseMenu(hasSaves: boolean, latestSaveId: string | null): void {
  const title = state.currentRomName
    ? stripExtension(state.currentRomName).toUpperCase()
    : 'PAUSED';

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title,
    items:    buildPauseMenuItems(hasSaves),
    footer:   '▲▼ MOVE A SELECT B RESUME',
    onSelect: (item) => handlePauseMenuSelect(item, latestSaveId),
    onBack:   () => { resumeEmulation(); },
  });
}

async function showInGameMenu(): Promise<void> {
  if (!state.running || state.menuPending || state.paused) return;
  pauseEmulation();
  state.menuPending = true;
  log.event('showInGameMenu');
  const gen = state.menuGen;

  const [hasSaves, latestSave] = await Promise.all([
    fetchHasSaves(state.currentRomName),
    fetchLatestSaveId(state.currentRomName),
  ]);
  state.menuPending = false;

  // If state changed while fetching (resumed, quit, new game) — abort
  if (state.menuGen !== gen || !state.paused || !state.running) return;

  showPauseMenu(hasSaves, latestSave);
}

async function saveCurrentState(): Promise<void> {
  if (!state.emulator || !state.currentRomName) return;
  try {
    const blob = state.emulator.save_state();
    await fetch(`/api/save-states/${encodeURIComponent(state.currentRomName)}`, {
      method: 'POST',
      headers: romScopedHeaders(state.currentRomId, { 'content-type': 'application/octet-stream' }),
      // TS 5.7+: Uint8Array<ArrayBufferLike> is not assignable to BodyInit; runtime is fine.
      body: blob as unknown as BodyInit,
    });
    showSavedOverlay();
    log.debug(`save state uploaded: ${blob.length} bytes`);
  } catch (e) {
    log.warn(`save state upload failed: ${e}`);
  }
}

function showSavedOverlay(): void {
  const c = canvas.getContext('2d')!;
  c.save();
  const s = canvas.width / 160;
  c.scale(s, s);
  c.fillStyle = 'rgba(15,56,15,0.85)';
  c.fillRect(0, 60, 160, 24);
  c.fillStyle    = '#9BBC0F';
  c.font         = 'bold 10px monospace';
  c.textAlign    = 'center';
  c.textBaseline = 'middle';
  c.fillText('✓ SAVED', 80, 72);
  c.restore();
  setTimeout(() => { if (state.running && !state.paused) drawFrame(); }, 1500);
}

async function showSaveStateSlots(romName: string, onBack: () => void): Promise<void> {
  let saves: Array<{ id: string; updated_at: number }> = [];
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders(),
    });
    if (res.ok) saves = await res.json() as typeof saves;
  } catch (_) {}

  if (saves.length === 0) { onBack(); return; }

  const items: MenuItem[] = saves.map(s => ({
    label: formatSaveSlotLabel(s.updated_at),
    value: s.id,
  }));

  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title:  'LOAD STATE',
    items,
    footer: '▲▼ A LOAD SEL DEL B BACK',
    onSelect: async (item) => {
      state.activeMenu = null;
      try {
        const res = await fetch(`/api/save-states/by-id/${encodeURIComponent(item.value)}/data`);
        if (res.ok) {
          const buf = await res.arrayBuffer();
          state.emulator!.load_state(new Uint8Array(buf));
          log.debug(`save state loaded: ${buf.byteLength} bytes`);
        }
      } catch (e) { log.warn(`save state load failed: ${e}`); }
      resumeEmulation();
    },
    onBack: () => { onBack(); },
    onSelectBtn: async (selIdx) => {
      const id = items[selIdx]?.value;
      if (!id) return;
      try {
        await fetch(`/api/save-states/by-id/${encodeURIComponent(id)}`, { method: 'DELETE' });
        log.debug(`save state deleted: ${id}`);
      } catch (e) { log.warn(`save state delete failed: ${e}`); }
      await showSaveStateSlots(romName, onBack);
    },
  });
}

function formatSaveSlotLabel(unixSecs: number): string {
  const d      = new Date(unixSecs * 1000);
  const months = ['JAN','FEB','MAR','APR','MAY','JUN','JUL','AUG','SEP','OCT','NOV','DEC'];
  const mon    = months[d.getMonth()];
  const day    = String(d.getDate()).padStart(2, '0');
  const h      = String(d.getHours()).padStart(2, '0');
  const m      = String(d.getMinutes()).padStart(2, '0');
  return `${mon} ${day} ${h}:${m}`;
}

// ── Emulation loop ────────────────────────────────────────────────────────────

let loopOffscreenCanvas: HTMLCanvasElement | null = null;
let loopOffscreenCtx: CanvasRenderingContext2D | null = null;
let loopImageData: ImageData | null = null;
let loopGeneration = 0;

function startLoop(): void {
  loopOffscreenCanvas = document.createElement('canvas');
  loopOffscreenCanvas.width  = 160;
  loopOffscreenCanvas.height = 144;
  loopOffscreenCtx = loopOffscreenCanvas.getContext('2d')!;
  loopImageData    = loopOffscreenCtx.createImageData(160, 144);

  const myGen = ++loopGeneration;
  let frameCount = 0;
  log.event('startLoop: scheduling first frame');

  function frame(_now: DOMHighResTimeStamp): void {
    if (!state.running || !state.emulator || loopGeneration !== myGen) return;

    try {
      if (frameCount < 3) log.event(`frame ${frameCount}: run_frame begin`);
      state.emulator.run_frame();
      if (frameCount < 3) log.event(`frame ${frameCount}: run_frame end`);
    } catch (e) {
      log.error(`run_frame error: ${e}`);
      return;
    }

    if (state.audioCtx) {
      state.audioRing?.push(state.emulator.drain_audio_samples());
    }

    try {
      drawFrame();
    } catch (e) {
      log.error(`drawFrame error: ${e}`);
      return;
    }
    if (frameCount < 3) log.event(`frame ${frameCount}: drawFrame done`);
    frameCount++;

    state.rafId = requestAnimationFrame(frame);
  }

  state.rafId = requestAnimationFrame(frame);
}

/** Draw the current emulator framebuffer to the visible canvas. */
function drawFrame(): void {
  const rgba = state.emulator!.framebuffer_rgba();
  loopImageData!.data.set(rgba);
  loopOffscreenCtx!.putImageData(loopImageData!, 0, 0);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(loopOffscreenCanvas!, 0, 0, canvas.width, canvas.height);

  if (state.debugOverlay && state.emulator?.debug_state) {
    drawDebugOverlay(ctx, state.emulator, canvas.width / 160);
  }
}

/** Render the debug CPU-state overlay onto an already-scaled canvas. */
function drawDebugOverlay(
  destCtx: CanvasRenderingContext2D,
  emulator: EmulatorHandleDebug,
  scale: number,
): void {
  const lines    = emulator.debug_state!().split('\n');
  const fontSize = 10;
  const lineH    = fontSize + 3;
  const pad      = 3;

  destCtx.save();
  destCtx.scale(scale, scale);
  destCtx.font         = `bold ${fontSize}px monospace`;
  destCtx.textBaseline = 'top';

  let maxW = 0;
  for (const l of lines) {
    const m = destCtx.measureText(l).width;
    if (m > maxW) maxW = m;
  }

  const boxW = Math.min(maxW + pad * 2, 160);
  const boxH = lines.length * lineH + pad * 2;

  destCtx.fillStyle = '#000';
  destCtx.fillRect(0, 0, boxW, boxH);
  destCtx.fillStyle = '#9BBC0F';
  lines.forEach((line, i) => {
    destCtx.fillText(line, pad, pad + lineH * i);
  });
  destCtx.restore();
}

// ── Button handling ───────────────────────────────────────────────────────────

const PAUSE_BUTTON_KEY_MAP: Record<number, string> = {
  2: 'ArrowUp',
  3: 'ArrowDown',
  4: 'Enter',
  5: 'Escape',
  6: 'Select',
};

const MENU_REPEAT_KEYS = new Set(['ArrowUp', 'ArrowDown', 'w', 's']);
const MENU_REPEAT_DELAY_MS = 320;
const MENU_REPEAT_INTERVAL_MS = 115;
let menuRepeatKey: string | null = null;
let menuRepeatDelayTimer: number | null = null;
let menuRepeatIntervalTimer: number | null = null;

function stopMenuRepeat(key?: string): void {
  if (key && menuRepeatKey !== key) return;
  if (menuRepeatDelayTimer !== null) {
    window.clearTimeout(menuRepeatDelayTimer);
    menuRepeatDelayTimer = null;
  }
  if (menuRepeatIntervalTimer !== null) {
    window.clearInterval(menuRepeatIntervalTimer);
    menuRepeatIntervalTimer = null;
  }
  menuRepeatKey = null;
}

function sendMenuKey(key: string): boolean {
  if (!state.activeMenu?.isActive()) {
    stopMenuRepeat();
    return false;
  }
  state.activeMenu.handleInput(key);
  return true;
}

function startMenuRepeat(key: string): void {
  if (!MENU_REPEAT_KEYS.has(key)) {
    sendMenuKey(key);
    return;
  }
  if (menuRepeatKey === key) return;
  stopMenuRepeat();

  menuRepeatKey = key;
  if (!sendMenuKey(key)) return;
  menuRepeatDelayTimer = window.setTimeout(() => {
    menuRepeatDelayTimer = null;
    if (!sendMenuKey(key)) return;
    menuRepeatIntervalTimer = window.setInterval(() => {
      sendMenuKey(key);
    }, MENU_REPEAT_INTERVAL_MS);
  }, MENU_REPEAT_DELAY_MS);
}

function handleMenuButton(idx: number, pressed: boolean): boolean {
  const key = PAUSE_BUTTON_KEY_MAP[idx];
  if (!key || !state.activeMenu?.isActive()) return false;

  if (MENU_REPEAT_KEYS.has(key)) {
    if (pressed) startMenuRepeat(key);
    else stopMenuRepeat(key);
    return true;
  }

  if (!pressed) sendMenuKey(key);
  return true;
}

function sendButton(idx: number, pressed: boolean): void {
  log.event(`sendButton idx=${idx} pressed=${pressed}`);

  if (state.paused) {
    if (handleMenuButton(idx, pressed)) log.debug(`sendButton (paused) → menu idx=${idx}`);
    return;
  }

  if (state.emulator) {
    state.emulator.set_button(idx, pressed);
  } else if (handleMenuButton(idx, pressed)) {
    log.debug(`sendButton → menu idx=${idx}`);
  }
}

function bindButtons(): void {
  document.querySelectorAll<HTMLElement>('[data-btn]').forEach(el => {
    const idx = parseInt(el.dataset['btn']!, 10);
    let held = false;

    el.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      try { el.setPointerCapture(e.pointerId); } catch (_) {}
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

    el.addEventListener('lostpointercapture', () => {
      el.classList.remove('pressed');
      if (held) { held = false; sendButton(idx, false); }
    });
  });

  powerBtn.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    powerBtn.classList.add('pressed');
    flashResetLed();
  });

  powerBtn.addEventListener('pointerup', () => {
    powerBtn.classList.remove('pressed');
    if (state.menuPending) return;
    if (state.running && !state.paused) {
      showInGameMenu();
    } else if (state.paused && state.activeMenu) {
      resumeEmulation();
    } else if (!state.running) {
      returnToMenu();
    }
  });

  powerBtn.addEventListener('pointerleave',  () => { powerBtn.classList.remove('pressed'); });
  powerBtn.addEventListener('pointercancel', () => { powerBtn.classList.remove('pressed'); });
}

// ── On-screen D-pad (nipplejs) ─────────────────────────────────────────────────
//
// The four directions used to be four separate <button data-btn> cells, each
// grabbing a pointer capture on press. That capture is what made the controls
// feel broken: once your thumb landed on a cell you couldn't roll onto a
// diagonal or slide between directions, and sliding off a cell could strand a
// held direction. We replace them with a single nipplejs static joystick over
// the #dpadZone well. Its `move` event gives a normalized vector we threshold
// into up to two simultaneous directions (real diagonals); `end` releases all.
// Everything downstream still flows through sendButton(), so the menu/pause
// handling and keyboard path are unchanged.

const DPAD_DIR_IDX = { right: 0, left: 1, up: 2, down: 3 } as const;
type DpadDir = keyof typeof DPAD_DIR_IDX;

// Fraction of the joystick vector an axis must reach to register as pressed.
// Keep vertical slightly stricter because its current activation zone feels
// right; horizontal engages earlier so left/right are less far from center.
const DPAD_X_THRESHOLD = 0.4;
const DPAD_Y_THRESHOLD = 0.5;
const DPAD_JOYSTICK_SIZE = 132;

const dpadHeld: Record<DpadDir, boolean> = { right: false, left: false, up: false, down: false };
const DPAD_RELEASED: Record<DpadDir, boolean> = { right: false, left: false, up: false, down: false };
let activeDpadPointerId: number | null = null;
const activeDpadTouchIds = new Set<number>();
let dpadManager: ReturnType<typeof createNipple> | null = null;

/** Diff the desired direction state against what's held and emit only changes. */
function applyDpadState(next: Record<DpadDir, boolean>): void {
  for (const dir of Object.keys(next) as DpadDir[]) {
    if (next[dir] === dpadHeld[dir]) continue;
    dpadHeld[dir] = next[dir];
    sendButton(DPAD_DIR_IDX[dir], next[dir]);
  }
}

function isDpadHeld(): boolean {
  return Object.values(dpadHeld).some(Boolean);
}

function forceReleaseEmulatorDpadButtons(): void {
  if (!state.emulator) return;
  for (const dir of Object.keys(DPAD_DIR_IDX) as DpadDir[]) {
    state.emulator.set_button(DPAD_DIR_IDX[dir], false);
  }
}

function restDpadJoystick(): void {
  try {
    dpadManager?.getJoystickByUid()?.end();
  } catch (_) {
    // If nipplejs is already mid-cleanup, the app-level release above is still
    // the important part. A fresh pointerdown will reuse/recreate the joystick.
  }
}

function releaseDpad(resetJoystick = false): void {
  activeDpadPointerId = null;
  activeDpadTouchIds.clear();
  stopMenuRepeat();
  applyDpadState(DPAD_RELEASED);
  if (resetJoystick) {
    forceReleaseEmulatorDpadButtons();
    restDpadJoystick();
  }
}

function startMenuRepeatFromDpadPoint(clientX: number, clientY: number): void {
  if (!state.activeMenu?.isActive()) return;
  if (state.emulator && !state.paused) return;

  const zone = document.getElementById('dpadZone');
  if (!zone) return;

  const rect = zone.getBoundingClientRect();
  const centerY = rect.top + rect.height / 2;
  const threshold = DPAD_Y_THRESHOLD * (DPAD_JOYSTICK_SIZE / 2);
  const dy = clientY - centerY;

  if (dy < -threshold) startMenuRepeat('ArrowUp');
  else if (dy > threshold) startMenuRepeat('ArrowDown');
}

function trackDpadPointerStart(evt: PointerEvent): void {
  if (activeDpadPointerId !== null && activeDpadPointerId !== evt.pointerId) {
    releaseDpad(true);
  }
  activeDpadPointerId = evt.pointerId;
  startMenuRepeatFromDpadPoint(evt.clientX, evt.clientY);
}

function releaseDpadPointer(evt: PointerEvent, resetJoystick = false): void {
  if (activeDpadPointerId !== null && evt.pointerId !== activeDpadPointerId) return;
  if (activeDpadPointerId === null && !isDpadHeld()) return;
  releaseDpad(resetJoystick);
}

function releaseDpadPointerAndReset(evt: PointerEvent): void {
  releaseDpadPointer(evt, true);
}

function trackDpadTouchStart(evt: TouchEvent): void {
  if (activeDpadTouchIds.size > 0) releaseDpad(true);
  for (const touch of Array.from(evt.changedTouches)) {
    activeDpadTouchIds.add(touch.identifier);
    startMenuRepeatFromDpadPoint(touch.clientX, touch.clientY);
  }
}

function releaseDpadTouch(evt: TouchEvent): void {
  if (activeDpadTouchIds.size === 0) {
    if (isDpadHeld()) releaseDpad(true);
    return;
  }

  for (const touch of Array.from(evt.changedTouches)) {
    if (activeDpadTouchIds.has(touch.identifier)) {
      releaseDpad(true);
      return;
    }
  }
}

function releaseDpadOnHidden(): void {
  if (document.visibilityState !== 'visible') releaseDpad(true);
}

function releaseDpadAndReset(): void {
  releaseDpad(true);
}

function setupDpad(): void {
  const zone = document.getElementById('dpadZone');
  if (!zone) return; // e2e fixture / non-index pages have no d-pad zone

  zone.addEventListener('pointerdown', trackDpadPointerStart, { capture: true });
  document.addEventListener('pointerup', releaseDpadPointer, true);
  document.addEventListener('pointercancel', releaseDpadPointerAndReset, true);
  document.addEventListener('lostpointercapture', releaseDpadPointerAndReset, true);

  zone.addEventListener('touchstart', trackDpadTouchStart, { capture: true, passive: true });
  document.addEventListener('touchend', releaseDpadTouch, true);
  document.addEventListener('touchcancel', releaseDpadTouch, true);

  window.addEventListener('blur', releaseDpadAndReset);
  window.addEventListener('pagehide', releaseDpadAndReset);
  window.addEventListener('contextmenu', releaseDpadAndReset);
  document.addEventListener('visibilitychange', releaseDpadOnHidden);

  dpadManager = createNipple({
    zone,
    mode: 'static',
    position: { top: '50%', left: '50%' },
    size: 132,
    threshold: 0.15,
    fadeTime: 0,
    restJoystick: true,
    restOpacity: 1,
    color: { back: 'transparent', front: '#6E7290' },
  });

  dpadManager.on('move', (evt) => {
    const d = evt.data;
    if (!d?.vector) return;
    applyDpadState({
      right: d.vector.x >  DPAD_X_THRESHOLD,
      left:  d.vector.x < -DPAD_X_THRESHOLD,
      up:    d.vector.y >  DPAD_Y_THRESHOLD,
      down:  d.vector.y < -DPAD_Y_THRESHOLD,
    });
  });

  // nipplejs does not emit a final centred `move` before `end`, so release here.
  dpadManager.on('end', () => releaseDpad());
}

// ── Keyboard support ──────────────────────────────────────────────────────────

const KEY_MAP: Record<string, number> = {
  ArrowRight: 0, ArrowLeft: 1, ArrowUp: 2, ArrowDown: 3,
  z: 4, Z: 4,   // A
  x: 5, X: 5,   // B
  Shift: 6,      // Select
  Enter: 7,      // Start
  Backspace: -1, // Power / menu
};

const MENU_NAV_KEYS = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'w', 's', 'Enter', 'Escape', 'a', 'b', 'Shift']);

const heldKeys = new Set<string>();

function bindKeyboard(): void {
  document.addEventListener('keydown', (e) => {
    if (heldKeys.has(e.key)) {
      log.debug(`keydown IGNORED (held) key=${e.key}`);
      return;
    }
    heldKeys.add(e.key);
    log.debug(`keydown key=${e.key} activeMenu=${state.activeMenu?.isActive() ? state.activeMenu.title : 'none'}`);

    if (state.activeMenu?.isActive() && MENU_NAV_KEYS.has(e.key)) {
      e.preventDefault();
      const menuKey = e.key === 'Shift' ? 'Select' : e.key;
      if (MENU_REPEAT_KEYS.has(menuKey)) startMenuRepeat(menuKey);
      else sendMenuKey(menuKey);
      return;
    }

    if ((e.key === "'" || e.key === '`') &&
        typeof (EmulatorHandle.prototype as EmulatorHandleDebug).debug_state === 'function') {
      state.debugOverlay = !state.debugOverlay;
      return;
    }

    const idx = KEY_MAP[e.key];
    if (idx === undefined) return;
    e.preventDefault();

    if (idx === -1) {
      if (state.menuPending) return;
      if (state.running && !state.paused) showInGameMenu();
      else if (!state.running) returnToMenu();
    } else {
      sendButton(idx, true);
    }
  });

  document.addEventListener('keyup', (e) => {
    log.debug(`keyup key=${e.key}`);
    heldKeys.delete(e.key);
    const menuKey = e.key === 'Shift' ? 'Select' : e.key;
    if (MENU_REPEAT_KEYS.has(menuKey)) stopMenuRepeat(menuKey);
    const idx = KEY_MAP[e.key];
    if (idx === undefined || idx === -1) return;
    e.preventDefault();
    sendButton(idx, false);
  });
}

// ── LED helpers ───────────────────────────────────────────────────────────────

function setLed(mode: string): void {
  powerLed.className = 'power-led ' + (mode || '');
  if (resetLed) resetLed.className = 'reset-led' + (mode === 'on' ? ' on' : '');
}

function flashResetLed(): void {
  if (!resetLed) return;
  resetLed.classList.remove('flash');
  void resetLed.offsetWidth; // force reflow to restart animation
  resetLed.classList.add('flash');
  resetLed.addEventListener('animationend', () => resetLed!.classList.remove('flash'), { once: true });
}

// ── Start ─────────────────────────────────────────────────────────────────────

window.__appState = state;
boot();
