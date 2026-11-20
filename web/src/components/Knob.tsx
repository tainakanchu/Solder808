import { useId } from 'preact/hooks'

type Props = {
  label: string
  value: number
  min?: number
  max?: number
  step?: number
  format?: (v: number) => string
  onChange: (v: number) => void
  className?: string
}

export function Knob({ label, value, min = 0, max = 1, step = 0.01, format, onChange, className }: Props) {
  const id = useId()
  return (
    <div className={className ? `knob ${className}` : 'knob'}>
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onInput={(e) => onChange(Number(e.currentTarget.value))}
      />
      <output htmlFor={id}>{format ? format(value) : value.toFixed(2)}</output>
    </div>
  )
}
