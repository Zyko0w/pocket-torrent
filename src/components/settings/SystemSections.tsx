// Notifications, intégration Windows (démarrage, liens, veille, antivirus, mises à jour),
// programme externe, réseau et disque.
import { useEffect, useState } from 'react'
import { ArrowSquareOut, BellRinging, GithubLogo, SpinnerGap } from '@phosphor-icons/react'
import { getVersion } from '@tauri-apps/api/app'
import { api, errorMessage } from '../../api'
import { useToast } from '../Toast'
import { checkForUpdate } from '../UpdateBanner'
import { Check, Section, toInt, type SectionProps } from './fields'

export function NotificationsSection ({ settings, update }: SectionProps) {
  const toast = useToast()
  const notifications = settings.notifications
  return (
    <Section id='s-notify' title='Notifications' intro='Messages Windows en bas de l’écran.'>
      <Check checked={notifications.done} onChange={(done) => update({ notifications: { ...notifications, done } })} label='Quand un téléchargement est terminé' />
      <Check checked={notifications.error} onChange={(error) => update({ notifications: { ...notifications, error } })} label='Quand un problème survient' />
      <div>
        <button type='button' className='btn btn-ghost btn-sm' onClick={() => api.testNotification().catch((err) => toast(errorMessage(err)))}>
          <BellRinging size={16} /> Envoyer une notification de test
        </button>
      </div>
    </Section>
  )
}

interface WindowsSectionProps extends SectionProps {
  /** Réglages gardés par Windows (registre), pas dans le fichier de réglages. */
  startWithWindows: boolean
  onStartWithWindowsChange: (value: boolean) => void
  magnetHandler: boolean
  onMagnetHandlerChange: (value: boolean) => void
  defenderAvailable: boolean
}

export function WindowsSection (props: WindowsSectionProps) {
  const { settings, update } = props
  return (
    <Section id='s-windows' title='Windows' intro='Démarrage, liens magnet, fichiers .torrent et fermeture.'>
      <Check checked={props.startWithWindows} onChange={props.onStartWithWindowsChange} label='Lancer Pocket Torrent avec Windows' help='Il démarre discrètement près de l’horloge.' />
      <Check checked={props.magnetHandler} onChange={props.onMagnetHandlerChange} label='Ouvrir les liens magnet et fichiers .torrent avec Pocket Torrent' help='Un clic sur un lien magnet dans le navigateur, ou un double-clic sur un fichier .torrent, l’ajoute ici. L’ancien programme est remis si tu décoches.' />
      <Check checked={settings.closeToTray} onChange={(closeToTray) => update({ closeToTray })} label='Garder Pocket Torrent ouvert près de l’horloge quand je ferme la fenêtre' help='Les téléchargements continuent. Pour quitter : clic droit sur l’icône, Quitter.' />
      <Check
        checked={settings.preventSleep}
        onChange={(preventSleep) => update({ preventSleep })}
        label='Empêcher la mise en veille pendant un transfert actif'
        help='Windows ne s’endort pas tant qu’un téléchargement ou un partage est en cours ; redevient normal dès que tout est à l’arrêt.'
      />
      <Check
        checked={settings.scanDownloads}
        onChange={(scanDownloads) => update({ scanDownloads })}
        label='Analyser chaque téléchargement terminé avec Microsoft Defender'
        help={
          props.defenderAvailable
            ? 'Rapport seulement : rien n’est mis en quarantaine ni supprimé automatiquement, une notification s’affiche en cas de menace.'
            : 'Microsoft Defender introuvable sur ce PC : cette option n’aura aucun effet.'
        }
      />
      <Check
        checked={settings.checkUpdates}
        onChange={(checkUpdates) => update({ checkUpdates })}
        label='Chercher les mises à jour au démarrage'
        help='Une nouvelle version est proposée dans un bandeau, jamais installée sans ton accord. Les versions sont signées et vérifiées avant installation. Pas de recherche automatique en mode confidentialité renforcée.'
      />
      <UpdateCheck />
    </Section>
  )
}

/** Version installée et recherche manuelle d'une mise à jour. */
function UpdateCheck () {
  const toast = useToast()
  const [version, setVersion] = useState('')
  const [busy, setBusy] = useState(false)
  useEffect(() => { getVersion().then(setVersion).catch(() => {}) }, [])

  const run = async () => {
    setBusy(true)
    try {
      const update = await checkForUpdate()
      if (!update) toast('Pocket Torrent est à jour')
    } catch (err) {
      toast(`Recherche impossible : ${errorMessage(err)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className='panel-actions' style={{ alignItems: 'center' }}>
      <span className='help' style={{ margin: 0 }}>Version installée : {version || '…'}</span>
      <button type='button' className='btn btn-ghost btn-sm' onClick={run} disabled={busy}>
        {busy && <SpinnerGap size={14} className='spin' />} Rechercher une mise à jour
      </button>
    </div>
  )
}

/** Adresse affichée ; l'ouverture passe par une commande qui ne connaît que celle-ci (commands.rs). */
const PROJECT_PAGE = 'github.com/Zyko0w/pocket-torrent-releases'

/** Version, et page GitHub où sont publiées les versions et notes de mise à jour. */
export function AboutSection () {
  const toast = useToast()
  const [version, setVersion] = useState('')
  useEffect(() => { getVersion().then(setVersion).catch(() => {}) }, [])

  return (
    <Section id='s-about' title='À propos' intro='Pocket Torrent : torrents et liens directs, avec VPN intégré. Aucune télémétrie.'>
      <p className='help' style={{ margin: 0 }}>Version {version || '…'}</p>
      <div className='panel-actions' style={{ alignItems: 'center' }}>
        <button
          type='button' className='btn btn-ghost btn-sm'
          onClick={() => api.openProjectPage().catch((err) => toast(`Ouverture impossible : ${errorMessage(err)}`))}
        >
          <GithubLogo size={16} /> Page GitHub <ArrowSquareOut size={12} />
        </button>
        <span className='help mono selectable' style={{ margin: 0 }}>{PROJECT_PAGE}</span>
      </div>
      <p className='help'>Nouvelles versions, notes de mise à jour et installateurs.</p>
    </Section>
  )
}

export function NetworkSection ({ settings, update, listenPort, externalPort }: SectionProps & { listenPort: number | null; externalPort: number | null }) {
  return (
    <Section id='s-network' title='Réseau et disque' intro='Port des torrents et place à garder libre.'>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='torrent-port'>Port des torrents</label>
          <input id='torrent-port' className='input' type='number' min={0} max={65535} value={settings.torrentPort} onChange={(e) => update({ torrentPort: toInt(e.target.value, 65535) })} />
          <p className='help'>
            0 = automatique{listenPort ? ` (port actuel : ${listenPort})` : ''}.
            {externalPort && ` Ouvert sur la box par UPnP sous le port ${externalPort}.`}
          </p>
        </div>
        <div className='field'>
          <label htmlFor='disk-margin'>Espace à garder libre <span className='optional'>(Mo)</span></label>
          <input id='disk-margin' className='input' type='number' min={0} step={512} value={settings.diskMarginMb} onChange={(e) => update({ diskMarginMb: toInt(e.target.value) })} />
          <p className='help'>En dessous, les téléchargements se mettent en pause.</p>
        </div>
      </div>
      <Check
        checked={settings.utpEnabled}
        onChange={(utpEnabled) => update({ utpEnabled })}
        label='Activer µTP (BEP 29), en plus du TCP'
        help='Joint des pairs difficiles à atteindre en TCP (NAT strict). Désactivé par défaut : ce module est encore jeune dans le moteur utilisé ici.'
      />
      <div className='field'>
        <label htmlFor='max-peers'>Pairs connectés par torrent <span className='optional'>(0 = illimité)</span></label>
        <input
          id='max-peers' className='input input-narrow' type='number' min={0} max={10000} step={10}
          value={settings.maxPeersPerTorrent}
          onChange={(e) => update({ maxPeersPerTorrent: toInt(e.target.value, 10000) })}
        />
      </div>
      <div className='field'>
        <label htmlFor='upload-slots'>Places d’envoi par torrent <span className='optional'>(0 = tous les pairs)</span></label>
        <input
          id='upload-slots' className='input input-narrow' type='number' min={0} max={500}
          value={settings.uploadSlots}
          onChange={(e) => update({ uploadSlots: toInt(e.target.value, 500) })}
        />
        <p className='help'>
          Envoie en priorité aux pairs qui t’envoient le plus (réciprocité), plus une place qui tourne pour laisser leur
          chance aux nouveaux : tu télécharges plus vite. Tant qu’un torrent n’a pas plus de pairs intéressés que de places,
          tout le monde est servi.
        </p>
      </div>
    </Section>
  )
}
