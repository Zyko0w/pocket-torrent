import { useCallback, useState } from 'react'
import { Broadcast, Copy, SpinnerGap } from '@phosphor-icons/react'
import { api, copyText, errorMessage } from '../api'
import { plural } from '../format'
import { useInterval } from '../hooks'
import { useToast } from './Toast'

/** Trackers configurés pour un torrent. Pas d'état par tracker : librqbit ne le fait pas
 * remonter (voir engine.rs::trackers) — seulement la liste, à l'inverse de PeersPanel. */
export function TrackersPanel ({ jobId }: { jobId: string }) {
  const toast = useToast()
  const [trackers, setTrackers] = useState<string[] | null>(null)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setTrackers(await api.trackers(jobId))
      setError('')
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [jobId])

  useInterval(load, 5000)

  if (error) return <div className='panel'><p className='field-error' role='alert'>Impossible de charger les trackers : {error}</p></div>
  if (!trackers) {
    return <div className='panel'><span className='meta-item muted'><SpinnerGap size={16} className='spin' /> Chargement des trackers…</span></div>
  }
  if (!trackers.length) {
    return <div className='panel'><span className='meta-item muted'><Broadcast size={16} /> Aucun tracker (magnet sans tracker, DHT seul).</span></div>
  }

  return (
    <div className='panel'>
      <p className='muted' style={{ fontSize: 12.5 }}>
        {plural(trackers.length, 'tracker')} configuré{trackers.length > 1 ? 's' : ''}
      </p>
      <ul className='file-list'>
        {trackers.map((tracker) => (
          <li key={tracker}>
            <div className='panel-row'>
              <span className='mono selectable' style={{ wordBreak: 'break-all' }}>{tracker}</span>
              <button
                type='button' className='icon-btn' title='Copier' aria-label='Copier ce tracker'
                onClick={() => copyText(tracker).then(() => toast('Tracker copié')).catch((err) => toast(errorMessage(err)))}
              >
                <Copy size={16} />
              </button>
            </div>
          </li>
        ))}
      </ul>
    </div>
  )
}
