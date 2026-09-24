import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ToastProvider, useToast } from './Toast'

function Trigger () {
  const toast = useToast()
  return <button type='button' onClick={() => toast('Lien copié')}>Copier</button>
}

describe('Toast', () => {
  it('la coche utilise la couleur « succès » du thème (pas le noir par défaut)', () => {
    const { container } = render(<ToastProvider><Trigger /></ToastProvider>)
    fireEvent.click(screen.getByRole('button', { name: 'Copier' }))
    const icon = container.querySelector('.toast svg')!
    expect(icon.classList.contains('toast-icon')).toBe(true)
    // Phosphor dessine avec currentColor : c'est la classe (couleur --success) qui décide
    expect(icon.getAttribute('fill')).toBe('currentColor')
  })
})
