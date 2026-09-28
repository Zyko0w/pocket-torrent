// Envoi en ligne : fichiers terminés envoyés sur un service d'hébergement, avec un lien de partage.
import { useState } from 'react'
import { CheckCircle, SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage, type SecretsInput, type SettingsView } from '../../api'
import { hostInfo, hostOptions, type UploadHost } from '../../uploadHosts'
import { CustomHostsEditor } from './CustomHostsEditor'
import { useToast } from '../Toast'
import { Check, SecretField, Section, resetOf, type SectionProps } from './fields'
import { Select } from '../Select'
import { DOWNLOAD_CAP_OPTIONS, EXPIRY_OPTIONS } from '../SendDialog'

interface UploadSectionProps extends SectionProps {
  /** Comptes déjà rangés dans Windows (jamais leur valeur). */
  saved: SettingsView['secrets']
  secrets: SecretsInput
  onSecretsChange: (secrets: SecretsInput) => void
}

export function UploadSection (props: UploadSectionProps) {
  const { settings, update, saved, secrets, onSecretsChange } = props
  const toast = useToast()
  const [testing, setTesting] = useState(false)
  const storage = settings.storageTo
  const customHosts = storage.customHosts ?? []
  const host = hostInfo(storage.host, customHosts)
  const set = (patch: Partial<typeof storage>) => update({ storageTo: { ...storage, ...patch } })
  const secret = host.account?.secret

  const test = async () => {
    setTesting(true)
    try {
      toast(await api.testUploadHost(host.id))
    } catch (err) {
      toast(`${host.label} : ${errorMessage(err)}`)
    } finally {
      setTesting(false)
    }
  }

  return (
    <Section
      id='s-storage-to' title='Envoi en ligne' reset={resetOf(props, ['storageTo'])}
      intro='Envoi des fichiers terminés sur un service d’hébergement, avec un lien de partage : choix « Mettre en ligne une fois fini » dans la zone d’ajout, ou menu ⋯ d’un téléchargement terminé (ces réglages y sont préremplis).'
    >
      <div className='field'>
        <label htmlFor='storage-host'>Service</label>
        <Select
          id='storage-host' ariaLabel='Service d’hébergement' value={host.id} onChange={(v) => set({ host: v as UploadHost })}
          options={hostOptions(customHosts)}
        />
        <p className='help'>{host.about}</p>
      </div>
      {host.limits && (
        <>
          <div className='field'>
            <label htmlFor='storage-expiry'>Durée de vie des liens</label>
            <Select
              id='storage-expiry' ariaLabel='Durée de vie des liens' value={String(storage.expiryDays)} onChange={(v) => set({ expiryDays: Number(v) })}
              options={EXPIRY_OPTIONS}
            />
            <p className='help'>Les fichiers sont ensuite effacés de {host.label}.</p>
          </div>
          <div className='field'>
            <label htmlFor='storage-cap'>Effacer de {host.label}</label>
            <Select
              id='storage-cap' ariaLabel={`Effacer de ${host.label}`} value={String(storage.maxDownloads)} onChange={(v) => set({ maxDownloads: Number(v) })}
              options={DOWNLOAD_CAP_OPTIONS}
            />
            <p className='help'>Le lien cesse de fonctionner dès que ce nombre de téléchargements est atteint.</p>
          </div>
        </>
      )}
      {host.account && secret && (
        <>
          <SecretField
            key={secret} id={`storage-account-${host.id}`} label={host.account.label} isSet={saved[secret]}
            draft={secrets[secret]} onChange={(value) => onSecretsChange({ ...secrets, [secret]: value })}
          />
          <p className='help' style={{ marginTop: -4 }}>{host.account.help}</p>
        </>
      )}
      <Check
        checked={storage.deleteAfterUpload} onChange={(deleteAfterUpload) => set({ deleteAfterUpload })}
        label='Effacer les fichiers du PC une fois envoyés'
        help='Pour « Mettre en ligne une fois fini » choisi à l’ajout (le menu ⋯ le demande à chaque envoi). Attention : une fois effacés du service, les fichiers ne seront plus nulle part.'
      />
      <div>
        <button type='button' className='btn btn-ghost btn-sm' disabled={testing} onClick={test}>
          {testing ? <SpinnerGap size={16} className='spin' /> : <CheckCircle size={16} />} Tester la connexion
        </button>
        <p className='help'>Le test porte sur le service choisi, avec les réglages enregistrés{host.limits ? ', et indique le quota d’envoi restant' : ''}.</p>
      </div>
      <CustomHostsEditor
        hosts={customHosts}
        onChange={(next) => set({
          customHosts: next,
          // hébergeur choisi puis retiré : retour au service d'origine
          ...(storage.host.startsWith('custom:') && !next.some((h) => `custom:${h.id}` === storage.host) ? { host: 'storageTo' as const } : {})
        })}
        savedHeaders={saved.customHostHeaders ?? []}
        headers={secrets.customHostHeaders}
        onHeadersChange={(customHostHeaders) => onSecretsChange({ ...secrets, customHostHeaders })}
      />
    </Section>
  )
}
