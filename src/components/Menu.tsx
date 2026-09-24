import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { DotsThree } from '@phosphor-icons/react'

export interface MenuItem {
  label: string
  icon?: ReactNode
  onSelect: () => void
  checked?: boolean
  disabled?: boolean
  /** Trait de séparation avant cet élément. */
  separated?: boolean
  /** Action destructrice (suppression) : en rouge. */
  danger?: boolean
}

interface MenuProps {
  label: string
  items: MenuItem[]
  /** Ouvre le menu au-dessus du bouton de préférence (bas de la barre latérale). */
  up?: boolean
  /** Aligne le menu sur le bord gauche du bouton plutôt que le droit. */
  start?: boolean
  /** Contenu du bouton (trois points par défaut). */
  trigger?: ReactNode
  triggerClassName?: string
}

/** Écart entre le bouton et le menu, et marge minimale avec les bords de la fenêtre. */
const GAP = 6
const EDGE = 8

/**
 * Petit menu d'actions : s'ouvre au clic, se ferme avec Échap, un clic à côté ou un défilement.
 * Rendu hors de la liste (portail, position fixe) : ni coupé par la zone qui défile, ni recouvert
 * par les lignes suivantes, et ouvert vers le haut quand la place manque en dessous.
 */
export function Menu ({ label, items, up = false, start = false, trigger, triggerClassName = 'icon-btn' }: MenuProps) {
  const [open, setOpen] = useState(false)
  const [place, setPlace] = useState<{ style: CSSProperties; up: boolean } | null>(null)
  const button = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const id = useId()

  const close = (refocus = false) => {
    setOpen(false)
    setPlace(null)
    if (refocus) button.current?.focus()
  }

  // position recalculée à chaque rendu : la ligne peut bouger pendant que le menu est ouvert
  useLayoutEffect(() => {
    if (!open || !button.current || !menu.current) return
    const anchor = button.current.getBoundingClientRect()
    const height = menu.current.offsetHeight
    const below = window.innerHeight - anchor.bottom - GAP - EDGE
    const above = anchor.top - GAP - EDGE
    // vers le bas sauf si la place y manque et qu'il y en a plus au-dessus (l'inverse avec `up`)
    const goUp = up ? height <= above || above > below : height > below && above > below
    const top = goUp
      ? Math.max(EDGE, anchor.top - GAP - height)
      : Math.min(anchor.bottom + GAP, window.innerHeight - EDGE - height)
    const style: CSSProperties = {
      position: 'fixed',
      top: Math.max(EDGE, top),
      bottom: 'auto',
      // fenêtre trop basse pour tout le menu : il défile
      maxHeight: window.innerHeight - 2 * EDGE,
      overflowY: 'auto'
    }
    if (start) {
      style.left = Math.max(EDGE, anchor.left)
      style.right = 'auto'
    } else {
      style.right = Math.max(EDGE, window.innerWidth - anchor.right)
      style.left = 'auto'
    }
    const next = { style, up: goUp }
    setPlace((current) => JSON.stringify(current) === JSON.stringify(next) ? current : next)
  })

  useEffect(() => {
    if (!open) return
    const inside = (target: EventTarget | null) =>
      target instanceof Node && (button.current?.contains(target) || menu.current?.contains(target))
    const onPointer = (e: PointerEvent) => { if (!inside(e.target)) close() }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') close(true) }
    const onScroll = (e: Event) => { if (!inside(e.target)) close() }
    const onResize = () => close()
    document.addEventListener('pointerdown', onPointer)
    document.addEventListener('keydown', onKey)
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('resize', onResize)
    window.addEventListener('blur', onResize)
    return () => {
      document.removeEventListener('pointerdown', onPointer)
      document.removeEventListener('keydown', onKey)
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('resize', onResize)
      window.removeEventListener('blur', onResize)
    }
  }, [open])

  // focus sur le premier élément une fois le menu placé (pas avant : il sauterait à l'écran)
  const placed = place !== null
  useEffect(() => {
    if (open && placed) {
      menu.current?.querySelector<HTMLButtonElement>('.menu-item:not(:disabled)')?.focus({ preventScroll: true })
    }
  }, [open, placed])

  const onMenuKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Tab') {
      close(true)
      return
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    e.preventDefault()
    const buttons = [...(menu.current?.querySelectorAll<HTMLButtonElement>('.menu-item:not(:disabled)') ?? [])]
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const next = e.key === 'ArrowDown' ? index + 1 : index - 1
    buttons[(next + buttons.length) % buttons.length]?.focus()
  }

  return (
    <div className='menu-root'>
      <button
        ref={button}
        type='button' className={triggerClassName} title={label} aria-label={trigger ? undefined : label}
        aria-haspopup='menu' aria-expanded={open} aria-controls={open ? id : undefined}
        onClick={() => (open ? close() : setOpen(true))}
      >
        {trigger ?? <DotsThree size={18} weight='bold' />}
      </button>
      {open && createPortal(
        <div
          ref={menu}
          className={`menu${place?.up ? ' up' : ''}${start ? ' start' : ''}`}
          role='menu' id={id} aria-label={label} onKeyDown={onMenuKey}
          // mesuré avant d'être montré : invisible tant qu'il n'est pas placé
          style={place?.style ?? { position: 'fixed', top: 0, left: 0, visibility: 'hidden' }}
        >
          {items.map((item) => (
            <button
              key={item.label}
              type='button'
              role={item.checked === undefined ? 'menuitem' : 'menuitemradio'}
              aria-checked={item.checked}
              className={`menu-item${item.separated ? ' separated' : ''}${item.danger ? ' danger' : ''}`}
              disabled={item.disabled}
              onClick={() => { close(true); item.onSelect() }}
            >
              <span className='menu-icon' aria-hidden='true'>{item.icon}</span>
              {item.label}
              {item.checked && <span className='menu-check' aria-hidden='true'>●</span>}
            </button>
          ))}
        </div>,
        document.body
      )}
    </div>
  )
}
