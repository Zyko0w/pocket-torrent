import { useEffect, useState } from 'react'
import { SpinnerGap, WarningCircle } from '@phosphor-icons/react'
import { api, errorMessage, type AddOptions, type Preview } from '../api'
import { Modal } from './Modal'
import { FilePicker } from './FilePicker'

export interface AddRequest {
  link: string
  options: AddOptions
}

interface AddDialogProps {
  request: AddRequest | null
  onClose: () => void
  onAdded: (name: string) => void
}

/** Aperçu d'un torrent : choix des fichiers avant de lancer le téléchargement. */
export function AddDialog ({ request, onClose, onAdded }: AddDialogProps) {
  const [preview, setPreview] = useState<Preview | null>(null)
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [error, setError] = useState('')
  const [adding, setAdding] = useState(false)
  const link = request?.link ?? null
  const dangerCount = preview?.files.filter((f) => f.danger).length ?? 0

  useEffect(() => {
    if (!link) return
    let cancelled = false
    setPreview(null)
    setError('')
    api.preview(link)
      .then((result) => {
        if (cancelled) return
        setPreview(result)
        // les fichiers potentiellement dangereux sont décochés par défaut, pas bloqués
        setSelected(new Set(result.files.filter((f) => !f.danger).map((f) => f.index)))
      })
      .catch((err) => { if (!cancelled) setError(errorMessage(err)) })
    return () => { cancelled = true }
  }, [link])

  const confirm = async () => {
    if (!request || !preview) return
    setAdding(true)
    try {
      const everything = selected.size === preview.files.length
      const report = await api.addLinks([request.link], { ...request.options, onlyFiles: everything ? null : [...selected] })
      if (report.errors.length) {
        setError(report.errors.join(' · '))
        return
      }
      onAdded(preview.name ?? 'Torrent')
      onClose()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setAdding(false)
    }
  }

  return (
    <Modal open={request !== null} onClose={onClose} labelledBy='add-title'>
      <div className='dialog-body'>
        <h2 id='add-title'>Choisir les fichiers</h2>
        {preview && <p className='dialog-name'>{preview.name ?? 'Torrent sans nom'}</p>}
        {!preview && !error && (
          <p className='meta-item muted'>
            <SpinnerGap size={16} className='spin' /> Recherche des infos du torrent auprès des pairs…
          </p>
        )}
        {error && (
          <p className='field-error' role='alert'>
            <WarningCircle size={16} /> {error}
          </p>
        )}
        {dangerCount > 0 && (
          <p className='field-error' role='alert'>
            <WarningCircle size={16} /> {dangerCount} fichier{dangerCount > 1 ? 's' : ''} potentiellement dangereux
            (déjà décoché{dangerCount > 1 ? 's' : ''}) : vérifie avant de {dangerCount > 1 ? 'les' : 'le'} sélectionner.
          </p>
        )}
        {preview && <FilePicker files={preview.files} selected={selected} onChange={setSelected} />}
      </div>
      <div className='dialog-actions'>
        <button type='button' className='btn btn-ghost' onClick={onClose}>Annuler</button>
        <button type='button' className='btn btn-primary' disabled={!preview || selected.size === 0 || adding} onClick={confirm}>
          {adding ? <SpinnerGap size={16} className='spin' /> : null}
          Télécharger la sélection
        </button>
      </div>
    </Modal>
  )
}
