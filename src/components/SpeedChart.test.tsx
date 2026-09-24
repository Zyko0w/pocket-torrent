import { fireEvent, render, screen } from '@testing-library/react'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { SpeedChart, niceMax } from './SpeedChart'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mocked = vi.mocked(invoke)

beforeAll(() => {
  // jsdom ne mesure rien : largeur fixe pour le graphique
  globalThis.ResizeObserver = class { observe () {} disconnect () {} unobserve () {} } as never
  Element.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, width: 600, height: 150, right: 600, bottom: 150, toJSON () {} })
})

describe('SpeedChart', () => {
  it('graduation haute ronde, jamais sous 16 Kio/s', () => {
    expect(niceMax(0)).toBe(20 * 1024)
    expect(niceMax(3.4 * 1024 * 1024)).toBe(5 * 1024 * 1024)
    expect(niceMax(900 * 1024)).toBe(1024 * 1024)
  })

  it('légende avec les valeurs actuelles, et valeurs du point survolé', async () => {
    const down = Array.from({ length: 300 }, (_, i) => i * 1024)
    const up = Array(300).fill(2048)
    mocked.mockResolvedValue({ down, up })
    const { container } = render(<SpeedChart jobId='t:abc' />)
    expect(await screen.findByText('299 Ko/s')).toBeTruthy()
    expect(mocked).toHaveBeenCalledWith('speed_history', { id: 't:abc' })
    expect(container.querySelector('svg')!.getAttribute('aria-label')).toContain('Débit des 5 dernières minutes')
    // survol au début de la courbe : il y a 5 min
    fireEvent.pointerMove(container.querySelector('.chart-box rect')!, { clientX: 0 })
    expect(screen.getByRole('status').textContent).toContain('il y a 5 min')
  })
})
