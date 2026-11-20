import processorUrl from './processor.ts?worker&url'
import wasmUrl from '../assets/solder808.wasm?url'
import type { FromWorklet, ToWorklet } from './protocol'

export type AudioEngine = {
  ctx: AudioContext
  send(msg: ToWorklet): void
  onMessage(fn: (msg: FromWorklet) => void): void
}

export type LatencyMode = 'interactive' | 'balanced' | 'playback'

export async function startEngine(latencyHint: LatencyMode = 'interactive'): Promise<AudioEngine> {
  const ctx = new AudioContext({ latencyHint })
  const [wasm] = await Promise.all([
    fetch(wasmUrl).then((r) => r.arrayBuffer()),
    ctx.audioWorklet.addModule(processorUrl),
  ])
  const node = new AudioWorkletNode(ctx, 'solder808', {
    numberOfInputs: 0,
    outputChannelCount: [2],
    processorOptions: { wasm },
  })
  node.connect(ctx.destination)
  await ctx.resume()
  const listeners: ((msg: FromWorklet) => void)[] = []
  node.port.onmessage = (ev: MessageEvent<FromWorklet>) => listeners.forEach((fn) => fn(ev.data))
  // Fired when the processor throws outside process() (e.g. in its constructor).
  node.onprocessorerror = () =>
    listeners.forEach((fn) => fn({ type: 'error', message: 'AudioWorklet processor error' }))
  return {
    ctx,
    send: (msg) => node.port.postMessage(msg),
    onMessage: (fn) => listeners.push(fn),
  }
}
