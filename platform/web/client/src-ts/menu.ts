/**
 * MenuRenderer — a thin adapter that drives the WASM menu renderer
 * (rustyboy_web_client's `WasmMenuRenderer`) onto the 160×144 game canvas and
 * forwards keyboard / touch input to it.
 *
 * Exposes `window.MenuRenderer` as a side-effect (built as an IIFE by esbuild).
 *
 * All menu pixels — palette, layout, the title landscape, the marquee — are
 * produced in Rust/WASM via `render_rgba()`. This class only blits that frame
 * to the canvas, runs the animation loop, and maps input to wasm calls; WASM is
 * the single source of truth for selection/scroll state. Menus are always shown
 * after the WASM module has initialised, so the wasm renderer is required (there
 * is deliberately no JS-canvas fallback — without WASM the emulator itself can't
 * run, so a plainer fallback menu would be pointless).
 *
 * Usage:
 *   const menu = new MenuRenderer(canvas);
 *   menu.show({ title: 'SELECT GAME', items: [...], onSelect, onBack });
 */

import type { MenuOptions, MenuRendererInstance } from './types.js';

// Logical Game Boy screen size; the wasm renderer outputs a W×H RGBA frame.
const W = 160;
const H = 144;

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

// ── Pure input helpers ────────────────────────────────────────────────────────

interface Point {
  x: number;
  y: number;
}

/**
 * Map a pointer/touch event's client coordinates into logical 160×144 canvas
 * space. Pure — depends only on the canvas's bounding rect and the input coords.
 */
function canvasCoords(canvas: HTMLCanvasElement, clientX: number, clientY: number): Point {
  const rect = canvas.getBoundingClientRect();
  return {
    x: (clientX - rect.left) * (W / rect.width),
    y: (clientY - rect.top) * (H / rect.height),
  };
}

type Gesture =
  | { kind: 'swipe'; delta: number }
  | { kind: 'tap'; x: number; y: number }
  | { kind: 'none' };

/**
 * Classify a touch-end event as a swipe, tap, or out-of-bounds gesture.
 * Pure — returns a discriminated union, no side-effects.
 */
function detectGesture(touch: Touch, startY: number, canvas: HTMLCanvasElement): Gesture {
  const dy = touch.clientY - startY;
  const rect = canvas.getBoundingClientRect();
  const outside =
    touch.clientX < rect.left || touch.clientX > rect.right ||
    touch.clientY < rect.top || touch.clientY > rect.bottom;

  if (outside) return { kind: 'none' };
  if (Math.abs(dy) > 12) return { kind: 'swipe', delta: dy < 0 ? 1 : -1 };

  const { x, y } = canvasCoords(canvas, touch.clientX, touch.clientY);
  return { kind: 'tap', x, y };
}

// ── MenuRenderer ──────────────────────────────────────────────────────────────

class MenuRenderer implements MenuRendererInstance {
  private readonly _canvas: HTMLCanvasElement;
  private readonly _ctx: CanvasRenderingContext2D;

  private _active = false;
  private _opts: MenuOptions | null = null;

  // Wasm renderer — produces every menu frame; this class blits it + forwards input.
  private _wasm: WasmRendererLike | null = null;
  private _animationRafId: number | null = null;
  private _offscreenCanvas: HTMLCanvasElement | null = null;
  private _offscreenCtx: CanvasRenderingContext2D | null = null;
  private _imageData: ImageData | null = null;

  // Touch tracking
  private _touchStartY: number | null = null;
  private _boundTouchStart: ((e: TouchEvent) => void) | null = null;
  private _boundTouchEnd: ((e: TouchEvent) => void) | null = null;

  constructor(canvas: HTMLCanvasElement) {
    this._canvas = canvas;
    this._ctx = canvas.getContext('2d')!;
  }

  // ── Public API ──────────────────────────────────────────────────────────────

  /** Title of the currently showing menu (undefined when hidden). */
  get title(): string | undefined {
    return this._opts?.title;
  }

  show(options: MenuOptions): void {
    this._opts = options;
    this._active = true;

    this._initWasmRenderer(options);
    if (!this._wasm) {
      // Menus are only shown after the WASM module has initialised, so this
      // should never happen; if it does, there is nothing to render.
      console.error('[rustyboy:menu] WASM menu renderer unavailable; cannot render menu');
      return;
    }

    this.render();
    this._attachTouchListeners();
    // The wasm renderer drives its own animation (scrolling title + selection names).
    this._startAnimation();
  }

  hide(): void {
    this._active = false;
    this._opts = null;
    this._detachTouchListeners();
    this._stopAnimation();
    if (this._wasm && typeof this._wasm.free === 'function') this._wasm.free();
    this._wasm = null;
  }

  isActive(): boolean {
    return this._active;
  }

  handleInput(key: string): void {
    const wasm = this._wasm;
    if (!this._active || !this._opts || !wasm) return;
    const items = this._opts.items ?? [];

    switch (key) {
      case 'ArrowUp':
      case 'w':
        wasm.move_selection(-1);
        this.render();
        break;

      case 'ArrowDown':
      case 's':
        wasm.move_selection(1);
        this.render();
        break;

      case 'Enter':
      case 'a':
        if (items.length > 0 && this._opts.onSelect) {
          const item = items[wasm.selected_index()];
          const cb = this._opts.onSelect;
          this.hide();
          cb(item);
        }
        break;

      case 'Escape':
      case 'b':
        if (this._opts.onBack) {
          const selIdx = wasm.selected_index();
          const cb = this._opts.onBack;
          this.hide();
          cb(selIdx);
        } else {
          this.hide();
        }
        break;

      case 'Select':
        if (this._opts.onSelectBtn) {
          const selIdx = wasm.selected_index();
          const cb = this._opts.onSelectBtn;
          this.hide();
          cb(selIdx);
        }
        break;
    }
  }

  /** Handle a tap at logical canvas coordinates (x, y). */
  handleTap(x: number, y: number): void {
    const wasm = this._wasm;
    if (!this._active || !this._opts || !wasm) return;
    const items = this._opts.items ?? [];

    const itemIdx = wasm.item_at(x, y);
    if (itemIdx < 0 || itemIdx >= items.length) return;
    wasm.set_selected(itemIdx);
    if (this._opts.onSelect) {
      const item = items[itemIdx];
      const cb = this._opts.onSelect;
      this.hide();
      cb(item);
    }
  }

  render(): void {
    if (!this._active || !this._opts) return;
    this._renderWasm();
  }

  // ── Private — wasm integration ──────────────────────────────────────────────

  private _initWasmRenderer(options: MenuOptions): void {
    const Ctor = (window as Window & { RustyBoyWasmMenuRenderer?: new () => WasmRendererLike })
      .RustyBoyWasmMenuRenderer;
    if (typeof Ctor !== 'function') return;

    try {
      this._wasm = new Ctor();
      const labels = (options.items ?? []).map((item) => String(item.label));
      this._wasm.show(String(options.title ?? ''), labels, options.footer ?? DEFAULT_FOOTER);

      this._offscreenCanvas = document.createElement('canvas');
      this._offscreenCanvas.width = W;
      this._offscreenCanvas.height = H;
      this._offscreenCtx = this._offscreenCanvas.getContext('2d')!;
      this._imageData = this._offscreenCtx.createImageData(W, H);
    } catch (err) {
      console.error('[rustyboy:menu] failed to create WASM menu renderer', err);
      this._wasm = null;
    }
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
      if (!this._active || !this._wasm) {
        this._animationRafId = null;
        return;
      }
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
    const wasm = this._wasm;
    if (!this._active || !wasm) return;
    const touch = e.changedTouches[0];
    const startY = this._touchStartY ?? touch.clientY;
    this._touchStartY = null;

    const gesture = detectGesture(touch, startY, this._canvas);

    switch (gesture.kind) {
      case 'none':
        return;

      case 'swipe':
        wasm.scroll_by(gesture.delta);
        this.render();
        break;

      case 'tap':
        this.handleTap(gesture.x, gesture.y);
        break;
    }
  }

  private _attachTouchListeners(): void {
    this._boundTouchStart = this._onTouchStart.bind(this);
    this._boundTouchEnd = this._onTouchEnd.bind(this);
    this._canvas.addEventListener('touchstart', this._boundTouchStart, { passive: true });
    this._canvas.addEventListener('touchend', this._boundTouchEnd, { passive: true });
  }

  private _detachTouchListeners(): void {
    if (this._boundTouchStart) {
      this._canvas.removeEventListener('touchstart', this._boundTouchStart);
      this._canvas.removeEventListener('touchend', this._boundTouchEnd!);
      this._boundTouchStart = null;
      this._boundTouchEnd = null;
    }
  }
}

// ── Global export ─────────────────────────────────────────────────────────────
// The esbuild IIFE wrapper means this side-effect executes on script load,
// mirroring the original `(function(){ … window.MenuRenderer = … })()` pattern.
//
// The Window.MenuRenderer declaration here must agree structurally with the one
// in app.ts (both files compile in the same tsc pass and their declarations merge).

declare global {
  interface Window {
    MenuRenderer: { new (canvas: HTMLCanvasElement): MenuRendererInstance };
  }
}
(window as Window).MenuRenderer = MenuRenderer;
