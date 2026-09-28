// Hébergeurs de fichiers ajoutés par l'utilisateur : saisie à la main, modèle, ou import .sxcu.
import { useRef, useState } from 'react'
import { FileArrowUp, Plus, Trash } from '@phosphor-icons/react'
import { CUSTOM_PRESETS, fromSxcu, newCustomHost, type CustomHost } from '../../uploadHosts'
import { errorMessage } from '../../api'
import { Select } from '../Select'
import { useToast } from '../Toast'

const MAX_CUSTOM_HOSTS = 10

interface CustomHostsEditorProps {
  hosts: CustomHost[]
  onChange: (hosts: CustomHost[]) => void
  /** Hébergeurs dont les en-têtes sont déjà rangés dans Windows (jamais leur valeur). */
  savedHeaders: string[]
  /** En-têtes modifiés, par identifiant (chaîne vide : effacer ; absent : inchangés). */
  headers: Record<string, string>
  onHeadersChange: (headers: Record<string, string>) => void
}

const fieldsToText = (fields: CustomHost['formFields']) => fields.map((f) => `${f.name}=${f.value}`).join('\n')

function textToFields (text: string): CustomHost['formFields'] {
  return text.split('\n').filter((line) => line.trim()).map((line) => {
    const at = line.indexOf('=')
    return at < 0 ? { name: line.trim(), value: '' } : { name: line.slice(0, at).trim(), value: line.slice(at + 1) }
  })
}

function HostCard ({ host, index, saved, headers, onChange, onHeaders, onRemove }: {
  host: CustomHost
  index: number
  saved: boolean
  headers: string | undefined
  onChange: (patch: Partial<CustomHost>) => void
  onHeaders: (value: string | undefined) => void
  onRemove: () => void
}) {
  // texte libre gardé tel quel pendant la saisie (« nom= » en cours de frappe)
  const [fieldsText, setFieldsText] = useState(() => fieldsToText(host.formFields))
  const id = `custom-host-${index}`
  const cleared = headers === '' && saved

  return (
    <li className='feed-card'>
      <div className='feed-head'>
        <input className='input' aria-label={`Nom de l’hébergeur ${index + 1}`} placeholder='Nom (ex. Mon hébergeur)' maxLength={40} value={host.name} onChange={(e) => onChange({ name: e.target.value })} />
        <button type='button' className='icon-btn danger' title='Retirer cet hébergeur' aria-label={`Retirer l’hébergeur ${host.name || index + 1}`} onClick={onRemove}>
          <Trash size={18} />
        </button>
      </div>
      <div className='field'>
        <label htmlFor={`${id}-url`}>Adresse d’envoi</label>
        <input id={`${id}-url`} className='input mono' spellCheck={false} placeholder='https://exemple.com/api/upload' value={host.url} onChange={(e) => onChange({ url: e.target.value })} />
        <p className='help'>En https://. « {'{filename}'} » y est remplacé par le nom du fichier.</p>
      </div>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor={`${id}-method`}>Envoi</label>
          <Select
            id={`${id}-method`} ariaLabel='Méthode d’envoi' value={host.method} onChange={(v) => onChange({ method: v as CustomHost['method'] })}
            options={[{ value: 'multipart', label: 'Formulaire (POST multipart)' }, { value: 'put', label: 'Fichier brut (PUT)' }]}
          />
        </div>
        {host.method === 'multipart' && (
          <div className='field'>
            <label htmlFor={`${id}-file-field`}>Champ du fichier</label>
            <input id={`${id}-file-field`} className='input mono' spellCheck={false} placeholder='file' value={host.fileField} onChange={(e) => onChange({ fileField: e.target.value })} />
          </div>
        )}
      </div>
      {host.method === 'multipart' && (
        <div className='field'>
          <label htmlFor={`${id}-fields`}>Autres champs du formulaire <span className='optional'>(un par ligne : nom=valeur)</span></label>
          <textarea
            id={`${id}-fields`} className='input mono' rows={2} spellCheck={false} placeholder='reqtype=fileupload' value={fieldsText}
            onChange={(e) => { setFieldsText(e.target.value); onChange({ formFields: textToFields(e.target.value) }) }}
          />
        </div>
      )}
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor={`${id}-path`}>Lien dans la réponse <span className='optional'>(chemin JSON)</span></label>
          <input id={`${id}-path`} className='input mono' spellCheck={false} placeholder='Vide : la réponse est le lien' value={host.linkPath} onChange={(e) => onChange({ linkPath: e.target.value })} />
        </div>
        <div className='field'>
          <label htmlFor={`${id}-template`}>Modèle du lien <span className='optional'>(facultatif)</span></label>
          <input id={`${id}-template`} className='input mono' spellCheck={false} placeholder='https://exemple.com/f/{value}' value={host.linkTemplate} onChange={(e) => onChange({ linkTemplate: e.target.value })} />
        </div>
      </div>
      <div className='field'>
        <label htmlFor={`${id}-headers`}>En-têtes <span className='optional'>(facultatif, un par ligne : Nom: valeur)</span></label>
        <div className='input-row'>
          <textarea
            id={`${id}-headers`} className='input mono' rows={2} spellCheck={false} autoComplete='off'
            placeholder={cleared ? 'Seront effacés à l’enregistrement' : saved ? 'Enregistrés (laisser vide pour les garder)' : 'Authorization: Bearer …'}
            value={headers ?? ''} onChange={(e) => onHeaders(e.target.value === '' ? undefined : e.target.value)}
          />
          {saved && (
            <button type='button' className='btn btn-ghost' onClick={() => onHeaders(cleared ? undefined : '')}>
              {cleared ? 'Garder' : 'Effacer'}
            </button>
          )}
        </div>
        <p className='help'>Clés d’API éventuelles, rangées dans le Gestionnaire d’identification de Windows.</p>
      </div>
    </li>
  )
}

export function CustomHostsEditor ({ hosts, onChange, savedHeaders, headers, onHeadersChange }: CustomHostsEditorProps) {
  const toast = useToast()
  const fileInput = useRef<HTMLInputElement>(null)
  const full = hosts.length >= MAX_CUSTOM_HOSTS

  const setHeaders = (id: string, value: string | undefined) => {
    const next = { ...headers }
    if (value === undefined) delete next[id]
    else next[id] = value
    onHeadersChange(next)
  }
  const add = (patch: Partial<CustomHost>, withHeaders = '') => {
    const host = newCustomHost(patch)
    onChange([...hosts, host])
    if (withHeaders) setHeaders(host.id, withHeaders)
  }
  const importSxcu = async (file: File | undefined) => {
    if (!file) return
    try {
      const { host, headers: imported } = fromSxcu(await file.text())
      add(host, imported)
      toast(`« ${host.name || file.name} » importé : enregistre les réglages pour l’utiliser`)
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <div className='field'>
      <span className='label'>Hébergeurs personnalisés</span>
      {hosts.length === 0 && (
        <p className='help' style={{ marginTop: 0 }}>
          Ajoute n’importe quel service qui accepte un envoi par formulaire ou en PUT et renvoie un lien : à la main, depuis un modèle,
          ou en important le fichier .sxcu (ShareX) que publient beaucoup d’hébergeurs.
        </p>
      )}
      <ul className='feed-list'>
        {hosts.map((host, i) => (
          <HostCard
            key={host.id} host={host} index={i}
            saved={savedHeaders.includes(host.id)} headers={headers[host.id]}
            onChange={(patch) => onChange(hosts.map((h, j) => (j === i ? { ...h, ...patch } : h)))}
            onHeaders={(value) => setHeaders(host.id, value)}
            onRemove={() => { setHeaders(host.id, undefined); onChange(hosts.filter((_, j) => j !== i)) }}
          />
        ))}
      </ul>
      <div className='input-row' style={{ flexWrap: 'wrap' }}>
        <button type='button' className='btn btn-ghost btn-sm' disabled={full} onClick={() => add({})}>
          <Plus size={14} /> Ajouter un hébergeur
        </button>
        <button type='button' className='btn btn-ghost btn-sm' disabled={full} onClick={() => fileInput.current?.click()}>
          <FileArrowUp size={14} /> Importer un fichier .sxcu
        </button>
        <input
          ref={fileInput} type='file' accept='.sxcu,application/json' hidden aria-label='Fichier .sxcu à importer'
          onChange={(e) => { void importSxcu(e.target.files?.[0]); e.target.value = '' }}
        />
        <Select
          compact ariaLabel='Partir d’un modèle' value='' disabled={full}
          onChange={(v) => { const preset = CUSTOM_PRESETS[Number(v)]; if (preset) add(preset.host) }}
          options={[{ value: '', label: 'Partir d’un modèle…' }, ...CUSTOM_PRESETS.map((p, i) => ({ value: String(i), label: p.label }))]}
        />
      </div>
    </div>
  )
}
