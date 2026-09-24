import { memo, useState, type ReactNode } from 'react'
import {
  ArrowClockwise, ArrowDown, Broadcast, CaretDown, CheckCircle, Clock, CloudArrowUp, Copy, FolderOpen, Gauge,
  GameController, HardDrives, Lightning, LinkSimple, ListChecks, MagnetStraight, MagnifyingGlass, Pause, Play,
  ShareNetwork, ShieldWarning, SpinnerGap, SquaresFour, Trash, Users, WarningCircle, X
} from '@phosphor-icons/react'
import { api, copyText, errorMessage, type Job, type Priority } from '../api'
import { PRIORITY_LABELS, formatDate, formatEta, formatRatio, formatSize, formatSpeed, percent, plural } from '../format'
import { FilesPanel } from './FilesPanel'
import { LinksPanel } from './LinksPanel'
import { Menu, type MenuItem } from './Menu'
import { PeersPanel } from './PeersPanel'
import { ProgressBar, type ProgressTone } from './ProgressBar'
import { PieceMapPanel } from './PieceMapPanel'
import { TrackersPanel } from './TrackersPanel'
import { useToast } from './Toast'

type Detail = 'files' | 'peers' | 'trackers' | 'pieces' | 'links'

interface StatusInfo { label: string; tone: string; icon: ReactNode }

function statusOf (job: Job): StatusInfo {
  const spinner = <SpinnerGap size={16} className='spin' />
  switch (job.status) {
    case 'metadata': return { label: 'Recherche', tone: 'tone-info', icon: spinner }
    case 'checking': return { label: 'Vérification', tone: 'tone-info', icon: spinner }
    case 'connecting': return { label: 'Connexion', tone: 'tone-info', icon: spinner }
    case 'queued': return { label: job.queuePosition ? `En attente · ${job.queuePosition}` : 'En attente', tone: '', icon: <Clock size={16} /> }
    case 'downloading': return { label: 'Téléchargement', tone: 'tone-active', icon: <ArrowDown size={16} weight='bold' /> }
    case 'paused':
      if (job.hold === 'game') return { label: 'Pause · jeu', tone: 'tone-warning', icon: <GameController size={16} /> }
      if (job.hold === 'disk') return { label: 'Pause · disque', tone: 'tone-danger', icon: <HardDrives size={16} /> }
      if (job.hold === 'vpn') return { label: 'Pause · VPN', tone: 'tone-danger', icon: <ShieldWarning size={16} /> }
      return { label: 'En pause', tone: 'tone-warning', icon: <Pause size={16} /> }
    case 'seeding': return { label: 'Partage', tone: 'tone-success', icon: <ShareNetwork size={16} /> }
    case 'uploading': return { label: 'Envoi R2', tone: 'tone-active', icon: <CloudArrowUp size={16} /> }
    case 'done':
      if (job.hold === 'seeded') return { label: 'Ratio atteint', tone: 'tone-success', icon: <CheckCircle size={16} /> }
      return { label: 'Terminé', tone: 'tone-success', icon: <CheckCircle size={16} /> }
    default: return { label: 'Erreur', tone: 'tone-danger', icon: <WarningCircle size={16} /> }
  }
}

function progressTone (job: Job): ProgressTone {
  switch (job.status) {
    case 'downloading': return 'active'
    case 'uploading': return 'upload'
    case 'checking':
    case 'connecting':
    case 'metadata': return 'checking'
    case 'queued': return 'queued'
    default: return 'paused'
  }
}

/** Chiffre principal de la colonne de droite : vitesse, pourcentage ou taille selon l'état. */
function figureOf (job: Job): ReactNode {
  const pct = Math.floor(percent(job.progressBytes, job.wantedBytes) * 100)
  switch (job.status) {
    case 'downloading': return formatSpeed(job.downSpeed)
    case 'seeding': return <><small>↑ </small>{formatSpeed(job.upSpeed)}</>
    case 'uploading': return job.delivery ? `${Math.floor(percent(job.delivery.sentBytes, job.delivery.totalBytes) * 100)} %` : null
    case 'done': return job.wantedBytes ? formatSize(job.wantedBytes) : null
    case 'metadata': return null
    default: return job.wantedBytes ? `${pct} %` : null
  }
}

function Meta ({ job }: { job: Job }) {
  const items: ReactNode[] = []
  const finished = job.status === 'done' || job.status === 'seeding'

  if (job.status === 'uploading' && job.delivery) {
    items.push(`${formatSize(job.delivery.sentBytes)} / ${formatSize(job.delivery.totalBytes)}`)
  } else if (finished) {
    if (job.kind === 'torrent' && job.uploadedBytes > 0) {
      items.push(`${formatSize(job.uploadedBytes)} envoyés`, `ratio ${formatRatio(job.uploadedBytes, job.wantedBytes)}`)
    }
    if (job.finishedAt) items.push(formatDate(job.finishedAt))
  } else if (job.status === 'metadata') {
    if (job.searchSecs !== null) items.push(<><Clock size={12} /> depuis {formatEta(job.searchSecs)}</>)
    if (job.peersFound !== null) {
      items.push(<><Users size={12} /> {job.peersFound ? `${plural(job.peersFound, 'pair')} trouvé${job.peersFound > 1 ? 's' : ''}` : 'aucun pair pour l’instant'}</>)
    }
  } else {
    if (job.wantedBytes) items.push(`${formatSize(job.progressBytes)} / ${formatSize(job.wantedBytes)}`)
    else if (job.progressBytes) items.push(formatSize(job.progressBytes))
    if (job.status === 'downloading' && job.etaSecs !== null) items.push(<><Clock size={12} /> {formatEta(job.etaSecs)}</>)
    if (job.limitKbps > 0) items.push(<><Gauge size={12} /> max {formatSpeed(job.limitKbps * 1024)}</>)
  }
  if (job.kind === 'torrent' && ['downloading', 'seeding', 'checking'].includes(job.status)) {
    items.push(<><Users size={12} /> {plural(job.peersLive, 'pair')}</>)
  }
  if (job.kind === 'torrent' && job.fileCount > 1) {
    items.push(job.selectedFiles < job.fileCount ? `${job.selectedFiles}/${job.fileCount} fichiers` : plural(job.fileCount, 'fichier'))
  }
  if (!items.length) return null

  return (
    <div className='job-meta'>
      {items.map((item, i) => (
        <span key={i} className='meta-item'>
          {i > 0 && <span className='sep' aria-hidden='true'>/</span>}
          {item}
        </span>
      ))}
    </div>
  )
}

interface JobRowProps {
  job: Job
  categoryName: string | null
  onDelete: (job: Job) => void
  onShare: (job: Job) => void
  onChanged: () => void
}

export const JobRow = memo(function JobRow ({ job, categoryName, onDelete, onShare, onChanged }: JobRowProps) {
  const toast = useToast()
  const [detail, setDetail] = useState<Detail | null>(null)
  const [limitOpen, setLimitOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [limit, setLimit] = useState(String(job.limitKbps))
  const status = statusOf(job)
  const isTorrent = job.kind === 'torrent'
  const isPending = job.id.startsWith('p:')
  const finished = job.status === 'done' || job.status === 'seeding'
  const name = job.name ?? (isTorrent ? 'Recherche du nom…' : job.link ?? 'Téléchargement')

  const canShowPeers = isTorrent && job.inEngine && (job.status === 'downloading' || job.status === 'seeding')
  const canPause = !isPending && ['checking', 'connecting', 'queued', 'downloading', 'seeding'].includes(job.status)
  const canResume = job.status === 'paused' || job.status === 'error' || (job.status === 'done' && isTorrent && job.inEngine)
  const showProgress = ['checking', 'connecting', 'queued', 'downloading', 'paused', 'uploading', 'metadata'].includes(job.status)
  const progressValue = job.status === 'uploading' && job.delivery
    ? percent(job.delivery.sentBytes, job.delivery.totalBytes)
    : percent(job.progressBytes, job.wantedBytes)

  // volets de détails disponibles pour ce téléchargement, dans l'ordre des onglets
  const details: { id: Detail; label: string; icon: ReactNode }[] = []
  if (isTorrent && job.inEngine) details.push({ id: 'files', label: 'Fichiers', icon: <ListChecks size={14} /> })
  if (canShowPeers) details.push({ id: 'peers', label: 'Pairs', icon: <Users size={14} /> })
  if (isTorrent && job.inEngine) details.push({ id: 'trackers', label: 'Trackers', icon: <Broadcast size={14} /> })
  if (isTorrent && job.inEngine) details.push({ id: 'pieces', label: 'Pièces', icon: <SquaresFour size={14} /> })
  if (job.delivery && job.delivery.links.length > 0) details.push({ id: 'links', label: 'Liens R2', icon: <LinkSimple size={14} /> })
  const shown = details.find((d) => d.id === detail) ? detail : null
  const toggleDetails = () => setDetail(shown ? null : details[0]?.id ?? null)

  const run = async (action: () => Promise<unknown>, done?: string) => {
    setBusy(true)
    try {
      await action()
      if (done) toast(done)
      onChanged()
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  const resumeLabel = job.status === 'error' ? 'Réessayer' : job.status === 'done' ? 'Partager à nouveau' : 'Reprendre'
  const pauseLabel = job.status === 'seeding' ? 'Arrêter le partage' : 'Mettre en pause'
  const removeLabel = finished || job.status === 'error' ? 'Supprimer…' : 'Arrêter et supprimer…'

  const menu: MenuItem[] = []
  if (job.status === 'queued' || job.status === 'paused') {
    menu.push({ label: 'Démarrer maintenant', icon: <Lightning size={16} />, onSelect: () => run(() => api.startNow(job.id)) })
  }
  if (!finished) {
    // la priorité règle l'ordre de la file d'attente (voir queue.rs), pas la vitesse
    const hints: Record<Priority, string> = {
      high: 'Démarre avant les autres dans la file',
      normal: 'Ordre d’ajout',
      low: 'Démarre après les autres dans la file'
    };
    (['high', 'normal', 'low'] as Priority[]).forEach((priority, i) => menu.push({
      label: `Priorité ${PRIORITY_LABELS[priority].toLowerCase()}`,
      hint: hints[priority],
      checked: job.priority === priority,
      separated: i === 0 && menu.length > 0,
      onSelect: () => run(() => api.setPriority(job.id, priority))
    }))
  }
  if (!isTorrent && !['done', 'error'].includes(job.status)) {
    menu.push({ label: 'Limiter la vitesse…', icon: <Gauge size={16} />, separated: true, onSelect: () => { setLimit(String(job.limitKbps)); setLimitOpen(true) } })
  }
  if (isTorrent && !isPending) {
    menu.push({
      label: 'Copier le lien magnet',
      icon: <MagnetStraight size={16} />,
      separated: true,
      onSelect: () => run(async () => copyText(await api.magnet(job.id)), 'Lien magnet copié')
    })
  }
  if (job.link && !isTorrent) {
    menu.push({ label: 'Copier le lien d’origine', icon: <Copy size={16} />, separated: true, onSelect: () => run(() => copyText(job.link!), 'Lien copié') })
  }
  if (job.canShare) {
    menu.push({ label: 'Partager en torrent…', icon: <ShareNetwork size={16} />, onSelect: () => onShare(job) })
  }
  if (isTorrent && job.inEngine) {
    menu.push({ label: 'Revérifier', icon: <MagnifyingGlass size={16} />, onSelect: () => run(() => api.recheck(job.id), 'Vérification relancée') })
  }
  menu.push({ label: 'Supprimer…', icon: <Trash size={16} />, separated: true, danger: true, onSelect: () => onDelete(job) })

  const figure = figureOf(job)

  return (
    <li className={`job${shown ? ' open' : ''}`}>
      <div className='job-line'>
        <span className={`job-icon ${status.tone}${job.status === 'downloading' || job.status === 'uploading' ? ' live' : ''}`} aria-hidden='true'>{status.icon}</span>
        <div className='job-main'>
          <div className='job-title'>
            {details.length
              ? (
                <button type='button' className='job-name-btn' aria-expanded={!!shown} onClick={toggleDetails} title={name}>
                  <h3 className='job-name'>{name}</h3>
                </button>
                )
              : <h3 className='job-name selectable' title={name}>{name}</h3>}
            {categoryName && <span className='tag'>{categoryName}</span>}
            {job.priority !== 'normal' && !finished && <span className={`tag${job.priority === 'high' ? ' accent' : ''}`}>{PRIORITY_LABELS[job.priority]}</span>}
            {job.kind === 'http' && <span className='tag'>Direct</span>}
            {(job.dest === 'r2' || job.delivery?.state === 'done') && <span className='tag'><CloudArrowUp size={11} /> R2</span>}
          </div>
          {showProgress && (
            <ProgressBar
              value={progressValue}
              tone={progressTone(job)}
              indeterminate={job.status === 'metadata' || (job.status === 'connecting' && !job.wantedBytes)}
            />
          )}
          <Meta job={job} />
        </div>

        <div className='job-state'>
          <span className={`status ${status.tone}`}>{status.label}</span>
          {figure && <span className='job-figure'>{figure}</span>}
        </div>

        <div className='job-actions'>
          {canResume
            ? (
              <button type='button' className='icon-btn primary-action' title={resumeLabel} aria-label={resumeLabel} disabled={busy} onClick={() => run(() => api.resume(job.id))}>
                {job.status === 'error' ? <ArrowClockwise size={16} /> : <Play size={16} weight='fill' />}
              </button>
              )
            : (
              <button type='button' className='icon-btn primary-action' title={pauseLabel} aria-label={pauseLabel} disabled={busy || !canPause} onClick={() => run(() => api.pause(job.id))}>
                <Pause size={16} weight='fill' />
              </button>
              )}
          <button type='button' className='icon-btn' title='Ouvrir le dossier' aria-label='Ouvrir le dossier' disabled={isPending} onClick={() => run(() => api.openFolder(job.id))}>
            <FolderOpen size={18} />
          </button>
          <button
            type='button' className='icon-btn' title='Détails' aria-label='Détails' aria-expanded={!!shown}
            disabled={!details.length} onClick={toggleDetails}
          >
            <CaretDown size={16} style={{ transform: shown ? 'rotate(180deg)' : undefined, transition: 'transform 150ms' }} />
          </button>
          <Menu label='Plus d’actions' items={menu} />
          <button
            type='button' className='icon-btn danger' title={removeLabel} aria-label={removeLabel}
            onClick={() => onDelete(job)}
          >
            <X size={16} weight='bold' />
          </button>
        </div>
      </div>

      {job.error && <p className='job-alert tone-danger selectable'><WarningCircle size={16} /> {job.error}</p>}
      {job.warning && !job.error && <p className='job-alert tone-warning'><HardDrives size={16} /> {job.warning}</p>}

      {limitOpen && (
        <form
          className='panel panel-row limit-panel'
          onSubmit={(e) => {
            e.preventDefault()
            const kbps = Math.max(0, Math.floor(Number(limit) || 0))
            run(() => api.setLimit(job.id, kbps), kbps ? `Limité à ${formatSpeed(kbps * 1024)}` : 'Limite retirée').then(() => setLimitOpen(false))
          }}
        >
          <label className='label' htmlFor={`limit-${job.id}`} style={{ margin: 0 }}>Vitesse max <span className='optional'>(Ko/s, 0 = illimitée)</span></label>
          <div className='input-row'>
            <input id={`limit-${job.id}`} className='input input-narrow' type='number' min={0} step={100} value={limit} onChange={(e) => setLimit(e.target.value)} />
            <button type='submit' className='btn btn-primary btn-sm' disabled={busy}>Appliquer</button>
            <button type='button' className='btn btn-ghost btn-sm' onClick={() => setLimitOpen(false)}>Fermer</button>
          </div>
        </form>
      )}

      {shown && (
        <div className='job-details'>
          <div className='subtabs' role='tablist' aria-label='Détails'>
            {details.map((d) => (
              <button key={d.id} type='button' role='tab' className='subtab' aria-selected={d.id === shown} onClick={() => setDetail(d.id)}>
                {d.icon} {d.label}
              </button>
            ))}
          </div>
          {shown === 'files' && <FilesPanel jobId={job.id} />}
          {shown === 'peers' && <PeersPanel jobId={job.id} />}
          {shown === 'trackers' && <TrackersPanel jobId={job.id} />}
          {shown === 'pieces' && <PieceMapPanel jobId={job.id} />}
          {shown === 'links' && job.delivery && <LinksPanel delivery={job.delivery} />}
        </div>
      )}
    </li>
  )
}, sameRow)

/**
 * La liste est relue chaque seconde et chaque téléchargement y arrive comme un nouvel objet :
 * sans cette comparaison, toutes les lignes seraient redessinées à chaque fois, même les
 * téléchargements terminés qui ne bougent plus.
 */
function sameRow (a: JobRowProps, b: JobRowProps): boolean {
  return a.categoryName === b.categoryName && a.onDelete === b.onDelete && a.onShare === b.onShare &&
    a.onChanged === b.onChanged && JSON.stringify(a.job) === JSON.stringify(b.job)
}
