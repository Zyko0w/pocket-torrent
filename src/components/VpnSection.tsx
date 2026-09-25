import { useCallback, useEffect, useState } from 'react'
import {
  ArrowClockwise, FileArrowUp, Globe, Plugs, PlugsConnected, ShieldCheck, SpinnerGap, Trash, WarningCircle, Wrench
} from '@phosphor-icons/react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type NetworkAdapter, type Settings, type TunnelInfo, type VpnMode } from '../api'
import { formatSize } from '../format'
import { useInterval } from '../hooks'
import { useToast } from './Toast'
import { Select } from './Select'
import { Check, SecretField } from './settings/fields'

interface VpnSectionProps {
  vpn: Settings['vpn']
  /** Mode actuellement enregistré (la protection change à l'enregistrement). */
  savedMode: VpnMode
  onChange: (vpn: Settings['vpn']) => void
  /** Mot de passe du proxy : enregistré ou non, brouillon (`null` : inchangé). */
  passwordSet: boolean
  password: string | null
  onPasswordChange: (password: string | null) => void
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
  },
  {
    mode: 'proxy',
    label: 'Proxy SOCKS5',
    help: 'Proxy fourni par ton VPN ou un serveur à toi. Tout passe par lui ; la DHT, les trackers UDP et les connexions entrantes sont coupés (ils ne passeraient pas par le proxy).'
  }
]

function handshakeText (secs: number | null) {
  if (secs === null) return 'aucun échange avec le serveur pour l’instant'
  if (secs < 180) return `dernier échange avec le serveur il y a ${secs} s`
  return `aucun échange depuis ${Math.round(secs / 60)} min : serveur injoignable ?`
}

export function VpnSection ({ vpn, savedMode, onChange, passwordSet, password, onPasswordChange }: VpnSectionProps) {
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

  // redirection de port : état réel (port obtenu, erreur), relu tant que l'option est active
  const [forwarding, setForwarding] = useState<{ port: number | null; error: string | null } | null>(null)
  const loadForwarding = useCallback(() => { api.forwardingView().then(setForwarding).catch(() => {}) }, [])
  useEffect(() => { if (vpn.portForwarding) loadForwarding() }, [vpn.portForwarding, loadForwarding])
  useInterval(() => { if (vpn.portForwarding && vpn.mode !== 'off') loadForwarding() }, 3000)

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

      {vpn.mode !== 'off' && vpn.mode !== 'proxy' && (
        <p className='help' style={{ marginTop: 0 }}>
          <ShieldCheck size={14} style={{ verticalAlign: '-2px' }} /> Arrêt automatique : si le VPN se coupe, tous les téléchargements
          se mettent en pause et rien ne sort sans lui ; ils reprennent dès son retour.
          {vpn.mode !== savedMode && ' Enregistre pour activer.'}
        </p>
      )}

      {vpn.mode === 'proxy' && (
        <>
          <p className='help' style={{ marginTop: 0 }}>
            <ShieldCheck size={14} style={{ verticalAlign: '-2px' }} /> Si le proxy ne répond plus, rien ne sort sans lui : les
            téléchargements attendent. Les noms (trackers, sites) sont résolus par le proxy.
            {vpn.mode !== savedMode && ' Enregistre pour activer.'}
          </p>
          <div className='field-pair'>
            <div className='field'>
              <label htmlFor='proxy-host'>Serveur du proxy</label>
              <input
                id='proxy-host' className='input mono' spellCheck={false} placeholder='proxy.exemple.com ou 10.0.0.1'
                value={vpn.proxyHost} onChange={(e) => onChange({ ...vpn, proxyHost: e.target.value })}
              />
            </div>
            <div className='field'>
              <label htmlFor='proxy-port'>Port</label>
              <input
                id='proxy-port' className='input' type='number' min={1} max={65535} placeholder='1080'
                value={vpn.proxyPort || ''} onChange={(e) => onChange({ ...vpn, proxyPort: Math.max(0, Math.min(65535, Number(e.target.value) || 0)) })}
              />
            </div>
          </div>
          <div className='field-pair'>
            <div className='field'>
              <label htmlFor='proxy-user'>Identifiant <span className='optional'>(facultatif)</span></label>
              <input
                id='proxy-user' className='input' spellCheck={false} autoComplete='off'
                value={vpn.proxyUser} onChange={(e) => onChange({ ...vpn, proxyUser: e.target.value })}
              />
            </div>
            <SecretField id='proxy-password' label='Mot de passe (rangé dans Windows)' isSet={passwordSet} draft={password} onChange={onPasswordChange} />
          </div>
        </>
      )}

      {vpn.mode === 'adapter' && (
        <div className='field'>
          <label htmlFor='vpn-adapter'>Carte réseau du VPN</label>
          <div className='input-row'>
            <Select
              id='vpn-adapter' ariaLabel='Carte réseau du VPN' value={vpn.adapter} onChange={(adapter) => onChange({ ...vpn, adapter })}
              options={[
                { value: '', label: 'Choisir…' },
                ...(vpn.adapter && !adapters?.some((a) => a.name === vpn.adapter) ? [{ value: vpn.adapter, label: `${vpn.adapter} (absente pour l’instant)` }] : []),
                ...(adapters ?? []).map((a) => ({ value: a.name, label: `${a.name}${a.likelyVpn ? ' · VPN probable' : ''}${a.up ? '' : ' · déconnectée'}` }))
              ]}
            />
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

      {vpn.mode !== 'off' && vpn.mode !== 'proxy' && (
        <div>
          <Check
            checked={vpn.portForwarding}
            onChange={(portForwarding) => onChange({ ...vpn, portForwarding })}
            label='Redirection de port (ProtonVPN)'
            help='Demande au serveur VPN un port ouvert vers toi (NAT-PMP) : les autres pairs peuvent se connecter à toi, donc plus de sources et de débit. Nécessite un serveur P2P avec la redirection de port activée chez ProtonVPN (option « NAT-PMP » dans le fichier WireGuard ou l’appli).'
          />
          {vpn.portForwarding && forwarding && (
            <p className={`help${forwarding.error ? ' tone-danger' : ''}`} role='status'>
              {forwarding.port
                ? <>Port ouvert par le VPN : <b className='mono'>{forwarding.port}</b> (le moteur écoute dessus){forwarding.error ? ` · dernier renouvellement : ${forwarding.error}` : ''}</>
                : forwarding.error ?? (savedMode === 'off' ? 'Enregistre pour activer.' : 'Demande en cours auprès du serveur VPN…')}
            </p>
          )}
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
