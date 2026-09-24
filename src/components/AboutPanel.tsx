import { useEffect, useState } from 'react'
import { ArrowSquareOut, GithubLogo } from '@phosphor-icons/react'
import { getVersion } from '@tauri-apps/api/app'
import { api, errorMessage } from '../api'
import { Logo } from './Logo'
import { useToast } from './Toast'

/** Adresse affichée ; l'ouverture passe par une commande qui ne connaît que celle-ci (commands.rs). */
const PROJECT_PAGE = 'github.com/Zyko0w/pocket-torrent-releases'

/** À propos, sous les onglets des réglages : version et page GitHub (versions publiées). */
export function AboutPanel () {
  const toast = useToast()
  const [version, setVersion] = useState('')
  useEffect(() => { getVersion().then(setVersion).catch(() => {}) }, [])

  return (
    <div className='about' aria-label='À propos'>
      <div className='about-head'>
        <Logo size={26} />
        <div>
          <b>Pocket Torrent</b>
          <span className='mono'>Version {version || '…'}</span>
        </div>
      </div>
      <p>Torrents et liens directs, avec VPN intégré. Aucune télémétrie.</p>
      <button
        type='button' className='about-link' title={PROJECT_PAGE}
        onClick={() => api.openProjectPage().catch((err) => toast(`Ouverture impossible : ${errorMessage(err)}`))}
      >
        <GithubLogo size={15} /> Page GitHub <ArrowSquareOut size={12} />
      </button>
      <p className='about-note'>Nouvelles versions et notes de mise à jour.</p>
    </div>
  )
}
