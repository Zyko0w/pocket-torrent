// Glisser-déposer dans la fenêtre : liens magnet / http(s) et fichiers .torrent.
// Tout arrive dans la zone d'ajout : l'utilisateur garde la main (catégorie, priorité, fichiers).
import { useEffect, useRef, useState } from 'react'
import { FileArrowDown } from '@phosphor-icons/react'
import { api, errorMessage } from '../api'
import { useToast } from './Toast'

const LINK = /^(magnet:\?|https?:\/\/)/i

/** Liens d'un glisser depuis le navigateur (text/uri-list, sinon texte brut). */
function droppedLinks (data: DataTransfer): string[] {
  const text = data.getData('text/uri-list') || data.getData('text/plain')
  return text.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && LINK.test(l))
}

export function DropZone ({ onDropped }: { onDropped: (items: string[]) => void }) {
  const toast = useToast()
  const [active, setActive] = useState(false)
  // dragenter/dragleave arrivent aussi pour chaque élément survolé : on compte
  const depth = useRef(0)

  useEffect(() => {
    const accepts = (e: DragEvent) => Boolean(e.dataTransfer?.types.some((t) => t === 'Files' || t === 'text/uri-list' || t === 'text/plain'))
    const onEnter = (e: DragEvent) => {
      if (!accepts(e)) return
      depth.current += 1
      setActive(true)
    }
    const onOver = (e: DragEvent) => {
      if (!accepts(e)) return
      e.preventDefault()
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
    }
    const onLeave = () => {
      depth.current = Math.max(0, depth.current - 1)
      if (depth.current === 0) setActive(false)
    }
    const onDrop = async (e: DragEvent) => {
      depth.current = 0
      setActive(false)
      const data = e.dataTransfer
      if (!data) return
      // du texte déposé dans un champ garde son comportement normal (insertion) ; un fichier
      // jamais : sans preventDefault, la vue web l'ouvrirait à la place de l'appli
      const inField = e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement
      if (inField && data.files.length === 0) return
      e.preventDefault()
      const items = droppedLinks(data)
      const files = Array.from(data.files)
      const torrents = files.filter((f) => f.name.toLowerCase().endsWith('.torrent'))
      if (files.length > torrents.length) toast('Seuls les fichiers .torrent peuvent être déposés.')
      for (const file of torrents) {
        try {
          items.push(await api.dropTorrentFile(file.name, new Uint8Array(await file.arrayBuffer())))
        } catch (err) {
          toast(errorMessage(err))
        }
      }
      if (items.length) onDropped(items)
    }
    window.addEventListener('dragenter', onEnter)
    window.addEventListener('dragover', onOver)
    window.addEventListener('dragleave', onLeave)
    window.addEventListener('drop', onDrop)
    return () => {
      window.removeEventListener('dragenter', onEnter)
      window.removeEventListener('dragover', onOver)
      window.removeEventListener('dragleave', onLeave)
      window.removeEventListener('drop', onDrop)
    }
  }, [onDropped, toast])

  if (!active) return null
  return (
    <div className='drop-overlay' aria-hidden='true'>
      <div className='drop-card'>
        <FileArrowDown size={32} />
        <p>Dépose un fichier .torrent ou un lien magnet</p>
      </div>
    </div>
  )
}
