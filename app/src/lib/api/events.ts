// Events pushed from Rust. Names mirror app/src-tauri/src/events.rs.
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { CloseBlockers, DownloadItem, GameStatus, Runner, StoreStatus } from '$types/index'

export interface EventPayloads {
  'library:updated': { runner: Runner | null }
  /** stores whose library is being fetched from the network; [] when done */
  'library:refreshing': Runner[]
  'game:status': GameStatus
  'download:queue': DownloadItem[]
  'store:login-changed': StoreStatus
  'store:login-error': { runner: Runner; message: string }
  /** a game's saves wait for an upload (played offline) or were just uploaded */
  'saves:pending': { runner: Runner; appName: string; pending: boolean }
  /** progress 0..1 of the in-game View+Start close chord */
  'gamepad:quit-combo': number
  /** quitting was asked while downloads / save syncs run: confirm, then quit with force */
  'app:close-requested': CloseBlockers
}

export function on<K extends keyof EventPayloads>(
  name: K,
  handler: (payload: EventPayloads[K]) => void
): Promise<UnlistenFn> {
  return listen<EventPayloads[K]>(name, (e) => handler(e.payload))
}
