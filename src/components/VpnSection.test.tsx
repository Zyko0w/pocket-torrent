import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import type { Settings } from '../api'
import { ToastProvider } from './Toast'
import { VpnSection } from './VpnSection'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
const mocked = vi.mocked(invoke)

const tunnel = { service: 'running', config: null, status: null, lastError: null }
const PROXY = { proxyHost: '', proxyPort: 0, proxyUser: '' }

function renderVpn (vpn: Settings['vpn'], forwarding: { enabled: boolean; port: number | null; error: string | null }, onPasswordChange = vi.fn()) {
  mocked.mockImplementation(async (cmd: string) => (cmd === 'forwarding_view' ? forwarding : cmd === 'tunnel_view' ? tunnel : null))
  const onChange = vi.fn()
  render(
    <ToastProvider>
      <VpnSection vpn={vpn} savedMode={vpn.mode} onChange={onChange} passwordSet={false} password={null} onPasswordChange={onPasswordChange} />
    </ToastProvider>
  )
  return onChange
}

describe('VpnSection — redirection de port', () => {
  beforeEach(() => mocked.mockReset())

  it('affiche le port ouvert par le VPN', async () => {
    renderVpn({ mode: 'wireguard', adapter: '', portForwarding: true, ...PROXY }, { enabled: true, port: 45678, error: null })
    expect(await screen.findByText('45678')).toBeTruthy()
  })

  it('affiche l’erreur du serveur VPN', async () => {
    renderVpn({ mode: 'wireguard', adapter: '', portForwarding: true, ...PROXY }, { enabled: true, port: null, error: 'redirection de port refusée par le serveur VPN' })
    expect(await screen.findByText(/refusée par le serveur VPN/)).toBeTruthy()
  })

  it('l’interrupteur active l’option (à enregistrer)', async () => {
    const onChange = renderVpn({ mode: 'wireguard', adapter: '', portForwarding: false, ...PROXY }, { enabled: false, port: null, error: null })
    fireEvent.click(await screen.findByRole('switch', { name: /Redirection de port/ }))
    expect(onChange).toHaveBeenCalledWith({ mode: 'wireguard', adapter: '', portForwarding: true, ...PROXY })
  })

  it('rien sans VPN', () => {
    renderVpn({ mode: 'off', adapter: '', portForwarding: false, ...PROXY }, { enabled: false, port: null, error: null })
    expect(screen.queryByRole('switch', { name: /Redirection de port/ })).toBeNull()
  })
})

describe('VpnSection — proxy SOCKS5', () => {
  beforeEach(() => mocked.mockReset())

  it('serveur, port et identifiant se règlent ; le mot de passe part à part (secret)', () => {
    const onPasswordChange = vi.fn()
    const vpn = { mode: 'proxy' as const, adapter: '', portForwarding: false, proxyHost: '', proxyPort: 0, proxyUser: '' }
    const onChange = renderVpn(vpn, { enabled: false, port: null, error: null }, onPasswordChange)
    fireEvent.change(screen.getByLabelText('Serveur du proxy'), { target: { value: 'proxy.exemple.com' } })
    expect(onChange).toHaveBeenCalledWith({ ...vpn, proxyHost: 'proxy.exemple.com' })
    fireEvent.change(screen.getByLabelText('Port'), { target: { value: '1080' } })
    expect(onChange).toHaveBeenCalledWith({ ...vpn, proxyPort: 1080 })
    fireEvent.change(screen.getByLabelText(/Mot de passe/), { target: { value: 's3cret' } })
    expect(onPasswordChange).toHaveBeenCalledWith('s3cret')
    expect(onChange).not.toHaveBeenCalledWith(expect.objectContaining({ password: expect.anything() }))
    // pas de redirection de port NAT-PMP par un proxy
    expect(screen.queryByRole('switch', { name: /Redirection de port/ })).toBeNull()
  })
})
