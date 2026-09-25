import { useEffect, useId, useState } from 'react'
import { CloudArrowUp } from '@phosphor-icons/react'
import { api, errorMessage, type Job, type SendOptions } from '../api'
import { Modal } from './Modal'
import { Select } from './Select'
import { useToast } from './Toast'

/** Durées de vie acceptées par storage.to sans compte. */
export const EXPIRY_OPTIONS = [1, 2, 3, 4, 5, 6, 7].map((days) => ({ value: String(days), label: `${days} jour${days > 1 ? 's' : ''}` }))

/** Effacement de storage.to après N téléchargements (0 : jamais, seulement à l'expiration). */
export const DOWNLOAD_CAP_OPTIONS = [0, 1, 2, 3, 5, 10, 25, 50, 100, 1000].map((n) => ({
  value: String(n),
  label: n === 0 ? 'Sans limite' : n === 1 ? 'Après 1 téléchargement' : `Après ${n} téléchargements`
}))

interface SendDialogProps {
  job: Job
  onClose: () => void
  onSent: () => void
}

/** Envoi sur storage.to d'un téléchargement terminé, avec les choix préremplis par les réglages. */
export function SendDialog ({ job, onClose, onSent }: SendDialogProps) {
  const toast = useToast()
  const titleId = useId()
  const [options, setOptions] = useState<SendOptions | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let live = true
    const fallback: SendOptions = { expiryDays: 7, maxDownloads: 0, deleteLocal: false }
    api.getSettings()
      .then(({ settings: { storageTo } }) => ({ expiryDays: storageTo.expiryDays, maxDownloads: storageTo.maxDownloads, deleteLocal: false }))
      .catch(() => fallback)
      .then((loaded) => { if (live) setOptions(loaded) })
    return () => { live = false }
  }, [])

  const set = (patch: Partial<SendOptions>) => setOptions((o) => o && { ...o, ...patch })

  const send = async () => {
    if (!options) return
    setBusy(true)
    try {
      await api.sendOnline(job.id, options)
      toast('Envoi sur storage.to lancé')
      onSent()
      onClose()
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open onClose={onClose} labelledBy={titleId}>
      <form onSubmit={(e) => { e.preventDefault(); void send() }}>
        <div className='dialog-body'>
          <h2 id={titleId}>Envoyer sur storage.to</h2>
          <p className='dialog-name'>{job.name ?? job.link ?? 'Téléchargement'}</p>
          {options && (
            <>
              <div className='field'>
                <label htmlFor={`${titleId}-expiry`}>Durée de vie du lien</label>
                <Select id={`${titleId}-expiry`} ariaLabel='Durée de vie du lien' value={String(options.expiryDays)} onChange={(v) => set({ expiryDays: Number(v) })} options={EXPIRY_OPTIONS} />
              </div>
              <div className='field'>
                <label htmlFor={`${titleId}-cap`}>Effacer de storage.to</label>
                <Select id={`${titleId}-cap`} ariaLabel='Effacer de storage.to' value={String(options.maxDownloads)} onChange={(v) => set({ maxDownloads: Number(v) })} options={DOWNLOAD_CAP_OPTIONS} />
                <p className='help'>Au plus tard à la fin de la durée de vie.</p>
              </div>
              <label className='check'>
                <input type='checkbox' checked={options.deleteLocal} onChange={(e) => set({ deleteLocal: e.target.checked })} />
                <span>
                  Effacer les fichiers du PC une fois envoyés
                  <small>Attention : une fois le lien expiré, les fichiers ne seront plus nulle part.</small>
                </span>
              </label>
            </>
          )}
        </div>
        <div className='dialog-actions'>
          <button type='button' className='btn btn-ghost' onClick={onClose}>Annuler</button>
          <button type='submit' className='btn btn-primary' disabled={!options || busy}><CloudArrowUp size={16} /> Envoyer</button>
        </div>
      </form>
    </Modal>
  )
}
