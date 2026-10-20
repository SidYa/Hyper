// Appearance: light / dark / system palette and the accent colour.
//
// The accent is used in three roles, each with its own contrast rule:
//   --accent         filled backgrounds (buttons, selected chips, keys)
//   --accent-text    text/icons ON the accent (picked black or white by luminance)
//   --accent-overlay accent used AS text/outline on the page background
//                    (links, focus rings); lightened on dark themes and
//                    darkened on light themes until it reaches WCAG 4.5:1.
export type Appearance = 'system' | 'light' | 'dark'

// Rainbow order. Purple keeps the default's value (settings.svelte.ts, the
// first-run accent), and the older presets keep theirs so saved choices still
// match a swatch.
export const ACCENT_PRESETS: Array<{ id: string; color: string }> = [
  { id: 'red', color: '#e5484d' },
  { id: 'orange', color: '#f07c1b' },
  { id: 'yellow', color: '#f5c518' },
  { id: 'green', color: '#1fa463' },
  { id: 'cyan', color: '#1fb5d6' },
  { id: 'blue', color: '#2f7cf6' },
  { id: 'purple', color: '#6a4df6' }
]
export const DEFAULT_ACCENT = '#6a4df6'

const BG = { dark: '#0f1120', light: '#f4f5fa' }

type RGB = [number, number, number]

function parseHex(hex: string): RGB | null {
  const m = hex.trim().match(/^#?([0-9a-f]{3}|[0-9a-f]{6})$/i)
  if (!m) return null
  let h = m[1]
  if (h.length === 3) h = h.split('').map((c) => c + c).join('')
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as RGB
}

const toHex = (c: RGB) => '#' + c.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join('')

function luminance([r, g, b]: RGB): number {
  const ch = (v: number) => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

export function contrast(a: RGB, b: RGB): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p)
  return (x + 0.05) / (y + 0.05)
}

const mix = (a: RGB, b: RGB, t: number): RGB => [0, 1, 2].map((i) => a[i] + (b[i] - a[i]) * t) as RGB

/** Move `c` toward white (dark bg) or black (light bg) until it is readable on `bg`. */
function readableOn(c: RGB, bg: RGB, target = 4.5): RGB {
  const toward: RGB = luminance(bg) < 0.5 ? [255, 255, 255] : [0, 0, 0]
  let out = c
  for (let t = 0; t <= 1.0001 && contrast(out, bg) < target; t += 0.05) out = mix(c, toward, t)
  return out
}

function toHsl([r, g, b]: RGB): [number, number, number] {
  const [rn, gn, bn] = [r / 255, g / 255, b / 255]
  const max = Math.max(rn, gn, bn)
  const min = Math.min(rn, gn, bn)
  const l = (max + min) / 2
  if (max === min) return [0, 0, l]
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  let h = max === rn ? (gn - bn) / d + (gn < bn ? 6 : 0) : max === gn ? (bn - rn) / d + 2 : (rn - gn) / d + 4
  return [h * 60, s, l]
}

function fromHsl(h: number, s: number, l: number): RGB {
  const k = (n: number) => (n + h / 30) % 12
  const a = s * Math.min(l, 1 - l)
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)))
  return [f(0) * 255, f(8) * 255, f(4) * 255]
}

/**
 * Background and text palette tinted with the accent's hue. Saturation is
 * scaled by the accent's own saturation, so a grey custom accent gives a
 * neutral grey theme. Lightness stays fixed per role, which keeps text
 * contrast the same for every accent.
 */
export function paletteVars(hex: string, mode: 'light' | 'dark') {
  const c = parseHex(hex) ?? parseHex(DEFAULT_ACCENT)!
  const [h, sat] = toHsl(c)
  const tint = (s: number, l: number) => toHex(fromHsl(h, s * Math.min(1, sat * 1.15), l))
  if (mode === 'dark') {
    return {
      '--body-background': tint(0.34, 0.085),
      '--background-darker': tint(0.34, 0.065),
      '--navbar-background': tint(0.34, 0.105),
      '--background': tint(0.3, 0.145),
      '--current-background': tint(0.34, 0.19),
      '--background-lighter': tint(0.26, 0.23),
      '--text-default': tint(0.4, 0.95),
      '--text-secondary': tint(0.16, 0.72),
      '--text-tertiary': tint(0.1, 0.54)
    }
  }
  return {
    '--body-background': tint(0.28, 0.96),
    '--background': '#ffffff',
    '--background-darker': tint(0.24, 0.925),
    '--navbar-background': tint(0.26, 0.94),
    '--current-background': tint(0.34, 0.885),
    '--background-lighter': tint(0.2, 0.87),
    '--text-default': tint(0.3, 0.12),
    '--text-secondary': tint(0.14, 0.34),
    '--text-tertiary': tint(0.08, 0.5)
  }
}

/** Large bold text (the logo) only needs this much contrast. */
const LOGO_CONTRAST = 2.5

/** Move `c` toward `away` until `text` on it reaches the logo's contrast. */
function keepContrast(c: RGB, text: RGB, away: RGB): RGB {
  let out = c
  for (let t = 0.05; t <= 1.0001 && contrast(out, text) < LOGO_CONTRAST; t += 0.05) out = mix(c, away, t)
  return out
}

export function accentVars(hex: string, mode: 'light' | 'dark') {
  const c = parseHex(hex) ?? parseHex(DEFAULT_ACCENT)!
  const bg = parseHex(paletteVars(hex, mode)['--body-background']) ?? parseHex(BG[mode])!
  const white: RGB = [255, 255, 255]
  const black: RGB = [17, 17, 17]
  const onAccent = contrast(c, white) >= contrast(c, black) ? white : black
  const overlay = readableOn(c, bg)
  const hover = mix(c, onAccent === white ? white : black, 0.12)
  // Logo: the word is light on the accent, dark only on light accents (white,
  // yellow…). It is large bold text, so a softer bar than buttons applies.
  const logoText = contrast(c, white) >= LOGO_CONTRAST ? white : black
  // The tile's animated gradient: the accent, a neighbouring hue and a
  // deeper (or, under dark text, paler) shade, each kept readable under the
  // word.
  const [h, s, l] = toHsl(c)
  const awayFromText: RGB = logoText === white ? [0, 0, 0] : white
  // greys and white have no hue to shift: shimmer in lightness instead
  const neutral = s < 0.12
  // orange / yellow shift toward red; the other way they turn olive
  const neighbour = neutral ? mix(c, logoText, 0.08) : fromHsl((h + (h >= 15 && h <= 75 ? 325 : 40)) % 360, s, l)
  const deeper = mix(c, awayFromText, 0.2)
  // white cannot get any paler under dark text: deepen toward the text instead
  const shade = contrast(deeper, c) < 1.1 ? mix(c, logoText, 0.16) : deeper
  const logoStops = [c, neighbour, shade].map((stop) => keepContrast(stop, logoText, awayFromText))
  return {
    '--logo-text': toHex(logoText),
    '--logo-a': toHex(logoStops[0]),
    '--logo-b': toHex(logoStops[1]),
    '--logo-c': toHex(logoStops[2]),
    '--accent': toHex(c),
    '--accent-text': toHex(onAccent),
    '--accent-overlay': toHex(overlay),
    '--primary': toHex(c),
    '--primary-hover': toHex(hover)
  }
}

let media: MediaQueryList | null = null
let mediaListener: (() => void) | null = null

function resolve(mode: Appearance): 'light' | 'dark' {
  if (mode !== 'system') return mode
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
}

/**
 * Apply theme + accent to <html>. `forceDark` keeps console mode dark (it is
 * a TV interface drawn over artwork).
 */
export function applyAppearance(mode: Appearance, accent: string, forceDark = false) {
  const root = document.documentElement
  const set = () => {
    const resolved = forceDark ? 'dark' : resolve(mode)
    root.dataset.theme = resolved
    root.style.colorScheme = resolved
    const vars = { ...paletteVars(accent, resolved), ...accentVars(accent, resolved) }
    for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v)
  }
  set()
  if (media && mediaListener) media.removeEventListener('change', mediaListener)
  media = null
  mediaListener = null
  if (mode === 'system' && !forceDark) {
    media = window.matchMedia('(prefers-color-scheme: light)')
    mediaListener = set
    media.addEventListener('change', set)
  }
}
