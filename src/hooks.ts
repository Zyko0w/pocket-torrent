import { useEffect, useRef, useState } from 'react'

/** Vrai tant que la fenêtre est visible (fausse quand elle est réduite ou cachée près de l'horloge). */
function usePageVisible (): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== 'hidden')
  useEffect(() => {
    const onChange = () => setVisible(document.visibilityState !== 'hidden')
    document.addEventListener('visibilitychange', onChange)
    return () => document.removeEventListener('visibilitychange', onChange)
  }, [])
  return visible
}

/**
 * Appelle `callback` tout de suite puis toutes les `delay` ms (null = arrêté). En pause quand
 * la fenêtre n'est pas visible : l'appli passe l'essentiel de son temps cachée près de
 * l'horloge, inutile d'y recalculer la liste chaque seconde. Rappel immédiat au retour.
 */
export function useInterval (callback: () => void, delay: number | null) {
  const saved = useRef(callback)
  const visible = usePageVisible()
  useEffect(() => {
    saved.current = callback
  }, [callback])
  useEffect(() => {
    if (delay === null || !visible) return
    saved.current()
    const id = window.setInterval(() => saved.current(), delay)
    return () => window.clearInterval(id)
  }, [delay, visible])
}
