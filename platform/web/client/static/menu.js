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
    }

    // ── Public API ──────────────────────────────────────────────────────────

    show(options) {
      this._opts    = options;
      this._selIdx  = 0;
      this._scrollY = 0;
      this._active  = true;
      this.render();
      this._attachTouchListeners();
    }

    hide() {
      this._active = false;
      this._opts   = null;
      this._detachTouchListeners();
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
            const cb = this._opts.onBack;
            this.hide();
            cb();
          } else {
            this.hide();
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

      // ── Background ────────────────────────────────────────────────────────
      ctx.fillStyle = C0;
      ctx.fillRect(0, 0, W, H);

      // ── Header ────────────────────────────────────────────────────────────
      ctx.fillStyle = C1;
      ctx.fillRect(0, 0, W, HEADER_H);
      ctx.fillStyle = C3;
      ctx.font      = 'bold 8px monospace';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(this._opts.title || '', W / 2, HEADER_H / 2);

      // ── Footer ────────────────────────────────────────────────────────────
      const footerY = H - FOOTER_H;
      ctx.fillStyle = C1;
      ctx.fillRect(0, footerY, W, FOOTER_H);
      ctx.fillStyle = C3;
      ctx.font      = '7px monospace';
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
