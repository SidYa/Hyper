import { errorMessage, getSettings, saveSettings } from '$lib/api/commands'
import { notify } from '$lib/state/toast.svelte'
import type { AppSettings } from '$types/index'

const DEFAULTS: AppSettings = {
  language: 'en',
  defaultInstallPath: '',
  theme: 'midnightMirage',
  appearance: 'dark',
  accentColor: '#6a4df6',
  exitToTray: false,
  maxWorkers: 0,
  autoSyncSaves: false,
  libraryInstalledFirst: true,
  libraryGrayscaleUninstalled: false,
  hideDuplicates: false,
  hideDuplicatesFrom: 'gog',
  startInConsoleMode: false,
  consoleModeDefaultTab: 'all',
  consoleModeKeyboardLayout: 'qwerty',
  consoleModeTheme: 'default',
  consoleModeSounds: true,
  consoleModeShowCardTitles: false,
  consoleModeShowCardStore: false,
  consoleShowProgressPercent: true,
  winePrefixesDir: '',
  verboseLogs: false
}

export const settings = $state<{ value: AppSettings; loaded: boolean; saving: boolean }>({
  value: { ...DEFAULTS },
  loaded: false,
  saving: false
})

export async function loadSettings() {
  settings.value = await getSettings()
  settings.loaded = true
  return settings.value
}

/** Persist a partial change. Optimistic: the UI updates first, then the
 *  backend. Saves run one after the other (a later patch must not overtake an
 *  earlier one on the IPC), only the newest answer is applied, and a failed
 *  save puts the keys of its patch back and toasts the error. */
let chain: Promise<void> = Promise.resolve()
let seq = 0
export function updateSettings(patch: Partial<AppSettings>): Promise<void> {
  const prev = settings.value
  const next = { ...prev, ...patch }
  settings.value = next
  settings.saving = true
  const mine = ++seq
  const run = async () => {
    try {
      const saved = await saveSettings(next)
      if (mine === seq) settings.value = saved
    } catch (e) {
      // undo this patch only: a later one may already show on top of it
      const undo = Object.fromEntries(Object.keys(patch).map((k) => [k, prev[k as keyof AppSettings]])) as Partial<AppSettings>
      settings.value = { ...settings.value, ...undo }
      notify(errorMessage(e), 'error')
    } finally {
      if (mine === seq) settings.saving = false
    }
  }
  chain = chain.then(run, run)
  return chain
}
