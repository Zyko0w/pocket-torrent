import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { Tracker } from '../api'
import { ToastProvider } from './Toast'
import { TrackersPanel } from './TrackersPanel'

const trackers: Tracker[] = [
  { url: 'udp://a.org:1337/announce', state: 'working', peers: 12, error: null, lastSuccessAt: Date.now(), nextAnnounceAt: Date.now() + 1_800_000 },
  { url: 'udp://b.org:6969/announce', state: 'failing', peers: 0, error: 'délai dépassé', lastSuccessAt: null, nextAnnounceAt: Date.now() + 60_000 },
  { url: 'http://c.org/announce', state: 'skipped', peers: 0, error: 'ignoré : mode discret (trackers HTTPS seulement)', lastSuccessAt: null, nextAnnounceAt: null }
]

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => trackers) }))

describe('TrackersPanel', () => {
  it('affiche l’état de chaque tracker', async () => {
    render(<ToastProvider><TrackersPanel jobId='t:abc' /></ToastProvider>)
    expect(await screen.findByText('3 trackers · 1 fonctionne')).toBeTruthy()
    expect(screen.getByText(/12 pairs à la dernière annonce/)).toBeTruthy()
    expect(screen.getByText(/prochaine annonce dans 30 min/)).toBeTruthy()
    expect(screen.getByText(/délai dépassé/)).toBeTruthy()
    expect(screen.getByText(/mode discret/)).toBeTruthy()
  })
})
