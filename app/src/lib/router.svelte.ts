// Minimal hash router. Routes:
//   #/library            #/console           #/store/:runner?
//   #/game/:runner/:app  #/settings/:section?  #/downloads
export type RouteName = 'library' | 'console' | 'game' | 'settings' | 'downloads' | 'store'

export interface Route {
  name: RouteName
  params: Record<string, string>
  path: string
}

const PATTERNS: Array<{ name: RouteName; re: RegExp; keys: string[] }> = [
  { name: 'library', re: /^\/library\/?$/, keys: [] },
  { name: 'console', re: /^\/console\/?$/, keys: [] },
  { name: 'game', re: /^\/game\/([^/]+)\/([^/]+)\/?$/, keys: ['runner', 'appName'] },
  { name: 'settings', re: /^\/settings(?:\/([^/]+))?\/?$/, keys: ['section'] },
  { name: 'downloads', re: /^\/downloads\/?$/, keys: [] },
  { name: 'store', re: /^\/store(?:\/([^/]+))?\/?$/, keys: ['runner'] }
]

function parse(hash: string): Route {
  const path = hash.replace(/^#/, '') || '/library'
  for (const p of PATTERNS) {
    const m = path.match(p.re)
    if (!m) continue
    const params: Record<string, string> = {}
    p.keys.forEach((k, i) => {
      if (m[i + 1] !== undefined) params[k] = decodeURIComponent(m[i + 1])
    })
    return { name: p.name, params, path }
  }
  return { name: 'library', params: {}, path: '/library' }
}

export const router = $state<{ route: Route }>({
  route: parse(typeof location !== 'undefined' ? location.hash : '')
})

if (typeof window !== 'undefined') {
  window.addEventListener('hashchange', () => {
    router.route = parse(location.hash)
  })
}

export function navigate(path: string) {
  const next = path.startsWith('/') ? path : `/${path}`
  if (location.hash === `#${next}`) return
  location.hash = next
}

export function back() {
  history.back()
}
