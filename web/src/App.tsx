import { useCallback, useEffect, useRef, useState } from 'preact/hooks'
import { startEngine, type AudioEngine, type LatencyMode } from './audio/engine'
import { COMMON, STEPS, VOICES, slotVoice, type PartInfo, type ToWorklet } from './audio/protocol'
import { midiFrame, MIDI_MARGIN_MS, parseRealtime } from './audio/midi'
import { Knob } from './components/Knob'
import { PartsTable } from './components/PartsTable'
import { StepGrid } from './components/StepGrid'
import { SwitchToggle } from './components/SwitchToggle'
import {
  CHANNELS,
  completeKnobs,
  defaultKnobs,
  EXTRA_SLOTS,
  KNOBS,
  soundOf,
  SWITCH_SLOTS,
  TUNING_IDS,
  type KnobDef,
} from './music/channels'
import {
  arrange,
  copyBar1ToBar2,
  emptyPattern,
  parsePattern,
  patternToText,
  usesExtra,
  type Pattern,
} from './music/pattern'
import { PRESETS, presetPattern, type Preset } from './music/presets'
import { decodeShare, encodeShare } from './music/share'
import { DEFAULT_UNIT, drawUnit, type UnitState } from './music/units'
import './App.css'

/** Keys per slot, along the home row: A S D F G H for the original six, then J K L ; ' for LT MT HT RS CY. */
const SLOT_KEYS = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', "'"]
const KEY_LABELS = SLOT_KEYS.map((k) => k.toUpperCase())
// Physical positions for the punctuation keys, which differ by layout (JIS has : where US has ').
const CODE_SLOTS: Record<string, number> = { Semicolon: 9, Quote: 10 }

/** Engine voice → the slot whose name and switch its knob card shows (HH shows as CH/OH). */
const VOICE_SLOT = VOICES.map((_, v) => (v <= 2 ? v : v + 1))
const VOICE_TITLE = ['バスドラム', 'スネア', 'ハイハット']
/** Engine voice of the clap channel (CP/MA). */
const CLAP_VOICE = 3

function knobsFor(preset: Preset): number[][] {
  const ks = defaultKnobs()
  for (const [v, id, value] of preset.knobs ?? []) {
    const i = KNOBS[v].findIndex((k) => k.id === id)
    if (i >= 0) ks[v][i] = value
  }
  return ks
}

const shared = decodeShare(window.location.hash)
const sharedPattern = shared && parsePattern(shared.rows, shared.sw, shared.length)

const EXTRA_KEY = 'solder808.extraRows'
function savedExtraOpen(): boolean {
  try {
    return localStorage.getItem(EXTRA_KEY) === 'open'
  } catch {
    return false
  }
}

/** First added channel the pattern plays, for the knob card to show. */
function firstExtra(p: Pattern) {
  return EXTRA_SLOTS.find((s) => p.vel[s].slice(0, p.length).some((v) => v > 0)) ?? EXTRA_SLOTS[0]
}

const LATENCY_KEY = 'solder808.latency'
function savedLatency(): LatencyMode {
  try {
    const v = localStorage.getItem(LATENCY_KEY)
    if (v === 'interactive' || v === 'balanced' || v === 'playback') return v
  } catch {
    // Storage unavailable (private mode etc.): use the default.
  }
  return 'interactive'
}

/** 'auto' renders at 2x and drops to 1x for the rest of the session if the device cannot keep up. */
type Quality = 'auto' | 'high' | 'light'
const QUALITY_KEY = 'solder808.quality'
function savedQuality(): Quality {
  try {
    const v = localStorage.getItem(QUALITY_KEY)
    if (v === 'auto' || v === 'high' || v === 'light') return v
  } catch {
    // Storage unavailable: use the default.
  }
  return 'auto'
}
// Auto quality steps down after this many consecutive stats reports (0.5 s each) above AUTO_LOAD.
const AUTO_LOAD = 0.8
const AUTO_REPORTS = 4
// Load right after starting, a reset, a quality change or returning to the tab is not representative.
const SETTLE_MS = 1000
// The parts table is refreshed this long after the last knob, unit or part change.
const PARTS_DEBOUNCE_MS = 150

const footer = (
  <footer className="site-footer">
    <a href="https://github.com/tainakanchu/Solder808" target="_blank" rel="noopener noreferrer">
      GitHub
    </a>
  </footer>
)

export default function App() {
  const engineRef = useRef<AudioEngine | null>(null)
  const [status, setStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle')
  const [error, setError] = useState('')
  const [presetId, setPresetId] = useState<string | null>(shared?.preset ?? null)
  const [variant, setVariant] = useState(shared?.variant ?? 0)
  const [pattern, setPattern] = useState<Pattern>(() => sharedPattern ?? presetPattern(PRESETS[0]))
  const [bpm, setBpm] = useState(shared?.bpm ?? PRESETS[0].bpm)
  const [swing, setSwing] = useState(shared?.swing ?? PRESETS[0].swing)
  const [unit, setUnit] = useState<UnitState>(shared?.unit ?? DEFAULT_UNIT)
  const [knobs, setKnobs] = useState<number[][]>(() => (shared ? completeKnobs(shared.knobs) : defaultKnobs()))
  // The added rows (LT MT HT RS CY) fold away; they open by themselves when the pattern uses them.
  const [extraOpen, setExtraOpen] = useState(
    () => savedExtraOpen() || (sharedPattern ? usesExtra(sharedPattern) : false),
  )
  const [knobSlot, setKnobSlot] = useState(() => firstExtra(sharedPattern ?? presetPattern(PRESETS[0])))
  const [master, setMaster] = useState(0.8)
  const [accent, setAccent] = useState(shared?.accent ?? 0.8)
  const [playing, setPlaying] = useState(false)
  const [step, setStep] = useState(-1)
  const [parts, setParts] = useState<PartInfo[][]>([])
  const [partsVoice, setPartsVoice] = useState(0)
  const [hit, setHit] = useState(-1)
  const [copied, setCopied] = useState(false)
  const [load, setLoad] = useState({ load: 0, peakLoad: 0 })
  const [latencyMs, setLatencyMs] = useState(0)
  const [latencyMode] = useState<LatencyMode>(savedLatency)
  // Bumped when the worklet rebuilds its engine, so every setting is resent.
  const [gen, setGen] = useState(0)
  const [ctxState, setCtxState] = useState<AudioContextState | 'stalled'>('running')
  const [crash, setCrash] = useState('')
  const partsRef = useRef<PartInfo[][]>([])
  // 'internal' or the id of the MIDI input whose clock is followed.
  const [syncInput, setSyncInput] = useState('internal')
  const [midiInputs, setMidiInputs] = useState<{ id: string; name: string }[] | 'denied' | null>(null)
  const [clockBpm, setClockBpm] = useState(-1)
  const midiRef = useRef<MIDIAccess | null>(null)
  const syncRef = useRef('internal')
  const following = syncInput !== 'internal'
  const [quality, setQuality] = useState<Quality>(savedQuality)
  // Auto quality stepped down this session; it does not step back up on its own.
  const [autoLight, setAutoLight] = useState(false)
  const [lightNotice, setLightNotice] = useState(false)
  // The engine's oversampling as last reported: 0 = cannot switch, -1 = no report yet.
  const [oversample, setOversample] = useState(-1)
  const factor = quality === 'light' || (quality === 'auto' && autoLight) ? 1 : 2
  const [partsOpen, setPartsOpen] = useState(false)
  // Bumped by part edits so the parts table is refreshed from the engine.
  const [partEdits, setPartEdits] = useState(0)
  // Read by the worklet message handler, which is set up once.
  const liveRef = useRef({ quality, autoLight, playing: false })
  const hotReportsRef = useRef(0)
  const settleUntilRef = useRef(0)
  const settle = () => {
    settleUntilRef.current = performance.now() + SETTLE_MS
    hotReportsRef.current = 0
  }

  const send = useCallback((m: ToWorklet) => engineRef.current?.send(m), [])

  /** Auto quality: step down to 1x once the load stays above AUTO_LOAD while playing. */
  const watchLoad = (load: number, engineFactor: number) => {
    const live = liveRef.current
    const counting =
      live.quality === 'auto' &&
      !live.autoLight &&
      engineFactor === 2 &&
      live.playing &&
      document.visibilityState === 'visible' &&
      performance.now() >= settleUntilRef.current
    hotReportsRef.current = counting && load > AUTO_LOAD ? hotReportsRef.current + 1 : 0
    if (hotReportsRef.current < AUTO_REPORTS) return
    hotReportsRef.current = 0
    liveRef.current = { ...live, autoLight: true }
    setAutoLight(true)
    setLightNotice(true)
  }

  const chooseQuality = (q: Quality) => {
    try {
      localStorage.setItem(QUALITY_KEY, q)
    } catch {
      // Without storage the choice lasts until reload.
    }
    setQuality(q)
    setLightNotice(false)
  }

  const ensureEngine = async () => {
    if (engineRef.current) return true
    setStatus('loading')
    try {
      const eng = await startEngine(latencyMode)
      // The worklet runs ahead of the speakers by the output latency; delay
      // the step highlight by the same amount so it lines up with the sound.
      const latency = () => ((eng.ctx.baseLatency || 0) + (eng.ctx.outputLatency || 0)) * 1000
      setLatencyMs(Math.round(latency()))
      let lastStats = 0
      eng.onMessage((m) => {
        if (m.type === 'step') window.setTimeout(() => setStep(m.index), latency())
        if (m.type === 'parts') {
          partsRef.current = m.voices
          setParts(m.voices)
        }
        if (m.type === 'stats') {
          lastStats = performance.now()
          setLoad({ load: m.load, peakLoad: m.peakLoad })
          setClockBpm(m.clockBpm)
          setLatencyMs(Math.round(latency()))
          setOversample(m.oversample)
          watchLoad(m.load, m.oversample)
        }
        if (m.type === 'transport') setPlaying(m.playing)
        if (m.type === 'error') setCrash(m.message)
        if (m.type === 'reset') {
          partsRef.current.forEach((ps, voice) =>
            ps.forEach((p, index) => p.scale !== 1 && eng.send({ type: 'part', voice, index, scale: p.scale })),
          )
          settle()
          setGen((g) => g + 1)
        }
      })
      // Output device changes, sleep or OS interruptions can stop the context.
      eng.ctx.onstatechange = () => setCtxState(eng.ctx.state)
      // The worklet reports stats every 0.5 s while it runs.
      window.setInterval(() => {
        if (eng.ctx.state === 'running')
          setCtxState(lastStats > 0 && performance.now() - lastStats > 3000 ? 'stalled' : 'running')
      }, 1000)
      engineRef.current = eng
      setStatus('ready')
      return true
    } catch (e) {
      setError(String(e))
      setStatus('error')
      return false
    }
  }

  /** Show the added rows when a newly loaded pattern plays them. */
  const reveal = (p: Pattern) => {
    if (usesExtra(p)) setExtraOpen(true)
    setKnobSlot(firstExtra(p))
  }

  const toggleExtra = () =>
    setExtraOpen((open) => {
      try {
        localStorage.setItem(EXTRA_KEY, open ? 'closed' : 'open')
      } catch {
        // Without storage the choice lasts until reload.
      }
      return !open
    })

  // Editing or arranging that brings in an added row unfolds the group too.
  const extraUsed = usesExtra(pattern)
  const prevExtraUsedRef = useRef(extraUsed)
  useEffect(() => {
    if (extraUsed && !prevExtraUsedRef.current) setExtraOpen(true)
    prevExtraUsedRef.current = extraUsed
  }, [extraUsed])

  const loadPreset = (p: Preset) => {
    const pat = presetPattern(p)
    setPresetId(p.id)
    setVariant(0)
    setPattern(pat)
    reveal(pat)
    setBpm(p.bpm)
    setSwing(p.swing)
    setKnobs(knobsFor(p))
  }

  const playPreset = async (p: Preset) => {
    loadPreset(p)
    if (await ensureEngine()) setPlaying(true)
  }

  const startShared = async () => {
    if (await ensureEngine()) setPlaying(true)
  }

  const startBlank = async () => {
    setPresetId(null)
    setVariant(0)
    setPattern(emptyPattern())
    setKnobs(defaultKnobs())
    setKnobSlot(EXTRA_SLOTS[0])
    await ensureEngine()
  }

  const onArrange = () => {
    const base = PRESETS.find((p) => p.id === presetId)
    const next = variant + 1
    setVariant(next)
    if (base) setPattern((p) => arrange({ ...presetPattern(base), length: p.length }, next))
    else setPattern((p) => arrange(p, next))
  }

  const onMidiMessage = useCallback((ev: MIDIMessageEvent) => {
    const eng = engineRef.current
    const msg = ev.data && parseRealtime(ev.data)
    if (!eng || !msg) return
    if (msg.type === 'spp') {
      eng.send(msg)
      return
    }
    eng.send({ type: 'midi', kind: msg.kind, frame: midiFrame(eng.ctx, ev.timeStamp) })
    if (msg.kind === 'start' || msg.kind === 'continue') setPlaying(true)
    if (msg.kind === 'stop') setPlaying(false)
  }, [])

  const followInput = useCallback(
    (id: string) => {
      midiRef.current?.inputs.forEach((input) => {
        input.onmidimessage = input.id === id ? onMidiMessage : null
      })
      syncRef.current = id
      setSyncInput(id)
    },
    [onMidiMessage],
  )

  const onSyncChange = async (value: string) => {
    if (value !== 'request') {
      followInput(value)
      return
    }
    try {
      const access = await navigator.requestMIDIAccess()
      midiRef.current = access
      const list = () => [...access.inputs.values()].map((i) => ({ id: i.id, name: i.name ?? i.id }))
      const inputs = list()
      let count = inputs.length
      access.onstatechange = () => {
        const now = list()
        const cur = syncRef.current
        // Reattach to the followed input (a reconnected port is a new object); drop it if gone.
        if (cur !== 'internal') followInput(now.some((i) => i.id === cur) ? cur : 'internal')
        else if (count === 0 && now.length === 1) followInput(now[0].id)
        count = now.length
        setMidiInputs(now)
      }
      setMidiInputs(inputs)
      // With several inputs, let the user pick.
      if (inputs.length === 1) followInput(inputs[0].id)
    } catch {
      setMidiInputs('denied')
    }
  }

  useEffect(() => {
    if (status === 'ready') send({ type: 'sync', mode: following ? 'midi' : 'internal' })
  }, [status, following, send, gen])

  // Keep the share URL in sync with what is playing.
  useEffect(() => {
    if (status !== 'ready') return
    const hash = `#s=${encodeShare({
      preset: presetId,
      variant,
      rows: patternToText(pattern),
      length: pattern.length,
      sw: pattern.sw,
      bpm,
      swing,
      unit,
      knobs,
      accent,
    })}`
    window.history.replaceState(null, '', hash)
  }, [status, presetId, variant, pattern, bpm, swing, unit, knobs, accent])

  useEffect(() => {
    if (status === 'ready') send({ type: 'unit', seed: unit.seed, age: unit.age, temp: unit.temp })
  }, [status, unit, send, gen])

  // Knobs as last sent to the engine of generation `gen`: a drag sends only the knob
  // it moves, while a new engine (start or reset) gets every value.
  const sentKnobsRef = useRef<{ gen: number; knobs: number[][] } | null>(null)
  useEffect(() => {
    if (status !== 'ready') return
    const sent = sentKnobsRef.current?.gen === gen ? sentKnobsRef.current.knobs : null
    knobs.forEach((vals, v) =>
      vals.forEach((value, i) => {
        if (sent?.[v]?.[i] !== value) send({ type: 'knob', voice: v, id: KNOBS[v][i].id, value })
      }),
    )
    sentKnobsRef.current = { gen, knobs }
  }, [status, knobs, send, gen])

  // Reading the parts walks every part of every voice, so it only happens while the
  // table is open: at once when it opens, then after changes settle.
  const partsShownRef = useRef(false)
  useEffect(() => {
    if (status !== 'ready' || !partsOpen) {
      partsShownRef.current = false
      return
    }
    const delay = partsShownRef.current ? PARTS_DEBOUNCE_MS : 0
    partsShownRef.current = true
    const t = window.setTimeout(() => send({ type: 'getParts' }), delay)
    return () => window.clearTimeout(t)
  }, [status, partsOpen, knobs, unit, partEdits, send, gen])

  const setPartScale = (voice: number, index: number, scale: number) => {
    send({ type: 'part', voice, index, scale })
    // Show the new value right away; the engine's own reading follows after the debounce.
    const next = partsRef.current.map((ps, v) =>
      v !== voice
        ? ps
        : ps.map((p, i) => (i === index ? { ...p, scale, effective: (p.effective * scale) / p.scale } : p)),
    )
    partsRef.current = next
    setParts(next)
    setPartEdits((n) => n + 1)
  }

  useEffect(() => {
    if (status !== 'ready') return
    send({ type: 'oversample', factor })
    settle()
  }, [status, factor, send, gen])

  useEffect(() => {
    liveRef.current = { quality, autoLight, playing }
    if (playing) settle()
  }, [quality, autoLight, playing])

  useEffect(() => {
    const onVisibility = () => document.visibilityState === 'visible' && settle()
    document.addEventListener('visibilitychange', onVisibility)
    return () => document.removeEventListener('visibilitychange', onVisibility)
  }, [])

  // Sound switches; a string key so cell edits do not resend them.
  const swKey = SWITCH_SLOTS.map((s) => pattern.sw[s]).join('')
  useEffect(() => {
    if (status !== 'ready') return
    SWITCH_SLOTS.forEach((s, k) => send({ type: 'switch', voice: slotVoice(s), value: Number(swKey[k]) }))
  }, [status, swKey, send, gen])

  useEffect(() => {
    if (status === 'ready') send({ type: 'master', value: master })
  }, [status, master, send, gen])

  useEffect(() => {
    if (status === 'ready') send({ type: 'accent', value: accent })
  }, [status, accent, send, gen])

  useEffect(() => {
    if (status === 'ready')
      send({ type: 'seq', vel: pattern.vel, rolls: pattern.rolls, length: pattern.length, bpm, swing, playing })
  }, [status, pattern, bpm, swing, playing, send, gen])

  const trigger = useCallback(
    (slot: number, accent = 0.6) => {
      send({ type: 'trigger', slot, accent })
      setHit(slot)
      window.setTimeout(() => setHit((h) => (h === slot ? -1 : h)), 120)
    },
    [send],
  )

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (
        ev.repeat ||
        ev.target instanceof HTMLInputElement ||
        ev.target instanceof HTMLSelectElement ||
        status !== 'ready'
      )
        return
      if (ev.ctrlKey || ev.metaKey || ev.altKey) return
      let slot = SLOT_KEYS.indexOf(ev.key.toLowerCase())
      if (slot < 0) slot = CODE_SLOTS[ev.code] ?? -1
      if (slot >= 0) trigger(slot, ev.shiftKey ? 1 : 0.6)
      if (ev.key === ' ') {
        ev.preventDefault()
        setPlaying((p) => !p)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [trigger, status])

  const setKnob = (v: number, i: number, value: number) =>
    setKnobs((ks) => ks.map((vals, vi) => (vi === v ? vals.map((x, xi) => (xi === i ? value : x)) : vals)))

  const editCell = (s: number, i: number, roll: boolean) =>
    setPattern((p) => {
      const vel = p.vel.map((row) => [...row])
      const rolls = p.rolls.map((row) => [...row])
      if (roll) {
        if (vel[s][i] === 0) vel[s][i] = 1
        rolls[s][i] = (rolls[s][i] % 4) + 1
      } else {
        vel[s][i] = (vel[s][i] + 1) % 3
        if (vel[s][i] === 0) rolls[s][i] = 1
      }
      return { ...p, vel, rolls }
    })

  const toggleSwitch = (slot: number) => setPattern((p) => ({ ...p, sw: p.sw.map((v, s) => (s === slot ? 1 - v : v)) }))

  const audition = (slot: number) => {
    trigger(slot)
    if (EXTRA_SLOTS.includes(slot)) setKnobSlot(slot)
  }

  const knob = (v: number, k: KnobDef, extra?: { label?: string; className?: string }) => {
    const i = KNOBS[v].indexOf(k)
    return (
      <Knob
        key={k.id}
        label={extra?.label ?? k.label}
        className={extra?.className}
        value={knobs[v][i]}
        onChange={(val) => setKnob(v, i, val)}
      />
    )
  }

  /** Toms put the tuning of the selected sound first; the other stays reachable, dimmed. */
  const voiceKnobs = (v: number) => {
    const slot = VOICE_SLOT[v]
    const ks = KNOBS[v]
    const tom = ks.find((k) => k.id === TUNING_IDS.tom)
    const cga = ks.find((k) => k.id === TUNING_IDS.conga)
    // Clap channel on maracas: Tail only shapes the clap, so keep it reachable but dimmed.
    if (v === CLAP_VOICE && pattern.sw[slot] === 1)
      return ks.map((k) =>
        k.id === 0 ? knob(v, k) : knob(v, k, { label: `${k.label}（クラップ時）`, className: 'secondary' }),
      )
    if (!tom || !cga || !CHANNELS[slot].alt) return ks.map((k) => knob(v, k))
    const conga = pattern.sw[slot] === 1
    const [on, off] = conga ? [cga, tom] : [tom, cga]
    return [
      ...ks.filter((k) => k !== tom && k !== cga).map((k) => knob(v, k)),
      knob(v, on),
      knob(v, off, { label: `${off.label}（${conga ? 'タム' : 'コンガ'}時）`, className: 'secondary' }),
    ]
  }

  const gacha = () => setUnit(drawUnit(Math.floor(Math.random() * 1_000_000)))

  const copyLink = async () => {
    await navigator.clipboard.writeText(window.location.href)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1500)
  }

  const presetCards = (compact: boolean) => (
    <div className={compact ? 'preset-strip' : 'preset-grid'}>
      {PRESETS.map((p) => (
        <button
          key={p.id}
          className={`preset ${p.id === presetId && status === 'ready' ? 'active' : ''}`}
          onClick={() => playPreset(p)}
        >
          <strong>{p.name}</strong>
          <small>{p.bpm} BPM</small>
        </button>
      ))}
    </div>
  )

  const loadNote = [oversample === 1 && '軽量', load.peakLoad >= 0 && `瞬間 ${Math.round(load.peakLoad * 100)}%`]
    .filter(Boolean)
    .join('・')

  if (status !== 'ready') {
    return (
      <main className="splash">
        <h1>Solder808</h1>
        {shared && (
          <button className="primary shared" onClick={startShared} disabled={status === 'loading'}>
            共有されたビートを再生
          </button>
        )}
        {presetCards(false)}
        <button className="ghost" onClick={startBlank} disabled={status === 'loading'}>
          空のパターン
        </button>
        {status === 'loading' && <p className="lead">起動中…</p>}
        {status === 'error' && <p className="error">{error}</p>}
        {footer}
      </main>
    )
  }

  return (
    <main className="app">
      <header className="top">
        <h1>Solder808</h1>
        <div className="unit-card">
          <strong>{unit.name}</strong>
          <span className="unit-meta">
            経年 {unit.age} 年・{unit.temp} °C・#{unit.seed}
          </span>
        </div>
        <div className="top-actions">
          <button onClick={gacha}>個体ガチャ</button>
          <button onClick={copyLink}>{copied ? 'コピーしました' : '共有リンクをコピー'}</button>
        </div>
      </header>

      {presetCards(true)}

      <section className="seq">
        <div className="transport">
          <button className="primary play" onClick={() => setPlaying((p) => !p)}>
            {playing ? '■ 止める' : '▶ 再生'}
          </button>
          <button onClick={onArrange}>アレンジ{variant > 0 ? ` #${variant}` : ''}</button>
          {following ? (
            <span className="clock-bpm">テンポ {clockBpm > 0 ? `${clockBpm.toFixed(1)} BPM` : '-'}</span>
          ) : (
            <Knob label="テンポ" value={bpm} min={60} max={180} step={1} format={(v) => `${v} BPM`} onChange={setBpm} />
          )}
          {'requestMIDIAccess' in navigator && (
            <label className="sync">
              同期
              <select value={syncInput} onChange={(e) => void onSyncChange(e.currentTarget.value)}>
                <option value="internal">内部</option>
                {midiInputs === null ? (
                  <option value="request">MIDI クロック</option>
                ) : midiInputs === 'denied' ? (
                  <option value="none" disabled>
                    MIDI が許可されていません
                  </option>
                ) : midiInputs.length === 0 ? (
                  <option value="none" disabled>
                    MIDI 入力なし
                  </option>
                ) : (
                  midiInputs.map((i) => (
                    <option key={i.id} value={i.id}>
                      {i.name}
                    </option>
                  ))
                )}
              </select>
            </label>
          )}
          <Knob
            label="跳ね"
            value={swing}
            min={0}
            max={0.3}
            step={0.01}
            format={(v) => `${Math.round(v * 100)}%`}
            onChange={setSwing}
          />
          <Knob label="アクセント" value={accent} onChange={setAccent} />
          <Knob label="音量" value={master} onChange={setMaster} />
          <div className="bars" role="group" aria-label="小節数">
            {[1, 2].map((n) => (
              <button
                key={n}
                className={pattern.length === n * STEPS ? 'active' : ''}
                onClick={() => setPattern((p) => ({ ...p, length: n * STEPS }))}
              >
                {n} 小節
              </button>
            ))}
          </div>
          <span className={`meter ${load.peakLoad > 0.9 || load.load > 0.7 ? 'hot' : ''}`}>
            負荷 {Math.round(load.load * 100)}%{loadNote && `（${loadNote}）`}
            ・遅延 {following ? latencyMs + MIDI_MARGIN_MS : latencyMs} ms
          </span>
          {lightNotice && (
            <span className="notice" role="status">
              処理が追いつかないため軽量モードに切り替えました
              <button onClick={() => chooseQuality('high')}>高品質に戻す</button>
              <button className="ghost" onClick={() => setLightNotice(false)}>
                閉じる
              </button>
            </span>
          )}
          {status === 'ready' && ctxState !== 'running' && (
            <span className="stopped">
              {ctxState === 'stalled' ? '音の計算が止まっています' : `音の出力が止まりました（${ctxState}）`}
              <button onClick={() => void engineRef.current?.ctx.resume()}>音を再開</button>
            </span>
          )}
          {crash && (
            <span className="stopped" title={crash}>
              計算エラーから復帰しました
              <button onClick={() => setCrash('')}>閉じる</button>
            </span>
          )}
          {pattern.length > STEPS && (
            <button onClick={() => setPattern(copyBar1ToBar2)}>1 小節目を 2 小節目に写す</button>
          )}
        </div>
        <StepGrid
          pattern={pattern}
          step={playing ? step : -1}
          hit={hit}
          keys={KEY_LABELS}
          extraOpen={extraOpen}
          onToggleExtra={toggleExtra}
          onEdit={editCell}
          onTrigger={audition}
          onSwitch={toggleSwitch}
        />
      </section>

      <section className="voices">
        {VOICES.map((code, v) => {
          const slot = VOICE_SLOT[v]
          if (EXTRA_SLOTS.includes(slot)) return null
          const sound = soundOf(slot, pattern.sw)
          return (
            <div className="voice" key={code}>
              <h2>
                {v === 2 ? code : sound.code} <span>{VOICE_TITLE[v] ?? sound.name}</span>
                <SwitchToggle slot={slot} value={pattern.sw[slot]} onToggle={() => toggleSwitch(slot)} />
              </h2>
              {sound !== CHANNELS[slot] && <p className="voice-note">レベルは{CHANNELS[slot].name}と共通</p>}
              <div className="knobs">{voiceKnobs(v)}</div>
            </div>
          )
        })}
        <div className="voice extra-voices">
          <h2>タム・リム・シンバル</h2>
          <div className="voice-tabs" role="tablist" aria-label="つまみを出す音">
            {EXTRA_SLOTS.map((s) => (
              <button
                type="button"
                key={s}
                role="tab"
                aria-selected={s === knobSlot}
                className={s === knobSlot ? 'active' : ''}
                onClick={() => setKnobSlot(s)}
              >
                {soundOf(s, pattern.sw).code}
              </button>
            ))}
          </div>
          <div className="voice-sub">
            <strong>{soundOf(knobSlot, pattern.sw).name}</strong>
            <SwitchToggle slot={knobSlot} value={pattern.sw[knobSlot]} onToggle={() => toggleSwitch(knobSlot)} />
          </div>
          <div className="knobs">{voiceKnobs(slotVoice(knobSlot))}</div>
        </div>
      </section>

      <details className="parts" onToggle={(e) => setPartsOpen(e.currentTarget.open)}>
        <summary>詳細設定</summary>
        <div className="unit-controls">
          <label>
            シード
            <input
              type="number"
              value={unit.seed}
              min={0}
              onInput={(e) => {
                const seed = Number(e.currentTarget.value) | 0
                setUnit((u) => ({ ...u, seed, name: `手動で選んだ個体 #${seed}` }))
              }}
            />
          </label>
          <Knob
            label="経年"
            value={unit.age}
            min={0}
            max={50}
            step={1}
            format={(v) => `${v} 年`}
            onChange={(age) => setUnit((u) => ({ ...u, age, name: '手動で調整した個体' }))}
          />
          <Knob
            label="温度"
            value={unit.temp}
            min={0}
            max={45}
            step={1}
            format={(v) => `${v} °C`}
            onChange={(temp) => setUnit((u) => ({ ...u, temp, name: '手動で調整した個体' }))}
          />
          <button onClick={() => setUnit(DEFAULT_UNIT)}>工場出荷に戻す</button>
          <label>
            出力バッファ
            <select
              value={latencyMode}
              onChange={(e) => {
                try {
                  localStorage.setItem(LATENCY_KEY, e.currentTarget.value)
                } catch {
                  // Without storage the choice cannot survive the reload.
                }
                window.location.reload()
              }}
            >
              <option value="interactive">低遅延</option>
              <option value="balanced">標準</option>
              <option value="playback">安定</option>
            </select>
          </label>
          <label>
            音質
            <select
              className="quality"
              value={quality}
              disabled={oversample === 0}
              title={oversample === 0 ? 'この音の計算エンジンでは切り替えられません' : undefined}
              onChange={(e) => chooseQuality(e.currentTarget.value as Quality)}
            >
              <option value="auto">{autoLight ? '自動（いまは軽量）' : '自動（おすすめ）'}</option>
              <option value="high">高品質（2 倍オーバーサンプル）</option>
              <option value="light">軽量</option>
            </select>
          </label>
        </div>
        <div className="tabs">
          {VOICES.map((name, v) => (
            <button key={name} className={v === partsVoice ? 'active' : ''} onClick={() => setPartsVoice(v)}>
              {name}
            </button>
          ))}
          <button className={partsVoice === COMMON ? 'active' : ''} onClick={() => setPartsVoice(COMMON)}>
            共通
          </button>
        </div>
        {(parts[partsVoice]?.length ?? 0) === 0 && (
          <p className="voice-note">この音色の部品は、音の計算エンジンが対応すると表示されます。</p>
        )}
        <PartsTable
          parts={parts[partsVoice] ?? []}
          onScale={(index, scale) => setPartScale(partsVoice, index, scale)}
        />
      </details>
      {footer}
    </main>
  )
}
