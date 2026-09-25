import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import { CaretDown, Check } from '@phosphor-icons/react'

export interface SelectOption {
  value: string
  label: string
}

interface SelectProps {
  value: string
  options: SelectOption[]
  onChange: (value: string) => void
  id?: string
  /** Nom lu par les lecteurs d'écran quand aucun <label htmlFor> ne désigne la liste. */
  ariaLabel?: string
  title?: string
  disabled?: boolean
  /** Variante compacte (barre d'outils, zone d'ajout). */
  compact?: boolean
}

const GAP = 4
const EDGE = 8

/**
 * Liste déroulante au style de l'application. Remplace <select>, dont la liste ouverte est
 * dessinée par Windows (couleurs et survol impossibles à accorder au thème).
 * Clavier : flèches, Début/Fin, Entrée ou Espace, Échap, et première lettre d'une option.
 */
export function Select ({ value, options, onChange, id, ariaLabel, title, disabled = false, compact = false }: SelectProps) {
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(0)
  const [style, setStyle] = useState<CSSProperties | null>(null)
  const button = useRef<HTMLButtonElement>(null)
  const list = useRef<HTMLUListElement>(null)
  const listId = useId()
  const selectedIndex = Math.max(0, options.findIndex((o) => o.value === value))
  const current = options.find((o) => o.value === value)

  const openList = () => {
    if (disabled || options.length === 0) return
    setActive(selectedIndex)
    setOpen(true)
  }

  const close = (refocus = true) => {
    setOpen(false)
    setStyle(null)
    if (refocus) button.current?.focus()
  }

  const choose = (index: number) => {
    const option = options[index]
    close()
    if (option && option.value !== value) onChange(option.value)
  }

  // sous le bouton, ou au-dessus si la place manque ; au moins aussi large que le bouton
  useLayoutEffect(() => {
    if (!open || !button.current || !list.current) return
    const anchor = button.current.getBoundingClientRect()
    const height = list.current.offsetHeight
    const below = window.innerHeight - anchor.bottom - GAP - EDGE
    const above = anchor.top - GAP - EDGE
    const up = height > below && above > below
    const next: CSSProperties = {
      position: 'fixed',
      left: Math.max(EDGE, Math.min(anchor.left, window.innerWidth - EDGE - anchor.width)),
      minWidth: anchor.width,
      top: up ? Math.max(EDGE, anchor.top - GAP - height) : anchor.bottom + GAP,
      maxHeight: Math.max(120, up ? above : below),
      overflowY: 'auto'
    }
    setStyle((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next))
  })

  useEffect(() => {
    if (!open) return
    const inside = (target: EventTarget | null) =>
      target instanceof Node && (button.current?.contains(target) || list.current?.contains(target))
    const onPointer = (e: PointerEvent) => { if (!inside(e.target)) close(false) }
    const onScroll = (e: Event) => { if (!inside(e.target)) close(false) }
    const onResize = () => close(false)
    document.addEventListener('pointerdown', onPointer)
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('resize', onResize)
    window.addEventListener('blur', onResize)
    return () => {
      document.removeEventListener('pointerdown', onPointer)
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('resize', onResize)
      window.removeEventListener('blur', onResize)
    }
  }, [open])

  // option active toujours visible quand la liste défile
  useEffect(() => {
    if (open) list.current?.children[active]?.scrollIntoView?.({ block: 'nearest' })
  }, [open, active, style])

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
        e.preventDefault()
        openList()
      }
      return
    }
    const last = options.length - 1
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); setActive((i) => Math.min(last, i + 1)); break
      case 'ArrowUp': e.preventDefault(); setActive((i) => Math.max(0, i - 1)); break
      case 'Home': e.preventDefault(); setActive(0); break
      case 'End': e.preventDefault(); setActive(last); break
      case 'Enter':
      case ' ': e.preventDefault(); choose(active); break
      case 'Escape': e.preventDefault(); close(); break
      case 'Tab': close(false); break
      default:
        if (e.key.length === 1) {
          // première lettre : option suivante qui commence par elle
          const letter = e.key.toLowerCase()
          const order = [...options.keys()].map((k) => (active + 1 + k) % options.length)
          const found = order.find((k) => options[k].label.toLowerCase().startsWith(letter))
          if (found !== undefined) setActive(found)
        }
    }
  }

  return (
    <>
      <button
        ref={button} id={id} type='button' title={title} disabled={disabled}
        className={`select-trigger${compact ? ' compact' : ''}${open ? ' open' : ''}`}
        role='combobox' aria-haspopup='listbox' aria-expanded={open} aria-controls={open ? listId : undefined}
        aria-label={ariaLabel} aria-activedescendant={open ? `${listId}-${active}` : undefined}
        onClick={() => (open ? close() : openList())} onKeyDown={onKeyDown}
      >
        <span className='select-value'>{current?.label ?? ''}</span>
        <CaretDown size={14} weight='bold' className='select-caret' aria-hidden='true' />
      </button>
      {open && createPortal(
        <ul
          ref={list} id={listId} role='listbox' className='select-list' aria-label={ariaLabel}
          style={style ?? { position: 'fixed', top: 0, left: 0, visibility: 'hidden' }}
        >
          {options.map((option, i) => (
            <li
              key={option.value} id={`${listId}-${i}`} role='option' aria-selected={option.value === value}
              className={`select-option${i === active ? ' active' : ''}`}
              // pas de perte de focus du bouton au clic
              onPointerDown={(e) => e.preventDefault()}
              onPointerEnter={() => setActive(i)}
              onClick={() => choose(i)}
            >
              <span>{option.label}</span>
              {option.value === value && <Check size={14} weight='bold' className='select-check' aria-hidden='true' />}
            </li>
          ))}
        </ul>,
        document.body
      )}
    </>
  )
}
