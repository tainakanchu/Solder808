import { useCallback, useEffect, useRef, useState } from 'preact/hooks'
import { startEngine, type AudioEngine, type LatencyMode } from './audio/engine'
import { COMMON, SLOTS, STEPS, VOICES, type PartInfo, type ToWorklet } from './audio/protocol'
import { midiFrame, MIDI_MARGIN_MS, parseRealtime } from './audio/midi'
import { Knob } from './components/Knob'
import { PartsTable } from './components/PartsTable'
import { arrange, copyBar1ToBar2, emptyPattern, parsePattern, patternToText, type Pattern } from './music/pattern'
import { PRESETS, presetPattern, type Preset } from './music/presets'
import { decodeShare, encodeShare } from './music/share'
import { DEFAULT_UNIT, drawUnit, type UnitState } from './music/units'
import './App.css'

type KnobDef = { id: number; label: string; init: number }

const KNOBS: KnobDef[][] = [
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 2, label: 'Tone', init: 0.5 },
    { id: 3, label: 'Decay', init: 0.6 },
    { id: 1, label: 'Tune trim', init: 0.5 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tone', init: 0.5 },
    { id: 2, label: 'Snappy', init: 0.6 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'OH Decay', init: 0.4 },
  ],
  [
    { id: 0, label: 'Level', init: 0.8 },
    { id: 1, label: 'Tail', init: 0.4 },
  ],
  [
    { id: 0, label: 'Level', init: 0.7 },
    { id: 1, label: 'Decay', init: 0.4 },
  ],
]

const VOICE_NAMES = ['バスドラム', 'スネア', 'ハイハット', 'クラップ', 'カウベル']
const SLOT_NAMES = ['バスドラム', 'スネア', '閉じハット', '開きハット', 'クラップ', 'カウベル']
const SLOT_KEYS = ['a', 's', 'd', 'f', 'g', 'h']

const defaultKnobs = () => KNOBS.map((vs) => vs.map((k) => k.init))

function knobsFor(preset: Preset): number[][] {
  const ks = defaultKnobs()
  for (const [v, id, value] of preset.knobs ?? []) {
    const i = KNOBS[v].findIndex((k) => k.id === id)
    if (i >= 0) ks[v][i] = value
  }
  return ks
}

const shared = decodeShare(window.location.hash)

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

export default function App() {
  const engineRef = useRef<AudioEngine | null>(null)
  const [status, setStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle')
  const [error, setError] = useState('')
  const [presetId, setPresetId] = useState<string | null>(shared?.preset ?? null)
  const [variant, setVariant] = useState(shared?.variant ?? 0)
  const [pattern, setPattern] = useState<Pattern>(() =>
    shared ? parsePattern(shared.rows) : presetPattern(PRESETS[0]),
  )
  const [bpm, setBpm] = useState(shared?.bpm ?? PRESETS[0].bpm)
  const [swing, setSwing] = useState(shared?.swing ?? PRESETS[0].swing)
  const [unit, setUnit] = useState<UnitState>(shared?.unit ?? DEFAULT_UNIT)
  const [knobs, setKnobs] = useState<number[][]>(() => shared?.knobs ?? defaultKnobs())
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
  const [midiInputs, setMidiInputs] = useState<{ id: string; name: string }[] | null>(null)
  const [clockBpm, setClockBpm] = useState(-1)
  const midiRef = useRef<MIDIAccess | null>(null)
  const following = syncInput !== 'internal'

  const send = useCallback((m: ToWorklet) => engineRef.current?.send(m), [])

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
        }
        if (m.type === 'error') setCrash(m.message)
        if (m.type === 'reset') {
          partsRef.current.forEach((ps, voice) =>
            ps.forEach((p, index) => p.scale !== 1 && eng.send({ type: 'part', voice, index, scale: p.scale })),
          )
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

  const loadPreset = (p: Preset) => {
    setPresetId(p.id)
    setVariant(0)
    setPattern(presetPattern(p))
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
      access.onstatechange = () => setMidiInputs(list())
      const inputs = list()
      setMidiInputs(inputs)
      if (inputs.length > 0) followInput(inputs[0].id)
    } catch {
      setMidiInputs([])
    }
  }

  useEffect(() => {
    if (status === 'ready') send({ type: 'sync', mode: following ? 'midi' : 'internal' })
  }, [status, following, send, gen])

  // Keep the share URL in sync with what is playing.
  useEffect(() => {
    if (status !== 'ready') return
    const hash = `#s=${encodeShare({ preset: presetId, variant, rows: patternToText(pattern), bpm, swing, unit, knobs, accent })}`
    window.history.replaceState(null, '', hash)
  }, [status, presetId, variant, pattern, bpm, swing, unit, knobs, accent])

  useEffect(() => {
    if (status === 'ready') send({ type: 'unit', seed: unit.seed, age: unit.age, temp: unit.temp })
  }, [status, unit, send, gen])

  useEffect(() => {
    if (status !== 'ready') return
    knobs.forEach((vals, v) => vals.forEach((value, i) => send({ type: 'knob', voice: v, id: KNOBS[v][i].id, value })))
    send({ type: 'getParts' })
  }, [status, knobs, send, gen])

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
      if (ev.repeat || ev.target instanceof HTMLInputElement || status !== 'ready') return
      const slot = SLOT_KEYS.indexOf(ev.key.toLowerCase())
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
      return { vel, rolls, length: p.length }
    })

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
            負荷 {Math.round(load.load * 100)}%{load.peakLoad >= 0 && `（瞬間 ${Math.round(load.peakLoad * 100)}%）`}
            ・遅延 {following ? latencyMs + MIDI_MARGIN_MS : latencyMs} ms
          </span>
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
        <div className="grid">
          {Array.from({ length: pattern.length / STEPS }, (_, bar) => (
            <div className={`bar bar${bar + 1}`} key={bar}>
              {pattern.length > STEPS && <div className="bar-label">{bar + 1} 小節目</div>}
              {SLOTS.map((slot, s) => (
                <div className="row" key={slot}>
                  <button
                    className={`pad ${hit === s ? 'hit' : ''}`}
                    onPointerDown={() => trigger(s)}
                    title={`${SLOT_NAMES[s]}（キー ${SLOT_KEYS[s].toUpperCase()}）`}
                  >
                    <b>{slot}</b>
                    <span>{SLOT_NAMES[s]}</span>
                  </button>
                  {Array.from({ length: STEPS }, (_, k) => {
                    const i = bar * STEPS + k
                    const v = pattern.vel[s][i]
                    const roll = pattern.rolls[s][i]
                    return (
                      <button
                        key={i}
                        aria-label={`${SLOT_NAMES[s]} ${bar + 1} 小節目 ${k + 1} 番目`}
                        className={`cell c${v} ${playing && i === step ? 'now' : ''} ${k % 4 === 0 ? 'beat' : ''}`}
                        onClick={(e) => editCell(s, i, e.shiftKey)}
                        onContextMenu={(e) => {
                          e.preventDefault()
                          editCell(s, i, true)
                        }}
                      >
                        {v > 0 && roll > 1 ? roll : ''}
                      </button>
                    )
                  })}
                </div>
              ))}
            </div>
          ))}
        </div>
      </section>

      <section className="voices">
        {VOICES.map((name, v) => (
          <div className="voice" key={name}>
            <h2>
              {name} <span>{VOICE_NAMES[v]}</span>
            </h2>
            <div className="knobs">
              {KNOBS[v].map((k, i) => (
                <Knob key={k.id} label={k.label} value={knobs[v][i]} onChange={(val) => setKnob(v, i, val)} />
              ))}
            </div>
          </div>
        ))}
      </section>

      <details className="parts">
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
        <PartsTable
          parts={parts[partsVoice] ?? []}
          onScale={(index, scale) => send({ type: 'part', voice: partsVoice, index, scale })}
        />
      </details>
    </main>
  )
}
