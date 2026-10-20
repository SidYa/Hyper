// Typed wrappers over Tauri's invoke(). One function per #[tauri::command]
// registered in app/src-tauri/src/lib.rs.
import { invoke } from '@tauri-apps/api/core'
import type {
  Achievements,
  AppInfo,
  AppSettings,
  BackendError,
  CloseBlockers,
  CloudSaves,
  DlcEntry,
  DownloadItem,
  FolderListing,
  GameExtra,
  GameInfo,
  GameSettings,
  ImportCandidate,
  InstallInfo,
  InstallPlatform,
  PlaceEntry,
  PlayStats,
  Runner,
  SaveSyncMode,
  SideloadGame,
  SideloadInput,
  StoreStatus
} from '$types/index'

export function isBackendError(e: unknown): e is BackendError {
  return typeof e === 'object' && e !== null && 'kind' in e && 'message' in e
}

export function errorMessage(e: unknown): string {
  if (isBackendError(e)) return e.message
  if (e instanceof Error) return e.message
  return String(e)
}

// --- app ---
export const getAppInfo = () => invoke<AppInfo>('get_app_info')
export const logGamepad = (gp: Gamepad) =>
  invoke<void>('log_gamepad', { index: gp.index, id: gp.id, mapping: gp.mapping, buttons: gp.buttons.length, axes: gp.axes.length })
export const setFullscreen = (fullscreen: boolean) =>
  invoke<void>('set_fullscreen', { fullscreen })
export const openExternal = (url: string) => invoke<void>('open_external', { url })
export const openAppFolder = (which: 'config' | 'logs') => invoke<void>('open_app_folder', { which })
/** Leave the app; without `force` a running download / save sync makes the backend ask first (`app:close-requested`). */
export const quitApp = (force = false) => invoke<void>('quit', { force })
export const getCloseBlockers = () => invoke<CloseBlockers>('get_close_blockers')
/** Tell the backend an `app:close-requested` arrived; unanswered, it quits by itself after a few seconds. */
export const confirmCloseRequest = () => invoke<void>('confirm_close_request')
/** Folders inside `path` (the drives when null), for the console folder picker. */
export const listFolders = (path: string | null) => invoke<FolderListing>('list_folders', { path })
export const listFolderPlaces = () => invoke<PlaceEntry[]>('list_folder_places')
export const createFolder = (parent: string, name: string) => invoke<string>('create_folder', { parent, name })

// --- settings ---
export const getSettings = () => invoke<AppSettings>('get_settings')
export const saveSettings = (settings: AppSettings) =>
  invoke<AppSettings>('save_settings', { settings })

// --- library ---
export const getLibrary = (runner?: Runner) =>
  invoke<GameInfo[]>('get_library', { runner: runner ?? null })
export const getRefreshingStores = () => invoke<Runner[]>('get_refreshing_stores')
export const getStoreStatus = () => invoke<StoreStatus[]>('get_store_status')

// --- stores ---
/** `console`: open the login page maximized above the fullscreen console UI. */
export const storeLoginStart = (runner: Runner, console = false) =>
  invoke<void>('store_login_start', { runner, console })
export const storeLogout = (runner: Runner) => invoke<void>('store_logout', { runner })

// --- embedded store webview ---
export interface WebviewBounds {
  x: number
  y: number
  width: number
  height: number
}
export const storeWebviewShow = (runner: Runner, bounds: WebviewBounds) =>
  invoke<string>('store_webview_show', { runner, bounds })
export const storeWebviewBounds = (runner: Runner, bounds: WebviewBounds) =>
  invoke<void>('store_webview_bounds', { runner, bounds })
export const storeWebviewHide = (runner?: Runner) =>
  invoke<void>('store_webview_hide', { runner: runner ?? null })
export type StoreWebviewCommand =
  | { type: 'enter' }
  | { type: 'exit' }
  | { type: 'click' }
  | { type: 'goBack' }
  | { type: 'goForward' }
  | { type: 'focusSearch' }
  | { type: 'navigate'; direction: 'up' | 'down' | 'left' | 'right' }
  | { type: 'scroll'; direction: 'up' | 'down' }
export const storeWebviewReleaseFocus = () => invoke<void>('store_webview_release_focus')
/** Scrollbar colours of the app's theme for the store pages (thumb, thumb on hover). */
export const storeWebviewScrollbar = (thumb: string, hover: string) =>
  invoke<void>('store_webview_scrollbar', { thumb, hover })
export const storeWebviewCommand = (runner: Runner, command: StoreWebviewCommand) =>
  invoke<void>('store_webview_command', { runner, command })

// --- games ---
export const getInstallInfo = (runner: Runner, appName: string, platform?: InstallPlatform) =>
  invoke<InstallInfo>('get_install_info', { runner, appName, platform: platform ?? null })
export const getDlcs = (runner: Runner, appName: string) => invoke<DlcEntry[]>('get_dlcs', { runner, appName })
export const installGame = (runner: Runner, appName: string, path?: string, platform?: InstallPlatform, branch?: string, buildId?: string) =>
  invoke<DownloadItem[]>('install_game', { runner, appName, path: path ?? null, platform: platform ?? null, branch: branch ?? null, buildId: buildId ?? null })
export const moveGame = (runner: Runner, appName: string, path: string) => invoke<void>('move_game', { runner, appName, path })
export const getAchievements = (runner: Runner, appName: string) => invoke<Achievements>('get_achievements', { runner, appName })
export const getGameSettings = (runner: Runner, appName: string) => invoke<GameSettings>('get_game_settings', { runner, appName })
export const saveGameSettings = (runner: Runner, appName: string, settings: GameSettings) =>
  invoke<void>('save_game_settings', { runner, appName, settings })
export const updateGame = (runner: Runner, appName: string) =>
  invoke<DownloadItem[]>('update_game', { runner, appName })
export const repairGame = (runner: Runner, appName: string) =>
  invoke<DownloadItem[]>('repair_game', { runner, appName })
export const getGameExtra = (runner: Runner, appName: string) =>
  invoke<GameExtra>('get_game_extra', { runner, appName })
export const getPlayStats = (runner: Runner, appName: string) =>
  invoke<PlayStats>('get_play_stats', { runner, appName })
export const cancelDownload = (runner: Runner, appName: string) =>
  invoke<DownloadItem[]>('cancel_download', { runner, appName })
export const getDownloads = () => invoke<DownloadItem[]>('get_downloads')
export const resumeDownload = (runner: Runner, appName: string) =>
  invoke<DownloadItem[]>('resume_download', { runner, appName })
export const uninstallGame = (runner: Runner, appName: string) =>
  invoke<void>('uninstall_game', { runner, appName })
export const launchGame = (runner: Runner, appName: string) =>
  invoke<void>('launch_game', { runner, appName })
export const killGame = (runner: Runner, appName: string) =>
  invoke<void>('kill_game', { runner, appName })
export const findImportableGames = (paths: string[]) =>
  invoke<ImportCandidate[]>('find_importable_games', { paths })
export const importGame = (runner: Runner, appName: string, path: string) =>
  invoke<void>('import_game', { runner, appName, path })
export const openGameFolder = (runner: Runner, appName: string) =>
  invoke<void>('open_game_folder', { runner, appName })

// --- games added by hand ---
export const sideloadAdd = (input: SideloadInput) => invoke<GameInfo>('sideload_add', { input })
export const sideloadEdit = (appName: string, input: SideloadInput) =>
  invoke<GameInfo>('sideload_edit', { appName, input })
export const sideloadGet = (appName: string) => invoke<SideloadGame>('sideload_get', { appName })
/** PNG bytes of an executable's icon (null when it has none). */
export const exeIcon = (path: string) => invoke<number[] | null>('exe_icon', { path })

// --- cloud saves ---
export const getCloudSaves = (runner: Runner, appName: string) =>
  invoke<CloudSaves>('get_cloud_saves', { runner, appName })
export const syncSaves = (runner: Runner, appName: string, mode: SaveSyncMode) =>
  invoke<string>('sync_saves', { runner, appName, mode })
export const openSaveFolder = (runner: Runner, appName: string, name: string) =>
  invoke<void>('open_save_folder', { runner, appName, name })
