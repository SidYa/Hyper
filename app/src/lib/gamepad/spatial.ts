// DOM spatial navigation: move focus to the nearest focusable element in a
// direction. Same cone-and-distance heuristic as the store preload in the
// original project, applied to the app's own UI.
import type { Direction } from './actions'

export const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([type="hidden"]):not([disabled]),' +
  ' select:not([disabled]), textarea:not([disabled]),' +
  ' [tabindex]:not([tabindex="-1"]):not([disabled]), [role="button"]'

export function isVisible(el: HTMLElement): boolean {
  if (el.closest('[inert], [hidden], [aria-hidden="true"]')) return false
  const style = getComputedStyle(el)
  if (style.visibility === 'hidden' || style.display === 'none') return false
  const r = el.getBoundingClientRect()
  if (r.width === 0 || r.height === 0) return false
  const vh = window.innerHeight
  const vw = window.innerWidth
  return r.bottom > -200 && r.top < vh + 200 && r.right > -200 && r.left < vw + 200
}

/**
 * The element set navigation is confined to: the top-most open modal
 * (`[data-focus-scope]`) when there is one, else the whole document.
 */
export function scopeRoot(): HTMLElement {
  const scopes = Array.from(document.querySelectorAll<HTMLElement>('[data-focus-scope]')).filter(isVisible)
  return scopes[scopes.length - 1] ?? document.body
}

export function focusables(root: HTMLElement = scopeRoot()): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(isVisible)
}

export function currentFocus(): HTMLElement | null {
  const el = document.activeElement as HTMLElement | null
  if (!el || el === document.body) return null
  if (!scopeRoot().contains(el)) return null
  return el
}

export function focusElement(el: HTMLElement | null) {
  if (!el) return
  el.focus({ preventScroll: true })
  el.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' })
}

/** Top-left-most focusable, or a `[data-focus-initial]` when present. */
export function focusInitial(root: HTMLElement = scopeRoot()): HTMLElement | null {
  const preferred = root.querySelector<HTMLElement>('[data-focus-initial]')
  if (preferred && isVisible(preferred)) {
    focusElement(preferred)
    return preferred
  }
  const list = focusables(root)
  if (!list.length) return null
  list.sort((a, b) => {
    const ra = a.getBoundingClientRect()
    const rb = b.getBoundingClientRect()
    return ra.top - rb.top || ra.left - rb.left
  })
  focusElement(list[0])
  return list[0]
}

/** Pick the best candidate in `direction` from `from`. */
export function findInDirection(from: HTMLElement, direction: Direction, candidates: HTMLElement[]): HTMLElement | null {
  const cur = from.getBoundingClientRect()
  const cx = cur.left + cur.width / 2
  const cy = cur.top + cur.height / 2
  let best: HTMLElement | null = null
  let bestScore = Infinity
  for (const cand of candidates) {
    if (cand === from || from.contains(cand) || cand.contains(from)) continue
    const r = cand.getBoundingClientRect()
    const dx = r.left + r.width / 2 - cx
    const dy = r.top + r.height / 2 - cy
    let inCone = false
    let perp = 0
    switch (direction) {
      case 'right':
        inCone = dx > 8 && Math.abs(dy) <= Math.abs(dx) * 1.5
        perp = Math.abs(dy)
        break
      case 'left':
        inCone = dx < -8 && Math.abs(dy) <= Math.abs(dx) * 1.5
        perp = Math.abs(dy)
        break
      case 'down':
        inCone = dy > 8 && Math.abs(dx) <= Math.abs(dy) * 1.5
        perp = Math.abs(dx)
        break
      case 'up':
        inCone = dy < -8 && Math.abs(dx) <= Math.abs(dy) * 1.5
        perp = Math.abs(dx)
        break
    }
    if (!inCone) continue
    const score = Math.hypot(dx, dy) + perp * 2
    if (score < bestScore) {
      bestScore = score
      best = cand
    }
  }
  return best
}

function scrollable(el: HTMLElement | null): HTMLElement | null {
  let node: HTMLElement | null = el
  while (node && node !== document.body) {
    const s = getComputedStyle(node)
    if (/(auto|scroll)/.test(s.overflowY) && node.scrollHeight > node.clientHeight) return node
    node = node.parentElement
  }
  return document.querySelector<HTMLElement>('main.content')
}

/** Move focus; returns true when something happened (focus or scroll). */
export function navigate(direction: Direction): boolean {
  const root = scopeRoot()
  const from = currentFocus()
  if (!from) return !!focusInitial(root)
  const target = findInDirection(from, direction, focusables(root))
  if (target) {
    focusElement(target)
    return true
  }
  if (direction === 'up' || direction === 'down') {
    scrollable(from)?.scrollBy({ top: direction === 'down' ? 300 : -300, behavior: 'smooth' })
    return true
  }
  return false
}

export function scrollContent(delta: number) {
  scrollable(currentFocus())?.scrollBy({ top: delta })
}

export function isTextInput(el: HTMLElement | null): el is HTMLInputElement | HTMLTextAreaElement {
  if (!el) return false
  if (el.tagName === 'TEXTAREA') return true
  if (el.tagName !== 'INPUT') return false
  const type = (el as HTMLInputElement).type
  return ['text', 'search', 'url', 'email', 'password', 'number'].includes(type)
}
