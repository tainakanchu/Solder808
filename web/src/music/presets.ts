import { parsePattern, withFill, type Pattern, type PatternText } from './pattern'

export type Preset = {
  id: string
  name: string
  bpm: number
  swing: number
  /** Bar 1, or both bars when 32 chars and no `fill`. */
  rows: PatternText
  /** Last 4 steps of bar 2 (the fill), one 4-char string per slot. */
  fill?: PatternText
  /** Optional knob overrides: [voice, knob id, value]. */
  knobs?: [number, number, number][]
}

// Rows: BD, SD, CH, OH, CP, CB
export const PRESETS: Preset[] = [
  {
    id: 'house',
    name: '4 つ打ちハウス',
    bpm: 124,
    swing: 0.06,
    rows: [
      'X...x...X...x...',
      '................',
      'x...x...x...x...',
      '..x...x...x...x.',
      '....X.......X...',
      '................',
    ],
    fill: ['x...', '..xx', 'x...', '..x.', 'X.x.', '....'],
  },
  {
    id: 'ppap',
    name: 'PPAP',
    bpm: 136,
    swing: 0,
    rows: [
      'x.......x.......x.....x...x.x...',
      '....x.......x.......x.......x...',
      'x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.',
      '................................',
      '................................',
      '....x.......x.......x.......x...',
    ],
    knobs: [
      [0, 3, 0.76],
      [0, 1, 0.23],
      [1, 0, 0.84],
      [1, 2, 0.74],
      [2, 0, 0.7],
      [2, 1, 0.21],
      [4, 0, 0.79],
    ],
  },
  {
    id: 'electro',
    name: 'エレクトロ',
    bpm: 127,
    swing: 0,
    rows: [
      'X.....x.x.....x.',
      '....X.......X...',
      'x.xxx.xxx.xxx.x.',
      '................',
      '....x.......x...',
      'x..x..x...x..x..',
    ],
    fill: ['x.x.', 'X.xx', 'x.x.', '....', 'x...', '..x.'],
  },
  {
    id: 'miami',
    name: 'マイアミベース',
    bpm: 130,
    swing: 0,
    rows: [
      'X.....x.x.x.....',
      '....X.......X...',
      'x.x.x.x.x.x.x.x.',
      '................',
      '....X.......X.x.',
      '................',
    ],
    fill: ['x.xx', 'X.x.', 'x.x.', '....', 'X.xX', '....'],
    knobs: [[0, 3, 0.95]],
  },
  {
    id: 'boombap',
    name: 'ブームバップ',
    bpm: 90,
    swing: 0.18,
    rows: [
      'X......x..x.....',
      '....X.......X...',
      'X.x.X.x.X.x.X.x.',
      '................',
      '................',
      '................',
    ],
    fill: ['x..x', 'X..x', 'X.x.', '....', '....', '....'],
  },
  {
    id: 'trap',
    name: 'トラップ',
    bpm: 140,
    swing: 0,
    rows: [
      'X.....x...x.x...',
      '........X.......',
      'x.x.x.x.x3x.x4x.',
      '................',
      '........X.......',
      '................',
    ],
    fill: ['x.x.', '...x', '4.32', '....', '....', '....'],
    knobs: [
      [0, 3, 1],
      [0, 1, 0.2],
    ],
  },
  {
    id: 'disco',
    name: 'ディスコ',
    bpm: 118,
    swing: 0,
    rows: [
      'X...x...X...x...',
      '....X.......X...',
      'xx.xxx.xxx.xxx.x',
      '..x...x...x...x.',
      '................',
      '................',
    ],
    fill: ['x...', 'X.xx', 'xx.x', '..x.', '....', '....'],
  },
  {
    id: 'reggaeton',
    name: 'レゲトン',
    bpm: 95,
    swing: 0,
    rows: [
      'X...x...X...x...',
      '...x..x....x..x.',
      'x.x.x.x.x.x.x.x.',
      '................',
      '................',
      '................',
    ],
    fill: ['x...', 'x.xx', 'x.x.', '....', '....', '....'],
  },
  {
    id: 'eightbeat',
    name: '8 ビート',
    bpm: 110,
    swing: 0,
    rows: [
      'X.....x.X.x.....',
      '....X.......X...',
      'X.x.X.x.X.x.X.x.',
      '................',
      '................',
      '................',
    ],
    fill: ['x.x.', 'Xxxx', '....', '....', '....', '....'],
  },
]

export function presetPattern(p: Preset): Pattern {
  return parsePattern(p.fill ? withFill(p.rows, p.fill) : p.rows)
}
