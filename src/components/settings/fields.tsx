// Briques communes des sections de réglages.
import type { ReactNode } from 'react'
import { ArrowCounterClockwise } from '@phosphor-icons/react'
import { open } from '@tauri-apps/plugin-dialog'
import type { Settings } from '../../api'

/** Ce que reçoit chaque section : les réglages en cours d'édition et de quoi les modifier. */
export interface SectionProps {
  settings: Settings
  update: (patch: Partial<Settings>) => void
  /** Réglages d'origine, pour « valeurs par défaut ». */
  defaults: Settings
}

/** Remise à zéro d'une section : désactivée si ses réglages sont déjà ceux d'origine. */
export interface Reset {
  onReset: () => void
  disabled: boolean
}

/**
 * Remet les réglages `keys` à leur valeur d'origine dans le brouillon (rien n'est appliqué avant
 * « Enregistrer », et « Annuler » revient en arrière).
 */
export function resetOf ({ settings, update, defaults }: SectionProps, keys: (keyof Settings)[]): Reset {
  return {
    onReset: () => update(Object.fromEntries(keys.map((key) => [key, defaults[key]])) as Partial<Settings>),
    disabled: keys.every((key) => JSON.stringify(settings[key]) === JSON.stringify(defaults[key]))
  }
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
export function Section ({ id, title, intro, reset, children }: { id: string; title: string; intro: string; reset?: Reset; children: ReactNode }) {
  return (
    <section className='card' aria-labelledby={id}>
      <header className='card-head'>
        <div className='card-title-row'>
          <h2 className='card-title' id={id}>{title}</h2>
          {reset && (
            <button
              type='button' className='btn btn-ghost btn-sm card-reset' disabled={reset.disabled} onClick={reset.onReset}
              title={reset.disabled ? 'Déjà aux valeurs par défaut' : 'Remettre cette section aux valeurs par défaut (à enregistrer ensuite)'}
            >
              <ArrowCounterClockwise size={14} /> Valeurs par défaut
            </button>
          )}
        </div>
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
