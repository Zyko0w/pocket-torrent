import { useEffect, useRef, useState, type FormEvent } from 'react'
import { FileArrowUp, Plus, SpinnerGap, WarningCircle } from '@phosphor-icons/react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type AddOptions, type Dest, type Priority } from '../api'
import { plural } from '../format'
import { parseLinks } from '../links'
import type { AddRequest } from './AddDialog'
import { useToast } from './Toast'

interface ComposerProps {
  prefill: string | null
  /** Appelé une fois `prefill` reporté dans le champ, pour ne pas le remettre au prochain affichage. */
  onPrefillUsed: () => void
  categories: { id: string; name: string }[]
  /** Catégorie choisie dans la barre latérale : proposée par défaut. */
  defaultCategory: string
  onChooseFiles: (request: AddRequest) => void
  onAdded: () => void
}

const isTorrentLink = (link: string) => /^magnet:/i.test(link) || /\.torrent$/i.test(link)

export function Composer ({ prefill, onPrefillUsed, categories, defaultCategory, onChooseFiles, onAdded }: ComposerProps) {
  const toast = useToast()
  const textarea = useRef<HTMLTextAreaElement>(null)
  const [links, setLinks] = useState('')
  const [chooseFiles, setChooseFiles] = useState(false)
  const [category, setCategory] = useState(defaultCategory)
  const [priority, setPriority] = useState<Priority>('normal')
  const [dest, setDest] = useState<Dest>('local')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  const list = parseLinks(links)

  useEffect(() => {
    if (!prefill) return
    setLinks((current) => (current.includes(prefill) ? current : [current.trim(), prefill].filter(Boolean).join('\n')))
    textarea.current?.focus()
    onPrefillUsed()
  }, [prefill, onPrefillUsed])
  useEffect(() => { setCategory(defaultCategory) }, [defaultCategory])

  // catégorie supprimée dans les réglages
  useEffect(() => { if (category && !categories.some((c) => c.id === category)) setCategory('') }, [categories, category])

  // le champ grandit avec le nombre de liens
  useEffect(() => {
    const el = textarea.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`
  }, [links])

  const options = (): AddOptions => ({
    category: category || null,
    priority,
    dest,
    limitKbps: 0,
    onlyFiles: null
  })

  const addAll = async (items: string[]) => {
    try {
      const report = await api.addLinks(items, options())
      setLinks(report.failed.join('\n'))
      setError(report.errors.join(' · '))
      if (report.added) {
        toast(`${plural(report.added, 'téléchargement')} ajouté${report.added > 1 ? 's' : ''}`)
        onAdded()
      }
    } catch (err) {
      setError(errorMessage(err))
    }
  }

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (!list.length) {
      setError('Colle au moins un lien.')
      textarea.current?.focus()
      return
    }
    if (chooseFiles) {
      const torrents = list.filter(isTorrentLink)
      if (list.length > 1 || torrents.length === 0) {
        setError('Pour choisir les fichiers, ajoute un seul torrent (magnet ou .torrent) à la fois.')
        return
      }
      setError('')
      setLinks('')
      onChooseFiles({ link: list[0], options: options() })
      return
    }
    setBusy(true)
    await addAll(list)
    setBusy(false)
  }

  const openTorrentFiles = async () => {
    const picked = await open({ multiple: true, filters: [{ name: 'Fichiers torrent', extensions: ['torrent'] }] })
    const paths = Array.isArray(picked) ? picked : picked ? [picked] : []
    if (!paths.length) return
    if (chooseFiles && paths.length === 1) {
      onChooseFiles({ link: paths[0], options: options() })
      return
    }
    setBusy(true)
    await addAll(paths)
    setBusy(false)
  }

  return (
    <form className='composer' onSubmit={submit} noValidate>
      <div className='composer-row'>
        <label className='sr-only' htmlFor='links'>Liens à télécharger</label>
        <textarea
          id='links'
          ref={textarea}
          rows={1}
          spellCheck={false}
          value={links}
          aria-invalid={error ? 'true' : 'false'}
          aria-describedby='links-error'
          placeholder='Colle des liens magnet ou http(s), un par ligne — Ctrl+Entrée pour ajouter'
          onChange={(e) => { setLinks(e.target.value); if (error) setError('') }}
          onKeyDown={(e) => { if (e.key === 'Enter' && e.ctrlKey) e.currentTarget.form?.requestSubmit() }}
        />
        <button type='button' className='btn btn-ghost' title='Ouvrir un ou plusieurs fichiers .torrent' onClick={openTorrentFiles} disabled={busy}>
          <FileArrowUp size={16} /> .torrent
        </button>
        <button type='submit' className='btn btn-primary' disabled={busy}>
          {busy ? <SpinnerGap size={16} className='spin' /> : <Plus size={16} weight='bold' />}
          Ajouter
        </button>
      </div>
      {error && (
        <p className='field-error' id='links-error' role='alert'><WarningCircle size={16} /> {error}</p>
      )}
      <div className='composer-options'>
        <label className='toggle-row'>
          <input type='checkbox' role='switch' className='switch' checked={chooseFiles} onChange={(e) => setChooseFiles(e.target.checked)} />
          <span className='toggle-text'>Choisir les fichiers</span>
        </label>
        <div className='composer-selects'>
          {categories.length > 0 && (
            <label className='select-inline'>
              <span className='sr-only'>Catégorie</span>
              <select value={category} onChange={(e) => setCategory(e.target.value)}>
                <option value=''>Sans catégorie</option>
                {categories.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
              </select>
            </label>
          )}
          <label className='select-inline'>
            <span className='sr-only'>Priorité</span>
            <select value={priority} title='Ordre dans la file d’attente, et part du débit quand plusieurs téléchargent en même temps' onChange={(e) => setPriority(e.target.value as Priority)}>
              <option value='high'>Priorité haute</option>
              <option value='normal'>Priorité normale</option>
              <option value='low'>Priorité basse</option>
            </select>
          </label>
          <label className='select-inline'>
            <span className='sr-only'>Destination</span>
            <select value={dest} title='Garder les fichiers sur ce PC, ou les envoyer sur storage.to une fois terminés (lien de partage)' onChange={(e) => setDest(e.target.value as Dest)}>
              <option value='local'>Garder sur le PC</option>
              <option value='storageTo'>Envoyer sur storage.to</option>
            </select>
          </label>
        </div>
      </div>
    </form>
  )
}
