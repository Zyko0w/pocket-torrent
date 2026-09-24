// Barre de progression de Pocket Torrent : piste segmentée, aplat de couleur et rayures qui
// défilent pendant un transfert. L'état change la couleur et l'animation.

export type ProgressTone = 'active' | 'paused' | 'queued' | 'upload' | 'checking'

interface ProgressBarProps {
  /** Entre 0 et 1. */
  value: number
  tone: ProgressTone
  /** Avancement inconnu (recherche des infos, connexion) : un reflet qui passe en boucle. */
  indeterminate?: boolean
  label?: string
}

export function ProgressBar ({ value, tone, indeterminate = false, label = 'Progression' }: ProgressBarProps) {
  const pct = Math.max(0, Math.min(1, value)) * 100
  return (
    <div
      className={`pbar ${tone}${indeterminate ? ' indeterminate' : ''}`}
      role='progressbar'
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={indeterminate ? undefined : Math.floor(pct)}
    >
      <div className='pbar-fill' style={indeterminate ? undefined : { width: `${pct}%` }}>
        <span className='pbar-tip' aria-hidden='true' />
      </div>
    </div>
  )
}
