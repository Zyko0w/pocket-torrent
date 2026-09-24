import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { LinkHandlerPrompt } from './LinkHandlerPrompt'
import { ToastProvider } from './Toast'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mocked = vi.mocked(invoke)

function answerWith (ask: boolean) {
  mocked.mockImplementation(async (cmd: string) => (cmd === 'should_ask_link_handler' ? ask : true))
}

const renderPrompt = () => render(<ToastProvider><LinkHandlerPrompt /></ToastProvider>)

describe('LinkHandlerPrompt', () => {
  beforeEach(() => mocked.mockReset())

  it('pose la question au premier lancement et enregistre « oui »', async () => {
    answerWith(true)
    renderPrompt()
    fireEvent.click(await screen.findByRole('button', { name: /Oui, utiliser Pocket Torrent/ }))
    await waitFor(() => expect(mocked).toHaveBeenCalledWith('answer_link_handler', { enable: true }))
  })

  it('enregistre « non merci »', async () => {
    answerWith(true)
    renderPrompt()
    fireEvent.click(await screen.findByRole('button', { name: 'Non merci' }))
    await waitFor(() => expect(mocked).toHaveBeenCalledWith('answer_link_handler', { enable: false }))
  })

  it('ne dit rien si la question a déjà été posée', async () => {
    answerWith(false)
    renderPrompt()
    await waitFor(() => expect(mocked).toHaveBeenCalledWith('should_ask_link_handler'))
    expect(screen.queryByRole('button', { name: 'Non merci' })).toBeNull()
  })

  it('mode dev : prévient que rien n’a été changé', async () => {
    mocked.mockImplementation(async (cmd: string) => cmd === 'should_ask_link_handler')
    renderPrompt()
    fireEvent.click(await screen.findByRole('button', { name: /Oui, utiliser Pocket Torrent/ }))
    expect(await screen.findByRole('status')).toHaveProperty('textContent', expect.stringMatching(/Mode dev : réponse ignorée/))
  })
})
