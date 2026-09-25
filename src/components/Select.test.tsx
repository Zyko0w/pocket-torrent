import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { Select } from './Select'

const OPTIONS = [
  { value: 'high', label: 'Priorité haute' },
  { value: 'normal', label: 'Priorité normale' },
  { value: 'low', label: 'Priorité basse' }
]

function renderSelect (value = 'normal') {
  const onChange = vi.fn()
  render(<Select ariaLabel='Priorité' value={value} options={OPTIONS} onChange={onChange} />)
  return { onChange, trigger: screen.getByRole('combobox', { name: 'Priorité' }) }
}

describe('Select', () => {
  it('affiche la valeur choisie et ouvre la liste au clic', () => {
    const { onChange, trigger } = renderSelect()
    expect(trigger.textContent).toBe('Priorité normale')
    expect(screen.queryByRole('listbox')).toBeNull()
    fireEvent.click(trigger)
    expect(trigger.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByRole('option', { name: 'Priorité normale' }).getAttribute('aria-selected')).toBe('true')
    fireEvent.click(screen.getByRole('option', { name: 'Priorité basse' }))
    expect(onChange).toHaveBeenCalledWith('low')
    expect(screen.queryByRole('listbox')).toBeNull()
  })

  it('se pilote au clavier : flèches, Entrée, Échap, première lettre', () => {
    const { onChange, trigger } = renderSelect()
    fireEvent.keyDown(trigger, { key: 'ArrowDown' })
    expect(screen.getByRole('listbox')).toBeTruthy()
    fireEvent.keyDown(trigger, { key: 'ArrowDown' })
    fireEvent.keyDown(trigger, { key: 'Enter' })
    expect(onChange).toHaveBeenLastCalledWith('low')

    fireEvent.keyDown(trigger, { key: 'Enter' })
    fireEvent.keyDown(trigger, { key: 'Home' })
    fireEvent.keyDown(trigger, { key: ' ' })
    expect(onChange).toHaveBeenLastCalledWith('high')

    fireEvent.keyDown(trigger, { key: 'ArrowUp' })
    fireEvent.keyDown(trigger, { key: 'Escape' })
    expect(screen.queryByRole('listbox')).toBeNull()
    expect(onChange).toHaveBeenCalledTimes(2)
  })

  it('ne signale rien si la valeur ne change pas, et se ferme au clic à côté', () => {
    const { onChange, trigger } = renderSelect()
    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole('option', { name: 'Priorité normale' }))
    expect(onChange).not.toHaveBeenCalled()
    fireEvent.click(trigger)
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('listbox')).toBeNull()
  })
})
