import { bitsToSwitches, defaultSwitches, switchesToBits, type Switches } from './channels'
import type { PatternText } from './pattern'
import type { UnitState } from './units'

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

/** Returns null when the hash is missing or not one of ours. */
export function decodeShare(hash: string): ShareState | null {
  const m = /^#s=([\w-]+)$/.exec(hash)
  if (!m) return null
  try {
    const s = JSON.parse(fromBase64Url(m[1])) as Wire
    if (!Array.isArray(s.rows) || typeof s.bpm !== 'number' || !s.unit) return null
    const rows = s.rows.map((r) => (typeof r === 'string' ? r : ''))
    if (s.v === 2) {
      return {
        preset: s.preset,
        variant: s.variant,
        rows: rows.map(unpackRow),
        length: s.len,
        sw: bitsToSwitches(typeof s.sw === 'number' ? s.sw : 0),
        bpm: s.bpm,
        swing: s.swing,
        unit: s.unit,
        knobs: Array.isArray(s.knobs)
          ? s.knobs.map((vals) => (Array.isArray(vals) ? vals.map((x) => (typeof x === 'number' ? x / 100 : x)) : vals))
          : s.knobs,
        accent: s.accent,
      }
    }
    // Version 1: the original six rows; length comes from the rows, switches are at their defaults.
    return {
      preset: s.preset,
      variant: s.variant,
      rows,
      sw: defaultSwitches(),
      bpm: s.bpm,
      swing: s.swing,
      unit: s.unit,
      knobs: s.knobs,
      accent: s.accent,
    }
  } catch {
    return null
  }
}
