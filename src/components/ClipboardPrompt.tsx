import { useEffect, useState } from 'react'
import { MagnetStraight, X } from '@phosphor-icons/react'
import { api, errorMessage } from '../api'
import { useToast } from './Toast'

interface ClipboardPromptProps {
  /** Mettre le lien dans la zone d'ajout (catégorie, fichiers… à choisir). */
  onEdit: (link: string) => void
  onAdded: () => void
}

/**
 * Au retour sur la fenêtre, propose un lien magnet copié ailleurs (navigateur, messagerie…).
 * Le Rust ne renvoie que ce lien, jamais le reste du presse-papiers, et chaque lien une seule fois.
 */
export function ClipboardPrompt ({ onEdit, onAdded }: ClipboardPromptProps) {
  const toast = useToast()
  const [magnet, setMagnet] = useState<{ link: string; name: string | null } | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    const check = () => { api.clipboardMagnet().then((found) => { if (found) setMagnet(found) }).catch(() => {}) }
    check()
    window.addEventListener('focus', check)
    return () => window.removeEventListener('focus', check)
  }, [])

  if (!magnet) return null

  const add = async () => {
    setBusy(true)
    try {
      const report = await api.addLinks([magnet.link], { category: null, priority: 'normal', dest: 'local', limitKbps: 0, onlyFiles: null })
      if (report.errors.length) {
        toast(report.errors.join(' · '))
      } else {
        toast(`« ${magnet.name ?? 'Torrent'} » ajouté`)
        onAdded()
      }
      setMagnet(null)
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className='banner info' role='status'>
      <MagnetStraight size={16} />
      <span className='banner-text'>
        Lien magnet copié : <b>{magnet.name ?? 'torrent sans nom'}</b>
      </span>
      <button type='button' className='btn btn-primary btn-sm' disabled={busy} onClick={add}>Ajouter</button>
      <button type='button' className='btn btn-ghost btn-sm' disabled={busy} onClick={() => { onEdit(magnet.link); setMagnet(null) }}>
        Dans la zone d’ajout
      </button>
      <button type='button' className='icon-btn' title='Ignorer' aria-label='Ignorer' onClick={() => setMagnet(null)}>
        <X size={16} />
      </button>
    </div>
  )
}
