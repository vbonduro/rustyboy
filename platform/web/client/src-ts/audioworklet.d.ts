/**
 * Minimal AudioWorklet global declarations for the worklet scope.
 *
 * The AudioWorklet module runs in AudioWorkletGlobalScope, which is a worker-like
 * environment without Window or DOM. We declare only what audio-processor.ts needs
 * so tsc can type-check it with lib: ["ES2020"] (no DOM lib) without conflicts.
 */

// MessageEvent is a DOM/Worker type; declare a minimal form for the worklet scope.
declare class MessageEvent<T = unknown> {
  readonly data: T;
}

// MessagePort (a subset — only what the worklet uses).
// onmessage uses `any` so user code can narrow the event data with a cast.
declare class MessagePort {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onmessage: ((ev: MessageEvent<any>) => void) | null;
  postMessage(message: unknown): void;
}

declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort;
  abstract process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    parameters: Record<string, Float32Array>,
  ): boolean;
}

declare function registerProcessor(
  name: string,
  processorCtor: new () => AudioWorkletProcessor,
): void;
