import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ArrowDown, ArrowUp, Broom, CheckCircle, DownloadSimple, GameController, GearSix, MagnifyingGlass,
  Pause, Play, ShieldWarning, Tag, Tray, WarningCircle
} from '@phosphor-icons/react'
import { listen } from '@tauri-apps/api/event'
import { api, errorMessage, isFinished, type GameModeSwitch, type Job, type Overview } from './api'
import { formatSpeed, plural } from './format'
import { useInterval } from './hooks'
import { SORTS, loadSort, saveSort, searchJobs, sortJobs, type SortKey } from './jobList'
import { AddDialog, type AddRequest } from './components/AddDialog'
import { Composer } from './components/Composer'
import { DeleteDialog } from './components/DeleteDialog'
import { JobRow } from './components/JobRow'
import { Logo } from './components/Logo'
import { SettingsView, type SettingsTab } from './components/SettingsView'
import { ToastProvider, useToast } from './components/Toast'
import { UpdateBanner } from './components/UpdateBanner'
import { LinkHandlerPrompt } from './components/LinkHandlerPrompt'
import { ClipboardPrompt } from './components/ClipboardPrompt'
import { DropZone } from './components/DropZone'
import { ShutdownBanner, ShutdownToggle } from './components/PowerControls'

/** Page affichée : la liste (en cours ou terminés) ou les réglages. */
type View = 'active' | 'done' | 'settings'

// ------------------------------------------------------------------ barre latérale

const GAME_MODES: { mode: GameModeSwitch; short: string; label: string }[] = [
  { mode: 'auto', short: 'Auto', label: 'Automatique : s’active quand un jeu de la liste est lancé' },
  { mode: 'on', short: 'On', label: 'Toujours activé' },
  { mode: 'off', short: 'Off', label: 'Désactivé' }
]

interface SidebarProps {
  view: View
  onView: (view: View) => void
  overview: Overview | null
  counts: { active: number; done: number }
  category: string
  onCategory: (id: string) => void
  onOpenVpn: () => void
  refresh: () => void
}

function Sidebar ({ view, onView, overview, counts, category, onCategory, onOpenVpn, refresh }: SidebarProps) {
  const toast = useToast()
  const categories = overview?.categories ?? []
  const vpn = overview?.vpn
  const game = overview?.game
  const isList = view !== 'settings'

  const vpnInfo = !vpn || vpn.state === 'off'
    ? null
    : vpn.state === 'protected' && vpn.warning
      ? { dot: 'warn', text: 'VPN sans réponse', title: `${vpn.warning} Rien ne sort sans le VPN : les téléchargements attendent.` }
      : vpn.state === 'protected'
        ? { dot: 'ok', text: `VPN · ${vpn.adapter ?? 'actif'}`, title: `Téléchargements protégés${vpn.ip ? ` (${vpn.ip})` : ''}` }
        : { dot: 'bad', text: 'VPN coupé', title: 'Tous les téléchargements sont bloqués jusqu’à son retour' }

  const setGameMode = async (mode: GameModeSwitch) => {
    try {
      await api.setGameMode(mode)
      refresh()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <aside className='sidebar'>
      <div className='brand'>
        <Logo size={30} />
        <span className='brand-name'>Pocket<span>.</span>Torrent</span>
      </div>

      <nav className='side-scroll' aria-label='Sections'>
        <div className='nav-group'>Transferts</div>
        <button type='button' className='nav-item' aria-current={view === 'active' ? 'page' : undefined} onClick={() => onView('active')}>
          <DownloadSimple size={16} /> <span className='nav-label'>En cours</span> <span className='count'>{counts.active}</span>
        </button>
        <button type='button' className='nav-item' aria-current={view === 'done' ? 'page' : undefined} onClick={() => onView('done')}>
          <CheckCircle size={16} /> <span className='nav-label'>Terminés</span> <span className='count'>{counts.done}</span>
        </button>

        {categories.length > 0 && (
          <>
            <div className='nav-group'>Catégories</div>
            <button
              type='button' className='nav-item' aria-current={isList && category === '' ? 'true' : undefined}
              onClick={() => { onCategory(''); if (!isList) onView('active') }}
            >
              <Tag size={16} /> <span className='nav-label'>Toutes</span>
            </button>
            {categories.map((c) => (
              <button
                key={c.id} type='button' className='nav-item' aria-current={isList && category === c.id ? 'true' : undefined}
                onClick={() => { onCategory(c.id); if (!isList) onView('active') }}
              >
                <Tag size={16} /> <span className='nav-label'>{c.name}</span>
              </button>
            ))}
          </>
        )}

        <div className='nav-group'>Application</div>
        <button type='button' className='nav-item' aria-current={view === 'settings' ? 'page' : undefined} onClick={() => onView('settings')}>
          <GearSix size={16} /> <span className='nav-label'>Réglages</span>
        </button>
      </nav>

      <div className='side-status'>
        <div className='speed-block' aria-label='Vitesses totales'>
          <div className={`speed${(overview?.downSpeed ?? 0) > 0 ? ' moving' : ''}`} title='Réception totale'>
            <span className='eyebrow'><ArrowDown size={11} /> Réception</span>
            <b>{formatSpeed(overview?.downSpeed ?? 0)}</b>
          </div>
          <div className={`speed${(overview?.upSpeed ?? 0) > 0 ? ' moving' : ''}`} title='Envoi total'>
            <span className='eyebrow'><ArrowUp size={11} /> Envoi</span>
            <b>{formatSpeed(overview?.upSpeed ?? 0)}</b>
          </div>
        </div>
        {vpnInfo && (
          <button type='button' className='status-row' title={vpnInfo.title} onClick={onOpenVpn}>
            <span className={`dot ${vpnInfo.dot}`} aria-hidden='true' />
            <span className='status-text'>{vpnInfo.text}</span>
          </button>
        )}
        {game && (
          <div className='status-row' title={game.active ? `Mode jeu actif : ${game.action === 'pause' ? 'téléchargements en pause' : 'vitesse limitée'}` : 'Mode jeu inactif'}>
            <GameController size={15} weight={game.active ? 'fill' : 'regular'} className={game.active ? 'tone-active' : undefined} aria-hidden='true' />
            <span className='status-text'>{game.active ? <b>{game.process ?? 'Jeu en cours'}</b> : 'Mode jeu'}</span>
            <div className='segmented' role='radiogroup' aria-label='Mode jeu'>
              {GAME_MODES.map(({ mode, short, label }) => (
                <button
                  key={mode} type='button' role='radio' aria-checked={game.mode === mode} title={label}
                  onClick={() => { if (game.mode !== mode) setGameMode(mode) }}
                >
                  {short}
                </button>
              ))}
            </div>
          </div>
        )}
        <ShutdownToggle overview={overview} refresh={refresh} />
      </div>
    </aside>
  )
}

// ------------------------------------------------------------------ liste

interface DownloadsProps {
  view: 'active' | 'done'
  overview: Overview | null
  jobs: Job[]
  category: string
  refresh: () => void
  prefill: string | null
  onPrefillUsed: () => void
  onAdded: () => void
}

function Downloads ({ view, overview, jobs, category, refresh, prefill, onPrefillUsed, onAdded }: DownloadsProps) {
  const toast = useToast()
  const [query, setQuery] = useState('')
  const [sort, setSort] = useState<SortKey>(loadSort)
  const [choosing, setChoosing] = useState<AddRequest | null>(null)
  const [deleting, setDeleting] = useState<Job | null>(null)

  const categories = overview?.categories ?? []
  const categoryNames = useMemo(() => new Map(categories.map((c) => [c.id, c.name])), [categories])
  const shown = sortJobs(searchJobs(jobs, query), sort)
  const changeSort = (key: SortKey) => { setSort(key); saveSort(key) }
  const categoryName = category ? categoryNames.get(category) : null

  const confirmDelete = async (deleteFiles: boolean, deleteOnline: boolean, deleteSource: boolean) => {
    if (!deleting) return
    const target = deleting
    setDeleting(null)
    try {
      await api.remove(target.id, deleteFiles, deleteOnline, deleteSource)
      toast(deleteFiles || deleteOnline || deleteSource ? 'Téléchargement et fichiers supprimés' : 'Retiré de la liste')
      refresh()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  const bulk = async (action: 'pause' | 'resume' | 'clear') => {
    try {
      if (action === 'clear') {
        const count = await api.clearFinished()
        toast(count ? `${plural(count, 'téléchargement')} retiré${count > 1 ? 's' : ''} de la liste` : 'Rien à retirer')
      } else {
        const count = action === 'pause' ? await api.pauseAll() : await api.resumeAll()
        toast(action === 'pause' ? `${plural(count, 'téléchargement')} en pause` : `${plural(count, 'téléchargement')} repris`)
      }
      refresh()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  const active = view === 'active'

  return (
    <>
      <header className='page-head'>
        <div>
          <div className='crumbs eyebrow'>
            <span>/transferts</span><span>›</span><span>{active ? '/en-cours' : '/terminés'}</span>
            {categoryName && <><span>›</span><span>{categoryName}</span></>}
          </div>
          <h1 className='page-title'>
            {active ? 'En cours' : 'Terminés'}
            <span className='count-big'>{jobs.length}</span>
          </h1>
        </div>
        <div className='page-actions'>
          {active
            ? (
              <div className='btn-group' role='group' aria-label='Actions sur tous les téléchargements'>
                <button type='button' className='btn btn-ghost btn-sm' onClick={() => bulk('pause')}><Pause size={14} /> Tout en pause</button>
                <button type='button' className='btn btn-ghost btn-sm' onClick={() => bulk('resume')}><Play size={14} /> Tout reprendre</button>
              </div>
              )
            : (
              <button type='button' className='btn btn-ghost btn-sm' disabled={!jobs.some((j) => j.status === 'done')} onClick={() => bulk('clear')}>
                <Broom size={14} /> Vider la liste
              </button>
              )}
        </div>
      </header>

      {active && (
        <Composer
          prefill={prefill}
          onPrefillUsed={onPrefillUsed}
          categories={categories}
          defaultCategory={category}
          onChooseFiles={setChoosing}
          onAdded={onAdded}
        />
      )}

      <div className='list-tools'>
        <label className='search-inline'>
          <MagnifyingGlass size={14} aria-hidden='true' />
          <span className='sr-only'>Rechercher un téléchargement</span>
          <input
            type='search'
            value={query}
            placeholder='Rechercher par nom ou lien'
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Escape') setQuery('') }}
          />
        </label>
        <span className='spacer' />
        <label className='select-inline'>
          <span className='sr-only'>Trier</span>
          <select value={sort} onChange={(e) => changeSort(e.target.value as SortKey)}>
            {SORTS.map((s) => <option key={s.key} value={s.key}>{s.label}</option>)}
          </select>
        </label>
      </div>

      {shown.length
        ? (
          <ul className='jobs' aria-label={active ? 'Téléchargements en cours' : 'Téléchargements terminés'}>
            {shown.map((job) => (
              <JobRow
                key={job.id}
                job={job}
                categoryName={job.category ? categoryNames.get(job.category) ?? null : null}
                onDelete={setDeleting}
                onChanged={refresh}
              />
            ))}
          </ul>
          )
        : (
          <div className='empty'>
            <Tray size={28} />
            <p>{query ? `Aucun téléchargement ne correspond à « ${query} ».` : active ? 'Aucun téléchargement en cours. Colle un lien ci-dessus ou glisse un .torrent dans la fenêtre.' : 'Rien de terminé pour l’instant.'}</p>
          </div>
          )}

      <AddDialog request={choosing} onClose={() => setChoosing(null)} onAdded={(name) => { toast(`« ${name} » ajouté`); onAdded() }} />
      <DeleteDialog job={deleting} onCancel={() => setDeleting(null)} onConfirm={confirmDelete} />
    </>
  )
}

// ------------------------------------------------------------------ application

export default function App () {
  const [view, setView] = useState<View>('active')
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('general')
  const [category, setCategory] = useState('')
  const [overview, setOverview] = useState<Overview | null>(null)
  const [offline, setOffline] = useState(false)
  const [prefill, setPrefill] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      setOverview(await api.overview())
      setOffline(false)
    } catch {
      setOffline(true)
    }
  }, [])

  useInterval(refresh, 1000)

  // lien magnet ou fichier .torrent reçu au lancement, ou transmis par une autre ouverture de Pocket Torrent
  useEffect(() => {
    api.takeLaunchLinks().then((links) => { if (links.length) setPrefill(links.join('\n')) }).catch(() => {})
    const unlisten = listen<string>('link-received', (event) => {
      setView('active')
      setPrefill(event.payload)
    })
    return () => { unlisten.then((stop) => stop()) }
  }, [])

  // catégorie supprimée dans les réglages
  const categories = overview?.categories
  useEffect(() => { if (category && categories && !categories.some((c) => c.id === category)) setCategory('') }, [categories, category])

  const onPrefillUsed = useCallback(() => setPrefill(null), [])

  const onDropped = useCallback((items: string[]) => {
    setView('active')
    setPrefill(items.join('\n'))
  }, [])

  const inCategory = (overview?.jobs ?? []).filter((j) => !category || j.category === category)
  const activeJobs = inCategory.filter((j) => !isFinished(j))
  const doneJobs = inCategory.filter(isFinished)

  return (
    <ToastProvider>
      <DropZone onDropped={onDropped} />
      <LinkHandlerPrompt />
      <div className='app'>
        <Sidebar
          view={view}
          onView={setView}
          overview={overview}
          counts={{ active: activeJobs.length, done: doneJobs.length }}
          category={category}
          onCategory={setCategory}
          onOpenVpn={() => { setSettingsTab('network'); setView('settings') }}
          refresh={refresh}
        />
        <main className='content'>
          <div className='wrap'>
            {offline && (
              <div className='banner danger' role='alert'><WarningCircle size={16} /> Le moteur de téléchargement ne répond pas.</div>
            )}
            <UpdateBanner />
            <ClipboardPrompt onEdit={(link) => { setView('active'); setPrefill(link) }} onAdded={() => { setView('active'); refresh() }} />
            <ShutdownBanner overview={overview} refresh={refresh} />
            {overview?.vpn.state === 'blocked' && view !== 'settings' && (
              <div className='banner danger' role='status'>
                <ShieldWarning size={16} /> VPN coupé : les téléchargements sont en pause et rien ne passe sans lui. Ils reprendront dès son retour.
              </div>
            )}
            {view === 'settings'
              ? <SettingsView tab={settingsTab} onTabChange={setSettingsTab} onSaved={refresh} />
              : (
                <Downloads
                  key={view}
                  view={view}
                  overview={overview}
                  jobs={view === 'active' ? activeJobs : doneJobs}
                  category={category}
                  refresh={refresh}
                  prefill={prefill}
                  onPrefillUsed={onPrefillUsed}
                  onAdded={() => { setView('active'); refresh() }}
                />
                )}
          </div>
        </main>
      </div>
    </ToastProvider>
  )
}
