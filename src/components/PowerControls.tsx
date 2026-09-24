// « Éteindre le PC quand tout est terminé » : bouton bascule et bandeau d'annulation.
import { useState } from 'react'
import { Power } from '@phosphor-icons/react'
import { api, errorMessage, type Overview } from '../api'
import { useInterval } from '../hooks'
import { useToast } from './Toast'

/** Interrupteur de la barre latérale : active ou désactive l'arrêt à la fin (session en cours). */
export function ShutdownToggle ({ overview, refresh }: { overview: Overview | null; refresh: () => void }) {
  const toast = useToast()
  const armed = overview?.power.shutdownWhenDone ?? false

  const toggle = async () => {
    try {
      await api.setShutdownWhenDone(!armed)
      toast(armed ? 'Le PC ne s’éteindra pas' : 'Le PC s’éteindra quand tout sera terminé')
      refresh()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <label className='status-row' title='Éteint le PC une minute après la fin des téléchargements en cours (pour cette session seulement)'>
      <Power size={15} className={armed ? 'tone-active' : undefined} aria-hidden='true' />
      <span className='status-text'>Éteindre à la fin</span>
      <input type='checkbox' role='switch' className='switch' checked={armed} onChange={toggle} disabled={!overview} />
    </label>
  )
}

/** Bandeau pendant la minute qui précède l'arrêt, avec compte à rebours et annulation. */
export function ShutdownBanner ({ overview, refresh }: { overview: Overview | null; refresh: () => void }) {
  const toast = useToast()
  const [now, setNow] = useState(Date.now())
  const shutdownAt = overview?.power.shutdownAt ?? null
  useInterval(() => setNow(Date.now()), shutdownAt ? 1000 : null)
  if (!shutdownAt) return null

  const seconds = Math.max(0, Math.ceil((shutdownAt - now) / 1000))
  const cancel = async () => {
    try {
      await api.cancelShutdown()
      toast('Arrêt du PC annulé')
      refresh()
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <div className='banner info' role='alert'>
      <Power size={16} />
      <span className='banner-text'>
        Téléchargements terminés : le PC s’éteint {seconds ? <>dans <b>{seconds} s</b></> : 'maintenant'}.
      </span>
      <button type='button' className='btn btn-primary btn-sm' onClick={cancel}>Annuler l’arrêt</button>
    </div>
  )
}
