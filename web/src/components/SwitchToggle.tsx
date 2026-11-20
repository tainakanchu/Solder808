import { CHANNELS } from '../music/channels'

type Props = { slot: number; value: number; onToggle: () => void }

/** The panel's two-way sound switch as one tap target: both sounds are shown, the selected one filled. */
export function SwitchToggle({ slot, value, onToggle }: Props) {
  const c = CHANNELS[slot]
  if (!c.alt) return null
  const [cur, next] = value ? [c.alt, c] : [c, c.alt]
  return (
    <button
      type="button"
      className="sw"
      aria-pressed={value === 1}
      aria-label={`${c.name}／${c.alt.name}の切り替え（今は${cur.name}）`}
      title={`${cur.name} → ${next.name} に切り替え`}
      onClick={onToggle}
    >
      <span className={value ? '' : 'on'}>{c.code}</span>
      <span className={value ? 'on' : ''}>{c.alt.code}</span>
    </button>
  )
}
