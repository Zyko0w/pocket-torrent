// Confidentialité (chiffrement, modes discret et renforcé), liste de blocage d'IP, fichiers dangereux.
import { FolderOpen } from '@phosphor-icons/react'
import { open } from '@tauri-apps/plugin-dialog'
import type { Settings } from '../../api'
import { Check, Section, resetOf, type SectionProps } from './fields'

const ENCRYPTION_MODES = [
  ['require', 'Chiffrement obligatoire', 'Tous les échanges avec les pairs sont chiffrés. Les rares pairs qui ne chiffrent pas sont ignorés.'],
  ['prefer', 'Chiffrement préféré', 'Chiffre quand c’est possible, sinon échange en clair (plus de pairs, moins discret).'],
  ['off', 'Sans chiffrement', 'Échanges en clair, reconnaissables par le fournisseur d’accès.']
] as const

export function PrivacySection (props: SectionProps) {
  const { settings, update } = props
  const privacy = settings.privacy
  const setPrivacy = (patch: Partial<Settings['privacy']>) => update({ privacy: { ...privacy, ...patch } })

  return (
    <Section id='s-privacy' reset={resetOf(props, ['privacy'])} title='Confidentialité' intro='Empêcher ton fournisseur d’accès de reconnaître les torrents.'>
      <div className='mode-list' role='radiogroup' aria-label='Chiffrement'>
        {ENCRYPTION_MODES.map(([value, label, help]) => (
          <label key={value} className={`mode-option${privacy.encryption === value ? ' selected' : ''}`}>
            <input type='radio' name='encryption' checked={privacy.encryption === value} onChange={() => setPrivacy({ encryption: value })} />
            <span>{label}<small>{help}</small></span>
          </label>
        ))}
      </div>
      <Check
        checked={privacy.hideAnnounces}
        onChange={(hideAnnounces) => setPrivacy({ hideAnnounces })}
        label='Mode discret : masquer aussi la recherche de pairs'
        help='Le chiffrement ne couvre pas la DHT ni les trackers HTTP et UDP, qui restent reconnaissables. Ce mode les coupe et ne garde que les trackers HTTPS : beaucoup moins de pairs trouvés. Inutile avec le VPN, qui cache déjà tout.'
      />
      <p className='help' style={{ marginTop: -4 }}>
        Les noms des trackers passent toujours par un DNS chiffré. Pour que le fournisseur d’accès ne voie rien du tout, utilise le VPN.
      </p>
      <Check
        checked={privacy.hardenedMode}
        onChange={(hardenedMode) => setPrivacy({ hardenedMode })}
        label='Mode confidentialité renforcée'
        help='Regroupe tout : mode discret (ci-dessus), UPnP coupé, identifiant et version du client masqués auprès des pairs (ils ne voient plus « Pocket Torrent »), IP vue par le pair non révélée, pas de recherche de mise à jour au démarrage. Aucune télémétrie n’existe de toute façon dans Pocket Torrent. Moins de pairs trouvés, débits plus bas — combine avec le VPN pour une vraie protection.'
      />
    </Section>
  )
}

export function BlocklistSection (props: SectionProps) {
  const { settings, update } = props
  const blocklist = settings.blocklist
  const setBlocklist = (patch: Partial<Settings['blocklist']>) => update({ blocklist: { ...blocklist, ...patch } })
  const browse = async () => {
    const path = await open({ filters: [{ name: 'Liste de blocage', extensions: ['p2p', 'txt', 'gz'] }] })
    if (typeof path === 'string') setBlocklist({ source: path })
  }

  return (
    <Section id='s-blocklist' reset={resetOf(props, ['blocklist'])} title='Liste de blocage d’IP' intro='Refuse les pairs dont l’adresse figure dans une liste (format P2P, texte ou .gz).'>
      <Check
        checked={blocklist.enabled}
        onChange={(enabled) => setBlocklist({ enabled })}
        label='Activer la liste de blocage'
        help='Efficacité limitée en pratique : les adresses surveillées changent souvent. Ce n’est pas une vraie protection — utilise le VPN pour ça.'
      />
      {blocklist.enabled && (
        <div className='field'>
          <label htmlFor='blocklist-source'>Fichier ou adresse http(s)</label>
          <div className='input-row'>
            <input
              id='blocklist-source' className='input mono' spellCheck={false}
              placeholder='https://example.org/blocklist.p2p.gz'
              value={blocklist.source}
              onChange={(e) => setBlocklist({ source: e.target.value })}
            />
            <button type='button' className='btn btn-ghost' onClick={browse}>
              <FolderOpen size={16} /> Parcourir
            </button>
          </div>
          <p className='help'>Rechargée à chaque démarrage du moteur (nouveau réglage, VPN, redémarrage) — pas de vérification en continu.</p>
        </div>
      )}
    </Section>
  )
}

/** `extensions` : liste en texte libre (une extension par ligne), éditée à part. */
export function DangerousFilesSection ({ extensions, defaults, onChange }: { extensions: string; defaults: string[]; onChange: (text: string) => void }) {
  const original = defaults.join('\n')
  return (
    <Section id='s-security' reset={{ onReset: () => onChange(original), disabled: extensions.trim() === original }} title='Fichiers dangereux' intro='Repérer les exécutables déguisés en média (ex. « film.mkv.exe ») avant de les télécharger.'>
      <div className='field'>
        <label htmlFor='excluded-extensions'>Extensions signalées <span className='optional'>(une par ligne, avec le point)</span></label>
        <textarea
          id='excluded-extensions' className='input mono' rows={3} spellCheck={false}
          value={extensions} onChange={(e) => onChange(e.target.value)}
        />
        <p className='help'>
          Ces fichiers, et les noms de fichiers trompeurs (caractères d’inversion de texte), sont décochés
          automatiquement à l’ajout d’un torrent — tu peux quand même les sélectionner toi-même si tu es sûr.
        </p>
      </div>
    </Section>
  )
}
