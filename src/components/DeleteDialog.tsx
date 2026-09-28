import { useEffect, useState } from 'react'
import type { Job } from '../api'
import { hostLabel } from '../uploadHosts'
import { Modal } from './Modal'

interface DeleteDialogProps {
  job: Job | null
  onCancel: () => void
  onConfirm: (deleteFiles: boolean, deleteOnline: boolean, deleteSource: boolean) => void
}

export function DeleteDialog ({ job, onCancel, onConfirm }: DeleteDialogProps) {
  const [deleteFiles, setDeleteFiles] = useState(false)
  const [deleteOnline, setDeleteOnline] = useState(false)
  const [deleteSource, setDeleteSource] = useState(false)
  const finished = job ? job.status === 'done' || job.status === 'seeding' : false
  const running = Boolean(job && !finished && job.status !== 'error')
  // envoi sans compte chez un service qui ne permet pas de supprimer : rien à proposer
  const online = Boolean(job?.delivery?.links.length) && job?.delivery?.deletable !== false
  const source = job?.sourceFile ?? null
  // fichiers déjà envoyés puis effacés du PC : rien à supprimer localement
  const filesOnDisk = Boolean(job && !(job.kind === 'torrent' && !job.inEngine) && !job.id.startsWith('p:'))

  // un téléchargement inachevé n'a pas d'intérêt à être gardé : case cochée par défaut
  const jobId = job?.id
  useEffect(() => {
    // réinitialisé seulement quand la boîte s'ouvre pour un autre téléchargement
    if (jobId !== undefined) {
      setDeleteFiles(!finished)
      setDeleteOnline(false)
      setDeleteSource(false)
    }
  }, [jobId])

  return (
    <Modal open={job !== null} onClose={onCancel} labelledBy='delete-title'>
      <form onSubmit={(e) => { e.preventDefault(); onConfirm(filesOnDisk && deleteFiles, online && deleteOnline, Boolean(source) && deleteSource) }}>
        <div className='dialog-body'>
          <h2 id='delete-title'>{running ? 'Arrêter et supprimer ?' : 'Supprimer de la liste ?'}</h2>
          <p className='dialog-name'>{job?.name ?? job?.link ?? 'Téléchargement'}</p>
          <p className='help' style={{ margin: 0 }}>
            {running
              ? 'Le téléchargement sera arrêté et retiré de la liste.'
              : job?.status === 'seeding'
                ? 'Il sera retiré de la liste et ne sera plus partagé.'
                : 'Il sera retiré de la liste.'}
          </p>
          {filesOnDisk && (
            <label className='check'>
              <input type='checkbox' checked={deleteFiles} onChange={(e) => setDeleteFiles(e.target.checked)} />
              <span>
                Supprimer aussi les fichiers du disque
                <small>
                  {job && job.fileCount > 1
                    ? 'Les fichiers du torrent sont effacés, puis son dossier s’il est vide.'
                    : 'Le fichier téléchargé (ou commencé) est effacé.'}
                </small>
              </span>
            </label>
          )}
          {source && (
            <label className='check'>
              <input type='checkbox' checked={deleteSource} onChange={(e) => setDeleteSource(e.target.checked)} />
              <span>
                Supprimer aussi le fichier .torrent d’origine
                <small className='selectable' style={{ overflowWrap: 'anywhere' }}>{source}</small>
              </span>
            </label>
          )}
          {online && (
            <label className='check'>
              <input type='checkbox' checked={deleteOnline} onChange={(e) => setDeleteOnline(e.target.checked)} />
              <span>
                Supprimer aussi les fichiers sur {hostLabel(job?.delivery?.host, job?.delivery?.hostName)}
                <small>Les liens partagés ne fonctionneront plus.</small>
              </span>
            </label>
          )}
        </div>
        <div className='dialog-actions'>
          <button type='button' className='btn btn-ghost' onClick={onCancel}>Annuler</button>
          <button type='submit' className='btn btn-danger'>{running ? 'Arrêter et supprimer' : 'Supprimer'}</button>
        </div>
      </form>
    </Modal>
  )
}
