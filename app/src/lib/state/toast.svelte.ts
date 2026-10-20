export type ToastKind = 'info' | 'success' | 'error'

export interface Toast {
  id: number
  kind: ToastKind
  message: string
}

export const toasts = $state<{ list: Toast[] }>({ list: [] })

let nextId = 1

export function notify(message: string, kind: ToastKind = 'info', ttl = 5000) {
  const id = nextId++
  toasts.list.push({ id, kind, message })
  window.setTimeout(() => dismiss(id), ttl)
  return id
}

export function dismiss(id: number) {
  toasts.list = toasts.list.filter((t) => t.id !== id)
}
