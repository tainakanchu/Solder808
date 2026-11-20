import type { PatternText } from './pattern'
import type { UnitState } from './units'

export type ShareState = {
  preset: string | null
  variant: number
  rows: PatternText
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

export function encodeShare(state: ShareState): string {
  return toBase64Url(JSON.stringify(state))
}

/** Returns null when the hash is missing or not one of ours. */
export function decodeShare(hash: string): ShareState | null {
  const m = /^#s=([\w-]+)$/.exec(hash)
  if (!m) return null
  try {
    const s = JSON.parse(fromBase64Url(m[1])) as ShareState
    if (!Array.isArray(s.rows) || typeof s.bpm !== 'number' || !s.unit) return null
    return s
  } catch {
    return null
  }
}
