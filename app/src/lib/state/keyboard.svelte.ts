// On-screen keyboard state (Apple-TV style carousel or QWERTY rows). The
// component `TvosKeyboard.svelte` renders it; this module is the imperative
// API the gamepad code and screens use. Port of
// HGL/src/frontend/helpers/virtualKeyboard.ts + TVOSKeyboard state.

export type KeyLayout = 'lower' | 'upper' | 'numbers' | 'symbols'
export type KeyMode = 'tvos' | 'qwerty'

export const LAYOUTS: Record<KeyLayout, string[]> = {
  lower: 'abcdefghijklmnopqrstuvwxyz'.split(''),
  upper: 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split(''),
  numbers: '0123456789'.split(''),
  symbols: ['@', '#', '$', '%', '&', '*', '(', ')', '-', '+', '=', '/', ':', ';', '"', "'", ',', '.', '?', '!']
}

const QWERTY_LOWER = ['qwertyuiop'.split(''), 'asdfghjkl'.split(''), 'zxcvbnm'.split('')]
export const QWERTY_ROWS: Record<KeyLayout, string[][]> = {
  lower: QWERTY_LOWER,
  upper: QWERTY_LOWER.map((r) => r.map((c) => c.toUpperCase())),
  numbers: [
    ['1', '2', '3', '4', '5'],
    ['6', '7', '8', '9', '0']
  ],
  symbols: [
    ['@', '#', '$', '%', '&', '*', '('],
    [')', '-', '+', '=', '/', ':', ';'],
    ['"', "'", ',', '.', '?', '!']
  ]
}

export const NEXT_LAYOUT: Record<KeyLayout, KeyLayout> = {
  lower: 'upper',
  upper: 'numbers',
  numbers: 'symbols',
  symbols: 'lower'
}
export const LAYOUT_LABEL: Record<KeyLayout, string> = {
  lower: 'abc',
  upper: 'ABC',
  numbers: '123',
  symbols: '#+='
}

export const keyboard = $state<{
  open: boolean
  target: HTMLInputElement | null
  layout: KeyLayout
  mode: KeyMode
  text: string
  caret: number
  /** shown while nothing is typed; '' = the search hint */
  placeholder: string
}>({ open: false, target: null, layout: 'lower', mode: 'tvos', text: '', caret: 0, placeholder: '' })

/** `submitted`: closed with Done (true) or dismissed with B / Escape (false). */
export type KeyboardClosed = (submitted: boolean) => void
let onClosed: KeyboardClosed | null = null

/** Screens register a callback to know when the keyboard was dismissed.
 *  Returns the previous one, so an overlay can restore it when it closes. */
export function onKeyboardClosed(cb: KeyboardClosed | null): KeyboardClosed | null {
  const previous = onClosed
  onClosed = cb
  return previous
}

export function openKeyboard(input: HTMLInputElement, mode?: KeyMode, placeholder = '') {
  keyboard.target = input
  keyboard.text = input.value
  keyboard.caret = input.value.length
  keyboard.layout = 'lower'
  keyboard.placeholder = placeholder
  if (mode) keyboard.mode = mode
  keyboard.open = true
}

export function closeKeyboard(submitted = false) {
  if (!keyboard.open) return
  keyboard.open = false
  keyboard.target = null
  keyboard.text = ''
  keyboard.caret = 0
  keyboard.placeholder = ''
  onClosed?.(submitted)
}

function apply(next: string, caret: number) {
  const input = keyboard.target
  if (!input) return
  input.value = next
  input.dispatchEvent(new Event('input', { bubbles: true }))
  keyboard.text = next
  keyboard.caret = Math.max(0, Math.min(caret, next.length))
}

/** Type a key code: a character, or {space} {bksp} {clear} {layout} {done}. */
export function typeKey(code: string) {
  const value = keyboard.target?.value ?? keyboard.text
  const caret = Math.max(0, Math.min(keyboard.caret, value.length))
  switch (code) {
    case '{space}':
      apply(value.slice(0, caret) + ' ' + value.slice(caret), caret + 1)
      break
    case '{bksp}':
      if (caret === 0) return
      apply(value.slice(0, caret - 1) + value.slice(caret), caret - 1)
      break
    case '{clear}':
      apply('', 0)
      break
    case '{layout}':
      keyboard.layout = NEXT_LAYOUT[keyboard.layout]
      break
    case '{done}':
      closeKeyboard(true)
      break
    default:
      apply(value.slice(0, caret) + code + value.slice(caret), caret + 1)
  }
}

export function moveCaret(delta: number) {
  const len = keyboard.target?.value.length ?? keyboard.text.length
  keyboard.caret = Math.max(0, Math.min(keyboard.caret + delta, len))
}

/** Keep the preview in sync when the input is edited by a physical keyboard. */
export function syncFromTarget() {
  const input = keyboard.target
  if (!input) return
  keyboard.text = input.value
  keyboard.caret = Math.min(keyboard.caret, input.value.length)
}
