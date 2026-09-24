import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
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

  it('le menu explique l’effet de la priorité (file et débit)', () => {
    renderRow()
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    expect(screen.getByRole('menuitemradio', { name: /Priorité haute/ }).textContent).toContain('plus de débit que les autres')
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
