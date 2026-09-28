import { Copy, WarningCircle } from '@phosphor-icons/react'
import { copyText, errorMessage, type Delivery } from '../api'
import { formatDate, formatSize } from '../format'
import { hostLabel } from '../uploadHosts'
import { useToast } from './Toast'

/** Liens de partage des fichiers envoyés en ligne (collection en tête pour plusieurs fichiers). */
export function LinksPanel ({ delivery }: { delivery: Delivery }) {
  const toast = useToast()
  const expired = delivery.expiresAt !== null && delivery.expiresAt < Date.now()

  const copy = async (text: string, done: string) => {
    try {
      await copyText(text)
      toast(done)
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <div className='panel'>
      <div className='panel-row'>
        <p className={expired ? 'tone-danger meta-item' : 'muted'} style={{ fontSize: 12.5 }}>
          {expired && <WarningCircle size={14} />}
          {delivery.expiresAt === null
            ? `Liens ${hostLabel(delivery.host, delivery.hostName)}, effacés par le service après une période sans téléchargement.`
            : expired
              ? `Liens expirés le ${formatDate(delivery.expiresAt)}.`
              : `Liens valables jusqu’au ${formatDate(delivery.expiresAt)}.`}
        </p>
        {delivery.links.filter((l) => l.kind !== 'collection').length > 1 && (
          <button type='button' className='btn btn-ghost btn-sm' onClick={() => copy(delivery.links.filter((l) => l.kind !== 'collection').map((l) => l.url).join('\n'), 'Tous les liens copiés')}>
            <Copy size={14} /> Copier tous les liens
          </button>
        )}
      </div>
      <ul className='file-list'>
        {delivery.links.map((link) => (
          <li key={link.key} className='link-row'>
            <span className='file-name selectable' title={link.name}>{link.kind === 'collection' ? <b>{link.name}</b> : link.name}</span>
            <span className='file-size'>{formatSize(link.size)}</span>
            <button type='button' className='icon-btn' title='Copier le lien' aria-label={`Copier le lien de ${link.name}`} disabled={expired} onClick={() => copy(link.url, 'Lien copié')}>
              <Copy size={16} />
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}
