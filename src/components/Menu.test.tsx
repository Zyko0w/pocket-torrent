import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { Menu } from './Menu'

/** Place le bouton du menu à `top` px du haut d'une fenêtre de 800 px, et donne au menu 300 px de haut. */
function layout (top: number) {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return { top, bottom: top + 32, left: 900, right: 932, width: 32, height: 32, x: 900, y: top, toJSON () {} } as DOMRect
  })
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(300)
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: 800 })
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1000 })
}

const items = (onSelect = vi.fn()) => [
  { label: 'Priorité haute', checked: false, onSelect },
  { label: 'Priorité normale', checked: true, onSelect: vi.fn() },
  { label: 'Supprimer…', danger: true, onSelect: vi.fn() }
]

describe('Menu', () => {
  it('s’ouvre sous le bouton quand la place suffit, hors de la ligne (portail)', () => {
    layout(100)
    const { container } = render(<Menu label='Plus d’actions' items={items()} />)
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    const menu = screen.getByRole('menu')
    expect(container.contains(menu)).toBe(false)
    expect(menu.style.position).toBe('fixed')
    expect(menu.style.top).toBe('138px') // 100 + 32 + 6
    expect(menu.classList.contains('up')).toBe(false)
  })

  it('s’ouvre au-dessus du bouton en bas de la fenêtre (dernier téléchargement)', () => {
    layout(700)
    render(<Menu label='Plus d’actions' items={items()} />)
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    const menu = screen.getByRole('menu')
    expect(menu.classList.contains('up')).toBe(true)
    expect(menu.style.top).toBe('394px') // 700 - 6 - 300
  })

  it('choisir une priorité appelle l’action et ferme le menu', () => {
    layout(100)
    const onSelect = vi.fn()
    render(<Menu label='Plus d’actions' items={items(onSelect)} />)
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }))
    fireEvent.click(screen.getByRole('menuitemradio', { name: /Priorité haute/ }))
    expect(onSelect).toHaveBeenCalledOnce()
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('se ferme avec Échap et au défilement de la liste', () => {
    layout(100)
    render(<Menu label='Plus d’actions' items={items()} />)
    const button = screen.getByRole('button', { name: 'Plus d’actions' })
    fireEvent.click(button)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('menu')).toBeNull()
    fireEvent.click(button)
    fireEvent.scroll(document.body)
    expect(screen.queryByRole('menu')).toBeNull()
  })
})
