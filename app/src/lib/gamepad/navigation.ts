// Default (desktop UI) behaviour for gamepad actions that no screen consumed.
import { directionOf, type GamepadAction } from './actions'
import { currentFocus, focusInitial, isTextInput, navigate, scrollContent } from './spatial'
import { keyboard, openKeyboard } from '$lib/state/keyboard.svelte'
import { navigate as go, router } from '$lib/router.svelte'

const SIDEBAR_ITEMS = '.sidebar .item, .sidebar .store'

function stepSidebar(delta: 1 | -1) {
  const items = Array.from(document.querySelectorAll<HTMLElement>(SIDEBAR_ITEMS)).filter(
    (e) => !(e as HTMLButtonElement).disabled
  )
  if (!items.length) return
  const cur = currentFocus()
  const idx = cur ? items.indexOf(cur) : -1
  const next = idx === -1 ? (delta === 1 ? 0 : items.length - 1) : (idx + delta + items.length) % items.length
  items[next].focus()
}

export function defaultHandler(action: GamepadAction): boolean {
  const el = currentFocus()
  const dir = directionOf(action)
  if (dir) {
    if (!el) focusInitial()
    else navigate(dir)
    return true
  }
  switch (action) {
    case 'mainAction':
      if (!el) {
        focusInitial()
        return true
      }
      if (isTextInput(el)) {
        openKeyboard(el as HTMLInputElement)
        return true
      }
      el.click()
      return true
    case 'back': {
      if (keyboard.open) return false
      const dismiss = document.querySelector<HTMLElement>('[data-focus-scope] [data-dismiss]')
      if (dismiss) {
        dismiss.click()
        return true
      }
      if (router.route.name !== 'library') history.back()
      return true
    }
    case 'altAction': {
      // Y on a game card: the card's quick action (play / install)
      const card = el?.closest<HTMLElement>('[data-game-card]')
      if (card) {
        card.dispatchEvent(new CustomEvent('gpquick', { bubbles: false }))
        return true
      }
      return false
    }
    case 'rightStickUp':
      scrollContent(-60)
      return true
    case 'rightStickDown':
      scrollContent(60)
      return true
    case 'prevPage':
      stepSidebar(-1)
      return true
    case 'nextPage':
      stepSidebar(1)
      return true
    case 'guide':
      go(router.route.name === 'console' ? '/library' : '/console')
      return true
    default:
      return false
  }
}
