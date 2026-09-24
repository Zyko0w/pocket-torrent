// Briques communes des sections de réglages.
import type { ReactNode } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import type { Settings } from '../../api'

/** Ce que reçoit chaque section : les réglages en cours d'édition et de quoi les modifier. */
export interface SectionProps {
  settings: Settings
  update: (patch: Partial<Settings>) => void
}

export const toInt = (value: string, max = Number.MAX_SAFE_INTEGER) =>
  Math.min(max, Math.max(0, Math.floor(Number(value) || 0)))
export const toFloat = (value: string) => Math.max(0, Number(value) || 0)

/** Choix d'un dossier ; `null` si l'utilisateur annule. */
export async function pickFolder (current: string): Promise<string | null> {
  const folder = await open({ directory: true, defaultPath: current || undefined })
  return typeof folder === 'string' ? folder : null
}

/** Carte de réglages : titre, phrase d'explication, puis un réglage par ligne. */
export function Section ({ id, title, intro, children }: { id: string; title: string; intro: string; children: ReactNode }) {
  return (
    <section className='card' aria-labelledby={id}>
      <header className='card-head'>
        <h2 className='card-title' id={id}>{title}</h2>
        <p className='card-intro'>{intro}</p>
      </header>
      <div className='card-body'>{children}</div>
    </section>
  )
}

/** Réglage oui/non : intitulé et aide à gauche, interrupteur à droite. */
export function Check ({ checked, onChange, label, help }: { checked: boolean; onChange: (value: boolean) => void; label: string; help?: string }) {
  return (
    <label className='toggle-row'>
      <span className='toggle-text'>{label}{help && <small>{help}</small>}</span>
      <input type='checkbox' role='switch' className='switch' checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  )
}

interface SecretFieldProps {
  id: string
  label: string
  isSet: boolean
  draft: string | null
  onChange: (value: string | null) => void
}

/** Champ de clé secrète : jamais pré-rempli, « null » = clé inchangée. */
export function SecretField ({ id, label, isSet, draft, onChange }: SecretFieldProps) {
  const cleared = draft === '' && isSet
  return (
    <div className='field'>
      <label htmlFor={id}>{label}</label>
      <div className='input-row'>
        <input
          id={id}
          className='input'
          type='password'
          autoComplete='off'
          spellCheck={false}
          placeholder={cleared ? 'Sera effacée à l’enregistrement' : isSet ? 'Enregistrée (laisser vide pour la garder)' : 'Non définie'}
          value={draft ?? ''}
          onChange={(e) => onChange(e.target.value === '' ? null : e.target.value)}
        />
        {isSet && (
          <button type='button' className='btn btn-ghost' onClick={() => onChange(cleared ? null : '')}>
            {cleared ? 'Garder' : 'Effacer'}
          </button>
        )}
      </div>
    </div>
  )
}
