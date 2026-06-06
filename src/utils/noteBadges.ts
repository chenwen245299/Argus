export const NOTE_BADGE_STYLES = [
  { backgroundColor: '#eef6ff', borderColor: '#bfdbfe', color: '#1d4ed8' },
  { backgroundColor: '#f0fdf4', borderColor: '#bbf7d0', color: '#15803d' },
  { backgroundColor: '#fff7ed', borderColor: '#fed7aa', color: '#c2410c' },
  { backgroundColor: '#f5f3ff', borderColor: '#ddd6fe', color: '#6d28d9' },
  { backgroundColor: '#fdf2f8', borderColor: '#fbcfe8', color: '#be185d' },
  { backgroundColor: '#ecfeff', borderColor: '#a5f3fc', color: '#0e7490' },
]

export function noteBadgeStyle(title: string, index: number) {
  let hash = index
  for (let i = 0; i < title.length; i += 1) {
    hash = (hash * 31 + title.charCodeAt(i)) >>> 0
  }
  return NOTE_BADGE_STYLES[hash % NOTE_BADGE_STYLES.length]
}
