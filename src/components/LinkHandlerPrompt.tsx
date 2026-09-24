import { useEffect, useState } from 'react'
import { MagnetStraight, SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage } from '../api'
import { Modal } from './Modal'
import { useToast } from './Toast'

/**
 * Premier lancement : propose une fois d'ouvrir les liens magnet et les fichiers .torrent avec
 * Pocket Torrent. Le choix se change ensuite dans Réglages › Général.
 */
export function LinkHandlerPrompt () {
  const toast = useToast()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    api.shouldAskLinkHandler().then(setOpen).catch(() => {})
  }, [])

  const answer = async (enable: boolean) => {
    setBusy(true)
    try {
      await api.answerLinkHandler(enable)
      toast(enable ? 'Les liens magnet s’ouvriront dans Pocket Torrent' : 'Tu pourras changer d’avis dans Réglages › Général')
      setOpen(false)
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    // fermer (Échap) sans répondre : la question reviendra au prochain lancement
    <Modal open={open} onClose={() => setOpen(false)} labelledBy='link-handler-title'>
      <div className='dialog-body'>
        <h2 id='link-handler-title'><MagnetStraight size={20} /> Ouvrir les liens magnet avec Pocket Torrent ?</h2>
        <p className='help' style={{ margin: 0 }}>
          Un clic sur un lien magnet dans le navigateur, ou un double-clic sur un fichier .torrent, l’ajoutera
          directement ici. Si un autre programme s’en charge aujourd’hui, il sera remis si tu changes d’avis
          dans Réglages › Général.
        </p>
      </div>
      <div className='dialog-actions'>
        <button type='button' className='btn btn-ghost' disabled={busy} onClick={() => answer(false)}>Non merci</button>
        <button type='button' className='btn btn-primary' disabled={busy} onClick={() => answer(true)}>
          {busy && <SpinnerGap size={16} className='spin' />} Oui, utiliser Pocket Torrent
        </button>
      </div>
    </Modal>
  )
}
