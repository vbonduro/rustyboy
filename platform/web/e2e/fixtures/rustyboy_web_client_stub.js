export default async function init() { return {}; }

export class WasmMenuRenderer {
  constructor() {
    this.title = '';
    this.labels = [];
    this.footer = '';
    this.selected = 0;
    this.scrollY = 0;
  }

  show(title, labels, footer) {
    this.title = String(title || '');
    this.labels = Array.from(labels || [], label => String(label || ''));
    this.footer = String(footer || '');
    this.selected = 0;
    this.scrollY = 0;
  }

  selected_index() { return this.selected; }
  scroll_y() { return this.scrollY; }
  title_width_px() { return this.title.length * 8; }
  free() {}

  set_selected(index) {
    if (this.labels.length === 0) {
      this.selected = 0;
      this.scrollY = 0;
      return;
    }
    this.selected = Math.max(0, Math.min(index, this.labels.length - 1));
    this.#clampScroll();
  }

  move_selection(delta) {
    if (this.labels.length === 0) return;
    this.selected = (this.selected + delta + this.labels.length) % this.labels.length;
    this.#clampScroll();
  }

  scroll_by(delta) {
    const maxScroll = Math.max(0, this.labels.length - 14);
    this.scrollY = Math.max(0, Math.min(this.scrollY + delta, maxScroll));
  }

  item_at(x, y) {
    if (x < 0 || x >= 160 || y < 16 || y >= 128) return -1;
    const idx = this.scrollY + Math.floor((y - 16) / 8);
    return idx >= 0 && idx < this.labels.length ? idx : -1;
  }

  render_rgba() {
    const out = new Uint8Array(160 * 144 * 4);
    const fill = (x0, y0, w, h, color) => {
      for (let y = y0; y < y0 + h; y++) {
        for (let x = x0; x < x0 + w; x++) {
          const i = (y * 160 + x) * 4;
          out.set(color, i);
        }
      }
    };
    const C0 = [0x0F, 0x38, 0x0F, 0xFF];
    const C1 = [0x30, 0x62, 0x30, 0xFF];
    fill(0, 0, 160, 144, C0);
    fill(0, 0, 160, 16, C1);
    fill(0, 128, 160, 16, C1);
    const selectedY = 16 + (this.selected - this.scrollY) * 8;
    if (selectedY >= 16 && selectedY < 128) fill(0, selectedY, 160, 8, C1);
    return out;
  }

  #clampScroll() {
    if (this.selected < this.scrollY) {
      this.scrollY = this.selected;
    } else if (this.selected >= this.scrollY + 14) {
      this.scrollY = this.selected - 13;
    }
    this.scrollY = Math.max(0, Math.min(this.scrollY, Math.max(0, this.labels.length - 14)));
  }
}

export class EmulatorHandle {
  constructor(rom) {}
  run_frame() {}
  framebuffer_rgba() { return new Uint8Array(160 * 144 * 4); }
  drain_audio_samples() { return new Float32Array(0); }
  set_button(btn, pressed) {}
  save_state() { return new Uint8Array(0); }
  load_state(data) {}
  get_battery_save() { return new Uint8Array(0); }
  set_battery_save(data) {}
}
