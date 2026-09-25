// Lien magnet ou fichier .torrent reçu au lancement (double-clic sur un .torrent, lien magnet
// d'un navigateur). L'interface peut être prête avant le moteur : tant qu'il démarre, la
// demande échoue ; on réessaie au lieu de perdre le lien.

/** Réessaie `take` toutes les `delayMs` jusqu'à réussite (au plus `attempts` fois). Renvoie de
 * quoi arrêter (composant démonté). */
export function takeLaunchLinks (
  take: () => Promise<string[]>,
  onLinks: (links: string[]) => void,
  { attempts = 50, delayMs = 200 }: { attempts?: number, delayMs?: number } = {}
): () => void {
  let stopped = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const attempt = (n: number) => {
    take()
      .then((links) => { if (!stopped && links.length) onLinks(links) })
      .catch(() => {
        if (!stopped && n + 1 < attempts) timer = setTimeout(() => attempt(n + 1), delayMs)
      })
  }
  attempt(0)
  return () => {
    stopped = true
    clearTimeout(timer)
  }
}
