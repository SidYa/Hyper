export function formatBytes(bytes: number): string {
  if (!bytes || bytes < 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let v = bytes
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`
}

export function formatSpeed(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/s`
}

export function formatEta(seconds: number | null | undefined): string {
  if (seconds == null) return '—'
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  const s = seconds % 60
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
  return `${m}:${String(s).padStart(2, '0')}`
}

/** Play time as "12 h 5 min" / "35 min" / "< 1 min" (units translated by the caller). */
export function formatPlaytime(seconds: number, units: { h: string; min: string }): string {
  if (seconds < 60) return `< 1 ${units.min}`
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  return h > 0 ? `${h} ${units.h}${m ? ` ${m} ${units.min}` : ''}` : `${m} ${units.min}`
}

/** Store descriptions arrive as Markdown with images, links and some HTML;
 *  reduce them to plain paragraphs for the TV page. */
export function plainText(md: string): string {
  return md
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '') // images
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1') // links -> text
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, '') // other tags
    .replace(/^#{1,6}\s*/gm, '') // headings
    .replace(/(\*\*|__|\*|_)(\S[^*_]*?)\1/g, '$2') // bold / italic
    .replace(/^\s*[-*]\s+/gm, '• ')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}
