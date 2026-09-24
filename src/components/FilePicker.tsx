import { useState } from 'react'
import { ArrowDown, ArrowUp, Play, WarningCircle } from '@phosphor-icons/react'
import type { TorrentFile } from '../api'
import { formatSize, percent, plural } from '../format'

const PLAYABLE = /\.(mp4|mkv|avi|mov|wmv|m4v|webm|mp3|flac|wav|m4a|ogg)$/i
/** Lignes affichées d'un coup : au-delà, la fenêtre ralentit (torrents de milliers de fichiers). */
const PAGE = 300

interface FilePickerProps {
  files: TorrentFile[]
  selected: Set<number>
  onChange: (selected: Set<number>) => void
  showProgress?: boolean
  /** Bouton « lire » par fichier (§7.6) : seulement pour un torrent déjà ajouté au moteur. */
  onPlay?: (file: TorrentFile) => void
  /**
   * Priorité de téléchargement par fichier (§4.1/§5) : la liste `files` doit déjà être triée
   * par priorité (le moteur le fait). Seulement pour un torrent déjà ajouté et en téléchargement.
   */
  onReorder?: (index: number, up: boolean) => void
}

export function FilePicker ({ files, selected, onChange, showProgress = false, onPlay, onReorder }: FilePickerProps) {
  const [shown, setShown] = useState(PAGE)
  const total = files.filter((f) => selected.has(f.index)).reduce((sum, f) => sum + f.length, 0)
  const all = files.length > 0 && selected.size === files.length

  const toggle = (index: number, checked: boolean) => {
    const next = new Set(selected)
    if (checked) next.add(index)
    else next.delete(index)
    onChange(next)
  }

  return (
    <>
      <div className='panel-row'>
        <label className='check'>
          <input
            type='checkbox'
            checked={all}
            ref={(el) => { if (el) el.indeterminate = selected.size > 0 && !all }}
            onChange={(e) => onChange(new Set(e.target.checked ? files.map((f) => f.index) : []))}
          />
          <span>Tout sélectionner</span>
        </label>
        <span className='muted'>
          {plural(selected.size, 'fichier')} sélectionné{selected.size > 1 ? 's' : ''} · {formatSize(total)}
        </span>
      </div>
      <ul className='file-list'>
        {files.slice(0, shown).map((file, i) => (
          <li key={file.index}>
            <label className='check'>
              <input type='checkbox' checked={selected.has(file.index)} onChange={(e) => toggle(file.index, e.target.checked)} />
              <span className='file-name' title={file.path}>
                {file.danger && (
                  <WarningCircle size={14} className='tone-danger' style={{ verticalAlign: '-2px', marginRight: 4 }} aria-label={file.danger} />
                )}
                {file.path}
              </span>
              <span className='file-size'>
                {showProgress && file.included && `${Math.floor(percent(file.progressBytes, file.length) * 100)} % · `}
                {formatSize(file.length)}
              </span>
              {onReorder && (
                <>
                  <button
                    type='button' className='icon-btn' title='Prioriser' aria-label={`Prioriser ${file.path}`}
                    disabled={i === 0}
                    onClick={(e) => { e.preventDefault(); onReorder(file.index, true) }}
                  >
                    <ArrowUp size={16} />
                  </button>
                  <button
                    type='button' className='icon-btn' title='Dépriorister' aria-label={`Dépriorister ${file.path}`}
                    disabled={i === files.length - 1}
                    onClick={(e) => { e.preventDefault(); onReorder(file.index, false) }}
                  >
                    <ArrowDown size={16} />
                  </button>
                </>
              )}
              {onPlay && file.included && PLAYABLE.test(file.path) && (
                <button
                  type='button' className='icon-btn' title='Lire' aria-label={`Lire ${file.path}`}
                  onClick={(e) => { e.preventDefault(); onPlay(file) }}
                >
                  <Play size={16} />
                </button>
              )}
            </label>
            {file.danger && <p className='field-error' style={{ marginTop: 2, marginLeft: 26 }}>{file.danger}</p>}
          </li>
        ))}
      </ul>
      {files.length > shown && (
        <button type='button' className='btn btn-ghost btn-sm' onClick={() => setShown((n) => n + PAGE * 4)}>
          Afficher plus ({plural(files.length - shown, 'fichier')} restant{files.length - shown > 1 ? 's' : ''})
        </button>
      )}
    </>
  )
}
