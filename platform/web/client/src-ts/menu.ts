/**
 * MenuRenderer — draws GB-aesthetic menus directly onto the 160×144 game canvas.
 * Exposes window.MenuRenderer as a side-effect (built as IIFE by esbuild).
 *
 * Usage:
 *   const menu = new MenuRenderer(canvas);
 *   menu.show({
 *     title: 'SELECT GAME',
 *     items: [{ label: 'Tetris', value: 'tetris.gb' }, …],
 *     footer: '▲▼ MOVE  A SELECT  B BACK',
 *     onSelect: (item) => { … },
 *     onBack: () => { … },
 *   });
 */

import type { MenuItem, MenuOptions, MenuRendererInstance } from './types.js';

// ── GB phosphor palette (dark → light) ───────────────────────────────────────
const C0 = '#0F380F'; // darkest  — background
const C1 = '#306230'; // dark     — header/footer fill, selection bg
const C2 = '#8BAC0F'; // medium   — unselected text
const C3 = '#9BBC0F'; // lightest — selected text, header text

// ── Layout constants (all in logical 160×144 coordinate space) ────────────────
const W = 160;
const H = 144;

const HEADER_H    = 14;
const FOOTER_H    = 14;
const ITEM_H      = 14;
const LIST_TOP    = HEADER_H + 2;
const LIST_BOTTOM = H - FOOTER_H - 2;
const MAX_VISIBLE = Math.floor((LIST_BOTTOM - LIST_TOP) / ITEM_H);
const TEXT_PAD    = 6;

const DEFAULT_FOOTER = '▲▼ MOVE  A SELECT  B BACK';

// ── WasmMenuRenderer interface ────────────────────────────────────────────────
// Matches the wasm-bindgen export in rustyboy_web_client.d.ts.

interface WasmRendererLike {
  free?(): void;
  show(title: string, labels: string[], footer: string): void;
  selected_index(): number;
  scroll_y(): number;
  set_selected(idx: number): void;
  move_selection(delta: number): void;
  scroll_by(delta: number): void;
  item_at(x: number, y: number): number;
  render_rgba(frameArg: number): Uint8Array;
  title_width_px(): number;
}

// ── Pure geometry helpers ─────────────────────────────────────────────────────

interface Point {
  x: number;
  y: number;
}

/**
 * Map a pointer/touch event's client coordinates into logical 160×144 canvas space.
 * Pure — depends only on the canvas element's bounding rect and the input coords.
 */
function canvasCoords(canvas: HTMLCanvasElement, clientX: number, clientY: number): Point {
  const rect = canvas.getBoundingClientRect();
  return {
    x: (clientX - rect.left) * (W / rect.width),
    y: (clientY - rect.top)  * (H / rect.height),
  };
}

/**
 * Return a scrollY that keeps selIdx visible, clamped to [0, itemCount − MAX_VISIBLE].
 * Pure — takes all data as arguments, returns the new scrollY value.
 */
function clampScroll(selIdx: number, scrollY: number, itemCount: number): number {
  let next = scrollY;
  if (selIdx < scrollY) {
    next = selIdx;
  } else if (selIdx >= scrollY + MAX_VISIBLE) {
    next = selIdx - MAX_VISIBLE + 1;
  }
  return Math.max(0, Math.min(next, Math.max(0, itemCount - MAX_VISIBLE)));
}

// ── Marquee scroll animation ──────────────────────────────────────────────────

/** How many px a title overflows the header, and the total scroll distance. */
interface MarqueeConfig {
  overflow: number;  // px the title extends past the available width (0 = fits, no scroll)
  scrollMax: number; // total px to scroll before the title is fully off-screen
}

const MARQUEE_PAUSE_MS       = 1000;
const MARQUEE_SCROLL_PX_PER_MS = 0.03; // ~30 px/s

/**
 * Measure marquee dimensions from the canvas context.
 * Returns a MarqueeConfig; pure except for ctx.measureText side-effect.
 *
 * @param wasmTitleWidth - optional callback that returns the wasm renderer's
 *   pre-computed title width (avoids measuring text when wasm is active).
 */
function measureMarqueeConfig(
  ctx: CanvasRenderingContext2D,
  scale: number,
  title: string,
  wasmTitleWidth?: () => number,
): MarqueeConfig {
  ctx.save();
  ctx.scale(scale, scale);
  ctx.font = 'bold 8px monospace';
  const titleW = wasmTitleWidth ? wasmTitleWidth() : ctx.measureText(title).width;
  ctx.restore();
  return {
    overflow:  Math.max(0, Math.ceil(titleW - (W - TEXT_PAD * 2))),
    scrollMax: Math.ceil(TEXT_PAD + titleW),
  };
}

/**
 * Advance marquee state by one frame.
 * Pure — takes current state as arguments, returns next state without mutation.
 */
function tickMarquee(
  now: DOMHighResTimeStamp,
  phase: 'pause' | 'scroll',
  phaseAt: DOMHighResTimeStamp,
  scrollMax: number,
): { offset: number; phase: 'pause' | 'scroll'; phaseAt: DOMHighResTimeStamp } {
  const elapsed = now - phaseAt;
  if (phase === 'pause') {
    if (elapsed >= MARQUEE_PAUSE_MS) {
      return { offset: 0, phase: 'scroll', phaseAt: now };
    }
    return { offset: 0, phase, phaseAt };
  }
  const offset = elapsed * MARQUEE_SCROLL_PX_PER_MS;
  if (offset >= scrollMax) {
    return { offset: 0, phase: 'pause', phaseAt: now };
  }
  return { offset, phase, phaseAt };
}

/**
 * Drives the requestAnimationFrame loop for a long menu title.
 *
 * Usage:
 *   const marquee = new Marquee(config);
 *   marquee.start(() => render());   // fires onRender each frame
 *   marquee.stop();                  // cancels the loop
 *   marquee.offset                   // current scroll offset in px
 */
class Marquee {
  readonly overflow: number;
  private readonly _scrollMax: number;

  private _offset: number = 0;
  private _phase: 'pause' | 'scroll' = 'pause';
  private _phaseAt: DOMHighResTimeStamp;
  private _rafId: number | null = null;

  constructor(config: MarqueeConfig) {
    this.overflow    = config.overflow;
    this._scrollMax  = config.scrollMax;
    this._phaseAt    = performance.now();
  }

  /** Current horizontal scroll offset in logical px. */
  get offset(): number { return this._offset; }

  /**
   * Start the animation loop.
   * @param onRender - called after state is advanced; should call MenuRenderer.render().
   */
  start(onRender: () => void): void {
    if (this._rafId !== null) return;
    const loop = (now: DOMHighResTimeStamp): void => {
      const next    = tickMarquee(now, this._phase, this._phaseAt, this._scrollMax);
      this._offset  = next.offset;
      this._phase   = next.phase;
      this._phaseAt = next.phaseAt;
      onRender();
      this._rafId = requestAnimationFrame(loop);
    };
    this._rafId = requestAnimationFrame(loop);
  }

  stop(): void {
    if (this._rafId !== null) {
      cancelAnimationFrame(this._rafId);
      this._rafId = null;
    }
  }
}

// ── Canvas paint helpers ───────────────────────────────────────────────────────
// Module-level pure functions that take explicit data rather than reading from `this`.
// Each can be tested independently by providing a mock CanvasRenderingContext2D.

function paintBackground(ctx: CanvasRenderingContext2D): void {
  ctx.fillStyle = C0;
  ctx.fillRect(0, 0, W, H);
}

function paintHeader(
  ctx: CanvasRenderingContext2D,
  title: string,
  marqueeOffset: number,
  overflow: number,
): void {
  ctx.fillStyle = C1;
  ctx.fillRect(0, 0, W, HEADER_H);
  ctx.fillStyle    = C3;
  ctx.font         = 'bold 8px monospace';
  ctx.textBaseline = 'middle';

  if (overflow > 0) {
    // Clip to header width and scroll the title left
    ctx.save();
    ctx.rect(TEXT_PAD, 0, W - TEXT_PAD * 2, HEADER_H);
    ctx.clip();
    ctx.textAlign = 'left';
    ctx.fillText(title, TEXT_PAD - marqueeOffset, HEADER_H / 2);
    ctx.restore();
  } else {
    ctx.textAlign = 'center';
    ctx.fillText(title, W / 2, HEADER_H / 2);
  }
}

function paintFooter(ctx: CanvasRenderingContext2D, footer: string): void {
  const footerY = H - FOOTER_H;
  ctx.fillStyle    = C1;
  ctx.fillRect(0, footerY, W, FOOTER_H);
  ctx.fillStyle    = C3;
  ctx.font         = 'bold 7px monospace';
  ctx.textAlign    = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(footer, W / 2, footerY + FOOTER_H / 2);
}

function paintItemList(
  ctx: CanvasRenderingContext2D,
  items: MenuItem[],
  selIdx: number,
  scrollY: number,
): void {
  ctx.font         = 'bold 8px monospace';
  ctx.textAlign    = 'left';
  ctx.textBaseline = 'middle';

  const visible = Math.min(MAX_VISIBLE, items.length);
  for (let i = 0; i < visible; i++) {
    const itemIdx = scrollY + i;
    if (itemIdx >= items.length) break;
    const item    = items[itemIdx];
    const rowTop  = LIST_TOP + i * ITEM_H;
    const rowMidY = rowTop + ITEM_H / 2;

    if (itemIdx === selIdx) {
      ctx.fillStyle = C1;
      ctx.fillRect(0, rowTop, W, ITEM_H);
      ctx.fillStyle = C3;
      ctx.fillText('▶ ' + item.label, TEXT_PAD, rowMidY);
    } else {
      ctx.fillStyle = C2;
      ctx.fillText('  ' + item.label, TEXT_PAD, rowMidY);
    }
  }
}

function paintScrollIndicators(
  ctx: CanvasRenderingContext2D,
  scrollY: number,
  itemCount: number,
): void {
  ctx.fillStyle = C3;
  ctx.font      = '7px monospace';
  ctx.textAlign = 'right';

  if (scrollY > 0) {
    ctx.fillText('▲', W - 2, LIST_TOP + 4);
  }
  if (scrollY + MAX_VISIBLE < itemCount) {
    ctx.fillText('▼', W - 2, LIST_BOTTOM - 4);
  }
}

// ── Gesture detection ─────────────────────────────────────────────────────────

type Gesture =
  | { kind: 'swipe'; delta: number }
  | { kind: 'tap'; x: number; y: number }
  | { kind: 'none' };

/**
 * Classify a touch-end event as a swipe, tap, or out-of-bounds gesture.
 * Pure — returns a discriminated union, no side-effects.
 */
function detectGesture(touch: Touch, startY: number, canvas: HTMLCanvasElement): Gesture {
  const dy   = touch.clientY - startY;
  const rect = canvas.getBoundingClientRect();
  const outside =
    touch.clientX < rect.left || touch.clientX > rect.right ||
    touch.clientY < rect.top  || touch.clientY > rect.bottom;

  if (outside) return { kind: 'none' };
  if (Math.abs(dy) > 12) return { kind: 'swipe', delta: dy < 0 ? 1 : -1 };

  const { x, y } = canvasCoords(canvas, touch.clientX, touch.clientY);
  return { kind: 'tap', x, y };
}

// ── MenuRenderer ──────────────────────────────────────────────────────────────

class MenuRenderer implements MenuRendererInstance {
  private readonly _canvas: HTMLCanvasElement;
  private readonly _ctx: CanvasRenderingContext2D;

  private _active  = false;
  private _opts: MenuOptions | null = null;
  private _selIdx  = 0;
  private _scrollY = 0;
  private _scale   = 1;

  // Marquee — drives the JS-canvas title scroll animation
  private _marquee: Marquee | null = null;

  // Wasm renderer — optional accelerated renderer; replaces the JS canvas path
  private _wasm: WasmRendererLike | null = null;
  private _animationRafId: number | null = null;
  private _offscreenCanvas: HTMLCanvasElement | null = null;
  private _offscreenCtx: CanvasRenderingContext2D | null = null;
  private _imageData: ImageData | null = null;

  // Touch tracking
  private _touchStartY: number | null = null;
  private _boundTouchStart: ((e: TouchEvent) => void) | null = null;
  private _boundTouchEnd:   ((e: TouchEvent) => void) | null = null;

  constructor(canvas: HTMLCanvasElement) {
    this._canvas = canvas;
    this._ctx    = canvas.getContext('2d')!;
  }

  // ── Public API ──────────────────────────────────────────────────────────────

  /** Title of the currently showing menu (undefined when hidden). */
  get title(): string | undefined { return this._opts?.title; }

  show(options: MenuOptions): void {
    this._opts    = options;
    this._selIdx  = 0;
    this._scrollY = 0;
    this._active  = true;
    this._scale   = this._canvas.width / W;

    this._initWasmRenderer(options);

    const marqueeConfig = measureMarqueeConfig(
      this._ctx,
      this._scale,
      options.title ?? '',
      this._wasm ? () => this._wasm!.title_width_px() : undefined,
    );
    this._marquee = new Marquee(marqueeConfig);

    this.render();
    this._attachTouchListeners();

    // Wasm renderer drives its own animation (for scrolling title + selection names).
    // JS fallback uses the Marquee loop only when the title overflows the header.
    if (this._wasm) {
      this._startAnimation();
    } else if (marqueeConfig.overflow > 0) {
      this._marquee.start(() => this.render());
    }
  }

  hide(): void {
    this._active  = false;
    this._opts    = null;
    this._detachTouchListeners();
    this._marquee?.stop();
    this._marquee = null;
    this._stopAnimation();
    if (this._wasm && typeof this._wasm.free === 'function') this._wasm.free();
    this._wasm = null;
    this._scale = 1;
  }

  isActive(): boolean { return this._active; }

  handleInput(key: string): void {
    if (!this._active || !this._opts) return;
    const items = this._opts.items ?? [];

    switch (key) {
      case 'ArrowUp':
      case 'w':
        if (this._wasm) {
          this._syncStateToWasm();
          this._wasm.move_selection(-1);
          this._syncStateFromWasm();
        } else {
          this._selIdx  = (this._selIdx - 1 + items.length) % items.length;
          this._scrollY = clampScroll(this._selIdx, this._scrollY, items.length);
        }
        this.render();
        break;

      case 'ArrowDown':
      case 's':
        if (this._wasm) {
          this._syncStateToWasm();
          this._wasm.move_selection(1);
          this._syncStateFromWasm();
        } else {
          this._selIdx  = (this._selIdx + 1) % items.length;
          this._scrollY = clampScroll(this._selIdx, this._scrollY, items.length);
        }
        this.render();
        break;

      case 'Enter':
      case 'a':
        // Sync round-trip: push JS index to wasm, pull confirmed index back
        this._syncStateToWasm();
        this._syncStateFromWasm();
        if (items.length > 0 && this._opts.onSelect) {
          const item = items[this._selIdx];
          const cb   = this._opts.onSelect;
          this.hide();
          cb(item);
        }
        break;

      case 'Escape':
      case 'b':
        this._syncStateToWasm();
        this._syncStateFromWasm();
        if (this._opts.onBack) {
          const selIdx = this._selIdx;
          const cb     = this._opts.onBack;
          this.hide();
          cb(selIdx);
        } else {
          this.hide();
        }
        break;

      case 'Select':
        this._syncStateToWasm();
        this._syncStateFromWasm();
        if (this._opts.onSelectBtn) {
          const selIdx = this._selIdx;
          const cb     = this._opts.onSelectBtn;
          this.hide();
          cb(selIdx);
        }
        break;
    }
  }

  /** Handle a tap at logical canvas coordinates (x, y). */
  handleTap(x: number, y: number): void {
    if (!this._active || !this._opts) return;
    const items = this._opts.items ?? [];

    if (this._wasm) {
      this._syncStateToWasm();
      const itemIdx = this._wasm.item_at(x, y);
      if (itemIdx < 0 || itemIdx >= items.length) return;
      this._wasm.set_selected(itemIdx);
      this._syncStateFromWasm();
      if (this._opts.onSelect) {
        const item = items[itemIdx];
        const cb   = this._opts.onSelect;
        this.hide();
        cb(item);
      }
      return;
    }

    if (y < LIST_TOP || y > LIST_BOTTOM) return;
    const itemIdx = this._scrollY + Math.floor((y - LIST_TOP) / ITEM_H);
    if (itemIdx < 0 || itemIdx >= items.length) return;
    if (this._opts.onSelect) {
      const item = items[itemIdx];
      const cb   = this._opts.onSelect;
      this.hide();
      cb(item);
    }
  }

  render(): void {
    if (!this._active || !this._opts) return;
    if (this._wasm) { this._renderWasm(); return; }

    const ctx    = this._ctx;
    const items  = this._opts.items ?? [];
    const title  = this._opts.title ?? '';
    const footer = this._opts.footer ?? DEFAULT_FOOTER;

    ctx.imageSmoothingEnabled = false;
    ctx.save();
    ctx.scale(this._scale, this._scale);

    paintBackground(ctx);
    paintHeader(ctx, title, this._marquee?.offset ?? 0, this._marquee?.overflow ?? 0);
    paintFooter(ctx, footer);
    paintItemList(ctx, items, this._selIdx, this._scrollY);
    paintScrollIndicators(ctx, this._scrollY, items.length);

    ctx.restore();
  }

  // ── Private — wasm integration ──────────────────────────────────────────────

  private _initWasmRenderer(options: MenuOptions): void {
    const Ctor = (window as Window & { RustyBoyWasmMenuRenderer?: new () => WasmRendererLike })
      .RustyBoyWasmMenuRenderer;
    if (typeof Ctor !== 'function') return;

    try {
      this._wasm = new Ctor();
      const labels = (options.items ?? []).map(item => String(item.label));
      this._wasm.show(String(options.title ?? ''), labels, options.footer ?? DEFAULT_FOOTER);
      this._syncStateFromWasm();

      this._offscreenCanvas        = document.createElement('canvas');
      this._offscreenCanvas.width  = W;
      this._offscreenCanvas.height = H;
      this._offscreenCtx  = this._offscreenCanvas.getContext('2d')!;
      this._imageData     = this._offscreenCtx.createImageData(W, H);
    } catch (err) {
      console.warn('[rustyboy:menu] WASM menu renderer unavailable, falling back to JS canvas', err);
      this._wasm = null;
    }
  }

  /** Push JS selection index to wasm so both sides agree before a wasm operation. */
  private _syncStateToWasm(): void {
    if (!this._wasm) return;
    const items = this._opts?.items ?? [];
    if (items.length === 0) return;
    const idx = Number.isFinite(this._selIdx) ? this._selIdx : 0;
    this._wasm.set_selected(Math.max(0, Math.min(idx, items.length - 1)));
  }

  /** Pull selection + scroll state back from wasm after it performs an operation. */
  private _syncStateFromWasm(): void {
    if (!this._wasm) return;
    this._selIdx  = this._wasm.selected_index();
    this._scrollY = this._wasm.scroll_y();
  }

  private _renderWasm(): void {
    if (!this._wasm || !this._offscreenCtx || !this._imageData || !this._offscreenCanvas) return;
    const rgba = this._wasm.render_rgba(performance.now());
    this._imageData.data.set(rgba);
    this._offscreenCtx.putImageData(this._imageData, 0, 0);
    const ctx = this._ctx;
    ctx.imageSmoothingEnabled = false;
    ctx.clearRect(0, 0, this._canvas.width, this._canvas.height);
    ctx.drawImage(this._offscreenCanvas, 0, 0, this._canvas.width, this._canvas.height);
  }

  private _startAnimation(): void {
    if (this._animationRafId !== null) return;
    const loop = (): void => {
      if (!this._active || !this._wasm) { this._animationRafId = null; return; }
      this.render();
      this._animationRafId = requestAnimationFrame(loop);
    };
    this._animationRafId = requestAnimationFrame(loop);
  }

  private _stopAnimation(): void {
    if (this._animationRafId !== null) {
      cancelAnimationFrame(this._animationRafId);
      this._animationRafId = null;
    }
  }

  // ── Private — touch handling ────────────────────────────────────────────────

  private _onTouchStart(e: TouchEvent): void {
    if (!this._active) return;
    this._touchStartY = e.changedTouches[0].clientY;
  }

  private _onTouchEnd(e: TouchEvent): void {
    if (!this._active) return;
    const touch  = e.changedTouches[0];
    const startY = this._touchStartY ?? touch.clientY;
    this._touchStartY = null;

    const msg = `MenuRenderer._onTouchEnd dy=${(touch.clientY - startY).toFixed(1)} title=${this._opts?.title}`;
    console.debug('[rustyboy:menu]', msg);
    fetch('/dev/log', { method: 'POST', body: msg }).catch(() => {});

    const gesture = detectGesture(touch, startY, this._canvas);

    switch (gesture.kind) {
      case 'none':
        return;

      case 'swipe': {
        const items = this._opts?.items ?? [];
        if (this._wasm) {
          this._syncStateToWasm();
          this._wasm.scroll_by(gesture.delta);
          this._syncStateFromWasm();
        } else {
          this._scrollY = Math.max(0, Math.min(
            this._scrollY + gesture.delta,
            Math.max(0, items.length - MAX_VISIBLE),
          ));
        }
        this.render();
        break;
      }

      case 'tap':
        this.handleTap(gesture.x, gesture.y);
        break;
    }
  }

  private _attachTouchListeners(): void {
    this._boundTouchStart = this._onTouchStart.bind(this);
    this._boundTouchEnd   = this._onTouchEnd.bind(this);
    this._canvas.addEventListener('touchstart', this._boundTouchStart, { passive: true });
    this._canvas.addEventListener('touchend',   this._boundTouchEnd,   { passive: true });
  }

  private _detachTouchListeners(): void {
    if (this._boundTouchStart) {
      this._canvas.removeEventListener('touchstart', this._boundTouchStart);
      this._canvas.removeEventListener('touchend',   this._boundTouchEnd!);
      this._boundTouchStart = null;
      this._boundTouchEnd   = null;
    }
  }
}

// ── Global export ─────────────────────────────────────────────────────────────
// The esbuild IIFE wrapper means this side-effect executes on script load,
// mirroring the original (function(){ … window.MenuRenderer = … })() pattern.
//
// The Window.MenuRenderer declaration here must agree structurally with the one
// in app.ts (both files compile in the same tsc pass and their declarations merge).

declare global {
  interface Window {
    MenuRenderer: { new(canvas: HTMLCanvasElement): MenuRendererInstance };
  }
}
(window as Window).MenuRenderer = MenuRenderer;
