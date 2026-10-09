import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import type { Delivery, SecretsInput, Settings } from '../api'
import defaults from '../test/defaultSettings.json'
import { makeJob } from '../test/job'
import { Composer } from './Composer'
import { DeleteDialog } from './DeleteDialog'
import { SendDialog } from './SendDialog'
import { UploadSection } from './settings/UploadSection'
import { ToastProvider } from './Toast'
import { fromSxcu, type CustomHost } from '../uploadHosts'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
const mocked = vi.mocked(invoke)

const settings = defaults as unknown as Settings
const noSecrets: SecretsInput = { storageToToken: null, gofileToken: null, vikingfileUser: null, proxyPassword: null, customHostHeaders: {} }
const noneSaved = { storageToToken: false, gofileToken: false, vikingfileUser: false, proxyPassword: false, customHostHeaders: [] as string[] }

function delivery (patch: Partial<Delivery> = {}): Delivery {
  return {
    state: 'done', sentBytes: 10, totalBytes: 10, expiresAt: Date.now() + 86_400_000,
    links: [{ name: 'Film.mkv', size: 10, key: 'FQxyz1234', url: 'https://storage.to/FQxyz1234', kind: 'file' }],
    ...patch
  }
}

function renderSection (host: Settings['storageTo']['host'], update = vi.fn(), onSecretsChange = vi.fn(), customHosts: CustomHost[] = []) {
  const current = { ...settings, storageTo: { ...settings.storageTo, host, customHosts } }
  render(
    <ToastProvider>
      <UploadSection settings={current} update={update} defaults={settings} saved={noneSaved} secrets={noSecrets} onSecretsChange={onSecretsChange} />
    </ToastProvider>
  )
}

describe('envoi en ligne', () => {
  beforeEach(() => mocked.mockReset())

  it('la zone d’ajout propose « Mettre en ligne une fois fini » et transmet ce choix', async () => {
    mocked.mockResolvedValue({ added: 1, failed: [], errors: [] })
    render(
      <ToastProvider>
        <Composer prefill='magnet:?xt=urn:btih:aa' onPrefillUsed={vi.fn()} categories={[]} defaultCategory='' onChooseFiles={vi.fn()} onAdded={vi.fn()} />
      </ToastProvider>
    )
    fireEvent.click(screen.getByRole('combobox', { name: 'Destination' }))
    fireEvent.click(screen.getByRole('option', { name: 'Mettre en ligne une fois fini' }))
    fireEvent.click(screen.getByRole('button', { name: /Ajouter/ }))
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledWith('add_links', expect.objectContaining({
      options: expect.objectContaining({ dest: 'storageTo' })
    })))
  })

  it('supprimer propose d’effacer aussi les fichiers en ligne, chez le bon service', () => {
    const onConfirm = vi.fn()
    render(<DeleteDialog job={makeJob({ status: 'done', delivery: delivery() })} onCancel={vi.fn()} onConfirm={onConfirm} />)
    fireEvent.click(screen.getByRole('checkbox', { name: /sur storage\.to/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Supprimer' }))
    expect(onConfirm).toHaveBeenCalledWith(false, true, false)
  })

  it('pas de suppression en ligne proposée pour un envoi impossible à supprimer', () => {
    const job = makeJob({ status: 'done', delivery: delivery({ host: 'vikingfile', deletable: false, expiresAt: null }) })
    render(<DeleteDialog job={job} onCancel={vi.fn()} onConfirm={vi.fn()} />)
    expect(screen.queryByRole('checkbox', { name: /VikingFile/ })).toBeNull()
  })

  it('le test de connexion porte sur le service choisi et affiche sa réponse', async () => {
    mocked.mockResolvedValue('storage.to joignable : 99.9 Go d’envoi restants sur 24 h (sans compte).')
    renderSection('storageTo')
    fireEvent.click(screen.getByRole('button', { name: /Tester la connexion/ }))
    await vi.waitFor(() => expect(screen.getAllByText(/99\.9 Go/).length).toBeGreaterThan(0))
    expect(mocked).toHaveBeenCalledWith('test_upload_host', { host: 'storageTo' })
  })

  it('GoFile : ni durée de vie ni limite, et son propre compte', () => {
    const onSecretsChange = vi.fn()
    renderSection('gofile', vi.fn(), onSecretsChange)
    expect(screen.queryByRole('combobox', { name: 'Durée de vie des liens' })).toBeNull()
    fireEvent.change(screen.getByLabelText(/Jeton de compte GoFile/), { target: { value: 'abc' } })
    expect(onSecretsChange).toHaveBeenCalledWith({ ...noSecrets, gofileToken: 'abc' })
  })

  it('le service se choisit dans les réglages', () => {
    const update = vi.fn()
    renderSection('storageTo', update)
    expect(screen.getByRole('combobox', { name: 'Durée de vie des liens' })).toBeTruthy()
    fireEvent.click(screen.getByRole('combobox', { name: 'Service d’hébergement' }))
    fireEvent.click(screen.getByRole('option', { name: 'VikingFile' }))
    expect(update).toHaveBeenCalledWith({ storageTo: expect.objectContaining({ host: 'vikingfile' }) })
  })

  it('l’envoi à la demande peut changer de service', async () => {
    mocked.mockImplementation(async (cmd) => cmd === 'get_settings' ? { settings } : null)
    const job = makeJob({ status: 'done', finishedAt: 1, dest: 'local', delivery: null })
    render(<ToastProvider><SendDialog job={job} onClose={vi.fn()} onSent={vi.fn()} /></ToastProvider>)
    fireEvent.click(await screen.findByRole('combobox', { name: 'Service d’hébergement' }))
    fireEvent.click(screen.getByRole('option', { name: 'GoFile' }))
    expect(screen.queryByRole('combobox', { name: 'Durée de vie du lien' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /Envoyer/ }))
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledWith('send_job_online', {
      id: job.id, options: { host: 'gofile', expiryDays: 7, maxDownloads: 0, deleteLocal: false }
    }))
  })

  it('trop gros pour storage.to : envoi bloqué, un autre service le débloque', async () => {
    mocked.mockImplementation(async (cmd) => cmd === 'get_settings' ? { settings } : null)
    const job = makeJob({ status: 'done', finishedAt: 1, dest: 'local', delivery: null, wantedBytes: 30 * 1024 ** 3 })
    render(<ToastProvider><SendDialog job={job} onClose={vi.fn()} onSent={vi.fn()} /></ToastProvider>)
    const select = await screen.findByRole('combobox', { name: 'Service d’hébergement' })
    expect(screen.getByRole('alert').textContent).toMatch(/trop volumineux pour storage\.to.*choisis un autre hébergeur/)
    expect((screen.getByRole('button', { name: /Envoyer/ }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(select)
    fireEvent.click(screen.getByRole('option', { name: 'GoFile' }))
    expect(screen.queryByRole('alert')).toBeNull()
    expect((screen.getByRole('button', { name: /Envoyer/ }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('hébergeur personnalisé : ajouté depuis un modèle, puis choisi comme service', () => {
    const update = vi.fn()
    renderSection('storageTo', update)
    fireEvent.click(screen.getByRole('combobox', { name: 'Partir d’un modèle' }))
    fireEvent.click(screen.getByRole('option', { name: /catbox\.moe/ }))
    const added = update.mock.calls[0][0].storageTo.customHosts[0] as CustomHost
    expect(added).toMatchObject({ name: 'catbox.moe', method: 'multipart', fileField: 'fileToUpload', formFields: [{ name: 'reqtype', value: 'fileupload' }] })
    expect(added.id).toMatch(/^[a-z0-9]{8}$/)
  })

  it('hébergeur personnalisé : retiré alors qu’il est choisi, retour à storage.to et clés oubliées', () => {
    const update = vi.fn()
    const host: CustomHost = { id: 'h1', name: 'Mon hébergeur', url: 'https://a.example', method: 'multipart', fileField: 'file', formFields: [], linkPath: '', linkTemplate: '' }
    renderSection('custom:h1', update, vi.fn(), [host])
    expect(screen.queryByLabelText(/Jeton de compte/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Retirer l’hébergeur Mon hébergeur' }))
    expect(update).toHaveBeenCalledWith({ storageTo: expect.objectContaining({ customHosts: [], host: 'storageTo' }) })
  })

  it('l’envoi à la demande propose les hébergeurs personnalisés', async () => {
    const host: CustomHost = { id: 'h1', name: 'Mon hébergeur', url: 'https://a.example', method: 'multipart', fileField: 'file', formFields: [], linkPath: '', linkTemplate: '' }
    const withCustom = { ...settings, storageTo: { ...settings.storageTo, customHosts: [host] } }
    mocked.mockImplementation(async (cmd) => cmd === 'get_settings' ? { settings: withCustom } : null)
    const job = makeJob({ status: 'done', finishedAt: 1, dest: 'local', delivery: null })
    render(<ToastProvider><SendDialog job={job} onClose={vi.fn()} onSent={vi.fn()} /></ToastProvider>)
    fireEvent.click(await screen.findByRole('combobox', { name: 'Service d’hébergement' }))
    fireEvent.click(screen.getByRole('option', { name: 'Mon hébergeur' }))
    fireEvent.click(screen.getByRole('button', { name: /Envoyer/ }))
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledWith('send_job_online', {
      id: job.id, options: expect.objectContaining({ host: 'custom:h1' })
    }))
  })
})

describe('import .sxcu', () => {
  it('formulaire, champs, en-têtes et lien JSON dans un modèle', () => {
    const { host, headers } = fromSxcu(JSON.stringify({
      Version: '15.0.0', Name: 'Mon hébergeur', DestinationType: 'FileUploader', RequestMethod: 'POST',
      RequestURL: 'https://a.example/upload', Body: 'MultipartFormData', FileFormName: 'file',
      Arguments: { expires: '24h' }, Headers: { Authorization: 'Bearer abc' }, URL: 'https://a.example/f/{json:files[0].id}'
    }))
    expect(host).toMatchObject({
      name: 'Mon hébergeur', url: 'https://a.example/upload', method: 'multipart', fileField: 'file',
      formFields: [{ name: 'expires', value: '24h' }], linkPath: 'files.0.id', linkTemplate: 'https://a.example/f/{value}'
    })
    expect(headers).toBe('Authorization: Bearer abc')
  })

  it('ancienne syntaxe, fichier brut en PUT et réponse texte', () => {
    const { host } = fromSxcu('{"RequestMethod":"PUT","RequestURL":"https://a.example/$filename$","Body":"Binary","URL":"$response$"}')
    expect(host).toMatchObject({ url: 'https://a.example/{filename}', method: 'put', linkPath: '', linkTemplate: '' })
    expect(fromSxcu('{"RequestURL":"https://a.example","FileFormName":"f","URL":"$json:data.link$"}').host.linkPath).toBe('data.link')
  })

  it('refuse ce qui n’est pas pris en charge', () => {
    expect(() => fromSxcu('pas du json')).toThrow(/illisible/)
    expect(() => fromSxcu('{"RequestURL":"https://a.example","Body":"Binary"}')).toThrow(/non pris en charge/)
    expect(() => fromSxcu('{"RequestURL":"https://a.example","FileFormName":"f","URL":"{regex:1}"}')).toThrow(/non pris en charge/)
  })
})
