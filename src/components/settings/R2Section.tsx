// Cloudflare R2 : envoi des fichiers terminés en ligne.
import { useState } from 'react'
import { CheckCircle, SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage, type SecretsInput, type SettingsView } from '../../api'
import { useToast } from '../Toast'
import { Check, SecretField, Section, toInt, type SectionProps } from './fields'

interface R2SectionProps extends SectionProps {
  /** Clés déjà rangées dans Windows (jamais leur valeur). */
  saved: SettingsView['secrets']
  secrets: SecretsInput
  onSecretsChange: (secrets: SecretsInput) => void
}

export function R2Section ({ settings, update, saved, secrets, onSecretsChange }: R2SectionProps) {
  const toast = useToast()
  const [testing, setTesting] = useState(false)
  const r2 = settings.r2
  const setR2 = (patch: Partial<typeof r2>) => update({ r2: { ...r2, ...patch } })

  const test = async () => {
    setTesting(true)
    try {
      await api.testR2()
      toast('Connexion à Cloudflare R2 réussie')
    } catch (err) {
      toast(`R2 : ${errorMessage(err)}`)
    } finally {
      setTesting(false)
    }
  }

  return (
    <Section id='s-r2' title='Cloudflare R2' intro='Envoi des fichiers terminés en ligne, avec des liens de téléchargement.'>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='r2-account'>Identifiant du compte</label>
          <input id='r2-account' className='input mono' spellCheck={false} value={r2.accountId} onChange={(e) => setR2({ accountId: e.target.value })} />
        </div>
        <div className='field'>
          <label htmlFor='r2-bucket'>Bucket</label>
          <input id='r2-bucket' className='input mono' spellCheck={false} value={r2.bucket} onChange={(e) => setR2({ bucket: e.target.value })} />
        </div>
      </div>
      <div className='field-pair'>
        <SecretField id='r2-key' label='Access Key ID' isSet={saved.r2AccessKeyId} draft={secrets.r2AccessKeyId} onChange={(r2AccessKeyId) => onSecretsChange({ ...secrets, r2AccessKeyId })} />
        <SecretField id='r2-secret' label='Secret Access Key' isSet={saved.r2SecretAccessKey} draft={secrets.r2SecretAccessKey} onChange={(r2SecretAccessKey) => onSecretsChange({ ...secrets, r2SecretAccessKey })} />
      </div>
      <div className='field'>
        <label htmlFor='r2-public'>URL publique du bucket <span className='optional'>(facultatif)</span></label>
        <input id='r2-public' className='input mono' spellCheck={false} placeholder='https://fichiers.mondomaine.fr' value={r2.publicUrl} onChange={(e) => setR2({ publicUrl: e.target.value })} />
        <p className='help'>Avec une URL publique, les liens sont permanents. Sinon ils expirent.</p>
      </div>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='r2-expiry'>Durée des liens <span className='optional'>(jours, 7 max)</span></label>
          <input id='r2-expiry' className='input' type='number' min={1} max={7} disabled={Boolean(r2.publicUrl)} value={r2.linkExpiryDays} onChange={(e) => setR2({ linkExpiryDays: Math.max(1, toInt(e.target.value, 7)) })} />
        </div>
      </div>
      <Check checked={r2.deleteAfterUpload} onChange={(deleteAfterUpload) => setR2({ deleteAfterUpload })} label='Effacer les fichiers du PC une fois envoyés' />
      <div>
        <button type='button' className='btn btn-ghost btn-sm' disabled={testing} onClick={test}>
          {testing ? <SpinnerGap size={16} className='spin' /> : <CheckCircle size={16} />} Tester la connexion
        </button>
        <p className='help'>Le test utilise les réglages enregistrés.</p>
      </div>
    </Section>
  )
}
