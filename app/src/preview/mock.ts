// Dev-only browser preview (`/preview.html` on the Vite dev server, never in
// a build): the UI with Tauri IPC mocked, for layout checks and screenshots
// without the app (scripts/ui-shots.mjs drives it). Query parameters:
//   root=deck|desktop  DeckApp (console only, default) or App
//   lang=en|uk         UI language
//   platform=windows|linux
//   stores=both|epic|none   logged-in stores
//   empty              no games;  refreshing[=gog]  a store is "fetching"
//   pendingSaves       installed games have saves waiting for an upload
// Hooks for scripted checks: window.__mock.uninstall(title),
// window.__mock.savesPending(appName, pending) (the saves:pending event),
// window.__mock.queue(appName, kind, running?) (a download:queue event with one item),
// window.__mock.status(appName, status, extra?) (a game:status event, extra = message / progress) and
// window.__killed (appName of the last kill_game).
import { mount } from 'svelte'
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import { emit } from '@tauri-apps/api/event'
import { settings } from '$lib/state/settings.svelte'
import '../styles/app.css'

const q = new URLSearchParams(location.search)
const lang = q.get('lang') ?? 'en'
const platform = q.get('platform') ?? 'windows'
const stores = q.get('stores') ?? 'both'
const deck = q.get('root') !== 'desktop'
const runners = stores === 'both' ? ['legendary', 'gog'] : stores === 'epic' ? ['legendary'] : []

const titles = ['Disco Elysium', 'Limbo', 'Inside', 'Alan Wake 2', 'Control', 'Hades', 'Fortnite', 'Rocket League', 'Cyberpunk 2077', 'The Witcher 3: Wild Hunt', 'Death Stranding', 'Hollow Knight', 'Stardew Valley', 'Celeste', 'Firewatch', 'Outer Wilds']
const install = (title: string) => ({ executable: 'game.exe', installPath: `/games/${title}`, installSize: '12 GB', installSizeBytes: 12 * 1024 ** 3, isDlc: false, version: '1.0', platform: 'windows' })
const games = q.has('empty') || !runners.length
  ? []
  : titles.map((title, i) => ({
      runner: runners[i % runners.length],
      appName: `mock-${i}`,
      title,
      artCover: '',
      artSquare: '',
      developer: 'Studio',
      isInstalled: i < 3,
      installable: true,
      isDlc: false,
      canRunOffline: true,
      cloudSaveEnabled: true,
      updateAvailable: i === 4,
      // an EA title bought on Epic: installed and started through the EA app
      thirdPartyManagedApp: i === 5 ? 'the EA app' : undefined,
      install: i < 3 ? install(title) : null
    }))
// solid-colour "screenshots": corners and clipping are easy to see
const screenshots = ['#e5484d', '#f5c518', '#1fb5d6'].map(
  (c) => 'data:image/svg+xml,' + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="${c}"/></svg>`)
)

const hooks = window as unknown as { __mock: unknown; __killed?: string; __quit?: boolean }
hooks.__mock = {
  status(appName: string, status: string, extra: Record<string, unknown> = {}) {
    const g = games.find((x) => x.appName === appName)
    return g && emit('game:status', { runner: g.runner, appName, status, ...extra })
  },
  // a download in progress (the download:queue event), e.g. to show a status note
  queue(appName: string, kind: string, running = true, paused = false) {
    const g = games.find((x) => x.appName === appName)
    return g && emit('download:queue', [{ runner: g.runner, appName, title: g.title, kind, platform: 'windows', running, paused }])
  },
  // give a game a cover (the mock library has none), e.g. to see the install effect
  cover(appName: string, src: string) {
    const g = games.find((x) => x.appName === appName)
    if (!g) return
    g.artCover = src
    return emit('library:updated', { runner: null })
  },
  // the backend asks before quitting while downloads / save syncs run
  closeRequested(downloads: string[], syncing: string[] = []) {
    return emit('app:close-requested', { downloads, syncing })
  },
  savesPending(appName: string, pending: boolean) {
    const g = games.find((x) => x.appName === appName)
    return g && emit('saves:pending', { runner: g.runner, appName, pending })
  },
  uninstall(title: string) {
    const g = games.find((x) => x.title === title)
    if (!g) return
    g.isInstalled = false
    g.install = null
    return emit('library:updated', { runner: null })
  }
}

mockIPC(
  (cmd, args) => {
    const a = args as Record<string, unknown>
    switch (cmd) {
      case 'get_settings':
        return { ...settings.value, language: lang, defaultInstallPath: platform === 'linux' ? '/home/deck/Games' : 'C:/Games' }
      case 'save_settings':
        return a.settings
      case 'get_app_info':
        return { version: '1.0.0', platform, configDir: '/config', logsDir: '/logs', startInConsoleCli: deck, alwaysConsole: deck }
      case 'get_library':
        return structuredClone(games)
      case 'get_refreshing_stores':
        // `refreshing=gog`: a store with no games yet (its first load)
        return q.has('refreshing') ? (q.get('refreshing') ? [q.get('refreshing')] : runners.slice(0, 1)) : []
      case 'get_store_status':
        return ['legendary', 'gog'].map((runner) => ({
          runner,
          loggedIn: runners.includes(runner),
          username: runners.includes(runner) ? 'player' : null,
          helperAvailable: true
        }))
      case 'get_cloud_saves':
        return platform === 'linux'
          ? { supported: false, locations: [] }
          : { supported: true, locations: [{ name: 'Saves', path: 'C:/Users/player/Saves', exists: true }], pendingUpload: q.has('pendingSaves') }
      case 'get_game_extra':
        return { description: 'A long description of the game. '.repeat(12), developer: 'Studio', publisher: 'Publisher', releaseDate: '2023-10-27', tags: ['Action', 'Adventure'], screenshots }
      case 'get_install_info':
        // GOG games (odd indexes) show a beta branch and two builds per branch
        return {
          appName: a.appName, title: '', version: '1.0', downloadSize: 9.5e9, diskSize: 14.2e9, platform: 'windows',
          branches: a.runner === 'gog' ? ['beta'] : [],
          builds: a.runner === 'gog'
            ? [
                { buildId: 'b4', version: '1.2.1', date: '2026-10-01T10:00:00Z' },
                { buildId: 'b3', version: '1.2.0', date: '2026-08-12T10:00:00Z' },
                { buildId: 'b5', version: '1.3.0-beta', date: '2026-10-10T10:00:00Z', branch: 'beta' },
                { buildId: 'b2', version: '1.2.9-beta', date: '2026-09-20T10:00:00Z', branch: 'beta' }
              ]
            : []
        }
      case 'get_dlcs':
        return [
          { appName: `${a.appName}-dlc1`, title: 'Season Pass', installed: true, installable: a.runner === 'legendary' },
          { appName: `${a.appName}-dlc2`, title: 'Soundtrack', installed: false, installable: a.runner === 'legendary' }
        ]
      case 'get_achievements': {
        const mk = (i: number, unlocked: boolean) => ({
          id: `a${i}`, name: unlocked ? `Achievement ${i}` : `Locked one ${i}`, description: 'Do the thing that this achievement asks for, properly and in time.',
          iconUnlocked: screenshots[i % 3], iconLocked: screenshots[(i + 1) % 3], rarity: Math.max(0.5, 80 - i * 7), rarityLabel: unlocked ? 'Common' : 'Rare',
          hidden: !unlocked && i % 3 === 0, unlocked, unlockedAt: unlocked ? `2026-10-${String(1 + i).padStart(2, '0')}T12:00:00Z` : undefined
        })
        // a long list, so the page has to scroll well past the section's top
        const list = Array.from({ length: 40 }, (_, i) => mk(i + 1, i < 3))
        return { total: list.length, unlocked: 3, list }
      }
      case 'get_game_settings':
        return { launchArgs: '', targetExe: '', wrapper: '', env: [], gogBranchPassword: '' }
      case 'save_game_settings':
      case 'move_game':
        return null
      case 'get_play_stats':
        return { totalSecs: 45 * 3600, lastPlayed: '2026-10-10T20:00:00Z' }
      case 'list_folders': {
        // a small fake tree with '/' separators: drives, then four folders two levels deep
        const p = typeof a.path === 'string' && a.path ? a.path.replace(/\/$/, '') : ''
        if (!p) return { path: '', parent: null, folders: [{ name: 'C:', path: 'C:', free: 120e9, total: 931e9 }, { name: 'D:', path: 'D:', free: 1.4e12, total: 2e12 }] }
        const parts = p.split('/')
        const parent = parts.length > 1 ? parts.slice(0, -1).join('/') : ''
        const sub = parts.length > 3 ? [] : ['Games', 'Program Files', 'Users', 'Work'].map((n) => ({ name: n, path: p + '/' + n }))
        return { path: p, parent, folders: sub, free: 120e9 }
      }
      case 'list_folder_places':
        return [{ id: 'games', path: 'C:/Users/player/Games' }, { id: 'home', path: 'C:/Users/player' }, { id: 'documents', path: 'C:/Users/player/Documents' }]
      case 'create_folder':
        return String(a.parent) + '/' + String(a.name)
      case 'kill_game':
        hooks.__killed = String(a.appName)
        return null
      case 'get_downloads':
        // `paused`: a download left from the last session (the 4th game, not installed)
        return q.has('paused')
          ? [{ runner: games[3].runner, appName: games[3].appName, title: games[3].title, kind: 'install', platform: 'windows', running: false, paused: true }]
          : []
      case 'install_game':
      case 'resume_download':
        return []
      case 'get_close_blockers':
        return q.has('paused') ? { downloads: [games[3].title], syncing: [] } : { downloads: [], syncing: [] }
      case 'quit':
        hooks.__quit = (a.force as boolean) ?? false
        return null
      default:
        return null
    }
  },
  { shouldMockEvents: true }
)

// the desktop title bar asks for the current window
mockWindows('main')

const Root = deck ? (await import('../DeckApp.svelte')).default : (await import('../App.svelte')).default
mount(Root, { target: document.getElementById('app')! })
