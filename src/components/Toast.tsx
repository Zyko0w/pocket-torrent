import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from 'react'
import { Check } from '@phosphor-icons/react'

const ToastContext = createContext<(message: string) => void>(() => {})

export const useToast = () => useContext(ToastContext)

export function ToastProvider ({ children }: { children: ReactNode }) {
  const [message, setMessage] = useState('')
  const [visible, setVisible] = useState(false)
  const timer = useRef<number | undefined>(undefined)

  const show = useCallback((text: string) => {
    setMessage(text)
    setVisible(true)
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setVisible(false), 2600)
  }, [])

  return (
    <ToastContext.Provider value={show}>
      {children}
      {/* annonce pour les lecteurs d'écran */}
      <div className='sr-only' role='status' aria-atomic='true'>{visible ? message : ''}</div>
      <div className={`toast ${visible ? 'show' : ''}`} aria-hidden='true'>
        <Check size={16} color='var(--primary-text)' />
        {message}
      </div>
    </ToastContext.Provider>
  )
}
