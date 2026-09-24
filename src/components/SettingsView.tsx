// Page des réglages : état en cours d'édition, enregistrement, et rangement des sections par
// thème (chaque section vit dans `settings/`). Un thème à la fois, choisi dans la colonne de gauche.
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import {
  ChartBar, Cloud, FolderSimple, Gauge, ShieldCheck, SpinnerGap, Stethoscope, WarningCircle,
  WifiHigh
} from '@phosphor-icons/react'
import { api, errorMessage, type SecretsInput, type Settings, type SettingsView as View } from '../api'
import { restartsEngine } from '../engineConfig'
import { useToast } from './Toast'
import { VpnSection } from './VpnSection'
import { TrafficStatsPanel } from './TrafficStatsPanel'
import { TransferPanel } from './TransferPanel'
import { Section } from './settings/fields'
import { FoldersSection } from './settings/FoldersSection'
import { GameSection, ScheduleSection, SpeedSection } from './settings/SpeedSections'
import { BlocklistSection, DangerousFilesSection, PrivacySection } from './settings/PrivacySections'
import { NetworkSection, NotificationsSection, WindowsSection } from './settings/SystemSections'
import { AboutPanel } from './AboutPanel'
import { LogSection, PerformanceSection } from './settings/DiagnosticsSections'
import { StorageToSection } from './settings/StorageToSection'

export type SettingsTab = 'general' | 'transfers' | 'network' | 'privacy' | 'cloud' | 'data' | 'diagnostics'

export const SETTINGS_TABS: { id: SettingsTab; label: string; icon: ReactNode; advanced?: boolean }[] = [
  { id: 'general', label: 'Général', icon: <FolderSimple size={16} /> },
  { id: 'transfers', label: 'Vitesse et file', icon: <Gauge size={16} /> },
  { id: 'network', label: 'VPN et réseau', icon: <WifiHigh size={16} /> },
  { id: 'privacy', label: 'Confidentialité', icon: <ShieldCheck size={16} /> },
  { id: 'cloud', label: 'Envoi en ligne', icon: <Cloud size={16} /> },
  { id: 'data', label: 'Données', icon: <ChartBar size={16} /> },
  { id: 'diagnostics', label: 'Diagnostic', icon: <Stethoscope size={16} />, advanced: true }
]

const NO_SECRET_CHANGE: SecretsInput = { storageToToken: null }

/** Liste en texte libre (une valeur par ligne ou séparées par des virgules). */
const toList = (text: string) => text.split(/[\n,]/).map((p) => p.trim()).filter(Boolean)

interface Draft {
  settings: Settings
  secrets: SecretsInput
  startWithWindows: boolean
  magnetHandler: boolean
  // listes éditées en texte libre, converties à l'enregistrement
  processes: string
  excludedExtensions: string
}

const draftOf = (view: View): Draft => ({
  settings: view.settings,
  secrets: NO_SECRET_CHANGE,
  startWithWindows: view.startWithWindows,
  magnetHandler: view.magnetHandler,
  processes: view.settings.gameMode.processes.join('\n'),
  excludedExtensions: view.settings.security.excludedExtensions.join('\n')
})

interface SettingsViewProps {
  tab: SettingsTab
  onTabChange: (tab: SettingsTab) => void
  onSaved: () => void
}

/** Mode avancé : préférence d'affichage propre à ce PC (rien ne change dans les réglages). */
const ADVANCED_KEY = 'pocket-torrent.reglages-avances'

function loadAdvanced (): boolean {
  try { return localStorage.getItem(ADVANCED_KEY) === '1' } catch { return false }
}

/** Sections masquées hors du mode avancé : leurs réglages restent appliqués. */
function HiddenAdvanced ({ names, onShow }: { names: string[]; onShow: () => void }) {
  return (
    <p className='hidden-advanced'>
      Réglages avancés masqués : {names.join(', ')}.{' '}
      <button type='button' className='link-btn' onClick={onShow}>Afficher</button>
    </p>
  )
}

export function SettingsView ({ tab, onTabChange, onSaved }: SettingsViewProps) {
  const toast = useToast()
  const top = useRef<HTMLFormElement>(null)
  const [view, setView] = useState<View | null>(null)
  const [draft, setDraft] = useState<Draft | null>(null)
  const [error, setError] = useState('')
  const [status, setStatus] = useState<{ text: string; kind: '' | 'ok' | 'error' }>({ text: '', kind: '' })
  const [saving, setSaving] = useState(false)
  const [advanced, setAdvancedState] = useState(loadAdvanced)
  const setAdvanced = (value: boolean) => {
    setAdvancedState(value)
    try { localStorage.setItem(ADVANCED_KEY, value ? '1' : '0') } catch { /* stockage indisponible : réglage de la session seulement */ }
  }

  const load = (next: View) => {
    setView(next)
    setDraft(draftOf(next))
  }

  useEffect(() => {
    api.getSettings().then(load).catch((err) => setError(errorMessage(err)))
  }, [])

  if (!draft || !view) {
    return error ? <p className='field-error'><WarningCircle size={16} /> {error}</p> : null
  }

  const { settings } = draft
  const edit = (patch: Partial<Draft>) => {
    setDraft({ ...draft, ...patch })
    if (status.kind === 'ok') setStatus({ text: '', kind: '' })
  }
  const update = (patch: Partial<Settings>) => edit({ settings: { ...settings, ...patch } })
  const section = { settings, update, defaults: view.defaults }
  const dirty = JSON.stringify(draft) !== JSON.stringify(draftOf(view))
  const restarting = dirty && restartsEngine(view.settings, settings)

  const showError = (message: string) => {
    setError(message)
    setStatus({ text: 'Non enregistré', kind: 'error' })
    top.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  const save = async (e: FormEvent) => {
    e.preventDefault()
    setError('')
    setSaving(true)
    setStatus({ text: 'Enregistrement…', kind: '' })
    try {
      const next = {
        ...settings,
        gameMode: { ...settings.gameMode, processes: toList(draft.processes) },
        security: { excludedExtensions: toList(draft.excludedExtensions) }
      }
      load(await api.saveSettings({
        settings: next,
        secrets: draft.secrets,
        startWithWindows: draft.startWithWindows,
        magnetHandler: draft.magnetHandler
      }))
      setStatus({ text: 'Enregistré', kind: 'ok' })
      toast('Réglages enregistrés')
      onSaved()
    } catch (err) {
      showError(errorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  const discard = () => {
    setDraft(draftOf(view))
    setError('')
    setStatus({ text: '', kind: '' })
  }

  const panels: Record<SettingsTab, ReactNode> = {
    general: (
      <>
        <FoldersSection {...section} />
        <WindowsSection
          {...section}
          startWithWindows={draft.startWithWindows}
          onStartWithWindowsChange={(startWithWindows) => edit({ startWithWindows })}
          magnetHandler={draft.magnetHandler}
          onMagnetHandlerChange={(magnetHandler) => edit({ magnetHandler })}
          defenderAvailable={view.defenderAvailable}
        />
        <NotificationsSection {...section} />
      </>
    ),
    transfers: (
      <>
        <SpeedSection {...section} />
        <ScheduleSection {...section} />
        <GameSection {...section} processes={draft.processes} onProcessesChange={(processes) => edit({ processes })} />
      </>
    ),
    network: (
      <>
        <Section id='s-vpn' title='VPN' intro='Cacher ton adresse IP pendant les téléchargements, avec arrêt automatique si le VPN se coupe.'>
          <VpnSection vpn={settings.vpn} savedMode={view.settings.vpn.mode} onChange={(vpn) => update({ vpn })} />
        </Section>
        {advanced
          ? <NetworkSection {...section} listenPort={view.listenPort} externalPort={view.externalPort} />
          : <HiddenAdvanced names={['Réseau et disque']} onShow={() => setAdvanced(true)} />}
      </>
    ),
    privacy: (
      <>
        <PrivacySection {...section} />
        {advanced && <BlocklistSection {...section} />}
        {advanced && <DangerousFilesSection extensions={draft.excludedExtensions} defaults={view.defaults.security.excludedExtensions} onChange={(excludedExtensions) => edit({ excludedExtensions })} />}
        {!advanced && <HiddenAdvanced names={['Liste de blocage d’IP', 'Fichiers dangereux']} onShow={() => setAdvanced(true)} />}
      </>
    ),
    cloud: <StorageToSection {...section} saved={view.secrets} secrets={draft.secrets} onSecretsChange={(secrets) => edit({ secrets })} />,
    data: (
      <>
        <Section id='s-stats' title='Statistiques' intro='Tout ce qui a été reçu et envoyé (torrents et liens directs), et le ratio global.'>
          <TrafficStatsPanel />
        </Section>
        <Section id='s-transfer' title='Changer de PC' intro='Exporter la liste des téléchargements pour la reprendre sur un autre PC.'>
          <TransferPanel />
        </Section>
      </>
    ),
    diagnostics: (
      <>
        <PerformanceSection />
        <LogSection />
      </>
    )
  }

  const tabs = SETTINGS_TABS.filter((t) => advanced || !t.advanced)
  const current = tabs.find((t) => t.id === tab) ?? tabs[0]

  return (
    <form onSubmit={save} noValidate ref={top}>
      <header className='page-head'>
        <div>
          <div className='crumbs eyebrow'><span>/application</span><span>›</span><span>/réglages</span></div>
          <h1 className='page-title'>Réglages</h1>
          <p className='page-sub'>Les changements s’appliquent dès l’enregistrement.</p>
        </div>
        <label className='toggle-row advanced-toggle' title='Port, pairs, places d’envoi, liste de blocage, fichiers dangereux, diagnostic'>
          <span className='toggle-text'>Réglages avancés</span>
          <input type='checkbox' role='switch' className='switch' checked={advanced} onChange={(e) => setAdvanced(e.target.checked)} />
        </label>
      </header>
      {error && <div className='banner danger' role='alert'><WarningCircle size={16} /> {error}</div>}

      <div className='settings-layout'>
        <div className='settings-side'>
          <nav className='settings-nav' aria-label='Thèmes des réglages'>
            {tabs.map((t) => (
              <button key={t.id} type='button' aria-current={t.id === current.id} onClick={() => onTabChange(t.id)}>
                {t.icon} {t.label}
              </button>
            ))}
          </nav>
          <AboutPanel />
        </div>
        <div className='settings-panel' aria-label={current.label}>
          {panels[current.id]}
        </div>
      </div>

      <div className={`save-bar${dirty ? ' dirty' : ''}`}>
        <span className={`save-status ${dirty && !status.kind ? 'dirty' : status.kind}`} role='status'>
          {status.text || (dirty ? 'Modifications non enregistrées' : 'Tout est enregistré')}
          {restarting && !status.kind && (
            <small className='save-note' title='Port, µTP, pairs, places d’envoi, chiffrement, mode discret, liste de blocage ou VPN'>
              · redémarre le moteur : téléchargements coupés quelques secondes
            </small>
          )}
        </span>
        {dirty && (
          <button type='button' className='btn btn-ghost' onClick={discard} disabled={saving}>Annuler</button>
        )}
        <button type='submit' className='btn btn-primary' disabled={saving || !dirty}>
          {saving && <SpinnerGap size={16} className='spin' />} Enregistrer
        </button>
      </div>
    </form>
  )
}
