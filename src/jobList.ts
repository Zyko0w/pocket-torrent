// Recherche et tri de la liste des téléchargements (côté interface seulement).
import type { Job } from './api'

export type SortKey = 'default' | 'name' | 'size' | 'progress' | 'speed'

export const SORTS: { key: SortKey; label: string }[] = [
  { key: 'default', label: 'Plus récents d’abord' },
  { key: 'name', label: 'Nom (A → Z)' },
  { key: 'size', label: 'Taille (grands d’abord)' },
  { key: 'progress', label: 'Progression' },
  { key: 'speed', label: 'Vitesse' }
]

/** Minuscules et sans accents : « Été » trouve « ete ». */
function fold (text: string): string {
  return text.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase()
}

/** Garde les téléchargements dont le nom (ou le lien) contient tous les mots de `query`. */
export function searchJobs (jobs: Job[], query: string): Job[] {
  const words = fold(query).split(/\s+/).filter(Boolean)
  if (!words.length) return jobs
  return jobs.filter((job) => {
    const haystack = fold(`${job.name ?? ''} ${job.link ?? ''}`)
    return words.every((w) => haystack.includes(w))
  })
}

const progressOf = (job: Job) => (job.wantedBytes > 0 ? job.progressBytes / job.wantedBytes : 0)
const byName = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' })

/** Trie sans modifier `jobs` ; « default » garde l'ordre reçu (le plus récent en premier). */
export function sortJobs (jobs: Job[], key: SortKey): Job[] {
  const sorted = [...jobs]
  switch (key) {
    case 'name': return sorted.sort((a, b) => byName.compare(a.name ?? a.link ?? '', b.name ?? b.link ?? ''))
    case 'size': return sorted.sort((a, b) => b.wantedBytes - a.wantedBytes)
    case 'progress': return sorted.sort((a, b) => progressOf(b) - progressOf(a))
    case 'speed': return sorted.sort((a, b) => b.downSpeed + b.upSpeed - (a.downSpeed + a.upSpeed))
    default: return sorted
  }
}

const SORT_STORAGE = 'torrent-cloud:sort'

/** Tri mémorisé d'une ouverture à l'autre (confort seulement : ignoré si le stockage échoue). */
export function loadSort (): SortKey {
  try {
    const saved = localStorage.getItem(SORT_STORAGE)
    if (SORTS.some((s) => s.key === saved)) return saved as SortKey
  } catch {}
  return 'default'
}

export function saveSort (key: SortKey) {
  try { localStorage.setItem(SORT_STORAGE, key) } catch {}
}
