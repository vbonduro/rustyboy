/**
 * AudioWorklet processor for rustyboy audio.
 *
 * Loaded via: audioContext.audioWorklet.addModule('/static/audio-processor.js')
 * Runs in AudioWorkletGlobalScope — no Window or DOM APIs available.
 *
 * The main thread posts Float32Array interleaved stereo samples [L,R,L,R,…]
 * through the MessagePort. process() drains the ring buffer into the output
 * channels on each audio render quantum (~128 frames at 48 kHz).
 */

const RING_CAPACITY = 16384;

class AudioProcessor extends AudioWorkletProcessor {
  private readonly _buf = new Float32Array(RING_CAPACITY * 2); // interleaved L,R
  private _head = 0;
  private _tail = 0;
  private _size = 0;

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent<Float32Array>) => {
      this._enqueue(e.data);
    };
  }

  /** Enqueue interleaved stereo pairs from the main thread into the ring buffer. */
  private _enqueue(samples: Float32Array): void {
    const pairs = samples.length >> 1;
    for (let i = 0; i < pairs; i++) {
      if (this._size >= RING_CAPACITY) break; // drop when full
      this._buf[this._head * 2]     = samples[i * 2];
      this._buf[this._head * 2 + 1] = samples[i * 2 + 1];
      this._head = (this._head + 1) % RING_CAPACITY;
      this._size++;
    }
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][], _parameters: Record<string, Float32Array>): boolean {
    const left  = outputs[0][0];
    const right = outputs[0][1];
    for (let i = 0; i < left.length; i++) {
      if (this._size > 0) {
        left[i]  = this._buf[this._tail * 2];
        right[i] = this._buf[this._tail * 2 + 1];
        this._tail = (this._tail + 1) % RING_CAPACITY;
        this._size--;
      } else {
        left[i] = right[i] = 0;
      }
    }
    return true; // keep processor alive
  }
}

registerProcessor('audio-processor', AudioProcessor);
