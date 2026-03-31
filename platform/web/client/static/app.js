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
        const cfRes = await fetch('/auth/cf-access');
        if (cfRes.ok || cfRes.redirected) {
          // CF set a session cookie — reload to pick it up.
          window.location.href = '/';
          return false;
        }
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

function showMainMenu() {
  log.event('showMainMenu');
  menuOverlay.classList.add('hidden');
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  const rawName = state.user && (state.user.display_name || state.user.email) || '';
  const name = rawName.length > 12 ? rawName.slice(0, 12) + '...' : rawName;
  menu.show({
    title: 'RUSTYBOY',
    items: [
      { label: 'PLAY',   value: 'play' },
      { label: 'LOGOUT', value: 'logout' },
    ],
    footer: name ? ('HELLO, ' + name.toUpperCase()) : '\u25b2\u25bc MOVE  A SELECT',
    onSelect: (item) => {
      state.activeMenu = null;
      if (item.value === 'play') {
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
  // Fetch bytes
  let bytes;
  try {
    const res = await fetch(`/roms/${encodeURIComponent(name)}`);
    if (!res.ok) throw new Error(res.statusText);
    const buf = await res.arrayBuffer();
    bytes = new Uint8Array(buf);
  } catch (err) {
    showError('LOAD ERROR');
    log.error(err);
    return;
  }

  // Tear down previous
  stopEmulation();

  // Create emulator
  try {
    state.emulator = new EmulatorHandle(bytes);
  } catch (err) {
    showError('ROM ERROR');
    log.error(err);
    return;
  }

  state.lastRomName = name;
  localStorage.setItem('lastRom', name);
  state.running = true;

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

function stopEmulation() {
  if (state.rafId) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  if (state.emulator) {
    state.emulator.free?.();
    state.emulator = null;
  }
  state.running = false;
  stopAudio();
  screenInner.classList.remove('running', 'booting');
  screenBezel.classList.remove('running');
}

function returnToMenu() {
  stopEmulation();
  setLed('menu');
  showMainMenu();
}

// ── Emulation loop ─────────────────────────────────────────────────────────

let imageData = null;

// DMG runs at 4194304 Hz / 70224 cycles per frame = 59.7275 fps
const FRAME_DURATION_MS = 1000 / 59.7275;

function startLoop() {
  imageData = ctx.createImageData(160, 144);
  let lastFrameTime = performance.now();

  function frame(now) {
    if (!state.running || !state.emulator) return;

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

  // Reset button — animate press, flash LED, then return to menu
  powerBtn.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    powerBtn.classList.add('pressed');
    flashResetLed();
  });
  powerBtn.addEventListener('pointerup',     () => { powerBtn.classList.remove('pressed'); returnToMenu(); });
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
      returnToMenu();
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
