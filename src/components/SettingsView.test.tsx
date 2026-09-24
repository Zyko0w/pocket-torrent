import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import type { Settings, SettingsView as View } from '../api'
import defaults from '../test/defaultSettings.json'
import { SettingsView } from './SettingsView'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.1.2') }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }))
vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn(async () => null) }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }))

const mocked = vi.mocked(invoke)

/** Réglages enregistrés : limites et port changés par rapport aux valeurs d'origine. */
function viewWith (settings: Partial<Settings>): View {
  return {
    settings: { ...(defaults as unknown as Settings), ...settings },
    secrets: { r2AccessKeyId: false, r2SecretAccessKey: false },
    startWithWindows: false,
    magnetHandler: false,
    listenPort: 51413,
    externalPort: null,
    defenderAvailable: true,
    defaults: defaults as unknown as Settings
  }
}

function renderSettings (tab: 'transfers' | 'network' | 'privacy' | 'diagnostics', view: View) {
  mocked.mockImplementation(async (cmd: string) => (cmd === 'get_settings' ? view : null))
  render(<ToastProvider><SettingsView tab={tab} onTabChange={vi.fn()} onSaved={vi.fn()} /></ToastProvider>)
}

describe('SettingsView', () => {
  beforeEach(() => {
    mocked.mockReset()
    localStorage.clear()
  })

  it('« Valeurs par défaut » remet la section d’origine dans le brouillon, à enregistrer ensuite', async () => {
    renderSettings('transfers', viewWith({ limits: { downKbps: 900, upKbps: 1 } }))
    await screen.findAllByRole('button', { name: /Valeurs par défaut/ })
    const down = document.getElementById('limit-down')!
    expect((down as HTMLInputElement).value).toBe('900')

    const [speedReset] = screen.getAllByRole('button', { name: /Valeurs par défaut/ })
    fireEvent.click(speedReset)
    expect((down as HTMLInputElement).value).toBe(String(defaults.limits.downKbps))
    expect(screen.getByText('Modifications non enregistrées')).toBeTruthy()
    // rien n'est envoyé au moteur avant « Enregistrer »
    expect(mocked).not.toHaveBeenCalledWith('save_settings', expect.anything())
  })

  it('le bouton est désactivé quand la section est déjà d’origine', async () => {
    renderSettings('transfers', viewWith({}))
    const buttons = await screen.findAllByRole('button', { name: /Valeurs par défaut/ })
    expect(buttons.every((b) => (b as HTMLButtonElement).disabled)).toBe(true)
  })

  it('mode simple : réglages d’expert masqués, mais signalés', async () => {
    renderSettings('network', viewWith({}))
    expect(await screen.findByText(/Réglages avancés masqués : Réseau et disque/)).toBeTruthy()
    expect(screen.queryByLabelText(/Port des torrents/)).toBeNull()
    expect(screen.queryByRole('button', { name: /Diagnostic/ })).toBeNull()
  })

  it('mode avancé : tout est affiché, et le choix est retenu', async () => {
    renderSettings('network', viewWith({}))
    fireEvent.click(await screen.findByRole('switch', { name: 'Réglages avancés' }))
    expect(screen.queryByText(/Réglages avancés masqués/)).toBeNull()
    expect(screen.getByRole('button', { name: /Diagnostic/ })).toBeTruthy()
    expect(localStorage.getItem('pocket-torrent.reglages-avances')).toBe('1')
  })
})
