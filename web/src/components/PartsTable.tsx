import type { PartInfo } from '../audio/protocol'

const KINDS = [
  'カーボン抵抗',
  'フィルムコンデンサ',
  'セラミックコンデンサ',
  '電解コンデンサ',
  'ダイオード',
  'ボリューム',
]

const PREFIX: [number, string][] = [
  [1e6, 'M'],
  [1e3, 'k'],
  [1, ''],
  [1e-3, 'm'],
  [1e-6, 'µ'],
  [1e-9, 'n'],
  [1e-12, 'p'],
]

function si(v: number, unit: string) {
  if (v === 0) return `0 ${unit}`
  const [scale, p] = PREFIX.find(([s]) => Math.abs(v) >= s * 0.9995) ?? PREFIX[PREFIX.length - 1]
  const n = v / scale
  return `${n >= 100 ? n.toFixed(0) : n >= 10 ? n.toFixed(1) : n.toFixed(2)} ${p}${unit}`
}

function unitOf(kind: number) {
  if (kind === 4) return 'A (Is)'
  if (kind >= 1 && kind <= 3) return 'F'
  return 'Ω'
}

type Props = { parts: PartInfo[]; onScale: (index: number, scale: number) => void }

export function PartsTable({ parts, onScale }: Props) {
  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>部品</th>
            <th>種類</th>
            <th>公称</th>
            <th>この個体</th>
            <th>ずれ</th>
            <th>倍率</th>
          </tr>
        </thead>
        <tbody>
          {parts.map((p, i) => {
            const dev = p.nominal === 0 ? 0 : (p.effective / p.nominal - 1) * 100
            const logScale = Math.log2(p.scale)
            return (
              <tr key={p.name} className={p.scale !== 1 ? 'edited' : ''}>
                <td>{p.name}</td>
                <td>{KINDS[p.kind]}</td>
                <td className="num">{si(p.nominal, unitOf(p.kind))}</td>
                <td className="num">{si(p.effective, unitOf(p.kind))}</td>
                <td className={`num ${Math.abs(dev) > 10 ? 'far' : ''}`}>
                  {dev >= 0 ? '+' : ''}
                  {dev.toFixed(1)}%
                </td>
                <td className="scale">
                  <input
                    type="range"
                    aria-label={`${p.name} scale`}
                    min={-2}
                    max={2}
                    step={0.05}
                    value={logScale}
                    onInput={(e) => onScale(i, 2 ** Number(e.currentTarget.value))}
                  />
                  <span>×{p.scale.toFixed(2)}</span>
                  {p.scale !== 1 && <button onClick={() => onScale(i, 1)}>戻す</button>}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
