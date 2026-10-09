import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { isFinished } from '../api'
import defaults from '../test/defaultSettings.json'
import { makeJob } from '../test/job'
import { JobRow } from './JobRow'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

function renderRow (job = makeJob(), onDelete = vi.fn()) {
  render(
    <ToastProvider>
      <ul><JobRow job={job} categoryName={null} onDelete={onDelete} onChanged={vi.fn()} /></ul>
    </ToastProvider>
  )
  return onDelete
}

describe('JobRow', () => {
  it('la croix ouvre la suppression d’un téléchargement en cours', () => {
    const job = makeJob()
    const onDelete = renderRow(job)
    fireEvent.click(screen.getByRole('button', { name: 'Arrêter et supprimer…' }))
    expect(onDelete).toHaveBeenCalledWith(job)
  })

  it('recherche d’un magnet : temps écoulé et pairs trouvés', () => {
    renderRow(makeJob({ id: 'p:abc', status: 'metadata', name: null, inEngine: false, searchSecs: 95, peersFound: 4 }))
    expect(screen.getByText('Recherche du nom…')).toBeTruthy()
    expect(screen.getByText(/depuis 2 min/)).toBeTruthy()
    expect(screen.getByText(/4 pairs trouvés/)).toBeTruthy()
  })

  it('recherche sans pair pour l’instant', () => {
    renderRow(makeJob({ id: 'p:abc', status: 'metadata', name: null, inEngine: false, searchSecs: 3, peersFound: 0 }))
    expect(screen.getByText(/aucun pair pour l’instant/)).toBeTruthy()
  })

  it('affiche le résultat de l’analyse Microsoft Defender', () => {
    const done = { status: 'done' as const, progressBytes: 1000, finishedAt: 1 }
    renderRow(makeJob({ ...done, scan: { state: 'running' } }))
    expect(screen.getByText(/Analyse Microsoft Defender en cours/)).toBeTruthy()
    renderRow(makeJob({ ...done, id: 't:b', scan: { state: 'clean' } }))
    expect(screen.getByText(/aucune menace/)).toBeTruthy()
    renderRow(makeJob({ ...done, id: 't:c', scan: { state: 'threat', threats: ['Virus:DOS/EICAR_Test_File'] } }))
    expect(screen.getByText(/a détecté : Virus:DOS\/EICAR_Test_File/)).toBeTruthy()
    renderRow(makeJob({ ...done, id: 't:d', scan: { state: 'unavailable' } }))
    expect(screen.getByText(/Analyse Microsoft Defender impossible/)).toBeTruthy()
  })

  it('le menu explique l’effet de la priorité (file et débit)', () => {
    renderRow()
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    expect(screen.getByRole('menuitemradio', { name: /Priorité haute/ }).textContent).toContain('plus de débit que les autres')
  })
})

describe('JobRow (envoi en ligne)', () => {
  it('un téléchargement terminé gardé sur le PC peut être envoyé à la demande', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    const job = makeJob({ status: 'done', finishedAt: 1, dest: 'local', delivery: null })
    renderRow(job)
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    vi.mocked(invoke).mockImplementation(async (cmd) => cmd === 'get_settings' ? { settings: defaults } : null)
    fireEvent.click(screen.getByRole('menuitem', { name: /Envoyer en ligne/ }))
    // choix préremplis par les réglages, puis modifiés pour cet envoi
    await screen.findByRole('combobox', { name: 'Durée de vie du lien' })
    fireEvent.click(screen.getByRole('combobox', { name: 'Effacer de storage.to' }))
    fireEvent.click(screen.getByRole('option', { name: 'Après 1 téléchargement' }))
    fireEvent.click(screen.getByRole('checkbox', { name: /Effacer les fichiers du PC/ }))
    fireEvent.click(screen.getByRole('button', { name: /Envoyer/ }))
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('send_job_online', {
      id: job.id, options: { host: 'storageTo', expiryDays: 7, maxDownloads: 1, deleteLocal: true }
    }))
    vi.mocked(invoke).mockImplementation(async () => null)
  })

  it('envoi raté : reste dans les terminés et peut être relancé', () => {
    const delivery = { host: 'storageTo' as const,state: 'error' as const, sentBytes: 0, totalBytes: 10, links: [], expiresAt: null, deletable: true }
    const job = makeJob({ status: 'error', finishedAt: 1, dest: 'local', delivery })
    expect(isFinished(job)).toBe(true)
    expect(isFinished(makeJob({ ...job, status: 'uploading', delivery: { ...delivery, state: 'uploading' } }))).toBe(true)
    // téléchargement lui-même en erreur : pas terminé
    expect(isFinished(makeJob({ status: 'error', finishedAt: null, delivery: null }))).toBe(false)
    renderRow(job)
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    expect(screen.getByRole('menuitem', { name: /Envoyer en ligne/ })).toBeTruthy()
  })

  it('une fois envoyé, un bouton copie le lien du dossier complet', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    vi.mocked(invoke).mockImplementation(async () => null)
    const delivery = {
      state: 'done' as const, sentBytes: 20, totalBytes: 20, expiresAt: Date.now() + 86_400_000,
      links: [
        { name: 'Saison 1 (tous les fichiers)', size: 20, key: 'C1', url: 'https://storage.to/c/C1', kind: 'collection' as const },
        { name: 'E01.mkv', size: 10, key: 'F1', url: 'https://storage.to/F1', kind: 'file' as const }
      ]
    }
    renderRow(makeJob({ status: 'done', finishedAt: 1, delivery }))
    fireEvent.click(screen.getByRole('button', { name: 'Copier le lien storage.to' }))
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('copy_text', { text: 'https://storage.to/c/C1' }))
  })

  it('rien à proposer en cours de téléchargement', () => {
    renderRow()
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    expect(screen.queryByRole('menuitem', { name: /Envoyer en ligne/ })).toBeNull()
  })
})

describe('JobRow (rendu)', () => {
  it('une ligne identique relue n’est pas redessinée, une ligne changée l’est', () => {
    const job = makeJob({ status: 'done', finishedAt: 1 })
    const props = { categoryName: null, onDelete: vi.fn(), onChanged: vi.fn() }
    const { rerender } = render(<ToastProvider><ul><JobRow job={job} {...props} /></ul></ToastProvider>)
    const title = screen.getByText('Film de test 2026.mkv')
    rerender(<ToastProvider><ul><JobRow job={{ ...job }} {...props} /></ul></ToastProvider>)
    expect(screen.getByText('Film de test 2026.mkv')).toBe(title)
    rerender(<ToastProvider><ul><JobRow job={{ ...job, name: 'Autre nom' }} {...props} /></ul></ToastProvider>)
    expect(screen.getByText('Autre nom')).toBeTruthy()
  })
})

describe('JobRow (limite de vitesse)', () => {
  it('un torrent en cours se limite comme un lien direct', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    renderRow(makeJob({ limitKbps: 0 }))
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Limiter la vitesse/ }))
    fireEvent.change(screen.getByLabelText(/Vitesse max en réception/), { target: { value: '500' } })
    fireEvent.click(screen.getByRole('button', { name: 'Appliquer' }))
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('set_job_limit', { id: makeJob().id, limitKbps: 500 }))
  })

  it('la limite choisie s’affiche sur la ligne', () => {
    renderRow(makeJob({ limitKbps: 500 }))
    expect(screen.getByText(/max 500 Ko\/s/)).toBeTruthy()
  })
})
