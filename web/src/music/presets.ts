import { parsePattern, rowsToText, withFill, type Pattern, type PatternRows } from './pattern'

export type Preset = {
  id: string
  name: string
  bpm: number
  swing: number
  /**
   * Bar 1 per sound code (BD SD CH OH CP/MA CB LT/LC MT/MC HT/HC RS/CL CY), 16 chars;
   * 32 chars give both bars. Writing `LC`, `MA` etc. sets that channel's switch.
   */
  rows: PatternRows
  /** Last 4 steps of bar 2 (the fill), one 4-char string per sound code. */
  fill?: PatternRows
  /** Optional knob overrides: [voice, knob id, value]. */
  knobs?: [number, number, number][]
}

export const PRESETS: Preset[] = [
  {
    id: 'house',
    name: '4 つ打ちハウス',
    bpm: 124,
    swing: 0.06,
    rows: {
      BD: 'X...x...X...x...',
      CH: 'x...x...x...x...',
      OH: '..x...x...x...x.',
      CP: '....X.......X...',
      // Two conga touches against the off-beat hat.
      HC: '...x.......x....',
    },
    fill: { BD: 'x...', SD: '..xx', CH: 'x...', OH: '..x.', CP: 'X.x.' },
  },
  {
    id: 'ppap',
    name: 'PPAP',
    bpm: 136,
    swing: 0,
    rows: {
      BD: 'x.......x.......x.....x...x.x...',
      SD: '....x.......x.......x.......x...',
      CH: 'x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.',
      CB: '....x.......x.......x.......x...',
    },
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
    rows: {
      BD: 'X.....x.x.....x.',
      SD: '....X.......X...',
      CH: 'x.xxx.xxx.xxx.x.',
      CP: '....x.......x...',
      CB: 'x..x..x...x..x..',
      // Tom stabs answering the cowbell.
      HT: '.........x......',
      LT: '...........x....',
    },
    fill: { BD: 'x.x.', SD: 'X...', CH: 'x.x.', CP: 'x...', HT: '.x..', MT: '..x.', LT: '...X' },
    knobs: [[5, 1, 0.35]],
  },
  {
    id: 'miami',
    name: 'マイアミベース',
    bpm: 130,
    swing: 0,
    rows: {
      BD: 'X.....x.x.x.....',
      SD: '....X.......X...',
      CH: 'x.x.x.x.x.x.x.x.',
      CP: '....X.......X.x.',
      // The mid channel switched to conga; the low and high stay toms for the fill.
      MC: '......x....x....',
      CB: '..........x.....',
    },
    fill: { BD: 'x..x', SD: 'X...', CH: 'x.x.', HT: '.x..', MC: '..x.', LT: '...X' },
    knobs: [
      [0, 3, 0.95],
      [5, 1, 0.3],
    ],
  },
  {
    id: 'boombap',
    name: 'ブームバップ',
    bpm: 90,
    swing: 0.18,
    rows: {
      BD: 'X......x..x.....',
      SD: '....X.......X...',
      CH: 'X.x.X.x.X.x.X.x.',
      // Rim shot ghost notes between the hats.
      RS: '.......x.....x..',
    },
    fill: { BD: 'x..x', SD: 'X..x', CH: 'X.x.', RS: '..x.' },
  },
  {
    id: 'trap',
    name: 'トラップ',
    bpm: 140,
    swing: 0,
    rows: {
      BD: 'X.....x...x.x...',
      SD: '........X.......',
      CH: 'x.x.x.x.x3x.x4x.',
      CP: '........X.......',
    },
    // One short tom roll at the turnaround, nothing more.
    fill: { BD: 'x.x.', SD: '....', CH: '4.3.', HT: '..3.', LT: '...2' },
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
    rows: {
      BD: 'X...x...X...x...',
      SD: '....X.......X...',
      CH: 'xx.xxx.xxx.xxx.x',
      OH: '..x...x...x...x.',
      // Clap switched to maracas as a shaker on the eighths.
      MA: 'x.x.x.x.x.x.x.x.',
      MC: '..x.......x.....',
      HC: '......x.x....x..',
      // Crash only at the top of the two-bar loop.
      CY: 'X...............................',
    },
    fill: { BD: 'x...', SD: 'X.xx', CH: 'xx.x', OH: '..x.', MA: 'x.x.', CL: '.x.x' },
    knobs: [
      [3, 0, 0.55],
      [9, 2, 0.7],
    ],
  },
  {
    id: 'reggaeton',
    name: 'レゲトン',
    bpm: 95,
    swing: 0,
    rows: {
      BD: 'X...x...X...x...',
      SD: '...x..x....x..x.',
      CH: 'x.x.x.x.x.x.x.x.',
      // Claves double the dembow for the wooden click.
      CL: '...x..x....x..x.',
    },
    fill: { BD: 'x...', SD: 'x.xx', CH: 'x.x.', CL: 'x.x.' },
    knobs: [[8, 0, 0.6]],
  },
  {
    id: 'eightbeat',
    name: '8 ビート',
    bpm: 110,
    swing: 0,
    rows: {
      BD: 'X.....x.X.x.....',
      SD: '....X.......X...',
      CH: 'X.x.X.x.X.x.X.x.',
      CY: 'X...............................',
    },
    // Snare, then down the toms into the crash.
    fill: { BD: 'x...', SD: 'X...', HT: '.x..', MT: '..x.', LT: '...X' },
    knobs: [[9, 2, 0.75]],
  },
]

export function presetPattern(p: Preset): Pattern {
  const { text, sw } = rowsToText(p.rows)
  if (!p.fill) return parsePattern(text, sw)
  const fill = rowsToText(p.fill)
  for (let s = 0; s < sw.length; s++)
    if (fill.text[s] && text[s] && fill.sw[s] !== sw[s]) throw new Error(`preset ${p.id}: fill switches slot ${s}`)
  return parsePattern(
    withFill(text, fill.text),
    sw.map((v, s) => v || (text[s] ? 0 : fill.sw[s])),
  )
}
