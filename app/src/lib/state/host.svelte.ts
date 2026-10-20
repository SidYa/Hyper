import { getAppInfo } from '$lib/api/commands'
import type { AppInfo } from '$types/index'

/** What Rust knows about this process (`get_app_info`), fetched once at start:
 * the OS the app runs on (Linux starts Windows games through umu + Proton),
 * whether this is the Deck edition (console mode only, no tray), and the full
 * answer (version, folders, CLI flags) for Settings and the boot. */
export const host = $state<{ linux: boolean; alwaysConsole: boolean; info: AppInfo | null }>({
  linux: false,
  alwaysConsole: false,
  info: null
})

/** The one fetch; App awaits it at boot, everyone else reads `host`. */
export const hostReady: Promise<AppInfo> = getAppInfo().then((info) => {
  host.info = info
  host.linux = info.platform === 'linux'
  host.alwaysConsole = info.alwaysConsole
  return info
})
// the boot reports the failure; nobody else needs to
hostReady.catch(() => {})
