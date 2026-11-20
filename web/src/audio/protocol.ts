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

export type ToWorklet =
  | { type: 'trigger'; slot: number; accent: number }
  | { type: 'knob'; voice: number; id: number; value: number }
  | { type: 'unit'; seed: number; age: number; temp: number }
  | { type: 'part'; voice: number; index: number; scale: number }
  | { type: 'master'; value: number }
  | { type: 'accent'; value: number }
  | { type: 'seq'; vel: Velocities; rolls: Rolls; length: number; bpm: number; swing: number; playing: boolean }
  | { type: 'getParts' }

export type FromWorklet =
  | { type: 'ready' }
  | { type: 'step'; index: number }
  /** Share of the real-time budget spent rendering, 0..1+ (above 1 = dropouts); peakLoad −1 = unknown. */
  | { type: 'stats'; load: number; peakLoad: number }
  | { type: 'parts'; voices: PartInfo[][] }
  | { type: 'error'; message: string }
  /** The engine was rebuilt after an error; settings must be sent again. */
  | { type: 'reset' }
