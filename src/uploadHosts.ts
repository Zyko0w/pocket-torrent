// Services d'hébergement pour l'envoi en ligne. Pendant de `src-tauri/src/hosts` : pour en
// brancher un nouveau, l'ajouter à `UploadHost` et à `UPLOAD_HOSTS` (et son compte à SecretsInput).
// L'utilisateur peut aussi ajouter les siens (hébergeurs personnalisés, « custom:<id> »).

export type BuiltinHost = 'storageTo' | 'gofile' | 'vikingfile'
export type UploadHost = BuiltinHost | `custom:${string}`

/** Clés des comptes facultatifs, rangées dans le Gestionnaire d'identification de Windows. */
export type HostSecret = 'storageToToken' | 'gofileToken' | 'vikingfileUser'

export interface UploadHostInfo {
  id: UploadHost
  label: string
  /** Durée de vie et limite de téléchargements réglables à chaque envoi. */
  limits: boolean
  /** Ce que deviennent les fichiers en ligne, et les limites sans compte. */
  about: string
  /** Taille maximale d'un fichier envoyé (un téléchargement de plusieurs fichiers part en un seul zip). */
  maxFileBytes?: number
  account?: {
    secret: HostSecret
    label: string
    help: string
  }
}

/** Hébergeur ajouté par l'utilisateur (même principe que les fichiers .sxcu de ShareX). */
export interface CustomHost {
  id: string
  name: string
  /** Adresse d'envoi en https:// ; « {filename} » remplacé par le nom du fichier. */
  url: string
  /** Formulaire multipart (POST) ou fichier brut (PUT). */
  method: 'multipart' | 'put'
  fileField: string
  formFields: { name: string; value: string }[]
  /** Chemin du lien dans la réponse JSON (« data.url ») ; vide : la réponse est le lien. */
  linkPath: string
  /** « {value} » remplacé par la valeur lue ; vide : la valeur telle quelle. */
  linkTemplate: string
}

export const UPLOAD_HOSTS: Record<BuiltinHost, UploadHostInfo> = {
  storageTo: {
    id: 'storageTo',
    label: 'storage.to',
    limits: true,
    about: 'Liens valables 1 à 7 jours, effacement possible après un nombre de téléchargements. Sans compte : 50 fichiers et 100 Go par 24 h, 25 Go par fichier.',
    maxFileBytes: 25 * 1024 ** 3,
    account: {
      secret: 'storageToToken',
      label: 'Jeton de compte storage.to (facultatif)',
      help: 'Un compte gratuit lève les limites des envois sans compte (jeton à créer dans les réglages du compte storage.to).'
    }
  },
  gofile: {
    id: 'gofile',
    label: 'GoFile',
    limits: false,
    about: 'Un dossier et un seul lien par téléchargement. GoFile efface les fichiers après une période sans téléchargement.',
    account: {
      secret: 'gofileToken',
      label: 'Jeton de compte GoFile (facultatif)',
      help: 'Sans jeton, un compte invité est créé à chaque envoi (Pocket Torrent garde de quoi supprimer les fichiers). Jeton : page « Mon profil » de GoFile.'
    }
  },
  vikingfile: {
    id: 'vikingfile',
    label: 'VikingFile',
    limits: false,
    about: 'Un lien par fichier. VikingFile efface les fichiers après une période sans téléchargement.',
    account: {
      secret: 'vikingfileUser',
      label: 'Identifiant de compte VikingFile (facultatif)',
      help: 'Le « user hash » du compte. Sans lui, les fichiers envoyés ne peuvent pas être supprimés depuis Pocket Torrent.'
    }
  }
}

export const customHostId = (id: string): UploadHost => `custom:${id}`

/** Description d'un service, intégré ou personnalisé (storage.to s'il n'existe plus). */
export function hostInfo (host: UploadHost | undefined, customHosts: CustomHost[] = []): UploadHostInfo {
  if (host?.startsWith('custom:')) {
    const custom = customHosts.find((h) => customHostId(h.id) === host)
    if (custom) {
      return {
        id: host,
        label: custom.name || 'Hébergeur sans nom',
        limits: false,
        about: 'Hébergeur ajouté par toi : un lien par fichier. Durée de conservation selon le service ; pas de suppression depuis Pocket Torrent.'
      }
    }
  }
  return UPLOAD_HOSTS[host as BuiltinHost] ?? UPLOAD_HOSTS.storageTo
}

/** Choix du service : les intégrés, puis ceux de l'utilisateur. */
export function hostOptions (customHosts: CustomHost[] = []) {
  return [
    ...Object.values(UPLOAD_HOSTS).map((h) => ({ value: h.id, label: h.label })),
    ...customHosts.map((h) => ({ value: customHostId(h.id), label: h.name || 'Hébergeur sans nom' }))
  ]
}

/** Nom affiché du service d'un envoi (storage.to pour un envoi d'une version précédente). */
export function hostLabel (host: UploadHost | undefined, hostName?: string): string {
  if (hostName) return hostName
  if (host?.startsWith('custom:')) return 'l’hébergeur personnalisé'
  return hostInfo(host).label
}

export function newCustomHost (patch: Partial<CustomHost> = {}): CustomHost {
  const id = Math.random().toString(36).slice(2, 10).padEnd(8, '0')
  return { id, name: '', url: '', method: 'multipart', fileField: 'file', formFields: [], linkPath: '', linkTemplate: '', ...patch }
}

/** Modèles d'hébergeurs sans compte, prêts à l'emploi. */
export const CUSTOM_PRESETS: { label: string; host: Partial<CustomHost> }[] = [
  {
    label: 'catbox.moe (200 Mo par fichier, permanent)',
    host: { name: 'catbox.moe', url: 'https://catbox.moe/user/api.php', fileField: 'fileToUpload', formFields: [{ name: 'reqtype', value: 'fileupload' }] }
  },
  {
    label: 'litterbox (1 Go par fichier, 72 h)',
    host: { name: 'litterbox', url: 'https://litterbox.catbox.moe/resources/internals/api.php', fileField: 'fileToUpload', formFields: [{ name: 'reqtype', value: 'fileupload' }, { name: 'time', value: '72h' }] }
  },
  {
    label: '0x0.st (512 Mo par fichier, 30 jours à 1 an)',
    host: { name: '0x0.st', url: 'https://0x0.st', fileField: 'file' }
  }
]

/**
 * Hébergeur décrit par un fichier .sxcu de ShareX. Les en-têtes (souvent une clé d'API) sont
 * renvoyés à part : ils sont rangés comme un secret.
 */
export function fromSxcu (text: string): { host: Partial<CustomHost>; headers: string } {
  let config: Record<string, unknown>
  try {
    config = JSON.parse(text.replace(/^﻿/, ''))
  } catch {
    throw new Error('Fichier .sxcu illisible (JSON attendu).')
  }
  const str = (key: string) => (typeof config[key] === 'string' ? (config[key] as string) : '')
  const record = (key: string) => {
    const value = config[key]
    return value && typeof value === 'object' && !Array.isArray(value) ? Object.entries(value as Record<string, unknown>).map(([k, v]) => [k, String(v)]) : []
  }
  const url = str('RequestURL').replace(/\$filename\$/gi, '{filename}')
  if (!url) throw new Error('Fichier .sxcu sans adresse d’envoi (RequestURL).')
  const method = (str('RequestMethod') || 'POST').toUpperCase()
  const body = str('Body') || (config.FileFormName ? 'MultipartFormData' : '')
  let kind: CustomHost['method']
  if (body === 'MultipartFormData' && method === 'POST') kind = 'multipart'
  else if (body === 'Binary' && method === 'PUT') kind = 'put'
  else throw new Error(`Envoi « ${body || 'sans corps'} » en ${method} non pris en charge (formulaire en POST ou fichier brut en PUT seulement).`)

  // lien : « {json:data.url} », « $json:data.url$ », « {response} », dans un modèle ou seul
  const link = str('URL').trim()
  let linkPath = ''
  let linkTemplate = ''
  if (link && !/^(\{response\}|\$response\$)$/i.test(link)) {
    const match = link.match(/\{json:([^}]+)\}|\$json:([^$]+)\$/i)
    if (!match) throw new Error('Lien du .sxcu non pris en charge (seuls {json:…} et {response} le sont).')
    linkPath = (match[1] ?? match[2]).replace(/\[(\d+)\]/g, '.$1').replace(/^\./, '')
    linkTemplate = link === match[0] ? '' : link.replace(match[0], '{value}')
  }
  return {
    host: {
      name: str('Name').slice(0, 40),
      url,
      method: kind,
      fileField: str('FileFormName') || 'file',
      formFields: record('Arguments').map(([name, value]) => ({ name, value })),
      linkPath,
      linkTemplate
    },
    headers: record('Headers').map(([k, v]) => `${k}: ${v}`).join('\n')
  }
}
