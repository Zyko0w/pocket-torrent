// Découpage du texte de la zone d'ajout en liens et chemins de fichiers.

const URL_START = /^(magnet:|https?:\/\/)/i

/**
 * Un élément par ligne. Plusieurs liens collés sur une même ligne sont séparés aux espaces (un lien
 * magnet ou http n'en contient jamais), mais pas un chemin de fichier : « C:\…\Mon film 2026.torrent »
 * reste entier (un .torrent déposé ou ouvert depuis l'explorateur a souvent des espaces dans son nom).
 */
export function parseLinks (text: string): string[] {
  return text.split(/\r?\n/).flatMap((raw) => {
    // chemin copié avec « Copier en tant que chemin d'accès » : entre guillemets
    const line = raw.trim().replace(/^"(.*)"$/, '$1').trim()
    if (!line) return []
    return URL_START.test(line) ? line.split(/\s+/) : [line]
  })
}
