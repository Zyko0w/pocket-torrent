// Flux RSS de séries : liste des flux, filtres, test et état de la dernière vérification.
import { useCallback, useState } from 'react'
import { ArrowClockwise, CheckCircle, Flask, Plus, SpinnerGap, Trash, XCircle } from '@phosphor-icons/react'
import { api, errorMessage, type FeedStatus, type Settings } from '../../api'
import { formatDate } from '../../format'
import { useInterval } from '../../hooks'
import { useToast } from '../Toast'
import { Select } from '../Select'
import { Check, Section, type SectionProps } from './fields'

type Feed = Settings['rss']['feeds'][number]

const EMPTY: Feed = { id: '', name: '', url: '', include: '', exclude: '', category: null, enabled: true, skipRepeats: true, regex: false }

function FeedCard ({ feed, index, status, categories, onChange, onRemove }: {
  feed: Feed
  index: number
  status: FeedStatus | undefined
  categories: Settings['categories']
  onChange: (patch: Partial<Feed>) => void
  onRemove: () => void
}) {
  const [preview, setPreview] = useState<{ title: string; matched: boolean }[] | null>(null)
  const [testing, setTesting] = useState(false)
  const [error, setError] = useState('')
  const id = `rss-${index}`

  const test = async () => {
    setTesting(true)
    setError('')
    try {
      setPreview(await api.rssPreview(feed.url, feed.include, feed.exclude, feed.regex))
    } catch (err) {
      setPreview(null)
      setError(errorMessage(err))
    } finally {
      setTesting(false)
    }
  }
  const matched = preview?.filter((p) => p.matched).length ?? 0

  return (
    <li className='feed-card'>
      <div className='feed-head'>
        <input className='input' aria-label={`Nom du flux ${index + 1}`} placeholder='Nom (ex. Ma série)' value={feed.name} onChange={(e) => onChange({ name: e.target.value })} />
        <label className='toggle-row feed-enabled'>
          <span className='toggle-text'>Actif</span>
          <input type='checkbox' role='switch' className='switch' checked={feed.enabled} onChange={(e) => onChange({ enabled: e.target.checked })} />
        </label>
        <button type='button' className='icon-btn danger' title='Retirer ce flux' aria-label={`Retirer le flux ${feed.name || index + 1}`} onClick={onRemove}>
          <Trash size={18} />
        </button>
      </div>
      <div className='field'>
        <label htmlFor={`${id}-url`}>Adresse du flux</label>
        <input id={`${id}-url`} className='input mono' spellCheck={false} placeholder='https://…/rss' value={feed.url} onChange={(e) => onChange({ url: e.target.value })} />
      </div>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor={`${id}-include`}>Doit contenir <span className='optional'>{feed.regex ? '(expression régulière)' : '(tous ces mots)'}</span></label>
          <input id={`${id}-include`} className={`input${feed.regex ? ' mono' : ''}`} spellCheck={false} placeholder={feed.regex ? 's0[12]e\\d+.*1080p' : 'ma série 1080p'} value={feed.include} onChange={(e) => onChange({ include: e.target.value })} />
        </div>
        <div className='field'>
          <label htmlFor={`${id}-exclude`}>Ne doit pas contenir <span className='optional'>{feed.regex ? '(expression régulière)' : '(aucun de ces mots)'}</span></label>
          <input id={`${id}-exclude`} className={`input${feed.regex ? ' mono' : ''}`} spellCheck={false} placeholder={feed.regex ? '\\b(hdr|cam)\\b' : '720p cam'} value={feed.exclude} onChange={(e) => onChange({ exclude: e.target.value })} />
        </div>
      </div>
      <div className='field-pair'>
        {categories.length > 0 && (
          <div className='field'>
            <label htmlFor={`${id}-category`}>Catégorie</label>
            <Select
              id={`${id}-category`} ariaLabel='Catégorie' value={feed.category ?? ''} onChange={(v) => onChange({ category: v || null })}
              options={[{ value: '', label: 'Sans catégorie' }, ...categories.map((c) => ({ value: c.id, label: c.name }))]}
            />
          </div>
        )}
        <Check checked={feed.skipRepeats} onChange={(skipRepeats) => onChange({ skipRepeats })} label='Un seul fichier par épisode' help='S02E05 publié en 1080p puis en 720p : seul le premier est ajouté.' />
        <Check checked={feed.regex} onChange={(regex) => onChange({ regex })} label='Expressions régulières' help='Pour les filtres précis : « s0[12]e\\d+ » (saisons 1 et 2). Sans tenir compte des majuscules.' />
      </div>

      <div className='feed-foot'>
        <span className={`help${status?.error ? ' tone-danger' : ''}`} style={{ margin: 0 }} role='status'>
          {!feed.id
            ? 'Enregistre pour activer ce flux.'
            : status?.error
              ? status.error
              : status?.checkedAt
                ? `Vérifié le ${formatDate(status.checkedAt)} · ${status.added} ajouté${status.added > 1 ? 's' : ''}${status.lastAdded.length ? ` · dernier : ${status.lastAdded[0]}` : ''}`
                : 'Pas encore vérifié. À la première lecture, les éléments déjà présents sont ignorés.'}
        </span>
        <button type='button' className='btn btn-ghost btn-sm' disabled={testing || !feed.url.trim()} onClick={test}>
          {testing ? <SpinnerGap size={14} className='spin' /> : <Flask size={14} />} Tester les filtres
        </button>
      </div>
      {error && <p className='field-error' role='alert'>{error}</p>}
      {preview && (
        <div className='feed-preview'>
          <p className='help' style={{ margin: 0 }}>{preview.length} élément{preview.length > 1 ? 's' : ''} dans le flux, {matched} passe{matched > 1 ? 'nt' : ''} les filtres :</p>
          <ul>
            {preview.map((p, i) => (
              <li key={i} className={p.matched ? '' : 'muted'}>
                {p.matched ? <CheckCircle size={14} className='tone-success' aria-label='passe les filtres' /> : <XCircle size={14} aria-label='filtré' />}
                <span className='selectable'>{p.title}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </li>
  )
}

export function RssSection ({ settings, update }: SectionProps) {
  const toast = useToast()
  const rss = settings.rss
  const [status, setStatus] = useState<FeedStatus[]>([])
  const [checking, setChecking] = useState(false)
  const load = useCallback(() => { api.rssStatus().then(setStatus).catch(() => {}) }, [])
  useInterval(load, 5000)

  const setFeeds = (feeds: Feed[]) => update({ rss: { ...rss, feeds } })
  const checkNow = async () => {
    setChecking(true)
    try {
      const next = await api.rssCheckNow()
      setStatus(next)
      const added = next.reduce((n, s) => n + (s.lastAdded.length ? 1 : 0), 0)
      toast(next.some((s) => s.error) ? 'Vérification faite, avec des erreurs (voir les flux)' : added ? 'Flux vérifiés : nouveaux éléments ajoutés' : 'Flux vérifiés : rien de nouveau')
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setChecking(false)
    }
  }

  return (
    <Section id='s-rss' title='Flux RSS' intro='Récupère automatiquement les nouveaux épisodes publiés dans des flux RSS (sites de torrents, Jackett, Prowlarr).'>
      <div className='field'>
        <label htmlFor='rss-interval'>Vérifier les flux</label>
        <div className='input-row'>
          <Select
            id='rss-interval' ariaLabel='Vérifier les flux' value={String(rss.intervalMinutes)}
            onChange={(v) => update({ rss: { ...rss, intervalMinutes: Number(v) } })}
            options={[5, 15, 30, 60, 180].map((m) => ({ value: String(m), label: `toutes les ${m < 60 ? `${m} min` : `${m / 60} h`}` }))}
          />
          <button type='button' className='btn btn-ghost btn-sm' disabled={checking || !rss.feeds.some((f) => f.id && f.enabled)} onClick={checkNow}>
            {checking ? <SpinnerGap size={14} className='spin' /> : <ArrowClockwise size={14} />} Vérifier maintenant
          </button>
        </div>
      </div>
      {rss.feeds.length === 0 && <p className='help' style={{ marginTop: 0 }}>Aucun flux. Ajoute l’adresse RSS d’un site ou de Jackett/Prowlarr.</p>}
      <ul className='feed-list'>
        {rss.feeds.map((feed, i) => (
          <FeedCard
            key={feed.id || `nouveau-${i}`}
            feed={feed} index={i} categories={settings.categories}
            status={status.find((s) => s.id === feed.id)}
            onChange={(patch) => setFeeds(rss.feeds.map((f, j) => (j === i ? { ...f, ...patch } : f)))}
            onRemove={() => setFeeds(rss.feeds.filter((_, j) => j !== i))}
          />
        ))}
      </ul>
      <button type='button' className='btn btn-ghost btn-sm' disabled={rss.feeds.length >= 30} onClick={() => setFeeds([...rss.feeds, { ...EMPTY }])}>
        <Plus size={14} /> Ajouter un flux
      </button>
      <p className='help'>Les flux passent par le même chemin réseau que les téléchargements (VPN compris).</p>
    </Section>
  )
}
