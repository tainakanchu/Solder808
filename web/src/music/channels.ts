import { SLOTS } from '../audio/protocol'

export type Sound = { code: string; name: string }
/** A sequencer row; `alt` is the sound its panel switch selects instead. */
export type Channel = Sound & { alt?: Sound }

/** Indexed by trigger slot. */
export const CHANNELS: Channel[] = [
  { code: 'BD', name: 'バスドラム' },
  { code: 'SD', name: 'スネア' },
  { code: 'CH', name: '閉じハット' },
  { code: 'OH', name: '開きハット' },
  { code: 'CP', name: 'クラップ', alt: { code: 'MA', name: 'マラカス' } },
  { code: 'CB', name: 'カウベル' },
  { code: 'LT', name: 'ロータム', alt: { code: 'LC', name: 'ローコンガ' } },
  { code: 'MT', name: 'ミッドタム', alt: { code: 'MC', name: 'ミッドコンガ' } },
  { code: 'HT', name: 'ハイタム', alt: { code: 'HC', name: 'ハイコンガ' } },
  { code: 'RS', name: 'リムショット', alt: { code: 'CL', name: 'クラベス' } },
  { code: 'CY', name: 'シンバル' },
]

/** First slot of the rows added after the original six (LT MT HT RS CY), which can be folded away. */
export const EXTRA_FROM = 6
export const EXTRA_SLOTS = SLOTS.map((_, s) => s).filter((s) => s >= EXTRA_FROM)

/** Slots with a sound switch, in share-URL bit order. */
export const SWITCH_SLOTS = CHANNELS.flatMap((c, s) => (c.alt ? [s] : []))

/** Per slot: 0 = the channel's first sound, 1 = its `alt` (always 0 for slots without a switch). */
export type Switches = number[]

export const defaultSwitches = (): Switches => SLOTS.map(() => 0)

export const soundOf = (slot: number, sw: Switches): Sound => {
  const c = CHANNELS[slot]
  return sw[slot] && c.alt ? c.alt : c
}

/** Slot and switch state for a row code such as `LT` or `LC`. */
export function codeSlot(code: string): { slot: number; alt: number } | null {
  for (let s = 0; s < CHANNELS.length; s++) {
    if (CHANNELS[s].code === code) return { slot: s, alt: 0 }
    if (CHANNELS[s].alt?.code === code) return { slot: s, alt: 1 }
  }
  return null
}

export function switchesToBits(sw: Switches): number {
  return SWITCH_SLOTS.reduce((bits, s, k) => bits | ((sw[s] ? 1 : 0) << k), 0)
}

export function bitsToSwitches(bits: number): Switches {
  const sw = defaultSwitches()
  SWITCH_SLOTS.forEach((s, k) => (sw[s] = (bits >> k) & 1))
  return sw
}

export type KnobDef = { id: number; label: string; init: number }

/**
 * Knobs per engine voice, in panel order; `id` is the engine's knob id (0 = level).
 * Share links store values by position in these lists, so only ever append.
 */
export const KNOBS: KnobDef[][] = [
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 2, label: 'Tone', init: 0.5 },
    { id: 3, label: 'Decay', init: 0.6 },
    { id: 1, label: 'Tune trim', init: 0.5 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tone', init: 0.5 },
    { id: 2, label: 'Snappy', init: 0.6 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'OH Decay', init: 0.4 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tail', init: 0.4 },
  ],
  [
    { id: 0, label: 'Level', init: 0.7 },
    { id: 1, label: 'Decay', init: 0.4 },
  ],
  ...[0, 1, 2].map(() => [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tom tune', init: 0.5 },
    { id: 2, label: 'Conga tune', init: 0.5 },
  ]),
  [{ id: 0, label: 'Level', init: 0.8 }],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tone', init: 0.5 },
    { id: 2, label: 'Decay', init: 0.5 },
  ],
]

/** Tom voices: knob id 1 tunes the tom, id 2 the conga. */
export const TUNING_IDS = { tom: 1, conga: 2 }

export const defaultKnobs = () => KNOBS.map((vs) => vs.map((k) => k.init))

/** Fill in voices and knobs missing from older share links with defaults. */
export function completeKnobs(knobs: unknown): number[][] {
  const out = defaultKnobs()
  if (!Array.isArray(knobs)) return out
  knobs.forEach((vals, v) => {
    if (v >= out.length || !Array.isArray(vals)) return
    vals.forEach((x, i) => {
      if (i < out[v].length && typeof x === 'number' && Number.isFinite(x)) out[v][i] = x
    })
  })
  return out
}
