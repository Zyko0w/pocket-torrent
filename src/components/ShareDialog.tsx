import { useEffect, useState } from 'react'
import type { Job, ShareOptions } from '../api'
import { Modal } from './Modal'

interface ShareDialogProps {
  job: Job | null
  onCancel: () => void
  onConfirm: (options: ShareOptions) => void
}

const PIECE_SIZES_KIB = [0, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384]

function pieceLabel (kib: number) {
  if (kib === 0) return 'Automatique'
  return kib >= 1024 ? `${kib / 1024} Mio` : `${kib} Kio`
}

/** Options de création avant de partager un fichier téléchargé en torrent (§5, §6.1). */
export function ShareDialog ({ job, onCancel, onConfirm }: ShareDialogProps) {
  const [pieceLengthKib, setPieceLengthKib] = useState(0)
  const [extraTrackers, setExtraTrackers] = useState('')
  const [isPrivate, setIsPrivate] = useState(false)

  const jobId = job?.id
  useEffect(() => {
    if (jobId !== undefined) {
      setPieceLengthKib(0)
      setExtraTrackers('')
      setIsPrivate(false)
    }
  }, [jobId])

  const submit = () => {
    onConfirm({
      pieceLengthKib,
      extraTrackers: extraTrackers.split('\n').map((t) => t.trim()).filter(Boolean),
      isPrivate
    })
  }

  return (
    <Modal open={job !== null} onClose={onCancel} labelledBy='share-title'>
      <form onSubmit={(e) => { e.preventDefault(); submit() }}>
        <div className='dialog-body'>
          <h2 id='share-title'>Partager en torrent</h2>
          <p className='dialog-name'>{job?.name ?? job?.link ?? 'Téléchargement'}</p>

          <div className='field'>
            <label htmlFor='share-piece'>Taille de pièce</label>
            <select id='share-piece' value={pieceLengthKib} onChange={(e) => setPieceLengthKib(Number(e.target.value))}>
              {PIECE_SIZES_KIB.map((kib) => <option key={kib} value={kib}>{pieceLabel(kib)}</option>)}
            </select>
          </div>

          <div className='field'>
            <label htmlFor='share-trackers'>Trackers supplémentaires <span className='optional'>(un par ligne)</span></label>
            <textarea
              id='share-trackers' className='input mono' rows={3} spellCheck={false}
              placeholder='udp://tracker.exemple.org:1337/announce'
              value={extraTrackers} onChange={(e) => setExtraTrackers(e.target.value)}
            />
          </div>

          <label className='check'>
            <input type='checkbox' checked={isPrivate} onChange={(e) => setIsPrivate(e.target.checked)} />
            <span>
              Torrent privé
              <small>
                Coupe la recherche de pairs par DHT et par découverte locale : seuls les trackers listés ci-dessus
                comptent. Les trackers publics ajoutés par défaut sont alors retirés — indique au moins un tracker
                à toi.
              </small>
            </span>
          </label>
        </div>
        <div className='dialog-actions'>
          <button type='button' className='btn btn-ghost' onClick={onCancel}>Annuler</button>
          <button type='submit' className='btn btn-primary' disabled={isPrivate && !extraTrackers.trim()}>Créer et partager</button>
        </div>
      </form>
    </Modal>
  )
}
