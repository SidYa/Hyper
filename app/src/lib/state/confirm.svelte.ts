// Styled replacement for the system confirm(): `await askConfirm({...})`
// shows ConfirmDialog (mounted once in App) and resolves true / false.

export interface ConfirmRequest {
  title: string
  message?: string
  /** shown emphasised under the title (e.g. the game) */
  subject?: string
  confirmLabel: string
  cancelLabel: string
  /** destructive action: red button, Cancel focused first */
  danger?: boolean
}

interface Pending extends ConfirmRequest {
  resolve: (ok: boolean) => void
}

export const confirmState = $state<{ current: Pending | null }>({ current: null })

export function askConfirm(req: ConfirmRequest): Promise<boolean> {
  // a new question replaces (and cancels) one still open
  confirmState.current?.resolve(false)
  return new Promise((resolve) => {
    confirmState.current = { ...req, resolve }
  })
}

export function answerConfirm(ok: boolean) {
  const cur = confirmState.current
  confirmState.current = null
  cur?.resolve(ok)
}
