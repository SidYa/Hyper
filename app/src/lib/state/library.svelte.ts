import {
  cancelDownload,
  errorMessage,
  getDownloads,
  getLibrary,
  getRefreshingStores,
  getStoreStatus,
  installGame,
  isBackendError,
  launchGame,
  repairGame,
  resumeDownload,
  uninstallGame,
  updateGame
} from '$lib/api/commands'
import { on } from '$lib/api/events'
import { notify } from '$lib/state/toast.svelte'
import { t } from '$lib/i18n/index.svelte'
import { RUNNER_NAMES, RUNNERS, type ConsoleModeDefaultTab, type DownloadItem, type GameInfo, type GameStatus, type Runner, type StoreStatus } from '$types/index'

export const library = $state<{
  games: GameInfo[]
  statuses: Record<string, GameStatus>
  downloads: DownloadItem[]
  stores: StoreStatus[]
  /** stores fetching their library from the network (start, after login) */
  refreshing: Runner[]
  /** saves waiting for an upload (played offline), by statusKey; from `saves:pending` */
  pendingSaves: Record<string, boolean>
  loading: boolean
  error: string | null
}>({
  games: [],
  statuses: {},
  downloads: [],
  stores: [],
  refreshing: [],
  pendingSaves: {},
  loading: false,
  error: null
})

export const statusKey = (runner: Runner, appName: string) => `${runner}:${appName}`

export function gameStatus(game: GameInfo): GameStatus | undefined {
  return game.runner ? library.statuses[statusKey(game.runner, game.appName)] : undefined
}

/** Installed, or on its way (queued, downloading, paused): the library lists
 *  these together, ahead of the rest. */
export function isInstalledOrInstalling(game: GameInfo): boolean {
  const s = gameStatus(game)?.status
  return game.isInstalled || s === 'queued' || s === 'installing' || s === 'paused'
}

/** Saves of the game wait for an upload; `fetched` is its last `getCloudSaves` answer. */
export function savesPending(game: GameInfo, fetched?: boolean): boolean {
  return (game.runner ? library.pendingSaves[statusKey(game.runner, game.appName)] : undefined) ?? !!fetched
}

export function storeOf(runner: Runner): StoreStatus | undefined {
  return library.stores.find((s) => s.runner === runner)
}

/** Stores the user is logged into, in RUNNERS order. */
export function loggedInStores(): Runner[] {
  return RUNNERS.filter((r) => library.stores.some((s) => s.runner === r && s.loggedIn))
}

/** Stores that get a library tab: logged in (or with games still cached),
 *  then games added by hand. Shared by console mode, the desktop library and
 *  the "Default tab" setting. */
export function libraryStores(): Runner[] {
  const withGames = new Set(library.games.map((g) => g.runner))
  return [
    ...RUNNERS.filter((r) => loggedInStores().includes(r) || withGames.has(r)),
    ...(withGames.has('sideload') ? (['sideload'] as const) : [])
  ]
}

/** The tab actually shown for `wanted`. "All" only exists with two or more
 *  stores (with one it repeats that store's tab), and a store tab only while
 *  that store is there; otherwise "All", the only store, or "Installed". */
export function libraryTab(wanted: ConsoleModeDefaultTab): ConsoleModeDefaultTab {
  const stores = libraryStores()
  const showAll = stores.length >= 2
  if (wanted === 'installed' || (wanted === 'all' ? showAll : stores.includes(wanted))) return wanted
  return showAll ? 'all' : (stores[0] ?? 'installed')
}

export function findGame(runner: string, appName: string): GameInfo | undefined {
  return library.games.find((g) => g.runner === runner && g.appName === appName)
}

export async function loadLibrary() {
  library.loading = true
  library.error = null
  try {
    const [games, stores, downloads, refreshing] = await Promise.all([getLibrary(), getStoreStatus(), getDownloads(), getRefreshingStores()])
    library.refreshing = refreshing
    library.games = games
    library.stores = stores
    library.downloads = downloads
    // the queue of the last session was announced before the UI listened:
    // give its items a status (the worker's own pushes replace it)
    for (const d of downloads) {
      const key = statusKey(d.runner, d.appName)
      if (!d.running && !library.statuses[key]) {
        library.statuses[key] = { runner: d.runner, appName: d.appName, status: d.paused ? 'paused' : 'queued' }
      }
    }
  } catch (e) {
    library.error = errorMessage(e)
  } finally {
    library.loading = false
  }
}

/** Re-read the cached library (cheap) after the backend said it changed. */
async function reload() {
  try {
    const [games, stores] = await Promise.all([getLibrary(), getStoreStatus()])
    library.games = games
    library.stores = stores
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

// --- game actions (shared by Library, Game page and Console mode) ---

// The download commands answer with a snapshot of the queue, but the worker
// may already have started the item and pushed a newer state ("running")
// before that answer arrives. Apply a snapshot only if no push came in
// while the command ran, or the stale one would stick ("Queued" forever).
let queuePushes = 0
async function applyQueue(call: () => Promise<DownloadItem[]>) {
  const before = queuePushes
  const items = await call()
  if (queuePushes === before) library.downloads = items
}

export async function install(game: GameInfo, path?: string, build?: { branch?: string; buildId?: string }) {
  if (!game.runner) return
  try {
    await applyQueue(() => installGame(game.runner!, game.appName, path, undefined, build?.branch, build?.buildId))
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

export async function update(game: GameInfo) {
  if (!game.runner) return
  try {
    await applyQueue(() => updateGame(game.runner!, game.appName))
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

export async function repair(game: GameInfo) {
  if (!game.runner) return
  try {
    await applyQueue(() => repairGame(game.runner!, game.appName))
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

export async function cancel(runner: Runner, appName: string) {
  try {
    await applyQueue(() => cancelDownload(runner, appName))
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

/** Continue a download left paused from the last session. */
export async function resume(runner: Runner, appName: string) {
  try {
    await applyQueue(() => resumeDownload(runner, appName))
  } catch (e) {
    notify(errorMessage(e), 'error')
  }
}

/** Uninstall a store game, or forget a sideloaded one (its files stay). */
export async function uninstall(game: GameInfo): Promise<boolean> {
  if (!game.runner) return false
  try {
    await uninstallGame(game.runner, game.appName)
    // a sideloaded game is gone (with its images) right away; drop it now
    // instead of waiting for the reload, or its card asks for a deleted icon
    if (game.runner === 'sideload') {
      library.games = library.games.filter((g) => !(g.runner === 'sideload' && g.appName === game.appName))
    }
    notify(
      game.runner === 'sideload'
        ? t('sideload.removed', '{{title}} removed from the library', { title: game.title })
        : t('game.uninstalledToast', '{{title}} uninstalled', { title: game.title }),
      'success'
    )
    return true
  } catch (e) {
    notify(errorMessage(e), 'error')
    return false
  }
}

export async function launch(game: GameInfo): Promise<boolean> {
  if (!game.runner) return false
  try {
    await launchGame(game.runner, game.appName)
    return true
  } catch (e) {
    // cancelled from the launch overlay (hold B) before the game started
    if (isBackendError(e) && e.kind === 'cancelled') return false
    notify(errorMessage(e), 'error')
    return false
  }
}

/** Subscribe to backend pushes. Call once from App; returns an unsubscribe. */
export async function subscribeLibraryEvents(): Promise<() => void> {
  const offs = await Promise.all([
    on('library:updated', () => {
      void reload()
    }),
    on('library:refreshing', (stores) => {
      library.refreshing = stores
    }),
    on('game:status', (s) => {
      const key = statusKey(s.runner, s.appName)
      if (s.status === 'done') {
        delete library.statuses[key]
      } else if (s.status === 'error') {
        delete library.statuses[key]
        notify(s.message ?? t('common.operationFailed', 'Operation failed'), 'error', 8000)
      } else {
        // keep the last progress when a status-only update arrives
        const prev = library.statuses[key]
        library.statuses[key] = {
          ...s,
          progress: s.progress ?? (prev?.status === s.status ? prev?.progress : undefined)
        }
      }
    }),
    on('download:queue', (items) => {
      queuePushes++
      library.downloads = items
    }),
    on('store:login-changed', (status) => {
      library.stores = library.stores.map((s) => (s.runner === status.runner ? status : s))
      if (status.loggedIn) notify(`${RUNNER_NAMES[status.runner]}: ${status.username}`, 'success')
    }),
    on('saves:pending', ({ runner, appName, pending }) => {
      library.pendingSaves[statusKey(runner, appName)] = pending
      const title = library.games.find((g) => g.runner === runner && g.appName === appName)?.title ?? appName
      if (pending)
        notify(t('saves.pendingOffline', 'No connection: saves of {{title}} stay on this PC and go to the cloud once you are online', { title }), 'info', 8000)
      else notify(t('saves.pendingUploaded', 'Saves of {{title}} uploaded to the cloud', { title }), 'success')
    }),
    on('store:login-error', ({ runner, message }) => {
      notify(`${RUNNER_NAMES[runner]}: ${message}`, 'error', 8000)
    })
  ])
  return () => offs.forEach((off) => off())
}
