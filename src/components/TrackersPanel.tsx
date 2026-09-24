import { useCallback, useState } from 'react'
import { Broadcast, Copy, SpinnerGap } from '@phosphor-icons/react'
import { api, copyText, errorMessage, type Tracker } from '../api'
import { formatEta, plural } from '../format'
import { useInterval } from '../hooks'
import { useToast } from './Toast'

const STATES: Record<Tracker['state'], { dot: string; label: string }> = {
  working: { dot: 'ok', label: 'Fonctionne' },
  connecting: { dot: 'warn', label: 'Connexion…' },
  failing: { dot: 'bad', label: 'En échec' },
  skipped: { dot: '', label: 'Ignoré' },
  idle: { dot: '', label: 'Inactif' }
}

/** Détail d'un tracker : pairs reçus, prochaine annonce, ou raison de l'échec. */
function details (tracker: Tracker, now: number): string {
  const parts: string[] = []
  if (tracker.state === 'working') parts.push(`${plural(tracker.peers, 'pair')} à la dernière annonce`)
  if (tracker.state === 'idle') parts.push('torrent arrêté : aucune annonce')
  if (tracker.error) parts.push(tracker.error)
  if (tracker.state === 'failing' && tracker.lastSuccessAt) parts.push(`dernier succès il y a ${formatEta((now - tracker.lastSuccessAt) / 1000)}`)
  if (tracker.nextAnnounceAt && tracker.state !== 'skipped') {
    const wait = (tracker.nextAnnounceAt - now) / 1000
    parts.push(wait > 0 ? `prochaine annonce dans ${formatEta(wait)}` : 'annonce imminente')
  }
  return parts.join(' · ')
}

/** Trackers d'un torrent et état de leurs annonces (registre du moteur, voir engine.rs::trackers). */
export function TrackersPanel ({ jobId }: { jobId: string }) {
  const toast = useToast()
  const [trackers, setTrackers] = useState<Tracker[] | null>(null)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setTrackers(await api.trackers(jobId))
      setError('')
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [jobId])

  useInterval(load, 3000)

  if (error) return <div className='panel'><p className='field-error' role='alert'>Impossible de charger les trackers : {error}</p></div>
  if (!trackers) {
    return <div className='panel'><span className='meta-item muted'><SpinnerGap size={16} className='spin' /> Chargement des trackers…</span></div>
  }
  if (!trackers.length) {
    return <div className='panel'><span className='meta-item muted'><Broadcast size={16} /> Aucun tracker (magnet sans tracker, DHT seul).</span></div>
  }

  const now = Date.now()
  const working = trackers.filter((t) => t.state === 'working').length

  return (
    <div className='panel'>
      <p className='muted' style={{ fontSize: 12.5 }}>
        {plural(trackers.length, 'tracker')} · {working} fonctionne{working > 1 ? 'nt' : ''}
      </p>
      <ul className='file-list'>
        {trackers.map((tracker) => {
          const state = STATES[tracker.state]
          return (
            <li key={tracker.url}>
              <div className='panel-row tracker-row'>
                <span className={`dot ${state.dot}`} title={state.label} aria-hidden='true' />
                <div className='tracker-main'>
                  <span className='mono selectable' style={{ wordBreak: 'break-all' }}>{tracker.url}</span>
                  <small className='muted selectable'>
                    <b>{state.label}</b>{details(tracker, now) ? ` · ${details(tracker, now)}` : ''}
                  </small>
                </div>
                <button
                  type='button' className='icon-btn' title='Copier' aria-label='Copier ce tracker'
                  onClick={() => copyText(tracker.url).then(() => toast('Tracker copié')).catch((err) => toast(errorMessage(err)))}
                >
                  <Copy size={16} />
                </button>
              </div>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
