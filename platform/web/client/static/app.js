/**
 * rustyboy — Game Boy emulator frontend
 * ES module, no bundler required.
 * Loads rustyboy_wasm via wasm-bindgen --target web.
 */

import init, { EmulatorHandle } from '/static/rustyboy_web_client.js';

class Logger {
  #tag;
  #seq = 0;

  constructor(tag) { this.#tag = tag; }

  debug(msg) { console.debug(`[${this.#tag}]`, msg); }
  warn(msg)  { console.warn(`[${this.#tag}]`, msg); }
  error(msg) { console.error(`[${this.#tag}]`, msg); }

  /** Log a named app-state transition with sequence number, then POST to /dev/log. */
  event(label) {
    const seq = ++this.#seq;
    const activeMenu = state.activeMenu?.isActive() ? state.activeMenu._opts?.title : 'none';
    const msg = `#${seq} ${label} | activeMenu=${activeMenu} | emulator=${!!state.emulator}`;
    console.debug(`[${this.#tag}] ${msg}`);
    fetch('/dev/log', { method: 'POST', body: msg }).catch(() => {});
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

async function loadBatterySave(romName) {
  try {
    const res = await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`);
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

async function uploadBatterySave(romName) {
  if (!state.emulator) return;
  const data = state.emulator.get_battery_save();
  if (!data || data.length === 0) return;
  try {
    await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/octet-stream' },
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
    state.wasm = await init();
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

async function showMainMenu() {
  log.event('showMainMenu');
  menuOverlay.classList.add('hidden');

  // Check if user has any saves to show CONTINUE
  let hasSaves = false;
  if (state.user) {
    try {
      const res = await fetch('/api/save-states');
      if (res.ok) {
        const roms = await res.json();
        hasSaves = roms.length > 0;
      }
    } catch (_) {}
  }

  const items = [];
  if (hasSaves) items.push({ label: 'CONTINUE', value: 'continue' });
  items.push({ label: 'GAMES',  value: 'games' });
  items.push({ label: 'LOGOUT', value: 'logout' });

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
      } else if (item.value === 'logout') {
        fetch('/auth/logout', { method: 'POST' }).finally(() => {
          window.location.href = '/?logged_out=1';
        });
      }
    },
    onBack: () => { showMainMenu(); }, // B on main menu → stay on main menu
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
    // Get the latest save state for that ROM
    const latestRes = await fetch(`/api/save-states/${encodeURIComponent(romName)}/latest`);
    if (!latestRes.ok) { showMainMenu(); return; }
    const latestMeta = await latestRes.json();
    await launchRomWithSaveState(romName, latestMeta.id);
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
  // Check for a latest save state to auto-load
  let saveStateId = null;
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(name)}/latest`);
    if (res.ok) {
      const meta = await res.json();
      saveStateId = meta.id;
    }
  } catch (_) {}

  await launchRomWithSaveState(name, saveStateId);
}

async function launchRomWithSaveState(name, saveStateId) {
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

  // Tear down previous
  await stopEmulation();

  // Create emulator
  try {
    state.emulator = new EmulatorHandle(bytes);
  } catch (err) {
    showCanvasError('ROM ERROR');
    log.error(err);
    return;
  }

  state.lastRomName = name;
  state.currentRomName = name;
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
    await loadBatterySave(name);
  }
  startBatterySaveTimer(name);

  initAudio();
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

function showPauseMenu(hasSaves) {
  const items = [
    { label: 'RESUME', value: 'resume' },
    { label: 'SAVE',   value: 'save' },
  ];
  if (hasSaves) items.push({ label: 'LOAD', value: 'load' });
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
      } else if (item.value === 'load') {
        // When returning from load screen (e.g. deleted all slots), re-show pause menu without saves
        showSaveStateSlots(state.currentRomName, () => showPauseMenu(false));
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

  // Check if saves exist for current game
  let hasSaves = false;
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(state.currentRomName)}`);
    if (res.ok) {
      const saves = await res.json();
      hasSaves = saves.length > 0;
    }
  } catch (_) {}

  state.menuPending = false;

  // If state changed while we were fetching (resumed, quit, new game), abort
  if (state.menuGen !== gen || !state.paused || !state.running) return;

  showPauseMenu(hasSaves);
}

async function saveCurrentState() {
  if (!state.emulator || !state.currentRomName) return;
  try {
    const blob = state.emulator.save_state();
    await fetch(`/api/save-states/${encodeURIComponent(state.currentRomName)}`, {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream' },
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
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}`);
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
    footer: '\u25b2\u25bc MOVE  A LOAD  B DEL',
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
    onBack: async (selIdx) => {
      // B = delete the currently selected slot
      const id = items[selIdx]?.value;
      if (!id) { state.activeMenu = null; if (onBack) onBack(); return; }
      try {
        await fetch(`/api/save-states/by-id/${encodeURIComponent(id)}`, { method: 'DELETE' });
        log.debug(`save state deleted: ${id}`);
      } catch (e) {
        log.warn(`save state delete failed: ${e}`);
      }
      state.activeMenu = null;
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
let loopGeneration = 0; // incremented each time startLoop() is called; stale RAF callbacks self-cancel

// DMG runs at 4194304 Hz / 70224 cycles per frame = 59.7275 fps
const FRAME_DURATION_MS = 1000 / 59.7275;

function startLoop() {
  imageData = ctx.createImageData(160, 144);
  let lastFrameTime = performance.now();
  const myGen = ++loopGeneration;

  function frame(now) {
    if (!state.running || !state.emulator || loopGeneration !== myGen) return;

    const elapsed = now - lastFrameTime;
    if (elapsed >= FRAME_DURATION_MS) {
      const framesToRun = Math.min(2, Math.floor(elapsed / FRAME_DURATION_MS));
      lastFrameTime = now - (elapsed % FRAME_DURATION_MS);

      try {
        for (let i = 0; i < framesToRun; i++) state.emulator.run_frame();
      } catch(e) {
        log.error(`run_frame error: ${e}`);
        return;
      }

      if (state.audioCtx) {
        pushAudioSamples(state.emulator.drain_audio_samples());
      }

      drawFrame();
    }

    state.rafId = requestAnimationFrame(frame);
  }

  state.rafId = requestAnimationFrame(frame);
}

function drawFrame() {
  const rgba = state.emulator.framebuffer_rgba();
  imageData.data.set(rgba);
  ctx.putImageData(imageData, 0, 0);

  if (state.debugOverlay && typeof state.emulator.debug_state === 'function') {
    const lines = state.emulator.debug_state().split('\n');
    // Draw at a size that fills most of the canvas width when scaled up
    const fontSize = 10;
    const lineH = fontSize + 3;
    const pad = 3;
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
  }
}

// ── Button handling ────────────────────────────────────────────────────────

function sendButton(idx, pressed) {
  log.event(`sendButton idx=${idx} pressed=${pressed}`);
  // While paused, route button releases to the canvas menu (not the emulator)
  if (state.paused) {
    if (!pressed && state.activeMenu && state.activeMenu.isActive()) {
      const keyMap = { 2: 'ArrowUp', 3: 'ArrowDown', 4: 'Enter', 5: 'Escape' };
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
      const keyMap = { 2: 'ArrowUp', 3: 'ArrowDown', 4: 'Enter', 5: 'Escape' };
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

    el.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      el.classList.add('pressed');
      sendButton(idx, true);
    });

    el.addEventListener('pointerup', (e) => {
      e.preventDefault();
      el.classList.remove('pressed');
      sendButton(idx, false);
    });

    el.addEventListener('pointerleave', (e) => {
      if (el.classList.contains('pressed')) {
        el.classList.remove('pressed');
        sendButton(idx, false);
      }
    });

    el.addEventListener('pointercancel', () => {
      el.classList.remove('pressed');
      sendButton(idx, false);
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
    const MENU_NAV_KEYS = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'w', 's', 'Enter', 'Escape', 'a', 'b']);
    if (state.activeMenu && state.activeMenu.isActive() && MENU_NAV_KEYS.has(e.key)) {
      e.preventDefault();
      state.activeMenu.handleInput(e.key);
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
