import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import type { Settings } from '../api'
import defaults from '../test/defaultSettings.json'
import { makeJob } from '../test/job'
import { Composer } from './Composer'
import { DeleteDialog } from './DeleteDialog'
import { StorageToSection } from './settings/StorageToSection'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
const mocked = vi.mocked(invoke)

describe('storage.to', () => {
  beforeEach(() => mocked.mockReset())

  it('la zone d’ajout propose « Envoyer sur storage.to » et transmet ce choix', async () => {
    mocked.mockResolvedValue({ added: 1, failed: [], errors: [] })
    render(
      <ToastProvider>
        <Composer prefill='magnet:?xt=urn:btih:aa' onPrefillUsed={vi.fn()} categories={[]} defaultCategory='' onChooseFiles={vi.fn()} onAdded={vi.fn()} />
      </ToastProvider>
    )
    fireEvent.click(screen.getByRole('combobox', { name: 'Destination' }))
    fireEvent.click(screen.getByRole('option', { name: 'Envoyer sur storage.to' }))
    fireEvent.click(screen.getByRole('button', { name: /Ajouter/ }))
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledWith('add_links', expect.objectContaining({
      options: expect.objectContaining({ dest: 'storageTo' })
    })))
  })

  it('supprimer propose d’effacer aussi les fichiers en ligne', () => {
    const onConfirm = vi.fn()
    const delivery = {
      state: 'done' as const, sentBytes: 10, totalBytes: 10, expiresAt: Date.now() + 86_400_000,
      links: [{ name: 'Film.mkv', size: 10, key: 'FQxyz1234', url: 'https://storage.to/FQxyz1234', kind: 'file' as const }]
    }
    render(<DeleteDialog job={makeJob({ status: 'done', delivery })} onCancel={vi.fn()} onConfirm={onConfirm} />)
    fireEvent.click(screen.getByRole('checkbox', { name: /sur storage\.to/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Supprimer' }))
    expect(onConfirm).toHaveBeenCalledWith(false, true, false)
  })

  it('le test de connexion affiche le quota renvoyé', async () => {
    mocked.mockResolvedValue('storage.to joignable : 99.9 Go d’envoi restants sur 24 h (sans compte).')
    const settings = defaults as unknown as Settings
    render(
      <ToastProvider>
        <StorageToSection settings={settings} update={vi.fn()} defaults={settings} saved={{ storageToToken: false, proxyPassword: false }} secrets={{ storageToToken: null, proxyPassword: null }} onSecretsChange={vi.fn()} />
      </ToastProvider>
    )
    fireEvent.click(screen.getByRole('button', { name: /Tester la connexion/ }))
    await vi.waitFor(() => expect(screen.getAllByText(/99\.9 Go/).length).toBeGreaterThan(0))
    expect(mocked).toHaveBeenCalledWith('test_storage_to')
  })
})
