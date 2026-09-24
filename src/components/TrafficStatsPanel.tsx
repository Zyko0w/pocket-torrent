// Statistiques cumulées : reçu, envoyé et ratio global, depuis le début et pour cette session.
import { useCallback, useState } from 'react'
import { ArrowCounterClockwise } from '@phosphor-icons/react'
import { api, errorMessage, type TrafficStats } from '../api'
import { formatDate, formatRatio, formatSize } from '../format'
import { useInterval } from '../hooks'
import { useToast } from './Toast'

export function TrafficStatsPanel () {
  const toast = useToast()
  const [stats, setStats] = useState<TrafficStats | null>(null)
  const [confirming, setConfirming] = useState(false)

  const load = useCallback(() => { api.trafficStats().then(setStats).catch(() => {}) }, [])
  useInterval(load, 5000)

  const reset = async () => {
    if (!confirming) {
      setConfirming(true)
      return
    }
    setConfirming(false)
    try {
      setStats(await api.resetTrafficStats())
      toast('Statistiques remises à zéro')
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  if (!stats) return null
  return (
    <>
      <dl className='stats-list'>
        <div className='stat'><dt>Reçu</dt><dd>{formatSize(stats.downloaded)}</dd></div>
        <div className='stat'><dt>Envoyé</dt><dd>{formatSize(stats.uploaded)}</dd></div>
        <div className='stat'><dt>Ratio global</dt><dd>{formatRatio(stats.uploaded, stats.downloaded)}</dd></div>
      </dl>
      <p className='help'>
        {stats.since ? `Depuis le ${formatDate(stats.since)}` : 'Rien de compté pour l’instant'}
        {' · '}cette session : ↓ {formatSize(stats.sessionDownloaded)} · ↑ {formatSize(stats.sessionUploaded)}
      </p>
      <div className='panel-actions'>
        <button type='button' className='btn btn-ghost btn-sm' onClick={reset} onBlur={() => setConfirming(false)}>
          <ArrowCounterClockwise size={14} /> {confirming ? 'Confirmer la remise à zéro' : 'Remettre à zéro'}
        </button>
      </div>
    </>
  )
}
