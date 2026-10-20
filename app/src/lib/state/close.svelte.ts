// Quitting while something is busy. The backend asks before it exits when
// a download runs or waits, or cloud saves are being synced
// (`app:close-requested`, see src-tauri/src/close.rs): one confirm dialog,
// then `quit` with force. The request is confirmed on arrival; unconfirmed,
// the backend quits after 10 s on its own. Shared by the desktop and Deck roots.
import { on } from '$lib/api/events'
import { confirmCloseRequest, quitApp } from '$lib/api/commands'
import { askConfirm } from '$lib/state/confirm.svelte'
import { t } from '$lib/i18n/index.svelte'
import type { CloseBlockers } from '$types/index'

export const isBusy = (b: CloseBlockers | null | undefined): b is CloseBlockers =>
  !!b && (b.downloads.length > 0 || b.syncing.length > 0)

/** The warning text: one line per kind of work that would be interrupted. */
export function closeWarning(b: CloseBlockers): string {
  const parts: string[] = []
  if (b.downloads.length) {
    parts.push(
      t('close.downloads', 'Downloading: {{titles}}. The download stops now and continues from where it left off next time.', {
        titles: b.downloads.join(', ')
      })
    )
  }
  if (b.syncing.length) {
    parts.push(t('close.syncing', 'Syncing cloud saves: {{titles}}. Quitting now may leave the saves half synced.', { titles: b.syncing.join(', ') }))
  }
  return parts.join('\n')
}

export function subscribeCloseRequests(): Promise<() => void> {
  return on('app:close-requested', async (b) => {
    // first: the backend quits by itself when no answer comes (a hung UI)
    confirmCloseRequest().catch(() => {})
    const ok = await askConfirm({
      title: t('close.title', 'Quit Hyper?'),
      message: closeWarning(b),
      confirmLabel: t('close.quit', 'Quit anyway'),
      cancelLabel: t('close.stay', 'Stay'),
      danger: true
    })
    if (ok) await quitApp(true).catch(() => {})
  })
}
