// Tiny i18n in the spirit of i18next: nested JSON keys + English fallback
// text inline at the call site. Locale files live in public/locales/<lang>/.
// The original project's 49 locale files use the same key layout, so they
// can be copied over as-is once the UI stabilises.

type Dict = { [key: string]: string | Dict }

export const i18n = $state<{ locale: string; dict: Dict; ready: boolean }>({
  locale: 'en',
  dict: {},
  ready: false
})

export const SUPPORTED_LOCALES: Array<{ code: string; name: string }> = [
  { code: 'en', name: 'English' },
  { code: 'uk', name: 'Українська' }
]

function lookup(dict: Dict, key: string): string | undefined {
  let node: string | Dict | undefined = dict
  for (const part of key.split('.')) {
    if (typeof node !== 'object' || node === null) return undefined
    node = node[part]
  }
  return typeof node === 'string' ? node : undefined
}

function interpolate(text: string, vars?: Record<string, string | number>) {
  if (!vars) return text
  return text.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k) => String(vars[k] ?? ''))
}

export function t(key: string, fallback?: string, vars?: Record<string, string | number>): string {
  const found = lookup(i18n.dict, key)
  return interpolate(found ?? fallback ?? key, vars)
}

export async function setLocale(locale: string) {
  const code = SUPPORTED_LOCALES.some((l) => l.code === locale) ? locale : 'en'
  try {
    const res = await fetch(`/locales/${code}/translation.json`)
    i18n.dict = res.ok ? ((await res.json()) as Dict) : {}
  } catch {
    i18n.dict = {}
  }
  i18n.locale = code
  i18n.ready = true
  document.documentElement.lang = code
}
