// Button/axis tables for the controller families we recognise. Port of
// HGL/src/frontend/helpers/gamepad_layouts. Every reader calls `check` with
// the logical action and whether it is currently held.
import type { GamepadAction } from './actions'

export type Check = (action: GamepadAction, pressed: boolean) => void

const DEAD = 0.5

function sticks(axes: readonly number[], check: Check, lx = 0, ly = 1, rx = 2, ry = 3) {
  check('leftStickLeft', axes[lx] < -DEAD)
  check('leftStickRight', axes[lx] > DEAD)
  check('leftStickUp', axes[ly] < -DEAD)
  check('leftStickDown', axes[ly] > DEAD)
  check('rightStickLeft', axes[rx] < -DEAD)
  check('rightStickRight', axes[rx] > DEAD)
  check('rightStickUp', axes[ry] < -DEAD)
  check('rightStickDown', axes[ry] > DEAD)
}

const held = (b: GamepadButton | undefined) => !!b && (b.pressed || b.value > DEAD)

/** Chromium "standard" mapping: Xbox, DualShock/DualSense, Steam Input, most clones. */
export function checkStandard(buttons: readonly GamepadButton[], axes: readonly number[], check: Check) {
  check('mainAction', held(buttons[0]))
  check('back', held(buttons[1]))
  check('rightClick', held(buttons[2]))
  check('altAction', held(buttons[3]))
  check('prevPage', held(buttons[4]))
  check('nextPage', held(buttons[5]))
  check('leftTrigger', held(buttons[6]))
  check('rightTrigger', held(buttons[7]))
  check('select', held(buttons[8]))
  check('start', held(buttons[9]))
  check('padUp', held(buttons[12]))
  check('padDown', held(buttons[13]))
  check('padLeft', held(buttons[14]))
  check('padRight', held(buttons[15]))
  check('guide', held(buttons[16]))
  sticks(axes, check)
}

/** Switch Pro / Joy-Cons: same positions as standard, but A/B and X/Y labels
 *  sit swapped, so confirm/cancel follow the printed letters. */
export function checkNintendo(buttons: readonly GamepadButton[], axes: readonly number[], check: Check) {
  check('back', held(buttons[0]))
  check('mainAction', held(buttons[1]))
  check('altAction', held(buttons[2]))
  check('rightClick', held(buttons[3]))
  check('prevPage', held(buttons[4]))
  check('nextPage', held(buttons[5]))
  check('leftTrigger', held(buttons[6]))
  check('rightTrigger', held(buttons[7]))
  check('select', held(buttons[8]))
  check('start', held(buttons[9]))
  check('padUp', held(buttons[12]))
  check('padDown', held(buttons[13]))
  check('padLeft', held(buttons[14]))
  check('padRight', held(buttons[15]))
  check('guide', held(buttons[16]))
  sticks(axes, check)
}

/** Vendor 2563 / product 0523 PS3 clone: d-pad on axes 4/5. */
export function checkPS3Clone(buttons: readonly GamepadButton[], axes: readonly number[], check: Check) {
  check('altAction', held(buttons[0]))
  check('back', held(buttons[1]))
  check('mainAction', held(buttons[2]))
  check('rightClick', held(buttons[3]))
  check('prevPage', held(buttons[4]))
  check('nextPage', held(buttons[5]))
  check('leftTrigger', held(buttons[6]))
  check('rightTrigger', held(buttons[7]))
  check('select', held(buttons[8]))
  check('start', held(buttons[9]))
  check('padUp', axes[5] === -1)
  check('padDown', axes[5] === 1)
  check('padLeft', axes[4] === -1)
  check('padRight', axes[4] === 1)
  check('guide', false)
  sticks(axes, check)
}

export function readerFor(id: string) {
  if (/057e.*(2006|2007|2009)|nintendo|joy.?con|pro.?controller/i.test(id)) return checkNintendo
  if (/2563.*0523/i.test(id)) return checkPS3Clone
  return checkStandard
}
