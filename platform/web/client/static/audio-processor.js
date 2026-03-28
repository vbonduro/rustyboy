class AudioProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this._buf = new Float32Array(16384 * 2); // interleaved stereo ring
    this._head = 0;
    this._tail = 0;
    this._size = 0;
    this.port.onmessage = (e) => {
      const samples = e.data; // Float32Array interleaved [L,R,...]
      const pairs = samples.length >> 1;
      for (let i = 0; i < pairs; i++) {
        if (this._size >= 16384) break;
        this._buf[this._head * 2]     = samples[i * 2];
        this._buf[this._head * 2 + 1] = samples[i * 2 + 1];
        this._head = (this._head + 1) % 16384;
        this._size++;
      }
    };
  }

  process(inputs, outputs) {
    const left  = outputs[0][0];
    const right = outputs[0][1];
    for (let i = 0; i < left.length; i++) {
      if (this._size > 0) {
        left[i]  = this._buf[this._tail * 2];
        right[i] = this._buf[this._tail * 2 + 1];
        this._tail = (this._tail + 1) % 16384;
        this._size--;
      } else {
        left[i] = right[i] = 0;
      }
    }
    return true;
  }
}

registerProcessor('audio-processor', AudioProcessor);
