import { MAX_STEPS, SLOTS, STEPS, type Rolls, type Velocities } from '../audio/protocol'
import { codeSlot, defaultSwitches, EXTRA_SLOTS, type Switches } from './channels'

/**
 * Arrays always hold two bars; `length` (16 or 32) is how much is played.
 * `sw` is the per-slot sound switch (CP/MA, tom/conga, RS/CL), part of the pattern.
 */
export type Pattern = { vel: Velocities; rolls: Rolls; length: number; sw: Switches }

/**
 * One string per slot (BD SD CH OH CP CB LT MT HT RS CY), 16 or 32 chars each:
 * `.` off, `x` on, `X` accent, `2`–`4` on with a roll of that many hits,
 * `5`–`7` accented with a roll of 2–4 hits (added later, so older texts never contain them).
 */
export type PatternText = string[]

/**
 * Rows keyed by sound code. A switchable channel is written with the code of the
 * sound it should play (`LC` puts the low tom channel on conga, `MA` the clap on maracas).
 */
export type PatternRows = Partial<Record<string, string>>

/** Accented rolls are written 3 above the plain roll digit. */
const ACCENT_ROLL = 3
const cellVel = (c: string) => (c === 'X' || (c >= '5' && c <= '7') ? 2 : c === '.' ? 0 : 1)
const cellRoll = (c: string) => (c >= '2' && c <= '4' ? Number(c) : c >= '5' && c <= '7' ? Number(c) - ACCENT_ROLL : 1)

function cellText(vel: number, roll: number): string {
  if (vel === 0) return '.'
  if (roll > 1) return String(vel === 2 ? roll + ACCENT_ROLL : roll)
  return vel === 2 ? 'X' : 'x'
}

/**
 * A one-bar text is repeated into bar 2 so switching to two bars starts sensibly.
 * Without `length`, rows longer than a bar make it two bars (the original share format).
 */
export function parsePattern(rows: PatternText, sw: Switches = defaultSwitches(), length?: number): Pattern {
  const len = length === STEPS || length === MAX_STEPS ? length : rows.some((r) => r.length > STEPS) ? MAX_STEPS : STEPS
  const vel: Velocities = []
  const rolls: Rolls = []
  for (let s = 0; s < SLOTS.length; s++) {
    let row = (rows[s] ?? '').padEnd(len, '.').slice(0, len)
    if (len === STEPS) row = row + row
    vel.push([...row].map(cellVel))
    rolls.push([...row].map(cellRoll))
  }
  return { vel, rolls, length: len, sw: SLOTS.map((_, s) => (sw[s] ? 1 : 0)) }
}

/** Rows keyed by sound code to slot-ordered text and switch states. */
export function rowsToText(rows: PatternRows): { text: PatternText; sw: Switches } {
  const text: PatternText = SLOTS.map(() => '')
  const sw = defaultSwitches()
  for (const [code, row] of Object.entries(rows)) {
    const at = codeSlot(code)
    if (!at || row === undefined) throw new Error(`unknown row ${code}`)
    if (text[at.slot]) throw new Error(`two rows for slot ${SLOTS[at.slot]}`)
    text[at.slot] = row
    sw[at.slot] = at.alt
  }
  return { text, sw }
}

/**
 * Two-bar text: bar 1 as written, bar 2 the same with its last 4 steps replaced by `fill`.
 * A 32-char row gives both bars itself (e.g. a crash only at the top of the loop); the fill still applies.
 */
export function withFill(rows: PatternText, fill: PatternText): PatternText {
  return rows.map((r, s) => {
    const bar1 = r.padEnd(STEPS, '.').slice(0, STEPS)
    const bar2 = r.length > STEPS ? r.padEnd(MAX_STEPS, '.').slice(STEPS) : bar1
    return bar1 + bar2.slice(0, STEPS - 4) + (fill[s] ?? '....').padEnd(4, '.').slice(0, 4)
  })
}

export function patternToText(p: Pattern): PatternText {
  return p.vel.map((row, s) =>
    row
      .slice(0, p.length)
      .map((v, i) => cellText(v, p.rolls[s][i]))
      .join(''),
  )
}

export function emptyPattern(length = MAX_STEPS): Pattern {
  return { ...parsePattern([]), length }
}

export function copyBar1ToBar2(p: Pattern): Pattern {
  const copy = (row: number[]) => [...row.slice(0, STEPS), ...row.slice(0, STEPS)]
  return { ...p, vel: p.vel.map(copy), rolls: p.rolls.map(copy) }
}

/** Whether the played part of the pattern has any hit on the foldable rows. */
export function usesExtra(p: Pattern): boolean {
  return EXTRA_SLOTS.some((s) => p.vel[s].slice(0, p.length).some((v) => v > 0))
}

/** Small deterministic PRNG so the same arrange number gives the same variation. */
export function rng(seed: number) {
  let x = seed >>> 0 || 1
  return () => {
    x ^= x << 13
    x ^= x >>> 17
    x ^= x << 5
    return (x >>> 0) / 0x1_0000_0000
  }
}

const [BD, SD, CH, OH, CP, CB, LT, MT, HT, RS, CY] = SLOTS.map((_, s) => s)
const TOMS = [LT, MT, HT]

/**
 * Light-touch variations that keep the groove recognisable: the downbeat kick
 * and backbeat accents are never removed, and the added instruments only join
 * where the pattern already invites them.
 */
export function arrange(base: Pattern, variant: number): Pattern {
  if (variant === 0) return base
  const r = rng(variant * 7919)
  // With two bars, keep bar 1 as the reference groove and vary bar 2.
  const two = base.length > STEPS
  const off = two ? STEPS : 0
  const vel = base.vel.map((row) => [...row])
  const rolls = base.rolls.map((row) => [...row])
  const free = (s: number, i: number) => vel[s][i] === 0
  const pick = (from: number[]) => off + from[Math.floor(r() * from.length)]
  const used = (s: number) => vel[s].slice(0, base.length).some((v) => v > 0)
  const tomsUsed = TOMS.filter(used)
  const fillStart = off + STEPS - 4
  const fillHasToms = TOMS.some((s) => vel[s].slice(fillStart, fillStart + 4).some((v) => v > 0))
  // Each move returns false when it does not fit this pattern, so another is tried.
  const moves: (() => boolean)[] = [
    () => {
      const i = pick([7, 10, 14, 15])
      if (!free(BD, i)) return false
      vel[BD][i] = 1
      return true
    },
    () => {
      const i = pick([3, 6, 9, 11, 14, 15])
      if (!free(SD, i) || !free(CP, i)) return false
      vel[SD][i] = 1
      return true
    },
    () => {
      const on = vel[CH].slice(off, off + STEPS).flatMap((v, i) => (v > 0 ? [i] : []))
      if (!on.length) return false
      rolls[CH][pick(on)] = [2, 3, 3, 4][Math.floor(r() * 4)]
      return true
    },
    () => {
      const i = pick([6, 14])
      if (!free(OH, i)) return false
      vel[OH][i] = 1
      vel[CH][i] = 0
      return true
    },
    () => {
      // Cowbell only answers in patterns that already have it.
      if (!used(CB)) return false
      const i = pick([3, 7, 11, 15])
      if (!free(CB, i)) return false
      vel[CB][i] = 1
      return true
    },
    () => {
      const i = pick([2, 6, 10, 14])
      if (!free(CH, i) || !free(OH, i)) return false
      vel[CH][i] = 1
      return true
    },
    () => {
      // Tom pickup into the loop: high-mid-low over the last three steps, for patterns that already use toms.
      if (!two || fillHasToms || TOMS.some((s) => base.sw[s]) || tomsUsed.length === 0) return false
      const run = [HT, MT, LT]
      for (let k = 0; k < 3; k++) {
        const i = fillStart + 1 + k
        vel[run[k]][i] = k === 2 ? 2 : 1
        vel[SD][i] = 0
        vel[CP][i] = 0
      }
      return true
    },
    () => {
      // An extra tom/conga answer on an off-beat sixteenth in a row that already plays.
      if (tomsUsed.length === 0) return false
      const s = tomsUsed[Math.floor(r() * tomsUsed.length)]
      const i = pick([3, 7, 9, 11])
      if (!free(s, i)) return false
      vel[s][i] = 1
      return true
    },
    () => {
      // Rim shot / claves ghost note where the pattern already uses it.
      if (!used(RS)) return false
      const i = pick([3, 7, 10, 13, 15])
      if (!free(RS, i)) return false
      vel[RS][i] = 1
      return true
    },
    () => {
      // Crash on the downbeat that follows a tom fill or in a pattern that already crashes.
      if (!two || vel[CY][0] > 0 || !(used(CY) || fillHasToms)) return false
      vel[CY][0] = 2
      return true
    },
  ]
  const count = 2 + Math.floor(r() * 2)
  for (let k = 0, tries = 0; k < count && tries < 24; tries++) if (moves[Math.floor(r() * moves.length)]()) k++
  return { ...base, vel, rolls }
}
