// Vitesses, file d'attente, partage, planificateur et mode jeu.
import type { Settings } from '../../api'
import { Check, Section, resetOf, toFloat, toInt, type SectionProps } from './fields'

const DAY_LABELS = ['Dim', 'Lun', 'Mar', 'Mer', 'Jeu', 'Ven', 'Sam']
const minutesToTime = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`
const timeToMinutes = (t: string) => {
  const [h, m] = t.split(':').map(Number)
  return (h || 0) * 60 + (m || 0)
}

export function SpeedSection (props: SectionProps) {
  const { settings, update } = props
  return (
    <Section id='s-speed' reset={resetOf(props, ['limits', 'queue', 'seedWhenDone', 'seeding'])} title='Vitesse et file' intro='Limites pour tous les téléchargements. 0 = illimité.'>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='limit-down'>Réception max <span className='optional'>(Ko/s)</span></label>
          <input id='limit-down' className='input' type='number' min={0} step={100} value={settings.limits.downKbps} onChange={(e) => update({ limits: { ...settings.limits, downKbps: toInt(e.target.value) } })} />
        </div>
        <div className='field'>
          <label htmlFor='limit-up'>Envoi max <span className='optional'>(Ko/s)</span></label>
          <input id='limit-up' className='input' type='number' min={0} step={50} value={settings.limits.upKbps} onChange={(e) => update({ limits: { ...settings.limits, upKbps: toInt(e.target.value) } })} />
        </div>
      </div>
      <div className='field'>
        <label htmlFor='max-active'>Téléchargements en même temps</label>
        <input id='max-active' className='input input-narrow' type='number' min={0} max={50} value={settings.queue.maxActive} onChange={(e) => update({ queue: { ...settings.queue, maxActive: toInt(e.target.value, 50) } })} />
        <p className='help'>Les suivants attendent leur tour, par priorité. 0 = pas de limite.</p>
      </div>
      <Check
        checked={settings.queue.ignoreSlow}
        onChange={(ignoreSlow) => update({ queue: { ...settings.queue, ignoreSlow } })}
        label='Ne pas compter les téléchargements lents dans cette limite'
        help='Un torrent quasi à l’arrêt (plus de pair, par exemple) ne bloque plus une place pour les autres.'
      />
      {settings.queue.ignoreSlow && (
        <div className='field'>
          <label htmlFor='slow-threshold'>Vitesse considérée comme lente <span className='optional'>(Ko/s, réception + envoi)</span></label>
          <input
            id='slow-threshold' className='input input-narrow' type='number' min={0} step={5}
            value={settings.queue.slowThresholdKbps}
            onChange={(e) => update({ queue: { ...settings.queue, slowThresholdKbps: toInt(e.target.value) } })}
          />
        </div>
      )}
      <Check
        checked={settings.seedWhenDone}
        onChange={(seedWhenDone) => update({ seedWhenDone })}
        label='Continuer à partager les torrents terminés'
        help='Sinon ils s’arrêtent à la fin ; tu peux relancer le partage depuis la liste.'
      />
      {settings.seedWhenDone && (
        <div className='field-pair'>
          <div className='field'>
            <label htmlFor='seed-ratio'>Ratio max <span className='optional'>(envoyé / reçu)</span></label>
            <input
              id='seed-ratio' className='input' type='number' min={0} step={0.1} value={settings.seeding.ratioLimit}
              onChange={(e) => update({ seeding: { ...settings.seeding, ratioLimit: toFloat(e.target.value) } })}
            />
          </div>
          <div className='field'>
            <label htmlFor='seed-time'>Durée max de partage <span className='optional'>(minutes)</span></label>
            <input
              id='seed-time' className='input' type='number' min={0} step={30} value={settings.seeding.timeLimitMinutes}
              onChange={(e) => update({ seeding: { ...settings.seeding, timeLimitMinutes: toInt(e.target.value) } })}
            />
          </div>
        </div>
      )}
      {settings.seedWhenDone && <p className='help'>Le partage s’arrête dès que l’une des deux limites est atteinte. 0 = illimité — utile sur les trackers privés pour respecter un ratio minimum sans dépasser un partage abusif.</p>}
    </Section>
  )
}

export function ScheduleSection (props: SectionProps) {
  const { settings, update } = props
  const schedule = settings.schedule
  const setSchedule = (patch: Partial<Settings['schedule']>) => update({ schedule: { ...schedule, ...patch } })

  return (
    <Section id='s-schedule' reset={resetOf(props, ['schedule'])} title='Planificateur' intro='Applique d’autres limites de vitesse pendant une plage horaire (ex. la nuit).'>
      <Check checked={schedule.enabled} onChange={(enabled) => setSchedule({ enabled })} label='Activer le planificateur' />
      {schedule.enabled && (
        <>
          <div className='field-pair'>
            <div className='field'>
              <label htmlFor='sched-start'>De</label>
              <input
                id='sched-start' className='input' type='time' value={minutesToTime(schedule.startMinutes)}
                onChange={(e) => setSchedule({ startMinutes: timeToMinutes(e.target.value) })}
              />
            </div>
            <div className='field'>
              <label htmlFor='sched-end'>À</label>
              <input
                id='sched-end' className='input' type='time' value={minutesToTime(schedule.endMinutes)}
                onChange={(e) => setSchedule({ endMinutes: timeToMinutes(e.target.value) })}
              />
            </div>
          </div>
          <div className='field'>
            <span className='label'>Jours</span>
            <div className='mode-list days' role='group' aria-label='Jours du planificateur'>
              {DAY_LABELS.map((label, day) => (
                <label key={day} className='check'>
                  <input
                    type='checkbox' checked={schedule.days.includes(day)}
                    onChange={(e) => setSchedule({ days: e.target.checked ? [...schedule.days, day] : schedule.days.filter((d) => d !== day) })}
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
          </div>
          <div className='field-pair'>
            <div className='field'>
              <label htmlFor='sched-down'>Réception <span className='optional'>(Ko/s, 0 = illimité)</span></label>
              <input
                id='sched-down' className='input' type='number' min={0} step={100} value={schedule.downKbps}
                onChange={(e) => setSchedule({ downKbps: toInt(e.target.value) })}
              />
            </div>
            <div className='field'>
              <label htmlFor='sched-up'>Envoi <span className='optional'>(Ko/s, 0 = illimité)</span></label>
              <input
                id='sched-up' className='input' type='number' min={0} step={50} value={schedule.upKbps}
                onChange={(e) => setSchedule({ upKbps: toInt(e.target.value) })}
              />
            </div>
          </div>
          <p className='help'>Remplace les limites normales pendant cette plage ; le mode jeu reste prioritaire par-dessus.</p>
        </>
      )}
    </Section>
  )
}

/** `processes` : liste des programmes en texte libre (une ligne par programme), éditée à part. */
export function GameSection (props: SectionProps & { processes: string; onProcessesChange: (text: string) => void }) {
  const { settings, update, defaults, processes, onProcessesChange } = props
  const gameMode = settings.gameMode
  const originalProcesses = defaults.gameMode.processes.join('\n')
  const base = resetOf(props, ['gameMode'])
  const reset = {
    onReset: () => { base.onReset(); onProcessesChange(originalProcesses) },
    // la liste en texte fait foi (le brouillon ne la recopie dans gameMode qu'à l'enregistrement)
    disabled: JSON.stringify({ ...settings.gameMode, processes: [] }) === JSON.stringify({ ...defaults.gameMode, processes: [] }) &&
      processes.trim() === originalProcesses
  }
  const setGame = (patch: Partial<Settings['gameMode']>) => update({ gameMode: { ...gameMode, ...patch } })

  return (
    <Section id='s-game' reset={reset} title='Mode jeu' intro='Laisse la connexion aux jeux quand ils sont lancés.'>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='game-mode'>Activation</label>
          <select id='game-mode' value={gameMode.mode} onChange={(e) => setGame({ mode: e.target.value as Settings['gameMode']['mode'] })}>
            <option value='auto'>Automatique (programmes ci-dessous)</option>
            <option value='on'>Toujours activé</option>
            <option value='off'>Désactivé</option>
          </select>
        </div>
        <div className='field'>
          <label htmlFor='game-action'>Pendant le jeu</label>
          <select id='game-action' value={gameMode.action} onChange={(e) => setGame({ action: e.target.value as Settings['gameMode']['action'] })}>
            <option value='limit'>Limiter la vitesse</option>
            <option value='pause'>Mettre les téléchargements en pause</option>
          </select>
        </div>
      </div>
      <div className='field-pair'>
        <div className='field'>
          <label htmlFor='game-down'>Réception en jeu <span className='optional'>(Ko/s)</span></label>
          <input id='game-down' className='input' type='number' min={0} step={100} disabled={gameMode.action === 'pause'} value={gameMode.downKbps} onChange={(e) => setGame({ downKbps: toInt(e.target.value) })} />
        </div>
        <div className='field'>
          <label htmlFor='game-up'>Envoi en jeu <span className='optional'>(Ko/s)</span></label>
          <input id='game-up' className='input' type='number' min={0} step={10} value={gameMode.upKbps} onChange={(e) => setGame({ upKbps: toInt(e.target.value) })} />
        </div>
      </div>
      <div className='field'>
        <label htmlFor='game-processes'>Programmes qui activent le mode jeu <span className='optional'>(un par ligne)</span></label>
        <textarea id='game-processes' className='input mono' rows={3} spellCheck={false} value={processes} onChange={(e) => onProcessesChange(e.target.value)} />
        <p className='help'>Nom du programme tel qu’il apparaît dans le Gestionnaire des tâches, onglet Détails (ex. cs2.exe).</p>
      </div>
    </Section>
  )
}
