import { useCallback, useEffect, useRef, useState } from 'react'
import { SpinnerGap } from '@phosphor-icons/react'
import { api, errorMessage, type PieceMap } from '../api'
import { useInterval } from '../hooks'

const HEIGHT = 20

function draw (canvas: HTMLCanvasElement, map: PieceMap) {
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  const width = canvas.width
  const have = (i: number) => ((map.bits[i >> 3] ?? 0) >> (7 - (i & 7))) & 1
  const styles = getComputedStyle(canvas)
  const haveColor = styles.getPropertyValue('--primary').trim() || '#7c3aed'
  const missingColor = styles.getPropertyValue('--surface-2').trim() || '#222226'
  ctx.clearRect(0, 0, width, HEIGHT)
  if (map.totalPieces === 0) return
  for (let x = 0; x < width; x++) {
    const start = Math.floor((x / width) * map.totalPieces)
    const end = Math.max(start + 1, Math.floor(((x + 1) / width) * map.totalPieces))
    let haveCount = 0
    for (let i = start; i < end; i++) haveCount += have(i)
    const ratio = haveCount / (end - start)
    ctx.fillStyle = ratio > 0 ? haveColor : missingColor
    ctx.globalAlpha = ratio > 0 ? Math.max(0.3, ratio) : 1
    ctx.fillRect(x, 0, 1, HEIGHT)
  }
  ctx.globalAlpha = 1
}

/** Carte des pièces d'un torrent (§5) : une bande colorée, une portion par groupe de pièces. */
export function PieceMapPanel ({ jobId }: { jobId: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [map, setMap] = useState<PieceMap | null>(null)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      const next = await api.pieceMap(jobId)
      setMap(next)
      setError('')
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [jobId])

  useInterval(load, 3000)
  useEffect(() => { if (map && canvasRef.current) draw(canvasRef.current, map) }, [map])

  if (error) return <div className='panel'><p className='field-error' role='alert'>Impossible de charger la carte des pièces : {error}</p></div>
  if (!map) return <div className='panel'><span className='meta-item muted'><SpinnerGap size={16} className='spin' /> Chargement…</span></div>

  return (
    <div className='panel'>
      <p className='muted' style={{ fontSize: 12.5, marginTop: 0 }}>{map.totalPieces} pièces</p>
      <canvas ref={canvasRef} width={700} height={HEIGHT} style={{ width: '100%', height: HEIGHT, borderRadius: 4, display: 'block' }} />
    </div>
  )
}
