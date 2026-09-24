import { useCallback, useEffect, useState } from 'react'
import {
  ArrowClockwise, FileArrowUp, Globe, Plugs, PlugsConnected, ShieldCheck, SpinnerGap, Trash, WarningCircle, Wrench
} from '@phosphor-icons/react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type NetworkAdapter, type Settings, type TunnelInfo, type VpnMode } from '../api'
import { formatSize } from '../format'
import { useInterval } from '../hooks'
import { useToast } from './Toast'

interface VpnSectionProps {
  vpn: Settings['vpn']
  /** Mode actuellement enregistré (la protection change à l'enregistrement). */
  savedMode: VpnMode
  onChange: (vpn: Settings['vpn']) => void
}

const MODES: { mode: VpnMode; label: string; help: string }[] = [
  { mode: 'off', label: 'Sans VPN', help: 'Les téléchargements utilisent ta connexion normale.' },
  {
    mode: 'adapter',
    label: 'Mon appli VPN',
    help: 'NordVPN, Proton VPN, Mullvad… Les téléchargements passent uniquement par sa carte réseau. Le reste du PC fait comme d’habitude.'
  },
  {
    mode: 'wireguard',
    label: 'Tunnel WireGuard intégré',
    help: 'Avec le fichier .conf de ton VPN : seuls les téléchargements passent par le tunnel, pas les jeux ni le navigateur.'
  }
]

function handshakeText (secs: number | null) {
  if (secs === null) return 'aucun échange avec le serveur pour l’instant'
  if (secs < 180) return `dernier échange avec le serveur il y a ${secs} s`
  return `aucun échange depuis ${Math.round(secs / 60)} min : serveur injoignable ?`
}

export function VpnSection ({ vpn, savedMode, onChange }: VpnSectionProps) {
  const toast = useToast()
  const [adapters, setAdapters] = useState<NetworkAdapter[] | null>(null)
  const [tunnel, setTunnel] = useState<TunnelInfo | null>(null)
  const [busy, setBusy] = useState('')
  const [ipCheck, setIpCheck] = useState<{ vpn: string | null; normal: string | null; error: string | null } | null>(null)

  const loadAdapters = useCallback(() => {
    api.networkAdapters().then(setAdapters).catch((err) => toast(errorMessage(err)))
  }, [toast])

  const loadTunnel = useCallback(async () => {
    try {
      setTunnel(await api.tunnelView())
    } catch (err) {
      toast(errorMessage(err))
    }
  }, [toast])

  useEffect(() => { if (vpn.mode === 'adapter') loadAdapters() }, [vpn.mode, loadAdapters])
  useEffect(() => { if (vpn.mode === 'wireguard') loadTunnel() }, [vpn.mode, loadTunnel])
  useInterval(() => { if (vpn.mode === 'wireguard' && !busy) loadTunnel() }, 3000)

  const run = async (label: string, action: () => Promise<unknown>, done?: string) => {
    setBusy(label)
    try {
      await action()
      if (done) toast(done)
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy('')
      loadTunnel()
    }
  }

  const importConfig = async () => {
    const path = await open({ filters: [{ name: 'Configuration WireGuard', extensions: ['conf'] }] })
    if (typeof path !== 'string') return
    await run('import', () => api.tunnelImport(path), 'Configuration importée (rangée dans Windows)')
  }

  const checkIp = async () => {
    setBusy('ip')
    setIpCheck(null)
    try {
      setIpCheck(await api.checkPublicIp())
    } catch (err) {
      toast(errorMessage(err))
    } finally {
      setBusy('')
    }
  }

  const selected = adapters?.find((a) => a.name === vpn.adapter)
  const service = tunnel?.service
  const status = tunnel?.status

  return (
    <>
      <div className='mode-list' role='radiogroup' aria-label='Mode VPN'>
        {MODES.map(({ mode, label, help }) => (
          <label key={mode} className={`mode-option${vpn.mode === mode ? ' selected' : ''}`}>
            <input type='radio' name='vpn-mode' checked={vpn.mode === mode} onChange={() => onChange({ ...vpn, mode })} />
            <span>
              {label}
              <small>{help}</small>
            </span>
          </label>
        ))}
      </div>

      {vpn.mode !== 'off' && (
        <p className='help' style={{ marginTop: 0 }}>
          <ShieldCheck size={14} style={{ verticalAlign: '-2px' }} /> Arrêt automatique : si le VPN se coupe, tous les téléchargements
          se mettent en pause et rien ne sort sans lui ; ils reprennent dès son retour.
          {vpn.mode !== savedMode && ' Enregistre pour activer.'}
        </p>
      )}

      {vpn.mode === 'adapter' && (
        <div className='field'>
          <label htmlFor='vpn-adapter'>Carte réseau du VPN</label>
          <div className='input-row'>
            <select id='vpn-adapter' value={vpn.adapter} onChange={(e) => onChange({ ...vpn, adapter: e.target.value })}>
              <option value=''>Choisir…</option>
              {vpn.adapter && !adapters?.some((a) => a.name === vpn.adapter) && (
                <option value={vpn.adapter}>{vpn.adapter} (absente pour l’instant)</option>
              )}
              {adapters?.map((a) => (
                <option key={a.name} value={a.name}>
                  {a.name}{a.likelyVpn ? ' · VPN probable' : ''}{a.up ? '' : ' · déconnectée'}
                </option>
              ))}
            </select>
            <button type='button' className='btn btn-ghost' title='Actualiser la liste' onClick={loadAdapters}>
              <ArrowClockwise size={16} />
            </button>
          </div>
          <p className='help'>
            {selected
              ? `${selected.description} · ${selected.up ? `connectée${selected.ipv4 ? ` (${selected.ipv4})` : ''}` : 'déconnectée'}`
              : 'Connecte ton appli VPN, puis choisis sa carte (souvent marquée « VPN probable »).'}
          </p>
        </div>
      )}

      {vpn.mode === 'wireguard' && (
        <div className='tunnel-steps'>
          <div className='tunnel-step'>
            <span className='step-number'>1</span>
            <div className='step-body'>
              <strong>Service du tunnel</strong>
              <p className='help'>
                {!tunnel && 'Vérification…'}
                {service === 'running' && 'Installé et prêt.'}
                {service === 'notInstalled' && 'À installer une fois : Windows demandera l’autorisation administrateur.'}
                {service === 'outdated' && 'Une mise à jour est nécessaire.'}
                {service === 'stopped' && 'Installé mais arrêté.'}
              </p>
              <div className='panel-actions'>
                {service && service !== 'running' && (
                  <button type='button' className='btn btn-primary btn-sm' disabled={Boolean(busy)} onClick={() => run('install', api.tunnelInstallService, 'Service du tunnel installé')}>
                    {busy === 'install' ? <SpinnerGap size={14} className='spin' /> : <Wrench size={14} />}
                    {service === 'notInstalled' ? 'Installer le service' : 'Réinstaller le service'}
                  </button>
                )}
                {service && service !== 'notInstalled' && (
                  <button type='button' className='btn btn-ghost btn-sm' disabled={Boolean(busy)} onClick={() => run('uninstall', api.tunnelUninstallService, 'Service du tunnel retiré')}>
                    <Trash size={14} /> Désinstaller
                  </button>
                )}
              </div>
            </div>
          </div>

          <div className='tunnel-step'>
            <span className='step-number'>2</span>
            <div className='step-body'>
              <strong>Configuration du VPN</strong>
              <p className='help'>
                {tunnel?.config
                  ? `Serveur ${tunnel.config.endpoint ?? '?'} · adresse ${tunnel.config.addresses.join(', ')}`
                  : 'Télécharge le fichier WireGuard (.conf) sur le site de ton VPN, puis importe-le. Sa clé privée est rangée dans le Gestionnaire d’identification de Windows.'}
              </p>
              <div className='panel-actions'>
                <button type='button' className='btn btn-ghost btn-sm' disabled={Boolean(busy)} onClick={importConfig}>
                  <FileArrowUp size={14} /> {tunnel?.config ? 'Remplacer le fichier .conf' : 'Importer un fichier .conf'}
                </button>
                {tunnel?.config && (
                  <button type='button' className='btn btn-ghost btn-sm' disabled={Boolean(busy)} onClick={() => run('forget', api.tunnelForget, 'Configuration retirée')}>
                    <Trash size={14} /> Retirer
                  </button>
                )}
              </div>
            </div>
          </div>

          <div className='tunnel-step'>
            <span className='step-number'>3</span>
            <div className='step-body'>
              <strong>Connexion</strong>
              <p className={`help ${status?.up ? (status.handshakeAgeSecs !== null && status.handshakeAgeSecs < 180 ? 'tone-success' : 'tone-warning') : ''}`}>
                {status?.up
                  ? `Tunnel monté · ${handshakeText(status.handshakeAgeSecs)} · reçu ${formatSize(status.rxBytes)}, envoyé ${formatSize(status.txBytes)}`
                  : 'Tunnel arrêté.'}
              </p>
              {tunnel?.lastError && !status?.up && (
                <p className='field-error'><WarningCircle size={14} /> {tunnel.lastError}</p>
              )}
              <div className='panel-actions'>
                {status?.up
                  ? (
                    <button type='button' className='btn btn-ghost btn-sm' disabled={Boolean(busy)} onClick={() => run('down', api.tunnelDisconnect, 'Tunnel déconnecté')}>
                      <Plugs size={14} /> Déconnecter
                    </button>
                    )
                  : (
                    <button
                      type='button' className='btn btn-primary btn-sm'
                      disabled={Boolean(busy) || service !== 'running' || !tunnel?.config}
                      onClick={() => run('up', api.tunnelConnect, 'Tunnel connecté')}
                    >
                      {busy === 'up' ? <SpinnerGap size={14} className='spin' /> : <PlugsConnected size={14} />} Connecter
                    </button>
                    )}
              </div>
            </div>
          </div>
        </div>
      )}

      {vpn.mode !== 'off' && (
        <div>
          <button type='button' className='btn btn-ghost btn-sm' disabled={busy === 'ip'} onClick={checkIp}>
            {busy === 'ip' ? <SpinnerGap size={14} className='spin' /> : <Globe size={14} />} Vérifier mon adresse IP
          </button>
          {ipCheck && (
            <p className='help'>
              Sans VPN : <b className='mono'>{ipCheck.normal ?? '?'}</b>
              {' · '}Téléchargements : <b className='mono'>{ipCheck.vpn ?? 'bloqués (VPN coupé)'}</b>
              {ipCheck.vpn && ipCheck.normal && ipCheck.vpn === ipCheck.normal && (
                <span className='tone-danger'> · même adresse : le VPN ne masque pas ton IP !</span>
              )}
              {ipCheck.error && <span className='tone-danger'> · {ipCheck.error}</span>}
            </p>
          )}
        </div>
      )}
    </>
  )
}
