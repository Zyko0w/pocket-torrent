// Réglages dont dépend le moteur torrent, en valeurs effectives : les changer le redémarre
// (téléchargements coupés quelques secondes). Même règle que `EngineConfig` dans engine.rs.
import type { Settings } from './api'

function engineKey (s: Settings) {
  const blocklist = s.blocklist.enabled && s.blocklist.source.trim() ? s.blocklist.source.trim() : null
  return JSON.stringify([
    s.torrentPort,
    s.utpEnabled,
    s.maxPeersPerTorrent,
    s.uploadSlots,
    s.privacy.encryption,
    s.privacy.hideAnnounces || s.privacy.hardenedMode,
    s.privacy.hardenedMode,
    blocklist,
    // changer de VPN relie le moteur à une autre carte réseau
    s.vpn.mode,
    s.vpn.mode === 'adapter' ? s.vpn.adapter : ''
  ])
}

/** Enregistrer `next` à la place de `current` redémarre-t-il le moteur ? */
export const restartsEngine = (current: Settings, next: Settings) => engineKey(current) !== engineKey(next)
