import { MAX_STEPS, SLOTS, STEPS } from '../audio/protocol'
import { bitsToSwitches, defaultSwitches, switchesToBits, type Switches } from './channels'
import type { PatternText } from './pattern'
import { DEFAULT_UNIT, type UnitState } from './units'

export type ShareState = {
  preset: string | null
  variant: number
  rows: PatternText
  /** 16 or 32; absent in links from before the format had it (taken from the row lengths). */
  length?: number
  sw: Switches
  bpm: number
  swing: number
  unit: UnitState
  knobs: number[][]
  accent?: number
}

/**
 * Version 1 (no `v`): rows for BD SD CH OH CP CB at full length, five knob lists of 0–1 values.
 * Version 2: `v: 2`, rows for all eleven slots with trailing rests trimmed and runs of
 * rests packed (see `packRow`), `len`, `sw` (a bit mask of the sound switches in
 * SWITCH_SLOTS order) and knobs in hundredths.
 */
type Wire = {
  v?: number
  preset: string | null
  variant: number
  rows: PatternText
  len?: number
  sw?: number
  bpm: number
  swing: number
  unit: UnitState
  knobs: number[][]
  accent?: number
}

function toBase64Url(s: string) {
  const bytes = new TextEncoder().encode(s)
  let bin = ''
  bytes.forEach((b) => (bin += String.fromCharCode(b)))
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

function fromBase64Url(s: string) {
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/'))
  return new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)))
}

// A run of 2–24 rests becomes one letter A–W (X is taken by the accent).
const RUN_MIN = 2
const RUN_MAX = 24
const RUN_BASE = 'A'.charCodeAt(0) - RUN_MIN

function packRow(row: string): string {
  return row.replace(/\.+$/, '').replace(/\.{2,}/g, (run) => {
    let out = ''
    for (let n = run.length; n > 0; n -= RUN_MAX) {
      const k = Math.min(n, RUN_MAX)
      out += k < RUN_MIN ? '.' : String.fromCharCode(RUN_BASE + k)
    }
    return out
  })
}

function unpackRow(row: string): string {
  return row.replace(/[A-W]/g, (c) => '.'.repeat(c.charCodeAt(0) - RUN_BASE))
}

export function encodeShare(state: ShareState): string {
  const rows = state.rows.map(packRow)
  while (rows.length && !rows[rows.length - 1]) rows.pop()
  const wire: Wire = {
    v: 2,
    preset: state.preset,
    variant: state.variant,
    rows,
    len: state.length,
    sw: switchesToBits(state.sw),
    bpm: state.bpm,
    swing: state.swing,
    unit: state.unit,
    knobs: state.knobs.map((vals) => vals.map((x) => Math.round(x * 100))),
    accent: state.accent,
  }
  return toBase64Url(JSON.stringify(wire))
}

const finite = (x: unknown): x is number => typeof x === 'number' && Number.isFinite(x)

/** `x` limited to the range its control offers; `fallback` when it is not a number. */
function clamp(x: unknown, min: number, max: number, fallback: number) {
  return finite(x) ? Math.min(max, Math.max(min, x)) : fallback
}

// Ranges of the controls these values come from (App.tsx).
const BPM = [60, 180] as const
const SWING = [0, 0.3] as const
const AGE = [0, 50] as const
const TEMP = [0, 45] as const

function readUnit(u: Partial<UnitState>): UnitState {
  return {
    // The seed input coerces to a 32-bit integer the same way.
    seed: finite(u.seed) ? u.seed | 0 : DEFAULT_UNIT.seed,
    age: clamp(u.age, AGE[0], AGE[1], DEFAULT_UNIT.age),
    temp: clamp(u.temp, TEMP[0], TEMP[1], DEFAULT_UNIT.temp),
    name: typeof u.name === 'string' ? u.name : '共有された個体',
  }
}

/** Knob values divided by `scale` and limited to 0–1; anything else becomes NaN, which `completeKnobs` replaces. */
function readKnobs(knobs: unknown, scale: number): number[][] {
  if (!Array.isArray(knobs)) return []
  return knobs.map((vals) =>
    Array.isArray(vals) ? vals.map((x) => (finite(x) ? Math.min(1, Math.max(0, x / scale)) : NaN)) : [],
  )
}

/**
 * Returns null when the hash is missing, not one of ours, or from a newer format.
 * Values are limited to what the app's own controls produce, so a hand-made link
 * cannot push the engine or the sequencer outside the range they are built for.
 */
export function decodeShare(hash: string): ShareState | null {
  const m = /^#s=([\w-]+)$/.exec(hash)
  if (!m) return null
  try {
    const s = JSON.parse(fromBase64Url(m[1])) as Wire
    if (!s || !Array.isArray(s.rows) || typeof s.bpm !== 'number' || !s.unit) return null
    const v2 = s.v === 2
    if (!v2 && s.v !== undefined) return null
    // A row never holds more than two bars; packed v2 rows are never longer than unpacked ones.
    const rows = s.rows
      .slice(0, SLOTS.length)
      .map((r) => (typeof r === 'string' ? (v2 ? unpackRow(r.slice(0, MAX_STEPS)) : r).slice(0, MAX_STEPS) : ''))
    const common = {
      preset: typeof s.preset === 'string' ? s.preset : null,
      variant: finite(s.variant) ? Math.min(1e6, Math.max(0, Math.floor(s.variant))) : 0,
      bpm: clamp(s.bpm, BPM[0], BPM[1], 120),
      swing: clamp(s.swing, SWING[0], SWING[1], 0),
      unit: readUnit(typeof s.unit === 'object' ? s.unit : {}),
      accent: s.accent === undefined ? undefined : clamp(s.accent, 0, 1, 0.8),
    }
    if (v2) {
      return {
        ...common,
        rows,
        length: s.len === STEPS || s.len === MAX_STEPS ? s.len : undefined,
        sw: bitsToSwitches(finite(s.sw) ? s.sw : 0),
        knobs: readKnobs(s.knobs, 100),
      }
    }
    // Version 1: the original six rows; length comes from the rows, switches are at their defaults.
    return { ...common, rows, sw: defaultSwitches(), knobs: readKnobs(s.knobs, 1) }
  } catch {
    return null
  }
}
