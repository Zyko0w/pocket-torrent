import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { ToastProvider } from '../Toast'
import { AboutSection } from './SystemSections'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.1.2') }))

describe('AboutSection', () => {
  it('affiche la version et ouvre la page GitHub par la commande dédiée', async () => {
    render(<ToastProvider><AboutSection /></ToastProvider>)
    expect(await screen.findByText('Version 0.1.2')).toBeTruthy()
    expect(screen.getByText('github.com/Zyko0w/pocket-torrent-releases')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /Page GitHub/ }))
    expect(invoke).toHaveBeenCalledWith('open_project_page')
  })
})
