// Export et import de la liste des téléchargements, pour changer de PC.
import { useState } from 'react'
import { ArrowSquareIn, Export, SpinnerGap } from '@phosphor-icons/react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { api, errorMessage } from '../api'
import { plural } from '../format'
import { useToast } from './Toast'

const FILTERS = [{ name: 'Liste Pocket Torrent', extensions: ['json'] }]

export function TransferPanel () {
  const toast = useToast()
  const [includeFinished, setIncludeFinished] = useState(false)
  const [busy, setBusy] = useState(false)
  const [errors, setErrors] = useState<string[]>([])

  const exportList = async () => {
    const date = new Date().toISOString().slice(0, 10)
    const path = await save({ defaultPath: `pocket-torrent-liste-${date}.json`, filters: FILTERS })
    if (!path) return
    try {
      const count = await api.exportList(path)
      toast(`${plural(count, 'téléchargement')} exporté${count > 1 ? 's' : ''}`)
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  const importList = async () => {
    const path = await open({ multiple: false, filters: FILTERS })
    if (typeof path !== 'string') return
    setBusy(true)
    setErrors([])
    try {
      const report = await api.importList(path, includeFinished)
      setErrors(report.errors)
      toast(`${plural(report.added, 'téléchargement')} ajouté${report.added > 1 ? 's' : ''}` +
        (report.skipped ? `, ${report.skipped} ignoré${report.skipped > 1 ? 's' : ''}` : ''))
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <p className='help' style={{ marginTop: 0 }}>
        La liste contient les torrents (avec leur fichier .torrent), leurs catégories, priorités et fichiers choisis,
        et les liens directs pas encore finis. Les fichiers téléchargés ne sont pas dedans : copie-les à part dans les
        dossiers de téléchargement du nouveau PC pour que les torrents reprennent là où ils en étaient.
        {' '}<b>À garder pour toi :</b> la liste contient les fichiers .torrent, donc les passkeys de tes trackers privés.
      </p>
      <div className='panel-actions'>
        <button type='button' className='btn btn-ghost btn-sm' onClick={exportList}>
          <Export size={14} /> Exporter la liste…
        </button>
        <button type='button' className='btn btn-ghost btn-sm' onClick={importList} disabled={busy}>
          {busy ? <SpinnerGap size={14} className='spin' /> : <ArrowSquareIn size={14} />} Importer une liste…
        </button>
      </div>
      <label className='check' style={{ marginTop: 10 }}>
        <input type='checkbox' checked={includeFinished} onChange={(e) => setIncludeFinished(e.target.checked)} />
        <span>
          Importer aussi les téléchargements terminés
          <small>Seulement si leurs fichiers sont déjà sur ce PC : sinon, ils seront téléchargés à nouveau en entier.</small>
        </span>
      </label>
      {errors.length > 0 && (
        <ul className='field-error' style={{ display: 'block', paddingLeft: 18 }}>
          {errors.map((e) => <li key={e}>{e}</li>)}
        </ul>
      )}
    </>
  )
}
