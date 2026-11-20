import {
  COMMON,
  SLOTS,
  VOICES,
  type FromWorklet,
  type SyncMode,
  type PartInfo,
  type Rolls,
  type ToWorklet,
  type Velocities,
} from './protocol'

type Exports = {
  memory: WebAssembly.Memory
  engine_new(sr: number): number
  engine_out_ptr(e: number): number
  engine_block_size(): number
  engine_process(e: number, frames: number): void
  engine_trigger(e: number, slot: number, accent: number): void
  engine_set_knob(e: number, voice: number, id: number, value: number): void
  /** Missing in engines built before the tom/conga, RS/CL and CP/MA switches. */
  engine_set_switch?(e: number, voice: number, value: number): void
  /** Missing in engines built before switchable oversampling. */
  engine_set_oversample?(e: number, factor: number): void
  engine_oversample?(e: number): number
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
// Engines without engine_set_switch have only BD SD HH CP CB, with the shared
// circuitry at part-table index 5.
const LEGACY_VOICES = 5
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
  // MIDI clock following: 24 ticks per beat, so a sixteenth every 6 ticks.
  private sync: SyncMode = 'internal'
  private tick = -1
  private lastTickFrame = -1
  private tickLen = 0
  private queue: { frame: number; step: number }[] = []
  // A clock may start playback without Start/Continue (joining a running
  // sequence); cleared by Stop or a manual stop so a free-running clock
  // does not restart it.
  private joinable = false
  // A song position arrived since playback last started.
  private sppSet = false
  // Last requested oversampling, kept so a rebuilt engine starts at it (0 = engine default).
  private oversample = 0

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
    // The app re-sends every setting after a reset; applying this one at once
    // keeps an overloaded device from rendering the first blocks at 2x.
    if (this.oversample && typeof this.x.engine_set_oversample === 'function')
      this.x.engine_set_oversample(this.e, this.oversample)
  }

  private get legacy() {
    return typeof this.x.engine_set_switch !== 'function'
  }

  /** Engine part-table index for a UI voice, or −1 when this engine lacks it. */
  private partVoice(v: number) {
    if (!this.legacy) return v
    if (v === COMMON) return LEGACY_VOICES
    return v < LEGACY_VOICES ? v : -1
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
      case 'switch':
        if (typeof x.engine_set_switch === 'function') x.engine_set_switch(e, m.voice, m.value)
        break
      case 'unit':
        x.engine_set_unit(e, m.seed >>> 0, m.age, m.temp)
        break
      case 'part': {
        const v = this.partVoice(m.voice)
        if (v >= 0) x.engine_set_part_scale(e, v, m.index, m.scale)
        break
      }
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
        if (this.playing && !m.playing) this.joinable = false
        this.playing = m.playing
        break
      case 'oversample':
        this.oversample = m.factor
        if (typeof x.engine_set_oversample === 'function') x.engine_set_oversample(e, m.factor)
        break
      case 'getParts':
        this.sendParts()
        break
      case 'sync':
        // Re-sent after an engine reset; only a mode change re-arms joining.
        if (m.mode !== this.sync) this.joinable = m.mode === 'midi'
        this.sync = m.mode
        this.queue = []
        this.tickLen = 0
        this.lastTickFrame = -1
        this.untilStep = 0
        break
      case 'spp':
        this.tick = m.pos * 6 - 1
        this.sppSet = true
        break
      case 'midi':
        this.onMidi(m.kind, m.frame)
        break
    }
  }

  private onMidi(kind: 'clock' | 'start' | 'continue' | 'stop', frame: number) {
    if (kind === 'start') {
      // The first clock after Start is the downbeat.
      this.tick = -1
      this.queue = []
      this.rollLeft.fill(0)
      this.playing = true
      this.sppSet = false
    } else if (kind === 'continue') {
      this.playing = true
      this.sppSet = false
    } else if (kind === 'stop') {
      this.playing = false
      this.joinable = false
      this.queue = []
      this.rollLeft.fill(0)
    } else {
      if (this.lastTickFrame >= 0) {
        const dt = frame - this.lastTickFrame
        if (dt > 0 && dt < sampleRate * 0.25) this.tickLen = this.tickLen ? this.tickLen * 0.9 + dt * 0.1 : dt
      }
      this.lastTickFrame = frame
      if (!this.playing) {
        if (!this.joinable) return
        // Join mid-song: at the song position if one came, else this tick is the downbeat.
        if (!this.sppSet) this.tick = -1
        this.sppSet = false
        this.queue = []
        this.rollLeft.fill(0)
        this.playing = true
        this.send({ type: 'transport', playing: true })
      }
      this.tick += 1
      if (this.tick % 6 !== 0) return
      const step = (this.tick / 6) % this.length
      // Swing delays the off-beat sixteenths, as in the internal sequencer.
      const at = frame + (step % 2 === 1 ? this.swing * this.tickLen * 6 : 0)
      this.queue.push({ frame: at, step })
    }
  }

  /** Samples per sixteenth at the current tempo (internal or measured from clock). */
  private stepLen() {
    if (this.sync === 'midi' && this.tickLen > 0) return this.tickLen * 6
    return (sampleRate * 60) / this.bpm / 4
  }

  private sendParts() {
    const { x, e } = this
    const voices: PartInfo[][] = []
    for (let uv = 0; uv <= VOICES.length; uv++) {
      const parts: PartInfo[] = []
      const v = this.partVoice(uv)
      const n = v >= 0 ? x.engine_part_count(e, v) : 0
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
    const base = this.stepLen()
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
      const clockBpm = this.sync === 'midi' && this.tickLen > 0 ? (sampleRate * 60) / (this.tickLen * 24) : -1
      const { x } = this
      // 0 tells the app this engine cannot switch (it lacks the setter or the getter).
      const oversample =
        typeof x.engine_set_oversample === 'function' && typeof x.engine_oversample === 'function'
          ? x.engine_oversample(this.e)
          : 0
      this.send({
        type: 'stats',
        load: this.busyMs / this.audioMs,
        peakLoad: HIRES ? this.peakLoad : -1,
        clockBpm,
        oversample,
      })
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
      if (this.sync === 'midi') {
        // Steps come from the clock queue; a late tick fires at once.
        const head = this.queue[0]
        this.untilStep = head ? head.frame - (currentFrame + done) : Infinity
        if (this.playing && head && this.untilStep < 1) {
          this.queue.shift()
          this.step = head.step % this.length
          this.fireStep()
          continue
        }
      } else if (this.playing && this.untilStep < 1) this.fireStep()
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
