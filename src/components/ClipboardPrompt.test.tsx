import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { ClipboardPrompt } from './ClipboardPrompt'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mocked = vi.mocked(invoke)
const link = 'magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&dn=Film'

function renderPrompt (found: unknown) {
  mocked.mockImplementation(async (cmd: string) => cmd === 'clipboard_magnet' ? found : { added: 1, failed: [], errors: [] })
  const props = { onEdit: vi.fn(), onAdded: vi.fn() }
  render(<ToastProvider><ClipboardPrompt {...props} /></ToastProvider>)
  return props
}

describe('ClipboardPrompt', () => {
  beforeEach(() => mocked.mockReset())

  it('propose le magnet copié et l’ajoute', async () => {
    const props = renderPrompt({ link, name: 'Film' })
    fireEvent.click(await screen.findByRole('button', { name: 'Ajouter' }))
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledWith('add_links', expect.objectContaining({ links: [link] })))
    expect(props.onAdded).toHaveBeenCalled()
  })

  it('« Dans la zone d’ajout » passe le lien sans l’ajouter', async () => {
    const props = renderPrompt({ link, name: 'Film' })
    fireEvent.click(await screen.findByRole('button', { name: /zone d’ajout/ }))
    expect(props.onEdit).toHaveBeenCalledWith(link)
    expect(mocked).not.toHaveBeenCalledWith('add_links', expect.anything())
  })

  it('revérifie au retour sur la fenêtre, et ne montre rien sans magnet', async () => {
    renderPrompt(null)
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledTimes(1))
    fireEvent.focus(window)
    await vi.waitFor(() => expect(mocked).toHaveBeenCalledTimes(2))
    expect(screen.queryByRole('button', { name: 'Ajouter' })).toBeNull()
  })
})
