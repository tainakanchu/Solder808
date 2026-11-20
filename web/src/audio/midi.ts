/** Extra lead over the output latency so late-delivered clock ticks are still on time. */
export const MIDI_MARGIN_MS = 20

export type Realtime = { type: 'rt'; kind: 'clock' | 'start' | 'continue' | 'stop' } | { type: 'spp'; pos: number }

/** MIDI clock, transport and song position messages; null for anything else. */
export function parseRealtime(d: Uint8Array): Realtime | null {
  switch (d[0]) {
    case 0xf8:
      return { type: 'rt', kind: 'clock' }
    case 0xfa:
      return { type: 'rt', kind: 'start' }
    case 0xfb:
      return { type: 'rt', kind: 'continue' }
    case 0xfc:
      return { type: 'rt', kind: 'stop' }
    case 0xf2:
      return d.length >= 3 ? { type: 'spp', pos: d[1] | (d[2] << 7) } : null
    default:
      return null
  }
}

/**
 * Context frame at which a MIDI event received at `timeStamp` (performance.now
 * timebase) should sound: its time on the audible timeline plus the output
 * latency and a margin, so the worklet can always schedule it ahead.
 */
export function midiFrame(ctx: AudioContext, timeStamp: number): number {
  const ts = ctx.getOutputTimestamp()
  const lead = (ctx.baseLatency || 0) + (ctx.outputLatency || 0) + MIDI_MARGIN_MS / 1000
  const t = (ts.contextTime ?? ctx.currentTime) + (timeStamp - (ts.performanceTime ?? performance.now())) / 1000 + lead
  return Math.round(t * ctx.sampleRate)
}
