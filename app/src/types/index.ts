// Twin of core/src/types.rs and core/src/config/mod.rs. Keep in sync.

/** `sideload`: games added by hand (any executable, not from a store). */
export type Runner = 'legendary' | 'gog' | 'sideload'
export type StoreRunner = Exclude<Runner, 'sideload'>

/** The web stores (login, store tab, import scan). */
export const RUNNERS: StoreRunner[] = ['legendary', 'gog']

/** English names; for sideload prefer `runnerName()` (translated). */
export const RUNNER_NAMES: Record<Runner, string> = {
  legendary: 'Epic Games',
  gog: 'GOG',
  sideload: 'Other games'
}

export type InstallPlatform = 'windows' | 'linux' | 'mac'

export interface InstalledInfo {
  executable: string
  installPath: string
  installSize: string
  installSizeBytes: number
  isDlc: boolean
  version: string
  platform: InstallPlatform
  language?: string
  buildId?: string
  branch?: string
  installedDlcs?: string[]
  pinnedVersion?: boolean
}

export interface GameInfo {
  runner: Runner | null
  appName: string
  title: string
  artCover: string
  artSquare: string
  artLogo?: string
  artBackground?: string
  artIcon?: string
  developer?: string
  description?: string
  storeUrl?: string
  folderName?: string
  isInstalled: boolean
  installable: boolean
  isDlc: boolean
  canRunOffline: boolean
  cloudSaveEnabled: boolean
  saveFolder?: string
  version?: string
  thirdPartyManagedApp?: string
  updateAvailable: boolean
  install: InstalledInfo | null
}

export type GameStatusKind =
  | 'queued'
  | 'installing'
  | 'updating'
  | 'preparing'
  | 'launching'
  | 'playing'
  | 'uninstalling'
  | 'repairing'
  | 'moving'
  | 'syncing'
  /** a download left from the last session; waits for "Resume" */
  | 'paused'
  | 'error'
  | 'done'

export interface DownloadProgress {
  percent: number
  bytesDone: number
  bytesTotal: number
  speed: number
  diskSpeed: number
  eta: number | null
}

export interface GameStatus {
  appName: string
  runner: Runner
  status: GameStatusKind
  progress?: DownloadProgress
  message?: string
}

export interface StoreStatus {
  runner: Runner
  loggedIn: boolean
  username: string | null
  helperAvailable: boolean
}

export interface InstallInfo {
  appName: string
  title: string
  version: string
  downloadSize: number
  diskSize: number
  platform: InstallPlatform
  /** GOG: names of the other branches (beta…); the public one is unnamed */
  branches: string[]
  /** GOG: every installable build, newest first */
  builds: BuildEntry[]
}

export interface BuildEntry {
  buildId: string
  version: string
  date: string
  branch?: string
}

export interface Achievement {
  id: string
  name: string
  description: string
  iconUnlocked: string
  iconLocked: string
  rarity: number
  rarityLabel: string
  hidden: boolean
  unlocked: boolean
  unlockedAt?: string
}

export interface Achievements {
  total: number
  unlocked: number
  list: Achievement[]
}

export interface EnvVar {
  key: string
  value: string
}

/** Per-game launch options (core/src/config/game.rs) */
export interface GameSettings {
  launchArgs: string
  targetExe: string
  wrapper: string
  env: EnvVar[]
  gogBranchPassword: string
}

export interface DlcEntry {
  appName: string
  title: string
  installed: boolean
  installable: boolean
}

export interface ImportCandidate {
  runner: Runner
  appName: string
  title: string
  path: string
}

export type DownloadKind = 'install' | 'update' | 'repair'

export interface DownloadItem {
  runner: Runner
  appName: string
  title: string
  kind: DownloadKind
  path?: string
  platform: InstallPlatform
  running: boolean
  /** restored from the last session: waits in the queue until resumed */
  paused?: boolean
  branch?: string
  buildId?: string
}

/** What quitting now would interrupt (titles), see src-tauri/src/close.rs. */
export interface CloseBlockers {
  downloads: string[]
  syncing: string[]
}

export interface AppInfo {
  version: string
  platform: 'windows' | 'linux'
  configDir: string
  logsDir: string
  startInConsoleCli: boolean
  alwaysConsole: boolean
}

export type ConsoleModeDefaultTab = 'all' | 'installed' | Runner
export type ConsoleModeKeyboardLayout = 'tvos' | 'qwerty'

export interface AppSettings {
  language: string
  defaultInstallPath: string
  theme: string
  appearance: 'system' | 'light' | 'dark'
  accentColor: string
  exitToTray: boolean
  maxWorkers: number
  autoSyncSaves: boolean

  libraryInstalledFirst: boolean
  libraryGrayscaleUninstalled: boolean
  hideDuplicates: boolean
  hideDuplicatesFrom: Runner

  startInConsoleMode: boolean
  consoleModeDefaultTab: ConsoleModeDefaultTab
  consoleModeKeyboardLayout: ConsoleModeKeyboardLayout
  consoleModeTheme: string
  consoleModeSounds: boolean
  consoleModeShowCardTitles: boolean
  consoleModeShowCardStore: boolean
  consoleShowProgressPercent: boolean


  /** Linux: '' = <default install path>/Prefixes */
  winePrefixesDir: string

  verboseLogs: boolean
}

/** Shape every failed invoke() rejects with (see core/src/error.rs). */
export interface BackendError {
  kind:
    | 'io'
    | 'json'
    | 'not_logged_in'
    | 'helper_missing'
    | 'helper'
    | 'game_not_found'
    | 'busy'
    | 'cancelled'
    | 'secrets'
    | 'not_implemented'
    | 'other'
  message: string
}

/** Store-page details for the game page (core::types::GameExtra). */
export interface GameExtra {
  description?: string
  developer?: string
  publisher?: string
  /** YYYY-MM-DD */
  releaseDate?: string
  tags: string[]
  screenshots: string[]
}

/** Recorded play time (core::types::PlayStats). */
export interface PlayStats {
  totalSecs: number
  /** RFC 3339 */
  lastPlayed?: string
}

/** A game added by hand, as stored (image fields are file names). */
export interface SideloadGame {
  appName: string
  title: string
  executable: string
  launchArgs: string
  cover?: string
  background?: string
  icon?: string
}

/** Add / edit form. Images: `null` keeps the current one, `''` removes it,
 *  a path copies that file in. */
export interface SideloadInput {
  title: string
  executable: string
  launchArgs: string
  cover: string | null
  background: string | null
}

/** One cloud-save folder (Epic: a single one named "default"). */
export interface SaveLocation {
  name: string
  path: string
}

export interface SaveLocationInfo {
  name: string
  /** Empty when the store's path could not be worked out. */
  path: string
  exists: boolean
}

export interface CloudSaves {
  supported: boolean
  locations: SaveLocationInfo[]
  /** changed offline (or the upload failed); uploaded once the store is reachable */
  pendingUpload: boolean
}

export type SaveSyncMode = 'download' | 'upload' | 'forceDownload' | 'forceUpload'

/** A folder in the console-mode folder picker. */
export interface FolderEntry {
  name: string
  path: string
  /** drives only: bytes free / in total on the volume */
  free?: number
  total?: number
}

/** A place the picker jumps to (core::browse::PlaceEntry). */
export interface PlaceEntry {
  id: 'games' | 'home' | 'documents' | 'desktop' | 'downloads' | string
  path: string
}

export interface FolderListing {
  /** Empty for the list of drives. */
  path: string
  /** bytes free on the volume of `path` */
  free?: number
  /** One level up: "" for the drives, null when already there. */
  parent: string | null
  folders: FolderEntry[]
}
