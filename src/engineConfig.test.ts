import { describe, expect, it } from 'vitest'
import type { Settings } from './api'
import { restartsEngine } from './engineConfig'

const base = {
  torrentPort: 0,
  utpEnabled: true,
  maxPeersPerTorrent: 0,
  uploadSlots: 8,
  privacy: { encryption: 'prefer', hideAnnounces: false, hardenedMode: false },
  blocklist: { enabled: false, source: '' },
  vpn: { mode: 'off', adapter: '' },
  limits: { downKbps: 0, upKbps: 0 }
} as unknown as Settings

const edit = (patch: Partial<Settings>) => ({ ...base, ...patch }) as Settings

describe('restartsEngine', () => {
  it('pas de redémarrage pour les réglages appliqués à chaud', () => {
    expect(restartsEngine(base, edit({ limits: { downKbps: 500, upKbps: 50 } }))).toBe(false)
  })

  it('pas de redémarrage pour un changement sans effet', () => {
    expect(restartsEngine(base, edit({ blocklist: { enabled: false, source: 'C:/p2p.txt' } }))).toBe(false)
    const hardened = edit({ privacy: { encryption: 'prefer', hideAnnounces: false, hardenedMode: true } })
    const both = edit({ privacy: { encryption: 'prefer', hideAnnounces: true, hardenedMode: true } })
    expect(restartsEngine(hardened, both)).toBe(false)
  })

  it('redémarrage pour le port, le chiffrement ou le mode discret', () => {
    expect(restartsEngine(base, edit({ torrentPort: 51413 }))).toBe(true)
    expect(restartsEngine(base, edit({ privacy: { encryption: 'require', hideAnnounces: false, hardenedMode: false } }))).toBe(true)
    expect(restartsEngine(base, edit({ privacy: { encryption: 'prefer', hideAnnounces: true, hardenedMode: false } }))).toBe(true)
  })
})
