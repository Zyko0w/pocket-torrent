import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import type { Settings } from '../../api'
import defaults from '../../test/defaultSettings.json'
import { ToastProvider } from '../Toast'
import { RssSection } from './RssSection'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mocked = vi.mocked(invoke)
const base = defaults as unknown as Settings
const feed = { id: 'abc123', name: 'Ma série', url: 'https://site.example/rss', include: 'série 1080p', exclude: '', category: null, enabled: true, skipRepeats: true }

function renderRss (settings: Settings, answers: Record<string, unknown> = {}) {
  mocked.mockImplementation(async (cmd: string) => answers[cmd] ?? [])
  const update = vi.fn()
  render(<ToastProvider><RssSection settings={settings} update={update} defaults={base} /></ToastProvider>)
  return update
}

describe('RssSection', () => {
  beforeEach(() => mocked.mockReset())

  it('« Ajouter un flux » ajoute une fiche vide (à enregistrer)', () => {
    const update = renderRss(base)
    fireEvent.click(screen.getByRole('button', { name: /Ajouter un flux/ }))
    expect(update).toHaveBeenCalledWith({ rss: expect.objectContaining({ feeds: [expect.objectContaining({ id: '', enabled: true, skipRepeats: true })] }) })
  })

  it('« Tester les filtres » montre ce qui serait téléchargé, sans rien ajouter', async () => {
    renderRss({ ...base, rss: { intervalMinutes: 15, feeds: [feed] } }, {
      rss_preview: [{ title: 'Ma Série S01E02 1080p', matched: true }, { title: 'Ma Série S01E02 720p', matched: false }]
    })
    fireEvent.click(screen.getByRole('button', { name: /Tester les filtres/ }))
    expect(await screen.findByText(/2 éléments dans le flux, 1 passe les filtres/)).toBeTruthy()
    expect(mocked).toHaveBeenCalledWith('rss_preview', { url: feed.url, include: feed.include, exclude: '' })
    expect(mocked).not.toHaveBeenCalledWith('add_links', expect.anything())
  })

  it('affiche l’erreur de la dernière vérification', async () => {
    renderRss({ ...base, rss: { intervalMinutes: 15, feeds: [feed] } }, {
      rss_status: [{ id: 'abc123', checkedAt: 1, error: 'flux injoignable', added: 0, lastAdded: [], ready: true }]
    })
    expect(await screen.findByText('flux injoignable')).toBeTruthy()
  })
})
