const UNITS = ['o', 'Ko', 'Mo', 'Go', 'To']

export function formatSize (bytes: number): string {
  let value = Math.max(0, bytes || 0)
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit++
  }
  return `${value.toLocaleString('fr-FR', { maximumFractionDigits: unit ? 1 : 0 })} ${UNITS[unit]}`
}

export const formatSpeed = (bytesPerSecond: number) => `${formatSize(bytesPerSecond)}/s`

export function formatEta (seconds: number): string {
  const s = Math.max(0, Math.round(seconds))
  if (s < 60) return `${s} s`
  if (s < 3600) return `${Math.round(s / 60)} min`
  const hours = Math.floor(s / 3600)
  if (hours >= 48) return `${Math.round(hours / 24)} j`
  return `${hours} h ${String(Math.round((s % 3600) / 60)).padStart(2, '0')}`
}

export const plural = (count: number, word: string) => `${count} ${word}${count > 1 ? 's' : ''}`

export const percent = (done: number, total: number) => (total > 0 ? Math.min(1, done / total) : 0)

export const formatRatio = (uploaded: number, wanted: number) =>
  (wanted > 0 ? uploaded / wanted : 0).toLocaleString('fr-FR', { minimumFractionDigits: 2, maximumFractionDigits: 2 })

export function formatDate (ms: number): string {
  return new Date(ms).toLocaleString('fr-FR', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })
}

export const PRIORITY_LABELS = { high: 'Haute', normal: 'Normale', low: 'Basse' } as const
