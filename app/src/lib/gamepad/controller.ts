// Ported 1:1 from HGL/src/frontend/screens/ConsoleMode/controller.ts

export type ControllerLayout = 'ps4' | 'ps5' | 'xbox' | 'nintendo' | 'steam-deck'

// Standard gamepad button indices (Chromium "standard" mapping) for the raw
// B-hold read in Console.svelte; the full mapping lives in input.svelte.ts.
const BTN_ACTION = 0
const BTN_BACK = 1

// Nintendo controllers report the same Chromium "standard" positions as Xbox,
// but their A/B labels sit in swapped physical positions, so confirm/cancel
// must swap to match the on-screen glyphs.
export const getBackButtonIndex = (layout: ControllerLayout) =>
  layout === 'nintendo' ? BTN_ACTION : BTN_BACK

export function detectControllerLayout(id: string): ControllerLayout {
  if (/054c|PS3|0268|2563.*0523/i.test(id)) return 'ps4'
  if (/054c.*0ce6/i.test(id)) return 'ps5'
  if (/28de.*11ff/.test(id)) return 'steam-deck'
  if (/microsoft|xbox/i.test(id)) return 'xbox'
  if (/nintendo|057e|switch|joy.?con|pro.?controller/i.test(id)) return 'nintendo'
  return 'xbox'
}
