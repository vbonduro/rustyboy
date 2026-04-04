/**
 * MenuRenderer — draws GB-aesthetic menus directly onto the 160x144 game canvas.
 *
 * Usage:
 *   const menu = new MenuRenderer(canvas);
 *   menu.show({
 *     title: 'SELECT GAME',
 *     items: [{ label: 'Tetris', value: 'tetris.gb' }, ...],
 *     footer: '▲▼ MOVE  A SELECT  B BACK',   // optional, has default
 *     onSelect: (item) => { ... },
 *     onBack:   () => { ... },                // optional
 *   });
 */

(function () {
  'use strict';

  // GB phosphor palette (dark → light)
  const C0 = '#0F380F'; // darkest  (background)
  const C1 = '#306230'; // dark     (header/footer fill, selection bg)
  const C2 = '#8BAC0F'; // medium   (unselected text)
  const C3 = '#9BBC0F'; // lightest (selected text, header text)

  const W = 160;
  const H = 144;

  const HEADER_H    = 14;
  const FOOTER_H    = 14;
  const ITEM_H      = 14;
  const LIST_TOP    = HEADER_H + 2;
  const LIST_BOTTOM = H - FOOTER_H - 2;
  const MAX_VISIBLE = Math.floor((LIST_BOTTOM - LIST_TOP) / ITEM_H); // ≈ 8
  const TEXT_PAD    = 6;

  class MenuRenderer {
    constructor(canvas) {
      this._canvas  = canvas;
      this._ctx     = canvas.getContext('2d');
      this._active  = false;
      this._opts    = null;
      this._selIdx  = 0;
      this._scrollY = 0;       // index of first visible item
      this._touchStartY = null;
      this._scale   = 1;       // physical-to-logical scale factor
      // Marquee state
      this._marqueeOffset  = 0;      // px scrolled left so far
      this._marqueePhase   = 'pause'; // 'pause' | 'scroll'
      this._marqueePhaseAt = 0;      // timestamp when current phase started
      this._marqueeRafId    = null;
      this._marqueeOverflow = 0;     // how many px the title overflows the header
      this._marqueeScrollMax = 0;    // total px to scroll before reset (title fully off-screen)
    }

    // Scale canvas buffer to physical pixels for crisp rendering.
    // Saves original dimensions so hide() can restore them for the emulator.
    _scaleCanvas() {
      const dpr  = window.devicePixelRatio || 1;
      const rect = this._canvas.getBoundingClientRect();
      const physW = Math.round(rect.width  * dpr);
      const physH = Math.round(rect.height * dpr);
      this._savedCanvasW = this._canvas.width;
      this._savedCanvasH = this._canvas.height;
      this._canvas.width  = physW;
      this._canvas.height = physH;
      this._scale = physW / W;
    }

    _restoreCanvas() {
      if (this._savedCanvasW !== undefined) {
        this._canvas.width  = this._savedCanvasW;
        this._canvas.height = this._savedCanvasH;
        this._savedCanvasW  = undefined;
        this._savedCanvasH  = undefined;
      }
      this._scale = 1;
    }

    // ── Public API ──────────────────────────────────────────────────────────

    show(options) {
      this._opts           = options;
      this._selIdx         = 0;
      this._scrollY        = 0;
      this._marqueeOffset  = 0;
      this._marqueePhase   = 'pause';
      this._marqueePhaseAt = performance.now();
      this._active         = true;
      this._scaleCanvas();
      // Measure title overflow (needs font set first, using logical scale)
      this._ctx.save();
      this._ctx.scale(this._scale, this._scale);
      this._ctx.font = 'bold 8px monospace';
      const titleW = this._ctx.measureText(options.title || '').width;
      this._ctx.restore();
      const available = W - TEXT_PAD * 2;
      this._marqueeOverflow  = Math.max(0, Math.ceil(titleW - available));
      this._marqueeScrollMax = Math.ceil(TEXT_PAD + titleW); // scroll until fully off-screen
      this.render();
      this._attachTouchListeners();
      if (this._marqueeOverflow > 0) this._startMarquee();
    }

    hide() {
      this._active = false;
      this._opts   = null;
      this._detachTouchListeners();
      this._stopMarquee();
      this._restoreCanvas();
    }

    isActive() {
      return this._active;
    }

    handleInput(key) {
      if (!this._active || !this._opts) return;
      const items = this._opts.items || [];

      switch (key) {
        case 'ArrowUp':
        case 'w':
          this._selIdx = (this._selIdx - 1 + items.length) % items.length;
          this._clampScroll();
          this.render();
          break;

        case 'ArrowDown':
        case 's':
          this._selIdx = (this._selIdx + 1) % items.length;
          this._clampScroll();
          this.render();
          break;

        case 'Enter':
        case 'a':
          if (items.length > 0 && this._opts.onSelect) {
            const item = items[this._selIdx];
            const cb = this._opts.onSelect;
            this.hide();
            cb(item);
          }
          break;

        case 'Escape':
        case 'b':
          if (this._opts.onBack) {
            const selIdx = this._selIdx;
            const cb = this._opts.onBack;
            this.hide();
            cb(selIdx);
          } else {
            this.hide();
          }
          break;

        case 'Select':
          if (this._opts.onSelectBtn) {
            const selIdx = this._selIdx;
            const cb = this._opts.onSelectBtn;
            this.hide();
            cb(selIdx);
          }
          break;
      }
    }

    handleTap(x, y) {
      if (!this._active || !this._opts) return;
      const items = this._opts.items || [];

      // Check if tap is in the list area
      if (y < LIST_TOP || y > LIST_BOTTOM) return;

      const row = Math.floor((y - LIST_TOP) / ITEM_H);
      const itemIdx = this._scrollY + row;
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
      const ctx  = this._ctx;
      const items = this._opts.items || [];

      ctx.imageSmoothingEnabled = false;
      ctx.save();
      ctx.scale(this._scale, this._scale);

      // ── Background ────────────────────────────────────────────────────────
      ctx.fillStyle = C0;
      ctx.fillRect(0, 0, W, H);

      // ── Header ────────────────────────────────────────────────────────────
      ctx.fillStyle = C1;
      ctx.fillRect(0, 0, W, HEADER_H);
      ctx.fillStyle = C3;
      ctx.font      = 'bold 8px monospace';
      ctx.textBaseline = 'middle';
      const title = this._opts.title || '';
      if (this._marqueeOverflow > 0) {
        // Scrolling title: clip to header, draw left-aligned minus scroll offset
        ctx.save();
        ctx.rect(TEXT_PAD, 0, W - TEXT_PAD * 2, HEADER_H);
        ctx.clip();
        ctx.textAlign = 'left';
        ctx.fillText(title, TEXT_PAD - this._marqueeOffset, HEADER_H / 2);
        ctx.restore();
      } else {
        ctx.textAlign = 'center';
        ctx.fillText(title, W / 2, HEADER_H / 2);
      }

      // ── Footer ────────────────────────────────────────────────────────────
      const footerY = H - FOOTER_H;
      ctx.fillStyle = C1;
      ctx.fillRect(0, footerY, W, FOOTER_H);
      ctx.fillStyle = C3;
      ctx.font      = 'bold 7px monospace';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      const footer = this._opts.footer || '\u25b2\u25bc MOVE  A SELECT  B BACK';
      ctx.fillText(footer, W / 2, footerY + FOOTER_H / 2);

      // ── Item list ─────────────────────────────────────────────────────────
      ctx.font      = 'bold 8px monospace';
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';

      const visible = Math.min(MAX_VISIBLE, items.length);
      for (let i = 0; i < visible; i++) {
        const itemIdx = this._scrollY + i;
        if (itemIdx >= items.length) break;
        const item    = items[itemIdx];
        const rowTop  = LIST_TOP + i * ITEM_H;
        const rowMidY = rowTop + ITEM_H / 2;
        const selected = itemIdx === this._selIdx;

        if (selected) {
          ctx.fillStyle = C1;
          ctx.fillRect(0, rowTop, W, ITEM_H);
          ctx.fillStyle = C3;
          ctx.fillText('\u25b6 ' + item.label, TEXT_PAD, rowMidY);
        } else {
          ctx.fillStyle = C2;
          ctx.fillText('  ' + item.label, TEXT_PAD, rowMidY);
        }
      }

      // ── Scroll indicators ─────────────────────────────────────────────────
      if (this._scrollY > 0) {
        ctx.fillStyle = C3;
        ctx.font = '7px monospace';
        ctx.textAlign = 'right';
        ctx.fillText('\u25b2', W - 2, LIST_TOP + 4);
      }
      if (this._scrollY + MAX_VISIBLE < items.length) {
        ctx.fillStyle = C3;
        ctx.font = '7px monospace';
        ctx.textAlign = 'right';
        ctx.fillText('\u25bc', W - 2, LIST_BOTTOM - 4);
      }

      ctx.restore();
    }

    // ── Private ─────────────────────────────────────────────────────────────

    _startMarquee() {
      if (this._marqueeRafId !== null) return;
      const tick = (now) => {
        if (!this._active) { this._marqueeRafId = null; return; }
        this._tickMarquee(now);
        this.render();
        this._marqueeRafId = requestAnimationFrame(tick);
      };
      this._marqueeRafId = requestAnimationFrame(tick);
    }

    _stopMarquee() {
      if (this._marqueeRafId !== null) {
        cancelAnimationFrame(this._marqueeRafId);
        this._marqueeRafId = null;
      }
    }

    _tickMarquee(now) {
      const PAUSE_MS  = 1000;
      const SCROLL_PX_PER_MS = 0.03; // ~30px/s

      const elapsed = now - this._marqueePhaseAt;

      if (this._marqueePhase === 'pause') {
        if (elapsed >= PAUSE_MS) {
          this._marqueePhase   = 'scroll';
          this._marqueePhaseAt = now;
        }
      } else {
        this._marqueeOffset = elapsed * SCROLL_PX_PER_MS;
        if (this._marqueeOffset >= this._marqueeScrollMax) {
          // Title fully off-screen — reset and pause again
          this._marqueeOffset  = 0;
          this._marqueePhase   = 'pause';
          this._marqueePhaseAt = now;
        }
      }
    }

    _clampScroll() {
      const items = (this._opts && this._opts.items) || [];
      // Keep selected item visible
      if (this._selIdx < this._scrollY) {
        this._scrollY = this._selIdx;
      } else if (this._selIdx >= this._scrollY + MAX_VISIBLE) {
        this._scrollY = this._selIdx - MAX_VISIBLE + 1;
      }
      // Clamp to valid range
      const maxScroll = Math.max(0, items.length - MAX_VISIBLE);
      this._scrollY = Math.max(0, Math.min(this._scrollY, maxScroll));
    }

    _canvasCoords(clientX, clientY) {
      const rect    = this._canvas.getBoundingClientRect();
      const scaleX  = W / rect.width;
      const scaleY  = H / rect.height;
      return {
        x: (clientX - rect.left) * scaleX,
        y: (clientY - rect.top)  * scaleY,
      };
    }

    _onTouchStart(e) {
      if (!this._active) return;
      const t = e.changedTouches[0];
      this._touchStartY = t.clientY;
    }

    _onTouchEnd(e) {
      if (!this._active) return;
      const t   = e.changedTouches[0];
      const dy  = t.clientY - (this._touchStartY || t.clientY);
      this._touchStartY = null;

      // Ignore touches that ended outside the canvas bounds — this prevents
      // d-pad button presses (which share touch coordinates with the canvas
      // area on mobile) from accidentally triggering menu taps.
      const rect = this._canvas.getBoundingClientRect();
      const outside = t.clientX < rect.left || t.clientX > rect.right ||
                      t.clientY < rect.top  || t.clientY > rect.bottom;
      const msg = `MenuRenderer._onTouchEnd dy=${dy.toFixed(1)} outside=${outside} title=${this._opts?.title}`;
      console.debug('[rustyboy:menu]', msg);
      fetch('/dev/log', { method: 'POST', body: msg }).catch(() => {});
      if (outside) return;

      const items = (this._opts && this._opts.items) || [];

      if (Math.abs(dy) > 12) {
        // Swipe: scroll
        const delta = dy < 0 ? 1 : -1;
        this._scrollY = Math.max(0, Math.min(
          this._scrollY + delta,
          Math.max(0, items.length - MAX_VISIBLE)
        ));
        this.render();
      } else {
        // Tap: select item
        const coords = this._canvasCoords(t.clientX, t.clientY);
        this.handleTap(coords.x, coords.y);
      }
    }

    _attachTouchListeners() {
      this._boundTouchStart = this._onTouchStart.bind(this);
      this._boundTouchEnd   = this._onTouchEnd.bind(this);
      this._canvas.addEventListener('touchstart', this._boundTouchStart, { passive: true });
      this._canvas.addEventListener('touchend',   this._boundTouchEnd,   { passive: true });
    }

    _detachTouchListeners() {
      if (this._boundTouchStart) {
        this._canvas.removeEventListener('touchstart', this._boundTouchStart);
        this._canvas.removeEventListener('touchend',   this._boundTouchEnd);
        this._boundTouchStart = null;
        this._boundTouchEnd   = null;
      }
    }
  }

  window.MenuRenderer = MenuRenderer;
})();
