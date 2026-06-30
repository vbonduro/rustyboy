var __typeError = (msg) => {
  throw TypeError(msg);
};
var __accessCheck = (obj, member, msg) => member.has(obj) || __typeError("Cannot " + msg);
var __privateGet = (obj, member, getter) => (__accessCheck(obj, member, "read from private field"), getter ? getter.call(obj) : member.get(obj));
var __privateAdd = (obj, member, value) => member.has(obj) ? __typeError("Cannot add the same private member more than once") : member instanceof WeakSet ? member.add(obj) : member.set(obj, value);
var __privateSet = (obj, member, value, setter) => (__accessCheck(obj, member, "write to private field"), setter ? setter.call(obj, value) : member.set(obj, value), value);
var __privateMethod = (obj, member, method) => (__accessCheck(obj, member, "access private method"), method);
var __privateWrapper = (obj, member, setter, getter) => ({
  set _(value) {
    __privateSet(obj, member, value, setter);
  },
  get _() {
    return __privateGet(obj, member, getter);
  }
});

// src-ts/app.ts
import init, { EmulatorHandle, WasmMenuRenderer } from "/static/rustyboy_web_client.js?v=gemini-title-bg-crisp-csp-20260627";
var WASM_URL = "/static/rustyboy_web_client_bg.wasm?v=gemini-title-bg-crisp-csp-20260627";
var _tag, _seq, _Logger_instances, post_fn;
var Logger = class {
  constructor(tag) {
    __privateAdd(this, _Logger_instances);
    __privateAdd(this, _tag);
    __privateAdd(this, _seq, 0);
    __privateSet(this, _tag, tag);
  }
  debug(msg) {
    console.debug(`[${__privateGet(this, _tag)}]`, msg);
    __privateMethod(this, _Logger_instances, post_fn).call(this, "DEBUG", msg);
  }
  warn(msg) {
    console.warn(`[${__privateGet(this, _tag)}]`, msg);
    __privateMethod(this, _Logger_instances, post_fn).call(this, "WARN", msg);
  }
  error(msg) {
    console.error(`[${__privateGet(this, _tag)}]`, msg);
    __privateMethod(this, _Logger_instances, post_fn).call(this, "ERROR", String(msg));
  }
  /** Log a named app-state transition with sequence number. */
  event(label) {
    const seq = ++__privateWrapper(this, _seq)._;
    const menuTitle = state.activeMenu?.isActive() ? state.activeMenu.title : "none";
    const msg = `#${seq} ${label} | activeMenu=${menuTitle} | emulator=${!!state.emulator}`;
    console.debug(`[${__privateGet(this, _tag)}] ${msg}`);
    __privateMethod(this, _Logger_instances, post_fn).call(this, "EVENT", msg);
  }
};
_tag = new WeakMap();
_seq = new WeakMap();
_Logger_instances = new WeakSet();
post_fn = function(level, msg) {
  fetch("/dev/log", { method: "POST", body: `${level} ${msg}` }).catch(() => {
  });
};
var log = new Logger("rustyboy");
var BOOT_NOTES = [
  { freq: 1320, start: 0, dur: 0.08, gain: 0.25 },
  // "Vin"
  { freq: 1047, start: 0.09, dur: 0.08, gain: 0.25 },
  // "ten"
  { freq: 880, start: 0.18, dur: 0.08, gain: 0.25 },
  // "do"
  { freq: 523, start: 0.3, dur: 0.55, gain: 0.4 }
  // the ding
];
function playBootJingle() {
  let ctx2;
  try {
    ctx2 = new (window.AudioContext || window.webkitAudioContext)();
  } catch (_) {
    return;
  }
  const master = ctx2.createGain();
  master.gain.setValueAtTime(1, ctx2.currentTime);
  master.connect(ctx2.destination);
  for (const { freq, start, dur, gain } of BOOT_NOTES) {
    const osc = ctx2.createOscillator();
    const env = ctx2.createGain();
    osc.type = "square";
    osc.frequency.setValueAtTime(freq, ctx2.currentTime + start);
    env.gain.setValueAtTime(0, ctx2.currentTime + start);
    env.gain.linearRampToValueAtTime(gain, ctx2.currentTime + start + 0.01);
    env.gain.setValueAtTime(gain, ctx2.currentTime + start + dur * 0.6);
    env.gain.exponentialRampToValueAtTime(1e-3, ctx2.currentTime + start + dur);
    osc.connect(env);
    env.connect(master);
    osc.start(ctx2.currentTime + start);
    osc.stop(ctx2.currentTime + start + dur + 0.05);
  }
  setTimeout(() => ctx2.close(), 1200);
}
var RING_CAPACITY = 65536;
var RING_MASK = RING_CAPACITY - 1;
var AudioRingBuffer = class {
  constructor() {
    this._data = new Float32Array(RING_CAPACITY * 2);
    this._head = 0;
    this._tail = 0;
    this._size = 0;
  }
  /** Push interleaved stereo pairs; silently drops when full. */
  push(samples) {
    const pairs = samples.length >> 1;
    for (let i = 0; i < pairs; i++) {
      if (this._size >= RING_CAPACITY) break;
      this._data[this._head * 2] = samples[i * 2];
      this._data[this._head * 2 + 1] = samples[i * 2 + 1];
      this._head = this._head + 1 & RING_MASK;
      this._size++;
    }
  }
  /** Fill separate left/right output buffers from the ring (silence on underrun). */
  drain(left, right) {
    for (let i = 0; i < left.length; i++) {
      if (this._size > 0) {
        left[i] = this._data[this._tail * 2];
        right[i] = this._data[this._tail * 2 + 1];
        this._tail = this._tail + 1 & RING_MASK;
        this._size--;
      } else {
        left[i] = right[i] = 0;
      }
    }
  }
  clear() {
    this._head = 0;
    this._tail = 0;
    this._size = 0;
  }
};
var state = {
  wasm: null,
  emulator: null,
  roms: [],
  selectedIdx: 0,
  lastRomName: localStorage.getItem("lastRom"),
  running: false,
  rafId: null,
  audioCtx: null,
  audioNode: null,
  audioRing: null,
  debugOverlay: false,
  user: null,
  activeMenu: null,
  currentRomName: null,
  currentRomId: null,
  batterySaveTimer: null,
  paused: false,
  menuPending: false,
  menuGen: 0
};
var AUDIO_SAMPLE_RATE = 48e3;
async function initAudio() {
  if (state.audioCtx) return;
  try {
    const AudioCtx = window.AudioContext || window.webkitAudioContext;
    state.audioCtx = new AudioCtx({ sampleRate: AUDIO_SAMPLE_RATE });
    await state.audioCtx.resume();
    state.audioRing = new AudioRingBuffer();
    const node = state.audioCtx.createScriptProcessor(4096, 0, 2);
    node.onaudioprocess = (e) => {
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
function stopAudio() {
  state.audioNode?.disconnect();
  state.audioNode = null;
  state.audioCtx?.close();
  state.audioCtx = null;
  state.audioRing = null;
}
async function sha256Hex(bytes) {
  if (globalThis.crypto?.subtle) {
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  }
  return sha256HexJs(bytes);
}
function sha256HexJs(bytes) {
  const K = new Uint32Array([
    1116352408,
    1899447441,
    3049323471,
    3921009573,
    961987163,
    1508970993,
    2453635748,
    2870763221,
    3624381080,
    310598401,
    607225278,
    1426881987,
    1925078388,
    2162078206,
    2614888103,
    3248222580,
    3835390401,
    4022224774,
    264347078,
    604807628,
    770255983,
    1249150122,
    1555081692,
    1996064986,
    2554220882,
    2821834349,
    2952996808,
    3210313671,
    3336571891,
    3584528711,
    113926993,
    338241895,
    666307205,
    773529912,
    1294757372,
    1396182291,
    1695183700,
    1986661051,
    2177026350,
    2456956037,
    2730485921,
    2820302411,
    3259730800,
    3345764771,
    3516065817,
    3600352804,
    4094571909,
    275423344,
    430227734,
    506948616,
    659060556,
    883997877,
    958139571,
    1322822218,
    1537002063,
    1747873779,
    1955562222,
    2024104815,
    2227730452,
    2361852424,
    2428436474,
    2756734187,
    3204031479,
    3329325298
  ]);
  let h0 = 1779033703, h1 = 3144134277, h2 = 1013904242, h3 = 2773480762;
  let h4 = 1359893119, h5 = 2600822924, h6 = 528734635, h7 = 1541459225;
  const bitLen = bytes.length * 8;
  const withPad = (bytes.length + 8 >> 6 << 6) + 64;
  const m = new Uint8Array(withPad);
  m.set(bytes);
  m[bytes.length] = 128;
  const dv = new DataView(m.buffer);
  dv.setUint32(withPad - 4, bitLen >>> 0, false);
  dv.setUint32(withPad - 8, Math.floor(bitLen / 4294967296), false);
  const w = new Uint32Array(64);
  const rotr = (x, n) => x >>> n | x << 32 - n;
  for (let off = 0; off < withPad; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4, false);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ w[i - 15] >>> 3;
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ w[i - 2] >>> 10;
      w[i] = w[i - 16] + s0 + w[i - 7] + s1 | 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = e & f ^ ~e & g;
      const t1 = h + S1 + ch + K[i] + w[i] | 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = a & b ^ a & c ^ b & c;
      const t2 = S0 + maj | 0;
      h = g;
      g = f;
      f = e;
      e = d + t1 | 0;
      d = c;
      c = b;
      b = a;
      a = t1 + t2 | 0;
    }
    h0 = h0 + a | 0;
    h1 = h1 + b | 0;
    h2 = h2 + c | 0;
    h3 = h3 + d | 0;
    h4 = h4 + e | 0;
    h5 = h5 + f | 0;
    h6 = h6 + g | 0;
    h7 = h7 + h | 0;
  }
  return [h0, h1, h2, h3, h4, h5, h6, h7].map((x) => (x >>> 0).toString(16).padStart(8, "0")).join("");
}
function romScopedHeaders(romId = state.currentRomId, extra = {}) {
  const headers = { ...extra };
  if (romId) headers["x-rustyboy-rom-id"] = romId;
  return headers;
}
async function loadBatterySave(romName, romId = state.currentRomId) {
  try {
    const res = await fetch(`/api/battery-saves/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders(romId)
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
      method: "PUT",
      headers: romScopedHeaders(romId, { "content-type": "application/octet-stream" }),
      // TS 5.7+: Uint8Array<ArrayBufferLike> is not assignable to BodyInit; runtime is fine.
      body: data
    });
    log.debug(`battery save uploaded: ${data.length} bytes`);
  } catch (e) {
    log.warn(`battery save upload failed: ${e}`);
  }
}
function startBatterySaveTimer(romName) {
  stopBatterySaveTimer();
  state.batterySaveTimer = setInterval(() => uploadBatterySave(romName), 3e4);
}
function stopBatterySaveTimer() {
  if (state.batterySaveTimer !== null) {
    clearInterval(state.batterySaveTimer);
    state.batterySaveTimer = null;
  }
}
var canvas = document.getElementById("gameCanvas");
var ctx = canvas.getContext("2d");
var menuOverlay = document.getElementById("menuOverlay");
var powerBtn = document.getElementById("powerBtn");
var powerLed = document.getElementById("powerLed");
var resetLed = document.getElementById("resetLed");
var screenInner = canvas.parentElement;
var screenBezel = screenInner.parentElement;
new ResizeObserver((entries) => {
  for (const entry of entries) {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(entry.contentRect.width * dpr);
    const h = Math.round(entry.contentRect.height * dpr);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
  }
}).observe(canvas);
async function boot() {
  try {
    state.wasm = await init(WASM_URL);
    window.RustyBoyWasmMenuRenderer = WasmMenuRenderer;
  } catch (err) {
    showError("WASM LOAD FAILED");
    log.error(err);
    return;
  }
  setLed("menu");
  const authed = await checkAuth();
  bindButtons();
  bindKeyboard();
  if (!authed) {
    showLoginScreen();
    return;
  }
  await loadRomList();
  showMainMenu();
  if (typeof EmulatorHandle.prototype.debug_state === "function") {
    bindDebugButton();
  }
}
function bindDebugButton() {
  const housing = document.querySelector(".screen-housing");
  if (!housing) return;
  const btn = document.createElement("button");
  btn.id = "debugBtn";
  btn.textContent = "DBG";
  btn.style.cssText = "position:absolute;top:4px;right:8px;background:rgba(0,0,0,0.7);color:#9BBC0F;font:8px monospace;border:1px solid #9BBC0F;border-radius:2px;padding:3px 6px;z-index:50;cursor:pointer;touch-action:manipulation;-webkit-tap-highlight-color:transparent;";
  housing.appendChild(btn);
  btn.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    state.debugOverlay = !state.debugOverlay;
    btn.style.background = state.debugOverlay ? "#9BBC0F" : "rgba(0,0,0,0.7)";
    btn.style.color = state.debugOverlay ? "#000" : "#9BBC0F";
  });
}
async function checkAuth() {
  const params = new URLSearchParams(window.location.search);
  if (params.has("auth_error")) {
    await showLoginError();
    return false;
  }
  try {
    const res = await fetch("/api/me");
    if (res.ok) {
      state.user = await res.json();
      if (params.has("logged_in")) history.replaceState({}, "", "/");
      return true;
    }
  } catch (_) {
  }
  if (params.has("logged_out")) {
    history.replaceState({}, "", "/");
    return false;
  }
  try {
    const res = await fetch("/api/auth-method");
    if (res.ok) {
      const { methods } = await res.json();
      if (methods.includes("cf")) {
        await fetch("/auth/cf-access");
        try {
          const meRes = await fetch("/api/me");
          if (meRes.ok) {
            state.user = await meRes.json();
            window.location.href = "/";
            return false;
          }
        } catch (_) {
        }
      }
    }
  } catch (_) {
  }
  return false;
}
function showLoginScreen() {
  log.event("showLoginScreen");
  menuOverlay.classList.add("hidden");
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: "RUSTYBOY",
    items: [{ label: "SIGN IN WITH GOOGLE", value: "login" }],
    footer: "\u25B2\u25BC MOVE  A SELECT",
    onSelect: () => {
      window.location.href = "/auth/google";
    },
    onBack: () => {
      showLoginScreen();
    }
  });
}
async function showLoginError() {
  return new Promise(() => {
    menuOverlay.classList.add("hidden");
    const menu = new window.MenuRenderer(canvas);
    state.activeMenu = menu;
    menu.show({
      title: "AUTH FAILED",
      items: [{ label: "TRY AGAIN", value: "retry" }],
      footer: "A SELECT  B BACK",
      onSelect: () => {
        window.location.href = "/auth/google";
      },
      onBack: () => {
        showLoginScreen();
      }
    });
  });
}
function showError(msg) {
  try {
    const menu = new window.MenuRenderer(canvas);
    state.activeMenu = menu;
    menu.show({
      title: "ERROR",
      items: [{ label: msg, value: "error" }],
      footer: ""
    });
  } catch (_) {
    alert(msg);
  }
}
async function fetchLatestSaveId(romName, romId = state.currentRomId) {
  if (!romName) return null;
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}/latest`, {
      headers: romScopedHeaders(romId)
    });
    if (!res.ok) return null;
    const data = await res.json();
    return data.id ?? null;
  } catch (_) {
    return null;
  }
}
async function fetchHasSaves(romName, romId = state.currentRomId) {
  try {
    const url = romName ? `/api/save-states/${encodeURIComponent(romName)}` : "/api/save-states";
    const res = await fetch(url, {
      headers: romName ? romScopedHeaders(romId) : {}
    });
    if (!res.ok) return false;
    const data = await res.json();
    return data.length > 0;
  } catch (_) {
    return false;
  }
}
async function showMainMenu() {
  log.event("showMainMenu");
  menuOverlay.classList.add("hidden");
  const items = [
    { label: "CONTINUE", value: "continue" },
    { label: "GAMES", value: "games" }
  ];
  const rawName = state.user?.display_name ?? state.user?.email ?? "";
  const name = rawName.replace(/@[^@]+$/, "");
  const footer = name ? `HELLO, ${name.toUpperCase()}` : "\u25B2\u25BC MOVE  A SELECT";
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: "RUSTYBOY",
    items,
    footer,
    onSelect: async (item) => {
      state.activeMenu = null;
      if (item.value === "continue") {
        await continueLatestSave();
      } else {
        showRomList();
      }
    },
    onBack: () => {
      showMainMenu();
    },
    onSelectBtn: () => {
      fetch("/auth/logout", { method: "POST" }).finally(() => {
        window.location.href = "/?logged_out=1";
      });
    }
  });
}
async function continueLatestSave() {
  try {
    const res = await fetch("/api/save-states");
    if (!res.ok) {
      showMainMenu();
      return;
    }
    const roms = await res.json();
    if (roms.length === 0) {
      showMainMenu();
      return;
    }
    await launchRom(roms[0].rom_name);
  } catch (_) {
    showMainMenu();
  }
}
async function loadRomList() {
  try {
    const res = await fetch("/api/roms");
    if (!res.ok) throw new Error(res.statusText);
    state.roms = await res.json();
  } catch (err) {
    log.error(err);
    state.roms = [];
  }
}
function showRomList() {
  log.event("showRomList");
  if (state.roms.length === 0) {
    showCanvasError("NO ROMS FOUND");
    return;
  }
  const lastIdx = state.roms.indexOf(state.lastRomName ?? "");
  state.selectedIdx = lastIdx >= 0 ? lastIdx : 0;
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: "SELECT GAME",
    items: state.roms.map((name) => ({ label: stripExtension(name), value: name })),
    footer: "\u25B2\u25BC MOVE  A SELECT  B BACK",
    onSelect: (item) => {
      state.activeMenu = null;
      launchRom(item.value);
    },
    onBack: () => {
      state.activeMenu = null;
      showMainMenu();
    }
  });
}
function stripExtension(name) {
  return name.replace(/\.(gb|gbc)$/i, "");
}
function showCanvasError(msg) {
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: "ERROR",
    items: [{ label: msg, value: "error" }],
    footer: "B BACK",
    onBack: () => {
      state.activeMenu = null;
      showMainMenu();
    }
  });
}
async function launchRom(name) {
  await launchRomWithSaveState(name, void 0);
}
async function fetchRomBytes(name) {
  try {
    const res = await fetch(`/roms/${encodeURIComponent(name)}`);
    if (!res.ok) throw new Error(res.statusText);
    return new Uint8Array(await res.arrayBuffer());
  } catch (err) {
    showCanvasError("LOAD ERROR");
    log.error(err);
    return null;
  }
}
async function loadSaveOrBattery(romName, romId, saveStateId) {
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
    await loadBatterySave(romName, romId);
  }
}
async function launchRomWithSaveState(name, saveStateId) {
  log.event(`launch: start name=${name} saveStateId=${saveStateId}`);
  const bytes = await fetchRomBytes(name);
  if (!bytes) return;
  log.event(`launch: rom fetched ${bytes.length} bytes`);
  const romId = await sha256Hex(bytes);
  log.event(`launch: romId=${romId.slice(0, 8)}`);
  if (saveStateId === void 0) {
    saveStateId = await fetchLatestSaveId(name, romId);
    log.event(`launch: latest saveStateId=${saveStateId}`);
  }
  await stopEmulation();
  log.event("launch: stopEmulation done");
  try {
    state.emulator = new EmulatorHandle(bytes);
  } catch (err) {
    showCanvasError("ROM ERROR");
    log.error(err);
    return;
  }
  log.event("launch: EmulatorHandle created");
  state.lastRomName = name;
  state.currentRomName = name;
  state.currentRomId = romId;
  localStorage.setItem("lastRom", name);
  state.running = true;
  state.paused = false;
  await loadSaveOrBattery(name, romId, saveStateId ?? null);
  log.event("launch: saves loaded");
  startBatterySaveTimer(name);
  initAudio();
  log.event("launch: initAudio returned");
  playBootJingle();
  menuOverlay.classList.add("hidden");
  screenInner.classList.add("booting", "running");
  screenBezel.classList.add("running");
  setLed("on");
  screenInner.addEventListener("animationend", () => screenInner.classList.remove("booting"), { once: true });
  startLoop();
}
async function stopEmulation() {
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
  state.currentRomId = null;
  state.running = false;
  state.paused = false;
  state.menuPending = false;
  state.menuGen++;
  stopAudio();
  screenInner.classList.remove("running", "booting");
  screenBezel.classList.remove("running");
}
function pauseEmulation() {
  if (!state.running || state.paused) return;
  if (state.emulator) {
    for (let i = 0; i < 8; i++) state.emulator.set_button(i, false);
  }
  state.paused = true;
  state.menuGen++;
  if (state.rafId !== null) {
    cancelAnimationFrame(state.rafId);
    state.rafId = null;
  }
  state.audioRing?.clear();
}
function resumeEmulation() {
  if (!state.running || !state.paused) return;
  state.paused = false;
  state.menuGen++;
  state.activeMenu?.hide();
  state.activeMenu = null;
  startLoop();
}
async function returnToMenu() {
  await stopEmulation();
  setLed("menu");
  showMainMenu();
}
function buildPauseMenuItems(hasSaves) {
  const items = [
    { label: "RESUME", value: "resume" },
    { label: "SAVE", value: "save" }
  ];
  if (hasSaves) {
    items.push({ label: "QUICK LOAD", value: "quickload" });
    items.push({ label: "LOAD", value: "load" });
  }
  items.push({ label: "RESET", value: "reset" });
  items.push({ label: "QUIT", value: "quit" });
  return items;
}
async function handlePauseMenuSelect(item, latestSaveId) {
  state.activeMenu = null;
  switch (item.value) {
    case "resume":
      resumeEmulation();
      break;
    case "save":
      await saveCurrentState();
      resumeEmulation();
      break;
    case "quickload":
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
      break;
    case "load":
      showSaveStateSlots(state.currentRomName, async () => {
        const [hasSaves, latestSave] = await Promise.all([
          fetchHasSaves(state.currentRomName),
          fetchLatestSaveId(state.currentRomName)
        ]);
        showPauseMenu(hasSaves, latestSave);
      });
      break;
    case "reset": {
      const romName = state.currentRomName;
      await stopEmulation();
      await launchRomWithSaveState(romName, null);
      break;
    }
    case "quit":
      await returnToMenu();
      break;
  }
}
function showPauseMenu(hasSaves, latestSaveId) {
  const title = state.currentRomName ? stripExtension(state.currentRomName).toUpperCase() : "PAUSED";
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title,
    items: buildPauseMenuItems(hasSaves),
    footer: "\u25B2\u25BC MOVE  A SELECT  B RESUME",
    onSelect: (item) => handlePauseMenuSelect(item, latestSaveId),
    onBack: () => {
      resumeEmulation();
    }
  });
}
async function showInGameMenu() {
  if (!state.running || state.menuPending || state.paused) return;
  pauseEmulation();
  state.menuPending = true;
  log.event("showInGameMenu");
  const gen = state.menuGen;
  const [hasSaves, latestSave] = await Promise.all([
    fetchHasSaves(state.currentRomName),
    fetchLatestSaveId(state.currentRomName)
  ]);
  state.menuPending = false;
  if (state.menuGen !== gen || !state.paused || !state.running) return;
  showPauseMenu(hasSaves, latestSave);
}
async function saveCurrentState() {
  if (!state.emulator || !state.currentRomName) return;
  try {
    const blob = state.emulator.save_state();
    await fetch(`/api/save-states/${encodeURIComponent(state.currentRomName)}`, {
      method: "POST",
      headers: romScopedHeaders(state.currentRomId, { "content-type": "application/octet-stream" }),
      // TS 5.7+: Uint8Array<ArrayBufferLike> is not assignable to BodyInit; runtime is fine.
      body: blob
    });
    showSavedOverlay();
    log.debug(`save state uploaded: ${blob.length} bytes`);
  } catch (e) {
    log.warn(`save state upload failed: ${e}`);
  }
}
function showSavedOverlay() {
  const c = canvas.getContext("2d");
  c.save();
  const s = canvas.width / 160;
  c.scale(s, s);
  c.fillStyle = "rgba(15,56,15,0.85)";
  c.fillRect(0, 60, 160, 24);
  c.fillStyle = "#9BBC0F";
  c.font = "bold 10px monospace";
  c.textAlign = "center";
  c.textBaseline = "middle";
  c.fillText("\u2713 SAVED", 80, 72);
  c.restore();
  setTimeout(() => {
    if (state.running && !state.paused) drawFrame();
  }, 1500);
}
async function showSaveStateSlots(romName, onBack) {
  let saves = [];
  try {
    const res = await fetch(`/api/save-states/${encodeURIComponent(romName)}`, {
      headers: romScopedHeaders()
    });
    if (res.ok) saves = await res.json();
  } catch (_) {
  }
  if (saves.length === 0) {
    onBack();
    return;
  }
  const items = saves.map((s) => ({
    label: formatSaveSlotLabel(s.updated_at),
    value: s.id
  }));
  const menu = new window.MenuRenderer(canvas);
  state.activeMenu = menu;
  menu.show({
    title: "LOAD STATE",
    items,
    footer: "\u25B2\u25BC MOVE  A LOAD  SEL DEL  B BACK",
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
      onBack();
    },
    onSelectBtn: async (selIdx) => {
      const id = items[selIdx]?.value;
      if (!id) return;
      try {
        await fetch(`/api/save-states/by-id/${encodeURIComponent(id)}`, { method: "DELETE" });
        log.debug(`save state deleted: ${id}`);
      } catch (e) {
        log.warn(`save state delete failed: ${e}`);
      }
      await showSaveStateSlots(romName, onBack);
    }
  });
}
function formatSaveSlotLabel(unixSecs) {
  const d = new Date(unixSecs * 1e3);
  const months = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
  const mon = months[d.getMonth()];
  const day = String(d.getDate()).padStart(2, "0");
  const h = String(d.getHours()).padStart(2, "0");
  const m = String(d.getMinutes()).padStart(2, "0");
  return `${mon} ${day} ${h}:${m}`;
}
var loopOffscreenCanvas = null;
var loopOffscreenCtx = null;
var loopImageData = null;
var loopGeneration = 0;
function startLoop() {
  loopOffscreenCanvas = document.createElement("canvas");
  loopOffscreenCanvas.width = 160;
  loopOffscreenCanvas.height = 144;
  loopOffscreenCtx = loopOffscreenCanvas.getContext("2d");
  loopImageData = loopOffscreenCtx.createImageData(160, 144);
  const myGen = ++loopGeneration;
  let frameCount = 0;
  log.event("startLoop: scheduling first frame");
  function frame(_now) {
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
function drawFrame() {
  const rgba = state.emulator.framebuffer_rgba();
  loopImageData.data.set(rgba);
  loopOffscreenCtx.putImageData(loopImageData, 0, 0);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(loopOffscreenCanvas, 0, 0, canvas.width, canvas.height);
  if (state.debugOverlay && state.emulator?.debug_state) {
    drawDebugOverlay(ctx, state.emulator, canvas.width / 160);
  }
}
function drawDebugOverlay(destCtx, emulator, scale) {
  const lines = emulator.debug_state().split("\n");
  const fontSize = 10;
  const lineH = fontSize + 3;
  const pad = 3;
  destCtx.save();
  destCtx.scale(scale, scale);
  destCtx.font = `bold ${fontSize}px monospace`;
  destCtx.textBaseline = "top";
  let maxW = 0;
  for (const l of lines) {
    const m = destCtx.measureText(l).width;
    if (m > maxW) maxW = m;
  }
  const boxW = Math.min(maxW + pad * 2, 160);
  const boxH = lines.length * lineH + pad * 2;
  destCtx.fillStyle = "#000";
  destCtx.fillRect(0, 0, boxW, boxH);
  destCtx.fillStyle = "#9BBC0F";
  lines.forEach((line, i) => {
    destCtx.fillText(line, pad, pad + lineH * i);
  });
  destCtx.restore();
}
var PAUSE_BUTTON_KEY_MAP = {
  2: "ArrowUp",
  3: "ArrowDown",
  4: "Enter",
  5: "Escape",
  6: "Select"
};
function sendButton(idx, pressed) {
  log.event(`sendButton idx=${idx} pressed=${pressed}`);
  if (state.paused) {
    if (!pressed && state.activeMenu?.isActive()) {
      const key = PAUSE_BUTTON_KEY_MAP[idx];
      log.debug(`sendButton (paused) \u2192 menu key=${key}`);
      if (key) state.activeMenu.handleInput(key);
    }
    return;
  }
  if (state.emulator) {
    state.emulator.set_button(idx, pressed);
  } else if (!pressed) {
    if (state.activeMenu?.isActive()) {
      const key = PAUSE_BUTTON_KEY_MAP[idx];
      log.debug(`sendButton \u2192 menu key=${key}`);
      if (key) {
        state.activeMenu.handleInput(key);
        return;
      }
    }
    handleMenuInput(idx);
  }
}
function handleMenuInput(_idx) {
}
function bindButtons() {
  document.querySelectorAll("[data-btn]").forEach((el) => {
    const idx = parseInt(el.dataset["btn"], 10);
    let held = false;
    el.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      try {
        el.setPointerCapture(e.pointerId);
      } catch (_) {
      }
      el.classList.add("pressed");
      held = true;
      sendButton(idx, true);
    });
    el.addEventListener("pointerup", (e) => {
      e.preventDefault();
      el.classList.remove("pressed");
      if (held) {
        held = false;
        sendButton(idx, false);
      }
    });
    el.addEventListener("pointercancel", () => {
      el.classList.remove("pressed");
      if (held) {
        held = false;
        sendButton(idx, false);
      }
    });
    el.addEventListener("lostpointercapture", () => {
      el.classList.remove("pressed");
      if (held) {
        held = false;
        sendButton(idx, false);
      }
    });
  });
  powerBtn.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    powerBtn.classList.add("pressed");
    flashResetLed();
  });
  powerBtn.addEventListener("pointerup", () => {
    powerBtn.classList.remove("pressed");
    if (state.menuPending) return;
    if (state.running && !state.paused) {
      showInGameMenu();
    } else if (state.paused && state.activeMenu) {
      resumeEmulation();
    } else if (!state.running) {
      returnToMenu();
    }
  });
  powerBtn.addEventListener("pointerleave", () => {
    powerBtn.classList.remove("pressed");
  });
  powerBtn.addEventListener("pointercancel", () => {
    powerBtn.classList.remove("pressed");
  });
}
var KEY_MAP = {
  ArrowRight: 0,
  ArrowLeft: 1,
  ArrowUp: 2,
  ArrowDown: 3,
  z: 4,
  Z: 4,
  // A
  x: 5,
  X: 5,
  // B
  Shift: 6,
  // Select
  Enter: 7,
  // Start
  Backspace: -1
  // Power / menu
};
var MENU_NAV_KEYS = /* @__PURE__ */ new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "w", "s", "Enter", "Escape", "a", "b", "Shift"]);
var heldKeys = /* @__PURE__ */ new Set();
function bindKeyboard() {
  document.addEventListener("keydown", (e) => {
    if (heldKeys.has(e.key)) {
      log.debug(`keydown IGNORED (held) key=${e.key}`);
      return;
    }
    heldKeys.add(e.key);
    log.debug(`keydown key=${e.key} activeMenu=${state.activeMenu?.isActive() ? state.activeMenu.title : "none"}`);
    if (state.activeMenu?.isActive() && MENU_NAV_KEYS.has(e.key)) {
      e.preventDefault();
      state.activeMenu.handleInput(e.key === "Shift" ? "Select" : e.key);
      return;
    }
    if ((e.key === "'" || e.key === "`") && typeof EmulatorHandle.prototype.debug_state === "function") {
      state.debugOverlay = !state.debugOverlay;
      return;
    }
    const idx = KEY_MAP[e.key];
    if (idx === void 0) return;
    e.preventDefault();
    if (idx === -1) {
      if (state.menuPending) return;
      if (state.running && !state.paused) showInGameMenu();
      else if (!state.running) returnToMenu();
    } else {
      sendButton(idx, true);
    }
  });
  document.addEventListener("keyup", (e) => {
    log.debug(`keyup key=${e.key}`);
    heldKeys.delete(e.key);
    const idx = KEY_MAP[e.key];
    if (idx === void 0 || idx === -1) return;
    e.preventDefault();
    sendButton(idx, false);
  });
}
function setLed(mode) {
  powerLed.className = "power-led " + (mode || "");
  if (resetLed) resetLed.className = "reset-led" + (mode === "on" ? " on" : "");
}
function flashResetLed() {
  if (!resetLed) return;
  resetLed.classList.remove("flash");
  void resetLed.offsetWidth;
  resetLed.classList.add("flash");
  resetLed.addEventListener("animationend", () => resetLed.classList.remove("flash"), { once: true });
}
window.__appState = state;
boot();
