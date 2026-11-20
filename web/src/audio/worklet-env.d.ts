// Globals of AudioWorkletGlobalScope, which TypeScript's DOM lib does not include.
declare const sampleRate: number
/** Frame index of the first sample in the block being rendered. */
declare const currentFrame: number
declare class AudioWorkletProcessor {
  readonly port: MessagePort
  constructor(options?: unknown)
}
declare function registerProcessor(name: string, ctor: new (options: never) => AudioWorkletProcessor): void
