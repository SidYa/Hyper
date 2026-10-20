export type GamepadAction =
  | 'padUp'
  | 'padDown'
  | 'padLeft'
  | 'padRight'
  | 'leftStickUp'
  | 'leftStickDown'
  | 'leftStickLeft'
  | 'leftStickRight'
  | 'rightStickUp'
  | 'rightStickDown'
  | 'rightStickLeft'
  | 'rightStickRight'
  | 'mainAction' // A / Cross
  | 'back' // B / Circle
  | 'altAction' // Y / Triangle
  | 'rightClick' // X / Square
  | 'prevPage' // LB / L1
  | 'nextPage' // RB / R1
  | 'leftTrigger'
  | 'rightTrigger'
  | 'select'
  | 'start'
  | 'guide'

export type Direction = 'up' | 'down' | 'left' | 'right'

const DIRECTIONS: Partial<Record<GamepadAction, Direction>> = {
  padUp: 'up',
  padDown: 'down',
  padLeft: 'left',
  padRight: 'right',
  leftStickUp: 'up',
  leftStickDown: 'down',
  leftStickLeft: 'left',
  leftStickRight: 'right'
}

/** D-pad / left stick actions as a direction, else null. */
export function directionOf(action: GamepadAction): Direction | null {
  return DIRECTIONS[action] ?? null
}

/** Handlers return true when they consumed the action. */
export type ActionHandler = (action: GamepadAction) => boolean | void

const KEYS: Record<string, GamepadAction> = {
  ArrowUp: 'padUp',
  ArrowDown: 'padDown',
  ArrowLeft: 'padLeft',
  ArrowRight: 'padRight',
  Enter: 'mainAction',
  ' ': 'mainAction',
  Escape: 'back',
  Backspace: 'back',
  PageUp: 'prevPage',
  PageDown: 'nextPage'
}

/** Keyboard key as the gamepad action it stands for (desk use of console mode). */
export function keyToAction(e: KeyboardEvent): GamepadAction | null {
  return KEYS[e.key] ?? null
}
