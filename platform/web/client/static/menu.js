"use strict";
(() => {
  // src-ts/menu.ts
  var C0 = "#0F380F";
  var C1 = "#306230";
  var C2 = "#8BAC0F";
  var C3 = "#9BBC0F";
  var W = 160;
  var H = 144;
  var HEADER_H = 14;
  var FOOTER_H = 14;
  var ITEM_H = 14;
  var LIST_TOP = HEADER_H + 2;
  var LIST_BOTTOM = H - FOOTER_H - 2;
  var MAX_VISIBLE = Math.floor((LIST_BOTTOM - LIST_TOP) / ITEM_H);
  var TEXT_PAD = 6;
  var DEFAULT_FOOTER = "\u25B2\u25BC MOVE  A SELECT  B BACK";
  function canvasCoords(canvas, clientX, clientY) {
    const rect = canvas.getBoundingClientRect();
    return {
      x: (clientX - rect.left) * (W / rect.width),
      y: (clientY - rect.top) * (H / rect.height)
    };
  }
  function clampScroll(selIdx, scrollY, itemCount) {
    let next = scrollY;
    if (selIdx < scrollY) {
      next = selIdx;
    } else if (selIdx >= scrollY + MAX_VISIBLE) {
      next = selIdx - MAX_VISIBLE + 1;
    }
    return Math.max(0, Math.min(next, Math.max(0, itemCount - MAX_VISIBLE)));
  }
  var MARQUEE_PAUSE_MS = 1e3;
  var MARQUEE_SCROLL_PX_PER_MS = 0.03;
  function measureMarqueeConfig(ctx, scale, title, wasmTitleWidth) {
    ctx.save();
    ctx.scale(scale, scale);
    ctx.font = "bold 8px monospace";
    const titleW = wasmTitleWidth ? wasmTitleWidth() : ctx.measureText(title).width;
    ctx.restore();
    return {
      overflow: Math.max(0, Math.ceil(titleW - (W - TEXT_PAD * 2))),
      scrollMax: Math.ceil(TEXT_PAD + titleW)
    };
  }
  function tickMarquee(now, phase, phaseAt, scrollMax) {
    const elapsed = now - phaseAt;
    if (phase === "pause") {
      if (elapsed >= MARQUEE_PAUSE_MS) {
        return { offset: 0, phase: "scroll", phaseAt: now };
      }
      return { offset: 0, phase, phaseAt };
    }
    const offset = elapsed * MARQUEE_SCROLL_PX_PER_MS;
    if (offset >= scrollMax) {
      return { offset: 0, phase: "pause", phaseAt: now };
    }
    return { offset, phase, phaseAt };
  }
  var Marquee = class {
    constructor(config) {
      this._offset = 0;
      this._phase = "pause";
      this._rafId = null;
      this.overflow = config.overflow;
      this._scrollMax = config.scrollMax;
      this._phaseAt = performance.now();
    }
    /** Current horizontal scroll offset in logical px. */
    get offset() {
      return this._offset;
    }
    /**
     * Start the animation loop.
     * @param onRender - called after state is advanced; should call MenuRenderer.render().
     */
    start(onRender) {
      if (this._rafId !== null) return;
      const loop = (now) => {
        const next = tickMarquee(now, this._phase, this._phaseAt, this._scrollMax);
        this._offset = next.offset;
        this._phase = next.phase;
        this._phaseAt = next.phaseAt;
        onRender();
        this._rafId = requestAnimationFrame(loop);
      };
      this._rafId = requestAnimationFrame(loop);
    }
    stop() {
      if (this._rafId !== null) {
        cancelAnimationFrame(this._rafId);
        this._rafId = null;
      }
    }
  };
  function paintBackground(ctx) {
    ctx.fillStyle = C0;
    ctx.fillRect(0, 0, W, H);
  }
  function paintHeader(ctx, title, marqueeOffset, overflow) {
    ctx.fillStyle = C1;
    ctx.fillRect(0, 0, W, HEADER_H);
    ctx.fillStyle = C3;
    ctx.font = "bold 8px monospace";
    ctx.textBaseline = "middle";
    if (overflow > 0) {
      ctx.save();
      ctx.rect(TEXT_PAD, 0, W - TEXT_PAD * 2, HEADER_H);
      ctx.clip();
      ctx.textAlign = "left";
      ctx.fillText(title, TEXT_PAD - marqueeOffset, HEADER_H / 2);
      ctx.restore();
    } else {
      ctx.textAlign = "center";
      ctx.fillText(title, W / 2, HEADER_H / 2);
    }
  }
  function paintFooter(ctx, footer) {
    const footerY = H - FOOTER_H;
    ctx.fillStyle = C1;
    ctx.fillRect(0, footerY, W, FOOTER_H);
    ctx.fillStyle = C3;
    ctx.font = "bold 7px monospace";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(footer, W / 2, footerY + FOOTER_H / 2);
  }
  function paintItemList(ctx, items, selIdx, scrollY) {
    ctx.font = "bold 8px monospace";
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    const visible = Math.min(MAX_VISIBLE, items.length);
    for (let i = 0; i < visible; i++) {
      const itemIdx = scrollY + i;
      if (itemIdx >= items.length) break;
      const item = items[itemIdx];
      const rowTop = LIST_TOP + i * ITEM_H;
      const rowMidY = rowTop + ITEM_H / 2;
      if (itemIdx === selIdx) {
        ctx.fillStyle = C1;
        ctx.fillRect(0, rowTop, W, ITEM_H);
        ctx.fillStyle = C3;
        ctx.fillText("\u25B6 " + item.label, TEXT_PAD, rowMidY);
      } else {
        ctx.fillStyle = C2;
        ctx.fillText("  " + item.label, TEXT_PAD, rowMidY);
      }
    }
  }
  function paintScrollIndicators(ctx, scrollY, itemCount) {
    ctx.fillStyle = C3;
    ctx.font = "7px monospace";
    ctx.textAlign = "right";
    if (scrollY > 0) {
      ctx.fillText("\u25B2", W - 2, LIST_TOP + 4);
    }
    if (scrollY + MAX_VISIBLE < itemCount) {
      ctx.fillText("\u25BC", W - 2, LIST_BOTTOM - 4);
    }
  }
  function detectGesture(touch, startY, canvas) {
    const dy = touch.clientY - startY;
    const rect = canvas.getBoundingClientRect();
    const outside = touch.clientX < rect.left || touch.clientX > rect.right || touch.clientY < rect.top || touch.clientY > rect.bottom;
    if (outside) return { kind: "none" };
    if (Math.abs(dy) > 12) return { kind: "swipe", delta: dy < 0 ? 1 : -1 };
    const { x, y } = canvasCoords(canvas, touch.clientX, touch.clientY);
    return { kind: "tap", x, y };
  }
  var MenuRenderer = class {
    constructor(canvas) {
      this._active = false;
      this._opts = null;
      this._selIdx = 0;
      this._scrollY = 0;
      this._scale = 1;
      // Marquee — drives the JS-canvas title scroll animation
      this._marquee = null;
      // Wasm renderer — optional accelerated renderer; replaces the JS canvas path
      this._wasm = null;
      this._animationRafId = null;
      this._offscreenCanvas = null;
      this._offscreenCtx = null;
      this._imageData = null;
      // Touch tracking
      this._touchStartY = null;
      this._boundTouchStart = null;
      this._boundTouchEnd = null;
      this._canvas = canvas;
      this._ctx = canvas.getContext("2d");
    }
    // ── Public API ──────────────────────────────────────────────────────────────
    /** Title of the currently showing menu (undefined when hidden). */
    get title() {
      return this._opts?.title;
    }
    show(options) {
      this._opts = options;
      this._selIdx = 0;
      this._scrollY = 0;
      this._active = true;
      this._scale = this._canvas.width / W;
      this._initWasmRenderer(options);
      const marqueeConfig = measureMarqueeConfig(
        this._ctx,
        this._scale,
        options.title ?? "",
        this._wasm ? () => this._wasm.title_width_px() : void 0
      );
      this._marquee = new Marquee(marqueeConfig);
      this.render();
      this._attachTouchListeners();
      if (this._wasm) {
        this._startAnimation();
      } else if (marqueeConfig.overflow > 0) {
        this._marquee.start(() => this.render());
      }
    }
    hide() {
      this._active = false;
      this._opts = null;
      this._detachTouchListeners();
      this._marquee?.stop();
      this._marquee = null;
      this._stopAnimation();
      if (this._wasm && typeof this._wasm.free === "function") this._wasm.free();
      this._wasm = null;
      this._scale = 1;
    }
    isActive() {
      return this._active;
    }
    handleInput(key) {
      if (!this._active || !this._opts) return;
      const items = this._opts.items ?? [];
      switch (key) {
        case "ArrowUp":
        case "w":
          if (this._wasm) {
            this._syncStateToWasm();
            this._wasm.move_selection(-1);
            this._syncStateFromWasm();
          } else {
            this._selIdx = (this._selIdx - 1 + items.length) % items.length;
            this._scrollY = clampScroll(this._selIdx, this._scrollY, items.length);
          }
          this.render();
          break;
        case "ArrowDown":
        case "s":
          if (this._wasm) {
            this._syncStateToWasm();
            this._wasm.move_selection(1);
            this._syncStateFromWasm();
          } else {
            this._selIdx = (this._selIdx + 1) % items.length;
            this._scrollY = clampScroll(this._selIdx, this._scrollY, items.length);
          }
          this.render();
          break;
        case "Enter":
        case "a":
          this._syncStateToWasm();
          this._syncStateFromWasm();
          if (items.length > 0 && this._opts.onSelect) {
            const item = items[this._selIdx];
            const cb = this._opts.onSelect;
            this.hide();
            cb(item);
          }
          break;
        case "Escape":
        case "b":
          this._syncStateToWasm();
          this._syncStateFromWasm();
          if (this._opts.onBack) {
            const selIdx = this._selIdx;
            const cb = this._opts.onBack;
            this.hide();
            cb(selIdx);
          } else {
            this.hide();
          }
          break;
        case "Select":
          this._syncStateToWasm();
          this._syncStateFromWasm();
          if (this._opts.onSelectBtn) {
            const selIdx = this._selIdx;
            const cb = this._opts.onSelectBtn;
            this.hide();
            cb(selIdx);
          }
          break;
      }
    }
    /** Handle a tap at logical canvas coordinates (x, y). */
    handleTap(x, y) {
      if (!this._active || !this._opts) return;
      const items = this._opts.items ?? [];
      if (this._wasm) {
        this._syncStateToWasm();
        const itemIdx2 = this._wasm.item_at(x, y);
        if (itemIdx2 < 0 || itemIdx2 >= items.length) return;
        this._wasm.set_selected(itemIdx2);
        this._syncStateFromWasm();
        if (this._opts.onSelect) {
          const item = items[itemIdx2];
          const cb = this._opts.onSelect;
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
        const cb = this._opts.onSelect;
        this.hide();
        cb(item);
      }
    }
    render() {
      if (!this._active || !this._opts) return;
      if (this._wasm) {
        this._renderWasm();
        return;
      }
      const ctx = this._ctx;
      const items = this._opts.items ?? [];
      const title = this._opts.title ?? "";
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
    _initWasmRenderer(options) {
      const Ctor = window.RustyBoyWasmMenuRenderer;
      if (typeof Ctor !== "function") return;
      try {
        this._wasm = new Ctor();
        const labels = (options.items ?? []).map((item) => String(item.label));
        this._wasm.show(String(options.title ?? ""), labels, options.footer ?? DEFAULT_FOOTER);
        this._syncStateFromWasm();
        this._offscreenCanvas = document.createElement("canvas");
        this._offscreenCanvas.width = W;
        this._offscreenCanvas.height = H;
        this._offscreenCtx = this._offscreenCanvas.getContext("2d");
        this._imageData = this._offscreenCtx.createImageData(W, H);
      } catch (err) {
        console.warn("[rustyboy:menu] WASM menu renderer unavailable, falling back to JS canvas", err);
        this._wasm = null;
      }
    }
    /** Push JS selection index to wasm so both sides agree before a wasm operation. */
    _syncStateToWasm() {
      if (!this._wasm) return;
      const items = this._opts?.items ?? [];
      if (items.length === 0) return;
      const idx = Number.isFinite(this._selIdx) ? this._selIdx : 0;
      this._wasm.set_selected(Math.max(0, Math.min(idx, items.length - 1)));
    }
    /** Pull selection + scroll state back from wasm after it performs an operation. */
    _syncStateFromWasm() {
      if (!this._wasm) return;
      this._selIdx = this._wasm.selected_index();
      this._scrollY = this._wasm.scroll_y();
    }
    _renderWasm() {
      if (!this._wasm || !this._offscreenCtx || !this._imageData || !this._offscreenCanvas) return;
      const rgba = this._wasm.render_rgba(performance.now());
      this._imageData.data.set(rgba);
      this._offscreenCtx.putImageData(this._imageData, 0, 0);
      const ctx = this._ctx;
      ctx.imageSmoothingEnabled = false;
      ctx.clearRect(0, 0, this._canvas.width, this._canvas.height);
      ctx.drawImage(this._offscreenCanvas, 0, 0, this._canvas.width, this._canvas.height);
    }
    _startAnimation() {
      if (this._animationRafId !== null) return;
      const loop = () => {
        if (!this._active || !this._wasm) {
          this._animationRafId = null;
          return;
        }
        this.render();
        this._animationRafId = requestAnimationFrame(loop);
      };
      this._animationRafId = requestAnimationFrame(loop);
    }
    _stopAnimation() {
      if (this._animationRafId !== null) {
        cancelAnimationFrame(this._animationRafId);
        this._animationRafId = null;
      }
    }
    // ── Private — touch handling ────────────────────────────────────────────────
    _onTouchStart(e) {
      if (!this._active) return;
      this._touchStartY = e.changedTouches[0].clientY;
    }
    _onTouchEnd(e) {
      if (!this._active) return;
      const touch = e.changedTouches[0];
      const startY = this._touchStartY ?? touch.clientY;
      this._touchStartY = null;
      const msg = `MenuRenderer._onTouchEnd dy=${(touch.clientY - startY).toFixed(1)} title=${this._opts?.title}`;
      console.debug("[rustyboy:menu]", msg);
      fetch("/dev/log", { method: "POST", body: msg }).catch(() => {
      });
      const gesture = detectGesture(touch, startY, this._canvas);
      switch (gesture.kind) {
        case "none":
          return;
        case "swipe": {
          const items = this._opts?.items ?? [];
          if (this._wasm) {
            this._syncStateToWasm();
            this._wasm.scroll_by(gesture.delta);
            this._syncStateFromWasm();
          } else {
            this._scrollY = Math.max(0, Math.min(
              this._scrollY + gesture.delta,
              Math.max(0, items.length - MAX_VISIBLE)
            ));
          }
          this.render();
          break;
        }
        case "tap":
          this.handleTap(gesture.x, gesture.y);
          break;
      }
    }
    _attachTouchListeners() {
      this._boundTouchStart = this._onTouchStart.bind(this);
      this._boundTouchEnd = this._onTouchEnd.bind(this);
      this._canvas.addEventListener("touchstart", this._boundTouchStart, { passive: true });
      this._canvas.addEventListener("touchend", this._boundTouchEnd, { passive: true });
    }
    _detachTouchListeners() {
      if (this._boundTouchStart) {
        this._canvas.removeEventListener("touchstart", this._boundTouchStart);
        this._canvas.removeEventListener("touchend", this._boundTouchEnd);
        this._boundTouchStart = null;
        this._boundTouchEnd = null;
      }
    }
  };
  window.MenuRenderer = MenuRenderer;
})();
