// Glisser-déposer dans la fenêtre : liens magnet / http(s) et fichiers .torrent.
// Tout arrive dans la zone d'ajout : l'utilisateur garde la main (catégorie, priorité, fichiers).
import { useEffect, useRef, useState } from 'react'
import { FileArrowDown } from '@phosphor-icons/react'
import { listen } from '@tauri-apps/api/event'
import { api, errorMessage } from '../api'
import { useToast } from './Toast'

const LINK = /^(magnet:\?|https?:\/\/)/i

/** Liens d'un glisser depuis le navigateur (text/uri-list, sinon texte brut). */
function droppedLinks (data: DataTransfer): string[] {
  const text = data.getData('text/uri-list') || data.getData('text/plain')
  return text.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && LINK.test(l))
}

const LINK_IN_TEXT = /(magnet:\?[^\s"'<>]+|https?:\/\/[^\s"'<>]+)/gi

/**
 * Liens contenus dans les fichiers déposés qui ne sont pas des .torrent. Un lien glissé depuis le
 * navigateur arrive souvent aussi sous forme de raccourci (« lien.url » : `URL=magnet:…`) ou de
 * texte : ce n'est pas une erreur de l'utilisateur. Seuls les petits fichiers sont lus.
 */
export async function linksInFiles (files: File[]): Promise<{ links: string[]; rejected: number }> {
  const links: string[] = []
  let rejected = 0
  for (const file of files) {
    const text = file.size <= 64 * 1024 ? await file.text().catch(() => '') : ''
    const found = text.match(LINK_IN_TEXT) ?? []
    if (found.length) links.push(...found)
    else rejected += 1
  }
  return { links, rejected }
}

interface WebView2Bridge { postMessageWithAdditionalObjects?: (message: string, objects: ArrayLike<File>) => void }

/**
 * Chemin réel de chaque fichier déposé (chaîne vide si inconnu : fichier tiré d'une archive, hors
 * de WebView2…). La page ne voit que le contenu ; WebView2 transmet les fichiers au Rust
 * (`filedrop.rs`), qui renvoie leurs chemins.
 */
async function realPaths (files: File[]): Promise<string[]> {
  const bridge = (window as unknown as { chrome?: { webview?: WebView2Bridge } }).chrome?.webview
  if (!files.length || !bridge?.postMessageWithAdditionalObjects) return files.map(() => '')
  let stop: (() => void) | undefined
  const paths = await new Promise<string[]>((resolve) => {
    const timer = window.setTimeout(() => resolve([]), 1500)
    listen<string[]>('files-dropped', (event) => { window.clearTimeout(timer); resolve(event.payload) })
      .then((unlisten) => {
        stop = unlisten
        bridge.postMessageWithAdditionalObjects!('pocket-torrent:files-dropped', files)
      })
      .catch(() => resolve([]))
  })
  stop?.()
  return files.map((_, i) => paths[i] ?? '')
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
      const others = await linksInFiles(files.filter((f) => !torrents.includes(f)))
      for (const link of others.links) if (!items.includes(link)) items.push(link)
      // prévenir seulement si rien d'utilisable n'est arrivé avec ces fichiers
      if (others.rejected && !items.length && !torrents.length) {
        toast('Seuls les fichiers .torrent et les liens magnet ou http(s) peuvent être déposés.')
      }
      const paths = await realPaths(torrents)
      for (const [i, file] of torrents.entries()) {
        // le vrai fichier : c'est lui que « supprimer le .torrent d'origine » effacera
        if (paths[i].toLowerCase().endsWith('.torrent')) {
          items.push(paths[i])
          continue
        }
        // chemin inconnu : copie rangée par l'appli
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
