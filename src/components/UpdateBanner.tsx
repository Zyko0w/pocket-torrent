// Mises à jour signées (GitHub Releases) : proposées dans un bandeau, jamais installées d'office.
import { useEffect, useState } from 'react'
import { ArrowCircleUp, SpinnerGap, X } from '@phosphor-icons/react'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { api, errorMessage } from '../api'

const FOUND_EVENT = 'pocket-torrent:update-found'

/** Cherche une version plus récente ; `null` si l'appli est à jour. Annonce le bandeau sinon. */
export async function checkForUpdate (): Promise<Update | null> {
  const update = await check()
  if (update) window.dispatchEvent(new CustomEvent<Update>(FOUND_EVENT, { detail: update }))
  return update
}

export function UpdateBanner () {
  const [update, setUpdate] = useState<Update | null>(null)
  const [progress, setProgress] = useState<number | null>(null)
  const [error, setError] = useState('')

  useEffect(() => {
    const onFound = (e: Event) => { setUpdate((e as CustomEvent<Update>).detail); setError('') }
    window.addEventListener(FOUND_EVENT, onFound)
    // vérification au démarrage, sauf si désactivée ou en mode confidentialité renforcée (§9.6) ;
    // silencieuse en cas d'échec (hors ligne…)
    api.getSettings()
      .then(({ settings }) => settings.checkUpdates && !settings.privacy.hardenedMode ? checkForUpdate() : null)
      .catch(() => {})
    return () => window.removeEventListener(FOUND_EVENT, onFound)
  }, [])

  if (!update) return null

  const install = async () => {
    setError('')
    setProgress(0)
    let total = 0
    let received = 0
    try {
      // téléchargement et vérification de la signature, moteur encore en marche
      await update.download((event) => {
        if (event.event === 'Started') total = event.data.contentLength ?? 0
        if (event.event === 'Progress') {
          received += event.data.chunkLength
          if (total) setProgress(Math.min(99, Math.floor((received / total) * 100)))
        }
      })
    } catch (err) {
      setProgress(null)
      setError(`Mise à jour impossible : ${errorMessage(err)}`)
      return
    }
    try {
      // l'installeur ferme l'appli : on arrête d'abord le moteur proprement (état sauvegardé,
      // redirections de ports retirées de la box)
      await api.prepareForUpdate()
      await update.install()
      await relaunch()
    } catch {
      // moteur déjà arrêté : on redémarre sur l'ancienne version plutôt que rester à l'arrêt
      await relaunch()
    }
  }

  return (
    <div className='banner info' role='status'>
      <ArrowCircleUp size={16} />
      <span className='banner-text'>
        {error || <>Pocket Torrent <b>{update.version}</b> est disponible. Les téléchargements reprendront tout seuls après le redémarrage.</>}
      </span>
      {progress === null
        ? (
          <>
            <button type='button' className='btn btn-primary btn-sm' onClick={install}>Installer et redémarrer</button>
            <button type='button' className='icon-btn' title='Plus tard' aria-label='Plus tard' onClick={() => setUpdate(null)}><X size={16} /></button>
          </>
          )
        : <span className='banner-progress'><SpinnerGap size={14} className='spin' /> {progress} %</span>}
    </div>
  )
}
