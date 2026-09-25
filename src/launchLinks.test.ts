import { describe, expect, it, vi } from 'vitest'
import { takeLaunchLinks } from './launchLinks'

describe('takeLaunchLinks', () => {
  it('réessaie tant que le moteur démarre, puis transmet le .torrent reçu', async () => {
    vi.useFakeTimers()
    const take = vi.fn()
      .mockRejectedValueOnce(new Error('state not managed'))
      .mockRejectedValueOnce(new Error('state not managed'))
      .mockResolvedValueOnce(['C:\Téléchargements\film.torrent'])
    const onLinks = vi.fn()
    takeLaunchLinks(take, onLinks, { delayMs: 200 })
    await vi.advanceTimersByTimeAsync(1000)
    expect(take).toHaveBeenCalledTimes(3)
    expect(onLinks).toHaveBeenCalledWith(['C:\Téléchargements\film.torrent'])
    vi.useRealTimers()
  })

  it('rien reçu : rien à faire, pas de nouvel essai', async () => {
    const take = vi.fn().mockResolvedValue([])
    const onLinks = vi.fn()
    takeLaunchLinks(take, onLinks)
    await Promise.resolve()
    await Promise.resolve()
    expect(take).toHaveBeenCalledTimes(1)
    expect(onLinks).not.toHaveBeenCalled()
  })

  it('s’arrête quand la fenêtre est fermée et abandonne après le nombre d’essais', async () => {
    vi.useFakeTimers()
    const take = vi.fn().mockRejectedValue(new Error('pas prêt'))
    const stop = takeLaunchLinks(take, vi.fn(), { attempts: 5, delayMs: 100 })
    await vi.advanceTimersByTimeAsync(250)
    stop()
    const calls = take.mock.calls.length
    await vi.advanceTimersByTimeAsync(2000)
    expect(take.mock.calls.length).toBe(calls)

    const giveUp = vi.fn().mockRejectedValue(new Error('pas prêt'))
    takeLaunchLinks(giveUp, vi.fn(), { attempts: 5, delayMs: 100 })
    await vi.advanceTimersByTimeAsync(5000)
    expect(giveUp).toHaveBeenCalledTimes(5)
    vi.useRealTimers()
  })
})
