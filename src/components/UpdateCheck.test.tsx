import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { check } from '@tauri-apps/plugin-updater'
import defaults from '../test/defaultSettings.json'
import type { Settings } from '../api'
import { WindowsSection } from './settings/SystemSections'
import { ToastProvider } from './Toast'
import { UpdateBanner } from './UpdateBanner'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => ({ settings: { checkUpdates: false, privacy: {} } })) }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.1.2') }))
vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn() }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }))
const mockedCheck = vi.mocked(check)

function renderPage () {
  const settings = defaults as unknown as Settings
  render(
    <ToastProvider>
      <UpdateBanner />
      <WindowsSection
        settings={settings} update={vi.fn()} defaults={settings}
        startWithWindows={false} onStartWithWindowsChange={vi.fn()} magnetHandler={false} onMagnetHandlerChange={vi.fn()}
        defenderAvailable
      />
    </ToastProvider>
  )
}

describe('Rechercher une mise à jour', () => {
  beforeEach(() => {
    mockedCheck.mockReset()
    Element.prototype.scrollIntoView = vi.fn()
  })

  it('annonce la version trouvée et amène le bandeau d’installation à l’écran', async () => {
    mockedCheck.mockResolvedValue({ version: '0.2.0' } as never)
    renderPage()
    fireEvent.click(await screen.findByRole('button', { name: /Rechercher une mise à jour/ }))
    expect(await screen.findByText(/0\.2\.0 disponible/)).toBeTruthy()
    expect(screen.getByText(/Pocket Torrent/, { selector: '.banner-text' })).toBeTruthy()
    await vi.waitFor(() => expect(Element.prototype.scrollIntoView).toHaveBeenCalled())
  })

  it('dit quand l’application est à jour', async () => {
    mockedCheck.mockResolvedValue(null)
    renderPage()
    fireEvent.click(await screen.findByRole('button', { name: /Rechercher une mise à jour/ }))
    await vi.waitFor(() => expect(screen.getAllByText('Pocket Torrent est à jour').length).toBeGreaterThan(0))
  })
})
