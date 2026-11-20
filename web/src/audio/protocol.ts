export const VOICES = ['BD', 'SD', 'HH', 'CP', 'CB'] as const
/** Part-table index of the shared circuitry (supply, accent line, mixer). */
export const COMMON = VOICES.length
export const SLOTS = ['BD', 'SD', 'CH', 'OH', 'CP', 'CB'] as const
/** Steps per bar; patterns are one or two bars long. */
export const STEPS = 16
export const MAX_STEPS = 32

/** Per step: 0 = off, 1 = on, 2 = accent. */
export type Velocities = number[][]
/** Per step: how many evenly spaced hits (1 = normal, 2–4 = roll). */
export type Rolls = number[][]

export type PartInfo = {
  name: string
  kind: number
  nominal: number
  effective: number
  scale: number
}

export type SyncMode = 'internal' | 'midi'

export type ToWorklet =
  | { type: 'trigger'; slot: number; accent: number }
  | { type: 'knob'; voice: number; id: number; value: number }
  | { type: 'unit'; seed: number; age: number; temp: number }
  | { type: 'part'; voice: number; index: number; scale: number }
  | { type: 'master'; value: number }
  | { type: 'accent'; value: number }
  | { type: 'seq'; vel: Velocities; rolls: Rolls; length: number; bpm: number; swing: number; playing: boolean }
  | { type: 'getParts' }
  /** Follow the internal tempo or incoming MIDI clock. */
  | { type: 'sync'; mode: SyncMode }
  /** MIDI realtime message; `frame` is when it should take effect on the context timeline. */
  | { type: 'midi'; kind: 'clock' | 'start' | 'continue' | 'stop'; frame: number }
  /** MIDI song position pointer, in sixteenth notes. */
  | { type: 'spp'; pos: number }

export type FromWorklet =
  | { type: 'ready' }
  | { type: 'step'; index: number }
  /**
   * Share of the real-time budget spent rendering, 0..1+ (above 1 = dropouts); peakLoad −1 = unknown.
   * clockBpm is the tempo measured from MIDI clock, −1 when not following it.
   */
  | { type: 'stats'; load: number; peakLoad: number; clockBpm: number }
  | { type: 'parts'; voices: PartInfo[][] }
  | { type: 'error'; message: string }
  /** The engine was rebuilt after an error; settings must be sent again. */
  | { type: 'reset' }
