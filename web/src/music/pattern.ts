import { MAX_STEPS, SLOTS, STEPS, type Rolls, type Velocities } from '../audio/protocol'

/** Arrays always hold two bars; `length` (16 or 32) is how much is played. */
export type Pattern = { vel: Velocities; rolls: Rolls; length: number }

/**
 * One string per slot (BD SD CH OH CP CB), 16 or 32 chars each:
 * `.` off, `x` on, `X` accent, `2`–`4` on with a roll of that many hits.
 */
export type PatternText = string[]

const cellVel = (c: string) => (c === 'X' ? 2 : c === '.' ? 0 : 1)
const cellRoll = (c: string) => (c >= '2' && c <= '4' ? Number(c) : 1)

/** A one-bar text is repeated into bar 2 so switching to two bars starts sensibly. */
export function parsePattern(rows: PatternText): Pattern {
  const length = rows.some((r) => r.length > STEPS) ? MAX_STEPS : STEPS
  const vel: Velocities = []
  const rolls: Rolls = []
  for (let s = 0; s < SLOTS.length; s++) {
    let row = (rows[s] ?? '').padEnd(length, '.').slice(0, length)
    if (length === STEPS) row = row + row
    vel.push([...row].map(cellVel))
    rolls.push([...row].map(cellRoll))
  }
  return { vel, rolls, length }
}

/** Two-bar text: bar 1 as written, bar 2 the same with its last 4 steps replaced by `fill`. */
export function withFill(rows: PatternText, fill: PatternText): PatternText {
  return rows.map((r, s) => {
    const bar = r.padEnd(STEPS, '.').slice(0, STEPS)
    return bar + bar.slice(0, STEPS - 4) + (fill[s] ?? '....').padEnd(4, '.').slice(0, 4)
  })
}

export function patternToText(p: Pattern): PatternText {
  return p.vel.map((row, s) =>
    row
      .slice(0, p.length)
      .map((v, i) => (v === 0 ? '.' : v === 2 ? 'X' : p.rolls[s][i] > 1 ? String(p.rolls[s][i]) : 'x'))
      .join(''),
  )
}

export function emptyPattern(length = MAX_STEPS): Pattern {
  return { ...parsePattern([]), length }
}

export function copyBar1ToBar2(p: Pattern): Pattern {
  const copy = (row: number[]) => [...row.slice(0, STEPS), ...row.slice(0, STEPS)]
  return { vel: p.vel.map(copy), rolls: p.rolls.map(copy), length: p.length }
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

const [BD, SD, CH, OH, CP, CB] = [0, 1, 2, 3, 4, 5]

/**
 * Light-touch variations that keep the groove recognisable: the downbeat kick
 * and backbeat accents are never removed.
 */
export function arrange(base: Pattern, variant: number): Pattern {
  if (variant === 0) return base
  const r = rng(variant * 7919)
  // With two bars, keep bar 1 as the reference groove and vary bar 2.
  const off = base.length > STEPS ? STEPS : 0
  const vel = base.vel.map((row) => [...row])
  const rolls = base.rolls.map((row) => [...row])
  const free = (s: number, i: number) => vel[s][i] === 0
  const pick = (from: number[]) => off + from[Math.floor(r() * from.length)]
  const moves = [
    () => {
      const i = pick([7, 10, 14, 15])
      if (free(BD, i)) vel[BD][i] = 1
    },
    () => {
      const i = pick([3, 6, 9, 11, 14, 15])
      if (free(SD, i) && free(CP, i)) vel[SD][i] = 1
    },
    () => {
      const on = vel[CH].slice(off, off + STEPS).flatMap((v, i) => (v > 0 ? [i] : []))
      if (on.length) rolls[CH][pick(on)] = [2, 3, 3, 4][Math.floor(r() * 4)]
    },
    () => {
      const i = pick([6, 14])
      if (free(OH, i)) {
        vel[OH][i] = 1
        vel[CH][i] = 0
      }
    },
    () => {
      const i = pick([3, 7, 11, 15])
      if (free(CB, i)) vel[CB][i] = 1
    },
    () => {
      const i = pick([2, 6, 10, 14])
      if (free(CH, i) && free(OH, i)) vel[CH][i] = 1
    },
  ]
  const count = 2 + Math.floor(r() * 2)
  for (let k = 0; k < count; k++) moves[Math.floor(r() * moves.length)]()
  return { vel, rolls, length: base.length }
}
