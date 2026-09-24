import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { makeJob } from '../test/job'
import { DeleteDialog } from './DeleteDialog'

describe('DeleteDialog', () => {
  it('propose de supprimer le .torrent d’origine, décoché par défaut', () => {
    const onConfirm = vi.fn()
    const source = String.raw`C:\Users\x\Downloads\Mon film (1).torrent`
    render(<DeleteDialog job={makeJob({ sourceFile: source })} onCancel={vi.fn()} onConfirm={onConfirm} />)

    const box = screen.getByRole('checkbox', { name: /fichier \.torrent d’origine/ })
    expect((box as HTMLInputElement).checked).toBe(false)
    expect(screen.getByText(source)).toBeTruthy()

    fireEvent.click(box)
    fireEvent.click(screen.getByRole('button', { name: 'Arrêter et supprimer' }))
    // téléchargement inachevé : fichiers commencés supprimés par défaut
    expect(onConfirm).toHaveBeenCalledWith(true, false, true)
  })

  it('pas de case .torrent pour un magnet', () => {
    render(<DeleteDialog job={makeJob()} onCancel={vi.fn()} onConfirm={vi.fn()} />)
    expect(screen.queryByRole('checkbox', { name: /\.torrent/ })).toBeNull()
  })

  it('téléchargement terminé : fichiers gardés par défaut', () => {
    const onConfirm = vi.fn()
    render(<DeleteDialog job={makeJob({ status: 'done' })} onCancel={vi.fn()} onConfirm={onConfirm} />)
    fireEvent.click(screen.getByRole('button', { name: 'Supprimer' }))
    expect(onConfirm).toHaveBeenCalledWith(false, false, false)
  })
})
