import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { AboutPanel } from './AboutPanel'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.1.2') }))

describe('AboutPanel', () => {
  it('affiche la version et ouvre la page GitHub par la commande dédiée', async () => {
    render(<ToastProvider><AboutPanel /></ToastProvider>)
    expect(await screen.findByText('Version 0.1.2')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /Page GitHub/ }))
    expect(invoke).toHaveBeenCalledWith('open_project_page')
  })
})
