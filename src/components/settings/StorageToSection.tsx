// storage.to : envoi des fichiers terminés en ligne, avec un lien de partage.
import { useState } from 'react'
import { CheckCircle, SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage, type SecretsInput, type SettingsView } from '../../api'
import { useToast } from '../Toast'
import { Check, SecretField, Section, resetOf, type SectionProps } from './fields'
import { Select } from '../Select'
import { DOWNLOAD_CAP_OPTIONS, EXPIRY_OPTIONS } from '../SendDialog'

interface StorageToSectionProps extends SectionProps {
  /** Jeton déjà rangé dans Windows (jamais sa valeur). */
  saved: SettingsView['secrets']
  secrets: SecretsInput
  onSecretsChange: (secrets: SecretsInput) => void
}

export function StorageToSection (props: StorageToSectionProps) {
  const { settings, update, saved, secrets, onSecretsChange } = props
  const toast = useToast()
  const [testing, setTesting] = useState(false)
  const storage = settings.storageTo
  const set = (patch: Partial<typeof storage>) => update({ storageTo: { ...storage, ...patch } })

  const test = async () => {
    setTesting(true)
    try {
      toast(await api.testStorageTo())
    } catch (err) {
      toast(`storage.to : ${errorMessage(err)}`)
    } finally {
      setTesting(false)
    }
  }

  return (
    <Section
      id='s-storage-to' title='storage.to' reset={resetOf(props, ['storageTo'])}
      intro='Envoi des fichiers terminés sur storage.to, avec un lien de partage : choix « Envoyer sur storage.to » dans la zone d’ajout, ou menu ⋯ d’un téléchargement terminé (ces réglages y sont préremplis).'
    >
      <div className='field'>
        <label htmlFor='storage-expiry'>Durée de vie des liens</label>
        <Select
          id='storage-expiry' ariaLabel='Durée de vie des liens' value={String(storage.expiryDays)} onChange={(v) => set({ expiryDays: Number(v) })}
          options={EXPIRY_OPTIONS}
        />
        <p className='help'>Les fichiers sont ensuite effacés de storage.to.</p>
      </div>
      <div className='field'>
        <label htmlFor='storage-cap'>Effacer de storage.to</label>
        <Select
          id='storage-cap' ariaLabel='Effacer de storage.to' value={String(storage.maxDownloads)} onChange={(v) => set({ maxDownloads: Number(v) })}
          options={DOWNLOAD_CAP_OPTIONS}
        />
        <p className='help'>Le lien cesse de fonctionner dès que ce nombre de téléchargements est atteint.</p>
      </div>
      <SecretField
        id='storage-token' label='Jeton de compte storage.to (facultatif)' isSet={saved.storageToToken}
        draft={secrets.storageToToken} onChange={(storageToToken) => onSecretsChange({ ...secrets, storageToToken })}
      />
      <p className='help' style={{ marginTop: -4 }}>
        Sans compte : 50 fichiers et 100 Go par 24 h, 25 Go par fichier. Un compte gratuit lève ces limites
        (jeton à créer dans les réglages du compte storage.to).
      </p>
      <Check
        checked={storage.deleteAfterUpload} onChange={(deleteAfterUpload) => set({ deleteAfterUpload })}
        label='Effacer les fichiers du PC une fois envoyés'
        help='Pour « Envoyer sur storage.to » choisi à l’ajout (le menu ⋯ le demande à chaque envoi). Attention : les liens expirent, les fichiers ne seront alors plus nulle part.'
      />
      <div>
        <button type='button' className='btn btn-ghost btn-sm' disabled={testing} onClick={test}>
          {testing ? <SpinnerGap size={16} className='spin' /> : <CheckCircle size={16} />} Tester la connexion
        </button>
        <p className='help'>Le test utilise les réglages enregistrés et indique le quota d’envoi restant.</p>
      </div>
    </Section>
  )
}
