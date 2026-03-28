/**
 * rustyboy — Game Boy emulator frontend
 * ES module, no bundler required.
 * Loads rustyboy_wasm via wasm-bindgen --target web.
 */

import init, { EmulatorHandle } from '/static/rustyboy_web_client.js';

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
};

// ── DOM refs ───────────────────────────────────────────────────────────────

const canvas      = document.getElementById('gameCanvas');
const ctx         = canvas.getContext('2d');
const menuOverlay = document.getElementById('menuOverlay');
const romList     = document.getElementById('romList');
const powerBtn    = document.getElementById('powerBtn');
const powerLed    = document.getElementById('powerLed');
const screenInner = canvas.parentElement;
const screenBezel = screenInner.parentElement;

// ── Boot ───────────────────────────────────────────────────────────────────

async function boot() {
  try {
    state.wasm = await init();
  } catch (err) {
    showError('WASM LOAD FAILED');
    console.error(err);
    return;
  }

  setLed('menu');
  await loadRomList();
  bindButtons();
  bindKeyboard();
}

// ── ROM list ───────────────────────────────────────────────────────────────

async function loadRomList() {
  try {
    const res = await fetch('/api/roms');
    if (!res.ok) throw new Error(res.statusText);
    state.roms = await res.json();
  } catch (err) {
    showError('NO ROMS FOUND');
    console.error(err);
    return;
  }

  if (state.roms.length === 0) {
    showError('NO ROMS FOUND');
    return;
  }

  // Restore last selection
  const lastIdx = state.roms.indexOf(state.lastRomName);
  state.selectedIdx = lastIdx >= 0 ? lastIdx : 0;

  renderMenu();
}

function renderMenu() {
  const maxVisible = 7;
  const total      = state.roms.length;
  const sel        = state.selectedIdx;

  // Scroll window so selected is visible
  const start = Math.max(0, Math.min(sel - 2, total - maxVisible));
  const end   = Math.min(start + maxVisible, total);
  const slice = state.roms.slice(start, end);

  romList.innerHTML = '';
  slice.forEach((name, i) => {
    const realIdx = start + i;
    const el = document.createElement('div');
    el.className = 'menu-item' + (realIdx === sel ? ' selected' : '');
    el.textContent = stripExtension(name);
    el.dataset.idx = realIdx;
    el.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      if (realIdx === sel) {
        launchRom(state.roms[realIdx]);
      } else {
        state.selectedIdx = realIdx;
        renderMenu();
      }
    });
    romList.appendChild(el);
  });
}

function stripExtension(name) {
  return name.replace(/\.(gb|gbc)$/i, '');
}

function showError(msg) {
  romList.innerHTML = `<div class="menu-loading">${msg}</div>`;
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
    console.error(err);
    return;
  }

  // Tear down previous
  stopEmulation();

  // Create emulator
  try {
    state.emulator = new EmulatorHandle(bytes);
  } catch (err) {
    showError('ROM ERROR');
    console.error(err);
    return;
  }

  state.lastRomName = name;
  localStorage.setItem('lastRom', name);
  state.running = true;

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
  screenInner.classList.remove('running', 'booting');
  screenBezel.classList.remove('running');
}

function returnToMenu() {
  stopEmulation();
  setLed('menu');
  menuOverlay.classList.remove('hidden');

  // Reset selection to last ROM
  const lastIdx = state.roms.indexOf(state.lastRomName);
  state.selectedIdx = lastIdx >= 0 ? lastIdx : 0;
  renderMenu();
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
        console.error('run_frame error:', e);
        return;
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
}

// ── Button handling ────────────────────────────────────────────────────────

function sendButton(idx, pressed) {
  if (state.emulator) {
    state.emulator.set_button(idx, pressed);
  } else if (!pressed) {
    // Menu navigation on button release
    handleMenuInput(idx);
  }
}

function handleMenuInput(idx) {
  if (state.roms.length === 0) return;
  const total = state.roms.length;

  switch (idx) {
    case 2: // Up
      state.selectedIdx = (state.selectedIdx - 1 + total) % total;
      renderMenu();
      break;
    case 3: // Down
      state.selectedIdx = (state.selectedIdx + 1) % total;
      renderMenu();
      break;
    case 4: // A
    case 7: // Start
      launchRom(state.roms[state.selectedIdx]);
      break;
  }
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

  // Power button
  powerBtn.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    returnToMenu();
  });
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

function bindKeyboard() {
  document.addEventListener('keydown', (e) => {
    if (heldKeys.has(e.key)) return;
    heldKeys.add(e.key);

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
}

// ── Start ──────────────────────────────────────────────────────────────────

boot();
