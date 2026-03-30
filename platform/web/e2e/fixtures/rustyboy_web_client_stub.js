export default async function init() { return {}; }
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
