// Shared presentation rules for game lists (Library and Console mode).
import { convertFileSrc } from '@tauri-apps/api/core'
import { t } from '$lib/i18n/index.svelte'
import { RUNNER_NAMES, type AppSettings, type GameInfo, type Runner } from '$types/index'

/** Local file paths (sideloaded games' images) become asset:// URLs. */
export function imageSrc(src: string | undefined | null): string {
  if (!src) return ''
  return /^(https?|data|blob|asset):/.test(src) ? src : convertFileSrc(src)
}

/** Store name for the UI ("Other games" is translated). */
export function runnerName(runner: Runner | null | undefined): string {
  if (!runner) return ''
  return runner === 'sideload' ? t('sideload.runner', 'Other games') : RUNNER_NAMES[runner]
}

/**
 * Portrait cover for a tile. Like Heroic we use the tall art and ask Epic's
 * CDN for a 300×400 rendition, which is ~10× smaller than the original and
 * already has the tile's aspect ratio (no cropping of titles).
 */
export function cardImage(game: GameInfo, size: 'card' | 'large' = 'card'): string {
  const src = game.artSquare || game.artCover
  if (!src) return ''
  if (game.runner === 'sideload') return imageSrc(src)
  if (game.runner === 'legendary' && /^https?:\/\//.test(src) && !src.includes('?')) {
    return size === 'large' ? `${src}?h=800&resize=1&w=600` : `${src}?h=400&resize=1&w=300`
  }
  return src
}

/** Wide key art for the game page hero (Epic CDN resized to 1920 wide). */
export function heroImage(game: GameInfo): string {
  const src = game.artBackground || game.artCover || game.artSquare
  if (!src) return ''
  if (game.runner === 'sideload') return imageSrc(src)
  if (game.runner === 'legendary' && /^https?:\/\//.test(src) && !src.includes('?')) return `${src}?w=1920&resize=1`
  return src
}

/** Title used to detect the same game in two stores. */
export function dedupeKey(title: string): string {
  return title
    .toLowerCase()
    .replace(/[™®©]/g, '')
    .replace(/\b(the|edition|game of the year|goty|definitive|complete)\b/g, '')
    .replace(/[^a-z0-9]+/g, '')
}

/** Drop the copy from `hideDuplicatesFrom` when a title exists in another store too. */
export function hideDuplicates(games: GameInfo[], settings: AppSettings): GameInfo[] {
  if (!settings.hideDuplicates) return games
  const hideFrom = settings.hideDuplicatesFrom
  const keep = new Map<string, Set<string>>()
  for (const g of games) {
    const k = dedupeKey(g.title)
    if (!keep.has(k)) keep.set(k, new Set())
    keep.get(k)!.add(g.runner ?? '')
  }
  return games.filter((g) => {
    if (g.runner !== hideFrom) return true
    const runners = keep.get(dedupeKey(g.title))
    // keep it when it is the only copy, or when this copy is installed and
    // the other one is not
    if (!runners || runners.size < 2) return true
    if (g.isInstalled) {
      const other = games.find((o) => o !== g && o.runner !== hideFrom && dedupeKey(o.title) === dedupeKey(g.title))
      if (other && !other.isInstalled) return true
    }
    return false
  })
}

/** Remove the other store's copy when ours is installed and theirs is not. */
function dropShadowedCopies(games: GameInfo[], settings: AppSettings): GameInfo[] {
  if (!settings.hideDuplicates) return games
  const hideFrom = settings.hideDuplicatesFrom
  return games.filter((g) => {
    if (g.runner === hideFrom || g.isInstalled) return true
    const twin = games.find((o) => o.runner === hideFrom && o.isInstalled && dedupeKey(o.title) === dedupeKey(g.title))
    return !twin
  })
}

/** `onTop` says which games form the first group (default: the installed
 *  ones; the screens also count games being installed). */
export function sortGames(games: GameInfo[], opts: { installedFirst: boolean; ascending?: boolean; onTop?: (g: GameInfo) => boolean }): GameInfo[] {
  const dir = opts.ascending === false ? -1 : 1
  const onTop = opts.onTop ?? ((g: GameInfo) => g.isInstalled)
  return [...games].sort((a, b) => {
    if (opts.installedFirst) {
      const ta = onTop(a)
      const tb = onTop(b)
      if (ta !== tb) return ta ? -1 : 1
    }
    return a.title.localeCompare(b.title) * dir
  })
}

/** Filter DLC/third-party, de-duplicate and sort, per the settings. */
export function prepareGames(games: GameInfo[], settings: AppSettings, ascending = true, onTop?: (g: GameInfo) => boolean): GameInfo[] {
  const base = games.filter((g) => !g.isDlc)
  const unique = dropShadowedCopies(hideDuplicates(base, settings), settings)
  return sortGames(unique, { installedFirst: settings.libraryInstalledFirst, ascending, onTop })
}

/** A store screenshot at `width` px. Epic's CDNs resize (and re-encode 4K PNGs
 *  as JPEG) on request, which cuts a 3 MB shot to ~150 KB. */
export function screenshotUrl(src: string, width: number): string {
  if (/^https:\/\/(cdn\d?\.unrealengine\.com|cdn\d\.epicgames\.com)\//.test(src) && !src.includes('?')) {
    return `${src}?resize=1&w=${width}`
  }
  return src
}
