import { SLOTS, STEPS } from '../audio/protocol'
import { CHANNELS, EXTRA_FROM, EXTRA_SLOTS, soundOf } from '../music/channels'
import type { Pattern } from '../music/pattern'
import { SwitchToggle } from './SwitchToggle'

type Props = {
  pattern: Pattern
  /** Playing step index, or −1. */
  step: number
  /** Slot flashed by an audition or key press, or −1. */
  hit: number
  keys: string[]
  extraOpen: boolean
  onToggleExtra: () => void
  onEdit: (slot: number, i: number, roll: boolean) => void
  onTrigger: (slot: number) => void
  onSwitch: (slot: number) => void
}

const GROUP_NAME = 'タム・リム・シンバル'

export function StepGrid({ pattern, step, hit, keys, extraOpen, onToggleExtra, onEdit, onTrigger, onSwitch }: Props) {
  const bars = pattern.length / STEPS
  const extraNames = EXTRA_SLOTS.map((s) => soundOf(s, pattern.sw).name)

  const row = (bar: number, s: number) => {
    const sound = soundOf(s, pattern.sw)
    const sw = CHANNELS[s].alt
    return (
      <div className={`row ${sw ? 'switchable' : ''}`} key={s}>
        <div className="label">
          <button
            type="button"
            className={`pad ${hit === s ? 'hit' : ''}`}
            onPointerDown={() => onTrigger(s)}
            title={`${sound.name}を鳴らす（キー ${keys[s]}）`}
          >
            <b>{sound.code}</b>
            <span>{sound.name}</span>
          </button>
          {sw && <SwitchToggle slot={s} value={pattern.sw[s]} onToggle={() => onSwitch(s)} />}
        </div>
        {Array.from({ length: STEPS }, (_, k) => {
          const i = bar * STEPS + k
          const v = pattern.vel[s][i]
          const roll = pattern.rolls[s][i]
          return (
            <button
              type="button"
              key={i}
              aria-label={`${sound.name} ${bar + 1} 小節目 ${k + 1} 番目`}
              className={`cell c${v} ${i === step ? 'now' : ''} ${k % 4 === 0 ? 'beat' : ''}`}
              onClick={(e) => onEdit(s, i, e.shiftKey)}
              onContextMenu={(e) => {
                e.preventDefault()
                onEdit(s, i, true)
              }}
            >
              {v > 0 && roll > 1 ? roll : ''}
            </button>
          )
        })}
      </div>
    )
  }

  const groupHead = (bar: number) => (
    <button
      type="button"
      className={`row group-head ${extraOpen ? 'open' : ''}`}
      aria-expanded={extraOpen}
      onClick={onToggleExtra}
      title={extraOpen ? `${GROUP_NAME}の行をたたむ` : `${extraNames.join('・')}の行を表示`}
    >
      <span className="label">
        <span className="chev" aria-hidden="true">
          {extraOpen ? '▾' : '▸'}
        </span>
        <span className="long">{GROUP_NAME}</span>
        <span className="short">タム他</span>
      </span>
      {/* Folded: one dot per step where any of the hidden rows plays, so nothing is out of sight unnoticed. */}
      {!extraOpen &&
        Array.from({ length: STEPS }, (_, k) => {
          const i = bar * STEPS + k
          const on = EXTRA_SLOTS.some((s) => pattern.vel[s][i] > 0)
          return <span key={k} className={`peek ${on ? 'on' : ''} ${i === step ? 'now' : ''}`} aria-hidden="true" />
        })}
      {extraOpen && <span className="hint">たたむ</span>}
    </button>
  )

  return (
    <div className="grid">
      {Array.from({ length: bars }, (_, bar) => (
        <div className={`bar bar${bar + 1}`} key={bar}>
          {bars > 1 && <div className="bar-label">{bar + 1} 小節目</div>}
          {SLOTS.map((_, s) => s)
            .filter((s) => s < EXTRA_FROM)
            .map((s) => row(bar, s))}
          {groupHead(bar)}
          {extraOpen && EXTRA_SLOTS.map((s) => row(bar, s))}
        </div>
      ))}
    </div>
  )
}
