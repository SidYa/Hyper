// Global gamepad engine. Port of the polling / repeat / routing core of
// HGL/src/frontend/helpers/gamepad.ts. Reads every connected controller each
// frame, turns buttons and sticks into logical actions with key-repeat, and
// hands each press to the top of a handler stack. Screens (console mode, the
// on-screen keyboard, dialogs, the store webview) push a handler while they
// own input; whatever is not consumed falls through to the default DOM
// spatial navigation in `navigation.ts`.
import { detectControllerLayout, type ControllerLayout } from './controller'
import { readerFor, type Check } from './layouts'
import type { ActionHandler, GamepadAction } from './actions'
import { logGamepad } from '$lib/api/commands'

// the same press arriving from two pads this close together is one press
// seen twice (a physical pad and its Steam Input twin)
const TWIN_WINDOW_MS = 30

const SCROLL_REPEAT_DELAY = 50

const REPEATING: GamepadAction[] = [
  'padUp',
  'padDown',
  'padLeft',
  'padRight',
  'leftStickUp',
  'leftStickDown',
  'leftStickLeft',
  'leftStickRight'
]
const SCROLLING: GamepadAction[] = ['rightStickUp', 'rightStickDown', 'rightStickLeft', 'rightStickRight']

interface ActionState {
  /** 0 = released, else timestamp of the last fire */
  triggeredAt: Map<number, number>
  hasRepeated: boolean
}

/** Reactive view of the controller situation, for hints and cursor hiding. */
export const gamepad = $state<{
  connected: boolean
  layout: ControllerLayout
  /** last input came from a controller (false again after a mouse move) */
  usingGamepad: boolean
}>({ connected: false, layout: 'xbox', usingGamepad: false })

// d-pad / stick auto-repeat: first repeat after INITIAL + INTERVAL ms, then every INTERVAL
const REPEAT_INTERVAL = 100
const REPEAT_INITIAL_DELAY = 500

const handlers: ActionHandler[] = []
let fallback: ActionHandler = () => false

/** Push an input handler; returns a function that removes it again. */
export function pushHandler(h: ActionHandler): () => void {
  handlers.push(h)
  return () => {
    const i = handlers.lastIndexOf(h)
    if (i >= 0) handlers.splice(i, 1)
  }
}

export function setFallbackHandler(h: ActionHandler) {
  fallback = h
}

let listener: ((action: GamepadAction) => void) | null = null

/** Observe every action before the handlers see it (interface sounds).
 *  Returns a function that removes the listener again. */
export function setActionListener(fn: (action: GamepadAction) => void): () => void {
  listener = fn
  return () => {
    if (listener === fn) listener = null
  }
}

function dispatch(action: GamepadAction) {
  listener?.(action)
  for (let i = handlers.length - 1; i >= 0; i--) {
    if (handlers[i](action)) return
  }
  fallback(action)
}

/** Fire an action as if a controller pressed it (used by keyboard shims). */
export function synthesize(action: GamepadAction) {
  dispatch(action)
}

const states = new Map<GamepadAction, ActionState>()
function stateOf(action: GamepadAction): ActionState {
  let s = states.get(action)
  if (!s) {
    s = { triggeredAt: new Map(), hasRepeated: false }
    states.set(action, s)
  }
  return s
}

let windowFocused = true
let started = false
let controllers: number[] = []
let keyboardActiveProbe: () => boolean = () => false

/** The on-screen keyboard tells us when it is open so a jostled mouse does not
 *  switch back to mouse mode mid-typing. */
export function setKeyboardActiveProbe(fn: () => boolean) {
  keyboardActiveProbe = fn
}

function markUsingGamepad(index: number) {
  if (!gamepad.usingGamepad) {
    gamepad.usingGamepad = true
    document.body.classList.add('using-gamepad')
    const onMove = () => {
      if (keyboardActiveProbe()) return
      window.removeEventListener('mousemove', onMove)
      gamepad.usingGamepad = false
      document.body.classList.remove('using-gamepad')
    }
    window.addEventListener('mousemove', onMove)
  }
  const gp = navigator.getGamepads()[index]
  if (gp) gamepad.layout = detectControllerLayout(gp.id)
}

function check(index: number): Check {
  return (action, pressed) => {
    const s = stateOf(action)
    const last = s.triggeredAt.get(index) ?? 0
    if (!pressed) {
      s.triggeredAt.set(index, 0)
      s.hasRepeated = false
      return
    }
    if (!windowFocused) return

    const now = performance.now()
    const wasActive = last !== 0
    let fire = !wasActive
    if (wasActive) {
      let delay: number | null = null
      if (REPEATING.includes(action)) delay = REPEAT_INTERVAL + (s.hasRepeated ? 0 : REPEAT_INITIAL_DELAY)
      else if (SCROLLING.includes(action)) delay = SCROLL_REPEAT_DELAY
      if (delay !== null && now - last > delay) {
        fire = true
        s.hasRepeated = true
      }
    }
    if (!fire) return
    s.triggeredAt.set(index, now)
    if (firedByTwin(s, index, now)) return
    markUsingGamepad(index)
    dispatch(action)
  }
}

/** Did another controller fire this action a moment ago? On Linux
 *  (WebKitGTK) ids carry no vendor, so `isMasked` cannot spot a Steam Input
 *  twin there; timing can. */
function firedByTwin(s: ActionState, index: number, now: number) {
  for (const [other, at] of s.triggeredAt) {
    if (other !== index && at !== 0 && now - at < TWIN_WINDOW_MS) return true
  }
  return false
}

// Steam Input exposes a virtual Valve pad on top of the physical one; ignore
// the physical twin or every press arrives twice.
const isValve = (gp: Gamepad | null) => !!gp && gp.id.includes('Vendor: 28de')
function isMasked(all: (Gamepad | null)[], gp: Gamepad) {
  if (isValve(gp)) return false
  return all.some((v) => isValve(v) && Math.abs(v!.timestamp - gp.timestamp) <= 10)
}

function poll() {
  const all = navigator.getGamepads()
  for (const index of controllers) {
    const gp = all[index]
    if (!gp || isMasked(all, gp)) continue
    try {
      readerFor(gp.id)(gp.buttons, gp.axes, check(index))
    } catch (e) {
      console.warn('gamepad read failed', e)
    }
  }
  if (controllers.length > 0) requestAnimationFrame(poll)
}

function refreshInfo() {
  const first = Array.from(navigator.getGamepads()).find((g): g is Gamepad => !!g)
  gamepad.connected = !!first
  if (first) gamepad.layout = detectControllerLayout(first.id)
}

/** Start polling. Safe to call once from App. */
export function startGamepadInput() {
  if (started) return
  started = true
  window.addEventListener('focus', () => (windowFocused = true))
  window.addEventListener('blur', () => (windowFocused = false))

  const add = (gp: Gamepad) => {
    if (/046d.*c24f/i.test(gp.id)) return // Logitech G29 wheel
    if (controllers.includes(gp.index)) return
    const wasEmpty = controllers.length === 0
    controllers.push(gp.index)
    logGamepad(gp).catch(() => {})
    refreshInfo()
    if (wasEmpty) requestAnimationFrame(poll)
  }
  window.addEventListener('gamepadconnected', (e) => add(e.gamepad))
  window.addEventListener('gamepaddisconnected', (e) => {
    controllers = controllers.filter((i) => i !== e.gamepad.index)
    refreshInfo()
  })
  // Pads connected before the page loaded do not fire `gamepadconnected`
  for (const gp of navigator.getGamepads()) if (gp) add(gp)
}
