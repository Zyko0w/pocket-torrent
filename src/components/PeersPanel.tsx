import { useCallback, useState } from 'react'
import { SpinnerGap, Users } from '@phosphor-icons/react'
import { api, errorMessage, type Peer } from '../api'
import { formatSize, formatSpeed, plural } from '../format'
import { useInterval } from '../hooks'

export function PeersPanel ({ jobId }: { jobId: string }) {
  const [peers, setPeers] = useState<Peer[] | null>(null)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setPeers(await api.peers(jobId))
      setError('')
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [jobId])

  useInterval(load, 2000)

  if (error) return <div className='panel'><p className='field-error' role='alert'>Impossible de charger les pairs : {error}</p></div>
  if (!peers) {
    return <div className='panel'><span className='meta-item muted'><SpinnerGap size={16} className='spin' /> Chargement des pairs…</span></div>
  }
  if (!peers.length) {
    return <div className='panel'><span className='meta-item muted'><Users size={16} /> Aucun pair connecté pour l'instant.</span></div>
  }

  return (
    <div className='panel'>
      <p className='muted' style={{ fontSize: 12.5 }}>
        {plural(peers.length, 'pair')} connecté{peers.length > 1 ? 's' : ''}, les plus actifs en premier
      </p>
      <div className='table-wrap' tabIndex={0} role='region' aria-label='Pairs connectés'>
        <table className='peers-table'>
          <thead>
            <tr>
              <th scope='col'>Adresse</th>
              <th scope='col'>Client</th>
              <th scope='col'>Connexion</th>
              <th scope='col' className='num'>↓ Reçu</th>
              <th scope='col' className='num'>↑ Envoyé</th>
            </tr>
          </thead>
          <tbody>
            {peers.map((peer) => (
              <tr key={peer.address}>
                <td className='mono selectable'>{peer.address}</td>
                <td className='client' title={peer.client ?? ''}>{peer.client ?? '—'}</td>
                <td className='muted'>{peer.connection?.toUpperCase() ?? '—'}</td>
                <td className='num'>{peer.downSpeed > 0 ? formatSpeed(peer.downSpeed) : '—'} <span className='muted'>({formatSize(peer.downloaded)})</span></td>
                <td className='num'>{peer.upSpeed > 0 ? formatSpeed(peer.upSpeed) : '—'} <span className='muted'>({formatSize(peer.uploaded)})</span></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
