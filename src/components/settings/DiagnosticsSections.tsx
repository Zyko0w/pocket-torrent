// Diagnostics : instantané de performance et journal. Indépendants des réglages en cours d'édition.
import { useState } from 'react'
import { CheckCircle, Export, FolderOpen, SpinnerGap } from '@phosphor-icons/react'
import { save } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type PerfSnapshot } from '../../api'
import { formatSpeed } from '../../format'
import { useToast } from '../Toast'
import { Section } from './fields'

export function PerformanceSection () {
  const toast = useToast()
  const [perf, setPerf] = useState<PerfSnapshot | null>(null)
  const [busy, setBusy] = useState(false)

  const measure = async () => {
    setBusy(true)
    try {
      setPerf(await api.performanceSnapshot())
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  const exportJson = async () => {
    const path = await save({ defaultPath: `pocket-torrent-perf-${Date.now()}.json`, filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (!path) return
    setBusy(true)
    try {
      await api.exportPerformanceSnapshot(path)
      toast('Instantané exporté')
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Section id='s-perf' title='Diagnostics' intro='Instantané de performance à comparer avant/après un changement (mise à jour, réglage réseau…).'>
      <div className='panel-actions'>
        <button type='button' className='btn btn-ghost btn-sm' disabled={busy} onClick={measure}>
          {busy ? <SpinnerGap size={14} className='spin' /> : <CheckCircle size={14} />} Mesurer maintenant
        </button>
        <button type='button' className='btn btn-ghost btn-sm' disabled={busy} onClick={exportJson}>
          <Export size={14} /> Exporter en JSON
        </button>
      </div>
      {perf && (
        <p className='help' style={{ marginTop: 8 }}>
          CPU {perf.cpuPercent.toFixed(1)} % · Mémoire {perf.memoryMb} Mo · Démarré depuis {Math.round(perf.uptimeSecs / 60)} min
          · {perf.torrents} torrent{perf.torrents > 1 ? 's' : ''} · {perf.peersLive} pair{perf.peersLive > 1 ? 's' : ''}
          · ↓ {formatSpeed(perf.downSpeed)} · ↑ {formatSpeed(perf.upSpeed)}
          {perf.listenPort !== null && ` · port ${perf.listenPort}`}
          {perf.diskFreeMb !== null && ` · ${(perf.diskFreeMb / 1024).toFixed(1)} Go libres`}
        </p>
      )}
    </Section>
  )
}

export function LogSection () {
  const toast = useToast()
  const [log, setLog] = useState('')
  const [busy, setBusy] = useState(false)

  const refresh = async () => {
    setBusy(true)
    try {
      setLog(await api.readLog(300))
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  const exportLog = async () => {
    const path = await save({ defaultPath: `pocket-torrent-${Date.now()}.log`, filters: [{ name: 'Journal', extensions: ['log'] }] })
    if (!path) return
    try {
      await api.exportLog(path)
      toast('Journal exporté')
    } catch (err) {
      toast(errorMessage(err))
    }
  }

  return (
    <Section id='s-log' title='Journal' intro='Ce qui se passe en coulisses : utile pour comprendre un problème ou le signaler.'>
      <div className='panel-actions'>
        <button type='button' className='btn btn-ghost btn-sm' disabled={busy} onClick={refresh}>
          {busy ? <SpinnerGap size={14} className='spin' /> : <CheckCircle size={14} />} Actualiser
        </button>
        <button type='button' className='btn btn-ghost btn-sm' onClick={exportLog}>
          <Export size={14} /> Exporter
        </button>
        <button type='button' className='btn btn-ghost btn-sm' onClick={() => api.openLogFolder().catch((err) => toast(errorMessage(err)))}>
          <FolderOpen size={14} /> Ouvrir le dossier
        </button>
      </div>
      {log && (
        <pre className='input mono selectable' style={{ marginTop: 8, maxHeight: 240, overflow: 'auto', whiteSpace: 'pre-wrap' }}>
          {log}
        </pre>
      )}
      {!log && <p className='help' style={{ marginTop: 0 }}>Clique sur « Actualiser » pour voir les dernières lignes.</p>}
    </Section>
  )
}
