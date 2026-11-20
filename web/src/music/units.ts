import { rng } from './pattern'

export type UnitState = { seed: number; age: number; temp: number; name: string }

const CONDITIONS: { label: string; ageFactor: number }[] = [
  { label: '新品同様', ageFactor: 0 },
  { label: 'コンデンサ交換済み', ageFactor: 0.15 },
  { label: '現役で使い込まれた', ageFactor: 0.7 },
  { label: '押し入れで眠っていた', ageFactor: 1 },
  { label: 'ジャンク品', ageFactor: 1.15 },
]

const PLACES: { label: string; temp: number }[] = [
  { label: '冬の倉庫', temp: 6 },
  { label: '自宅スタジオ', temp: 23 },
  { label: 'ライブハウスの機材卓', temp: 31 },
  { label: '真夏の野外ステージ', temp: 40 },
]

const NOW = 2026

/** Draw a named machine: build year, condition and where it is played. */
export function drawUnit(seed: number): UnitState {
  const r = rng(seed * 2654435761)
  const year = 1980 + Math.floor(r() * 4)
  const cond = CONDITIONS[Math.floor(r() * CONDITIONS.length)]
  const place = PLACES[Math.floor(r() * PLACES.length)]
  const age = Math.min(50, Math.round((NOW - year) * cond.ageFactor))
  return {
    seed,
    age,
    temp: place.temp,
    name: `${year} 年製・${cond.label}・${place.label}`,
  }
}

export const DEFAULT_UNIT: UnitState = { seed: 808, age: 0, temp: 25, name: '工場出荷したて' }
