import { describe, expect, it } from 'vitest'
import { parseLinks } from './links'

describe('parseLinks', () => {
  const dropped = String.raw`C:\Users\x\AppData\Roaming\app.pocket.torrent\deposes\Cocorico 2 TRUEFRENCH ULTRA HD 4K x265 2026 (1).torrent`

  it('garde entier un chemin de fichier plein d’espaces', () => {
    expect(parseLinks(dropped)).toEqual([dropped])
  })

  it('sépare les liens collés sur une même ligne, pas les chemins', () => {
    const text = `  magnet:?xt=urn:btih:aa  magnet:?xt=urn:btih:bb \n\n"${dropped}"\r\nhttps://x/y.torrent`
    expect(parseLinks(text)).toEqual(['magnet:?xt=urn:btih:aa', 'magnet:?xt=urn:btih:bb', dropped, 'https://x/y.torrent'])
  })

  it('ignore les lignes vides', () => {
    expect(parseLinks('   \n  ')).toEqual([])
  })
})
