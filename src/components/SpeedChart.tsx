// Graphique du débit en direct : réception et envoi sur les 5 dernières minutes (un point par seconde).
// Deux séries aux couleurs de la palette validée (--series-down / --series-up, voir styles.css),
// légende avec la valeur actuelle, réticule et info-bulle au survol.
import { useCallback, useEffect, useRef, useState, type PointerEvent } from 'react'
import { api, type SpeedHistory } from '../api'
import { formatEta, formatSpeed } from '../format'
import { useInterval } from '../hooks'

const HEIGHT = 150
const PAD = { top: 10, right: 12, bottom: 22, left: 64 }
const SERIES = [
  { key: 'down', label: 'Réception', color: 'var(--series-down)' },
  { key: 'up', label: 'Envoi', color: 'var(--series-up)' }
] as const

/** Graduation haute « ronde » (1, 2 ou 5 × 10^n dans l'unité affichée), 20 Kio/s au minimum ;
 * « 1000 Ko/s » devient « 1 Mo/s ». */
export function niceMax (value: number): number {
  const floor = 16 * 1024
  const v = Math.max(value, floor)
  const unit = 1024 ** Math.floor(Math.log(v) / Math.log(1024))
  const scaled = v / unit
  const magnitude = 10 ** Math.floor(Math.log10(scaled))
  const step = [1, 2, 5, 10].find((m) => m * magnitude >= scaled)! * magnitude
  return step >= 1000 ? unit * 1024 : step * unit
}

function path (values: number[], width: number, max: number): string {
  const plotW = width - PAD.left - PAD.right
  const plotH = HEIGHT - PAD.top - PAD.bottom
  const step = plotW / Math.max(1, values.length - 1)
  return values
    .map((v, i) => `${i ? 'L' : 'M'}${(PAD.left + i * step).toFixed(1)},${(PAD.top + plotH - (v / max) * plotH).toFixed(1)}`)
    .join('')
}

/** `jobId` absent : débit de l'ensemble. `compact` : petite courbe décorative, sans axes ni survol. */
export function SpeedChart ({ jobId, compact = false }: { jobId?: string; compact?: boolean }) {
  const [history, setHistory] = useState<SpeedHistory | null>(null)
  const [width, setWidth] = useState(0)
  const [hover, setHover] = useState<number | null>(null)
  const box = useRef<HTMLDivElement>(null)

  const load = useCallback(() => { api.speedHistory(jobId ?? null).then(setHistory).catch(() => {}) }, [jobId])
  useInterval(load, 1000)

  useEffect(() => {
    const el = box.current
    if (!el) return
    const observer = new ResizeObserver(([entry]) => setWidth(Math.floor(entry.contentRect.width)))
    observer.observe(el)
    setWidth(Math.floor(el.getBoundingClientRect().width))
    return () => observer.disconnect()
  }, [])

  if (compact) {
    const max = history ? niceMax(Math.max(...history.down, ...history.up)) : 1
    const h = 34
    const line = (values: number[]) => values
      .map((v, i) => `${i ? 'L' : 'M'}${((i / (values.length - 1)) * width).toFixed(1)},${(h - 2 - (v / max) * (h - 4)).toFixed(1)}`)
      .join('')
    return (
      <div ref={box} className='speed-spark' aria-hidden='true'>
        {history && width > 0 && (
          <svg width={width} height={h}>
            <path d={line(history.up)} className='chart-line' stroke='var(--series-up)' />
            <path d={line(history.down)} className='chart-line' stroke='var(--series-down)' />
          </svg>
        )}
      </div>
    )
  }

  const points = history?.down.length ?? 0
  const max = history ? niceMax(Math.max(...history.down, ...history.up)) : 1
  const plotW = Math.max(1, width - PAD.left - PAD.right)
  const plotH = HEIGHT - PAD.top - PAD.bottom
  const xOf = (i: number) => PAD.left + (i / Math.max(1, points - 1)) * plotW
  const yOf = (v: number) => PAD.top + plotH - (v / max) * plotH
  const last = points - 1

  const onMove = (e: PointerEvent<SVGRectElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
    setHover(Math.round(ratio * last))
  }

  const at = hover ?? last
  const peak = history ? Math.max(...history.down) : 0
  const summary = history
    ? `Débit des 5 dernières minutes : réception actuelle ${formatSpeed(history.down[last])}, maximum ${formatSpeed(peak)} ; envoi actuel ${formatSpeed(history.up[last])}.`
    : 'Chargement du débit'

  return (
    <div className='speed-chart'>
      <div className='chart-legend'>
        {SERIES.map((s) => (
          <span key={s.key} className='chart-key'>
            <span className='chart-swatch' style={{ background: s.color }} aria-hidden='true' />
            {s.label} <b>{history ? formatSpeed(history[s.key][last]) : '…'}</b>
          </span>
        ))}
      </div>
      <div ref={box} className='chart-box'>
        {history && width > 0 && (
          <svg width={width} height={HEIGHT} role='img' aria-label={summary}>
            {[0, 0.5, 1].map((f) => (
              <g key={f}>
                <line className='chart-grid' x1={PAD.left} x2={width - PAD.right} y1={yOf(max * f)} y2={yOf(max * f)} />
                <text className='chart-axis' x={PAD.left - 8} y={yOf(max * f)} dy='0.32em' textAnchor='end'>{formatSpeed(max * f)}</text>
              </g>
            ))}
            {[[0, '−5 min'], [0.5, '−2 min 30'], [1, 'maintenant']].map(([f, label]) => (
              <text key={label as string} className='chart-axis' x={PAD.left + (f as number) * plotW} y={HEIGHT - 6} textAnchor={f === 0 ? 'start' : f === 1 ? 'end' : 'middle'}>{label}</text>
            ))}
            {SERIES.map((s) => <path key={s.key} d={path(history[s.key], width, max)} className='chart-line' stroke={s.color} />)}
            {hover !== null && <line className='chart-crosshair' x1={xOf(hover)} x2={xOf(hover)} y1={PAD.top} y2={PAD.top + plotH} />}
            {SERIES.map((s) => (
              <circle key={s.key} className='chart-dot' cx={xOf(at)} cy={yOf(history[s.key][at])} r={4} fill={s.color} />
            ))}
            <rect
              x={PAD.left} y={PAD.top} width={plotW} height={plotH} fill='transparent'
              onPointerMove={onMove} onPointerLeave={() => setHover(null)}
            />
          </svg>
        )}
        {history && hover !== null && (
          <div
            className='chart-tooltip' role='status'
            style={{ left: Math.min(xOf(hover) + 12, width - 170), top: PAD.top }}
          >
            <span className='muted'>{hover === last ? 'maintenant' : `il y a ${formatEta(last - hover)}`}</span>
            {SERIES.map((s) => (
              <span key={s.key} className='chart-key'>
                <span className='chart-swatch' style={{ background: s.color }} aria-hidden='true' />
                {s.label} <b>{formatSpeed(history[s.key][hover])}</b>
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
