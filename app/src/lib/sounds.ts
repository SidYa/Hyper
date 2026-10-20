// Console-mode interface sounds, synthesised with WebAudio: short soft
// tones, so there are no audio files to bundle or license (and the CSP's
// media-src stays 'none'). Played for controller / keyboard navigation while
// the "Interface sounds" setting is on.
import type { GamepadAction } from '$lib/gamepad/actions'
import { directionOf } from '$lib/gamepad/actions'

export type Sound = 'move' | 'select' | 'back' | 'tab' | 'launch'

/** Overall loudness; the UI should sit well below game audio. */
const VOLUME = 0.22

let ctx: AudioContext | null = null
let master: GainNode | null = null

function audio(): { ctx: AudioContext; out: GainNode } | null {
  try {
    if (!ctx) {
      ctx = new AudioContext()
      master = ctx.createGain()
      master.gain.value = VOLUME
      master.connect(ctx.destination)
    }
    if (ctx.state === 'suspended') void ctx.resume()
    return master ? { ctx, out: master } : null
  } catch {
    return null
  }
}

/** One tone: `freq` gliding to `to`, a fast attack and an exponential fade. */
function tone(
  a: { ctx: AudioContext; out: GainNode },
  { freq, to = freq, start = 0, length, type = 'sine', gain = 0.5 }: {
    freq: number
    to?: number
    start?: number
    length: number
    type?: OscillatorType
    gain?: number
  }
) {
  const t0 = a.ctx.currentTime + start
  const osc = a.ctx.createOscillator()
  const env = a.ctx.createGain()
  osc.type = type
  osc.frequency.setValueAtTime(freq, t0)
  if (to !== freq) osc.frequency.exponentialRampToValueAtTime(to, t0 + length)
  env.gain.setValueAtTime(0.0001, t0)
  env.gain.exponentialRampToValueAtTime(gain, t0 + 0.006)
  env.gain.exponentialRampToValueAtTime(0.0001, t0 + length)
  osc.connect(env).connect(a.out)
  osc.start(t0)
  osc.stop(t0 + length + 0.02)
}

let lastMove = 0

export function playSound(sound: Sound) {
  // auto-repeat fires every 100 ms; keep the ticks from turning into a buzz
  if (sound === 'move') {
    const now = performance.now()
    if (now - lastMove < 45) return
    lastMove = now
  }
  const a = audio()
  if (!a) return
  switch (sound) {
    case 'move':
      tone(a, { freq: 1320, to: 1100, length: 0.035, gain: 0.18 })
      break
    case 'tab':
      tone(a, { freq: 760, to: 900, length: 0.05, type: 'triangle', gain: 0.3 })
      break
    case 'select':
      tone(a, { freq: 660, length: 0.06, type: 'triangle', gain: 0.4 })
      tone(a, { freq: 990, start: 0.05, length: 0.09, type: 'triangle', gain: 0.35 })
      break
    case 'back':
      tone(a, { freq: 700, to: 460, length: 0.09, type: 'triangle', gain: 0.35 })
      break
    case 'launch':
      tone(a, { freq: 523, length: 0.09, type: 'triangle', gain: 0.35 })
      tone(a, { freq: 659, start: 0.07, length: 0.09, type: 'triangle', gain: 0.35 })
      tone(a, { freq: 784, start: 0.14, length: 0.09, type: 'triangle', gain: 0.35 })
      tone(a, { freq: 1047, start: 0.21, length: 0.22, type: 'triangle', gain: 0.3 })
      break
  }
}

/** The sound a controller / keyboard action makes, if any. */
export function soundFor(action: GamepadAction): Sound | null {
  if (directionOf(action)) return 'move'
  switch (action) {
    case 'mainAction':
    case 'altAction':
      return 'select'
    case 'back':
      return 'back'
    case 'prevPage':
    case 'nextPage':
      return 'tab'
    default:
      return null
  }
}
