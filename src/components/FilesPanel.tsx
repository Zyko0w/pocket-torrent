import { useCallback, useRef, useState } from 'react'
import { SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage, type TorrentFile } from '../api'
import { useInterval } from '../hooks'
import { FilePicker } from './FilePicker'
import { useToast } from './Toast'

/** Fichiers d'un torrent : progression et modification de la sélection. */
export function FilesPanel ({ jobId }: { jobId: string }) {
  const toast = useToast()
  const [files, setFiles] = useState<TorrentFile[] | null>(null)
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [dirty, setDirtyState] = useState(false)
  // lu au retour de la requête (et non à son départ) : une case cochée pendant le chargement
  // n'est pas écrasée par la sélection du moteur
  const dirtyRef = useRef(false)
  const setDirty = (value: boolean) => { dirtyRef.current = value; setDirtyState(value) }
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)

  const load = useCallback(async () => {
    try {
      const list = await api.files(jobId)
      setFiles(list)
      setError('')
      if (!dirtyRef.current) setSelected(new Set(list.filter((f) => f.included).map((f) => f.index)))
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [jobId])

  // gros torrents : la liste complète pèse lourd, rafraîchie moins souvent
  useInterval(load, (files?.length ?? 0) > 2000 ? 10000 : 2000)

  const play = async (file: TorrentFile) => {
    try {
      await api.playFile(jobId, file.index)
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  const reorder = async (index: number, up: boolean) => {
    try {
      await api.moveFilePriority(jobId, index, up)
      await load()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  const apply = async () => {
    setSaving(true)
    try {
      await api.setFiles(jobId, [...selected])
      setDirty(false)
      toast('Sélection de fichiers mise à jour')
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  if (error && !files) return <div className='panel'><p className='field-error'>{error}</p></div>
  if (!files) {
    return (
      <div className='panel'>
        <span className='meta-item muted'><SpinnerGap size={16} className='spin' /> Chargement des fichiers…</span>
      </div>
    )
  }

  return (
    <div className='panel'>
      <FilePicker files={files} selected={selected} onChange={(next) => { setSelected(next); setDirty(true) }} showProgress onPlay={play} onReorder={reorder} />
      {error && <p className='field-error' role='alert'>{error}</p>}
      {dirty && (
        <div className='panel-actions'>
          <button type='button' className='btn btn-primary btn-sm' disabled={selected.size === 0 || saving} onClick={apply}>
            Appliquer la sélection
          </button>
          <button type='button' className='btn btn-ghost btn-sm' onClick={() => { setDirty(false); load() }}>
            Annuler
          </button>
        </div>
      )}
    </div>
  )
}
