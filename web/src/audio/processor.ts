import { SLOTS, VOICES, type FromWorklet, type PartInfo, type Rolls, type ToWorklet, type Velocities } from './protocol'

type Exports = {
  memory: WebAssembly.Memory
  engine_new(sr: number): number
  engine_out_ptr(e: number): number
  engine_block_size(): number
  engine_process(e: number, frames: number): void
  engine_trigger(e: number, slot: number, accent: number): void
  engine_set_knob(e: number, voice: number, id: number, value: number): void
  engine_set_master(e: number, value: number): void
  engine_set_accent(e: number, value: number): void
  engine_set_unit(e: number, seed: number, age: number, temp: number): void
  engine_set_part_scale(e: number, voice: number, part: number, scale: number): void
  engine_part_count(e: number, voice: number): number
  engine_part_name_ptr(e: number, voice: number, i: number): number
  engine_part_name_len(e: number, voice: number, i: number): number
  engine_part_kind(e: number, voice: number, i: number): number
  engine_part_nominal(e: number, voice: number, i: number): number
  engine_part_effective(e: number, voice: number, i: number): number
  engine_part_scale(e: number, voice: number, i: number): number
}

const SLOT_COUNT = SLOTS.length
const NORMAL = 0.35
// AudioWorkletGlobalScope often lacks `performance`; Date.now() is only
// millisecond-accurate, fine for the average but not for one 2.7 ms block.
const HIRES = typeof globalThis.performance?.now === 'function'
const now: () => number = HIRES ? () => globalThis.performance.now() : Date.now

class Solder808Processor extends AudioWorkletProcessor {
  private module: WebAssembly.Module
  private x!: Exports
  private e = 0
  private block: number
  private vel: Velocities = []
  private rolls: Rolls = []
  private bpm = 120
  private swing = 0
  private length = 16
  private busyMs = 0
  private audioMs = 0
  private peakLoad = 0
  private playing = false
  private step = 0
  private untilStep = 0
  // Pending roll hits per slot: remaining count, samples until next, spacing.
  private rollLeft = new Float64Array(SLOT_COUNT)
  private rollNext = new Float64Array(SLOT_COUNT)
  private rollGap = new Float64Array(SLOT_COUNT)
  private rollAccent = new Float64Array(SLOT_COUNT)

  constructor(options: { processorOptions: { wasm: ArrayBuffer } }) {
    super()
    this.module = new WebAssembly.Module(options.processorOptions.wasm)
    this.instantiate()
    this.block = this.x.engine_block_size()
    this.port.onmessage = (ev: MessageEvent<ToWorklet>) => this.onMessage(ev.data)
    this.send({ type: 'ready' })
  }

  private instantiate() {
    const instance = new WebAssembly.Instance(this.module, {})
    this.x = instance.exports as unknown as Exports
    this.e = this.x.engine_new(sampleRate)
  }

  private send(msg: FromWorklet) {
    this.port.postMessage(msg)
  }

  private onMessage(m: ToWorklet) {
    const { x, e } = this
    switch (m.type) {
      case 'trigger':
        x.engine_trigger(e, m.slot, m.accent)
        break
      case 'knob':
        x.engine_set_knob(e, m.voice, m.id, m.value)
        break
      case 'unit':
        x.engine_set_unit(e, m.seed >>> 0, m.age, m.temp)
        this.sendParts()
        break
      case 'part':
        x.engine_set_part_scale(e, m.voice, m.index, m.scale)
        this.sendParts()
        break
      case 'master':
        x.engine_set_master(e, m.value)
        break
      case 'accent':
        x.engine_set_accent(e, m.value)
        break
      case 'seq':
        this.vel = m.vel
        this.rolls = m.rolls
        this.bpm = m.bpm
        this.swing = m.swing
        this.length = m.length
        if (this.step >= this.length) this.step = 0
        if (m.playing && !this.playing) {
          this.step = 0
          this.untilStep = 0
        }
        if (!m.playing) this.rollLeft.fill(0)
        this.playing = m.playing
        break
      case 'getParts':
        this.sendParts()
        break
    }
  }

  private sendParts() {
    const { x, e } = this
    const voices: PartInfo[][] = []
    for (let v = 0; v <= VOICES.length; v++) {
      const parts: PartInfo[] = []
      const n = x.engine_part_count(e, v)
      for (let i = 0; i < n; i++) {
        const ptr = x.engine_part_name_ptr(e, v, i)
        const len = x.engine_part_name_len(e, v, i)
        const bytes = new Uint8Array(x.memory.buffer, ptr, len)
        // Part names are ASCII; TextDecoder may be missing in worklet scope.
        const name = String.fromCharCode(...bytes)
        parts.push({
          name,
          kind: x.engine_part_kind(e, v, i),
          nominal: x.engine_part_nominal(e, v, i),
          effective: x.engine_part_effective(e, v, i),
          scale: x.engine_part_scale(e, v, i),
        })
      }
      voices.push(parts)
    }
    this.send({ type: 'parts', voices })
  }

  private fireStep() {
    const base = (sampleRate * 60) / this.bpm / 4
    // Swing lengthens even steps and shortens odd ones.
    const len = base * (this.step % 2 === 0 ? 1 + this.swing : 1 - this.swing)
    for (let s = 0; s < SLOT_COUNT; s++) {
      const cell = this.vel[s]?.[this.step] ?? 0
      if (cell === 0) continue
      const accent = cell === 2 ? 1 : NORMAL
      this.x.engine_trigger(this.e, s, accent)
      const roll = this.rolls[s]?.[this.step] ?? 1
      if (roll > 1) {
        this.rollLeft[s] = roll - 1
        this.rollGap[s] = len / roll
        this.rollNext[s] = len / roll
        this.rollAccent[s] = accent * 0.8
      }
    }
    this.send({ type: 'step', index: this.step })
    this.step = (this.step + 1) % this.length
    this.untilStep += len
  }

  /** Samples until the next sequencer event (step or roll hit). */
  private nextEvent(): number {
    let t = this.untilStep
    for (let s = 0; s < SLOT_COUNT; s++) if (this.rollLeft[s] > 0) t = Math.min(t, this.rollNext[s])
    return t
  }

  private advance(n: number) {
    this.untilStep -= n
    for (let s = 0; s < SLOT_COUNT; s++) {
      if (this.rollLeft[s] <= 0) continue
      this.rollNext[s] -= n
      if (this.rollNext[s] < 1) {
        this.x.engine_trigger(this.e, s, this.rollAccent[s])
        this.rollLeft[s] -= 1
        this.rollNext[s] += this.rollGap[s]
      }
    }
  }

  private measure(busy: number, budget: number) {
    this.busyMs += busy
    this.audioMs += budget
    this.peakLoad = Math.max(this.peakLoad, busy / budget)
    if (this.audioMs >= 500) {
      this.send({ type: 'stats', load: this.busyMs / this.audioMs, peakLoad: HIRES ? this.peakLoad : -1 })
      this.busyMs = 0
      this.audioMs = 0
      this.peakLoad = 0
    }
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    try {
      this.render(outputs[0])
    } catch (err) {
      // A trap leaves the wasm state undefined: start a fresh instance rather
      // than let the browser kill the node (which silences it for good).
      for (const ch of outputs[0]) ch.fill(0)
      this.send({ type: 'error', message: String(err) })
      this.instantiate()
      this.rollLeft.fill(0)
      this.send({ type: 'reset' })
    }
    return true
  }

  private render(out: Float32Array[]) {
    const frames = out[0].length
    const { x, e } = this
    const t0 = now()
    let done = 0
    while (done < frames) {
      if (this.playing && this.untilStep < 1) this.fireStep()
      let n = Math.min(frames - done, this.block)
      if (this.playing) n = Math.min(n, Math.max(1, Math.ceil(this.nextEvent())))
      x.engine_process(e, n)
      const buf = new Float32Array(x.memory.buffer, x.engine_out_ptr(e), n)
      for (const ch of out) ch.set(buf, done)
      done += n
      if (this.playing) this.advance(n)
    }
    this.measure(now() - t0, (frames / sampleRate) * 1000)
  }
}

registerProcessor('solder808', Solder808Processor)
