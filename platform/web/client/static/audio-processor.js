"use strict";
(() => {
  // src-ts/audio-processor.ts
  var RING_CAPACITY = 16384;
  var AudioProcessor = class extends AudioWorkletProcessor {
    constructor() {
      super();
      this._buf = new Float32Array(RING_CAPACITY * 2);
      // interleaved L,R
      this._head = 0;
      this._tail = 0;
      this._size = 0;
      this.port.onmessage = (e) => {
        this._enqueue(e.data);
      };
    }
    /** Enqueue interleaved stereo pairs from the main thread into the ring buffer. */
    _enqueue(samples) {
      const pairs = samples.length >> 1;
      for (let i = 0; i < pairs; i++) {
        if (this._size >= RING_CAPACITY) break;
        this._buf[this._head * 2] = samples[i * 2];
        this._buf[this._head * 2 + 1] = samples[i * 2 + 1];
        this._head = (this._head + 1) % RING_CAPACITY;
        this._size++;
      }
    }
    process(_inputs, outputs, _parameters) {
      const left = outputs[0][0];
      const right = outputs[0][1];
      for (let i = 0; i < left.length; i++) {
        if (this._size > 0) {
          left[i] = this._buf[this._tail * 2];
          right[i] = this._buf[this._tail * 2 + 1];
          this._tail = (this._tail + 1) % RING_CAPACITY;
          this._size--;
        } else {
          left[i] = right[i] = 0;
        }
      }
      return true;
    }
  };
  registerProcessor("audio-processor", AudioProcessor);
})();
