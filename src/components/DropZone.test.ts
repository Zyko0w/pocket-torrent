import { describe, expect, it } from 'vitest'
import { linksInFiles } from './DropZone'

const magnet = 'magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&dn=Film&tr=udp%3A%2F%2Ft.org%3A1337'

describe('linksInFiles', () => {
  it('lit le lien d’un raccourci .url venu du navigateur', async () => {
    const shortcut = new File([`[InternetShortcut]\r\nURL=${magnet}\r\n`], 'Film.url')
    expect(await linksInFiles([shortcut])).toEqual({ links: [magnet], rejected: 0 })
  })

  it('refuse un fichier sans lien', async () => {
    const image = new File([new Uint8Array([0xff, 0xd8, 0xff])], 'photo.jpg')
    expect(await linksInFiles([image])).toEqual({ links: [], rejected: 1 })
  })
})
