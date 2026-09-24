// Appels au moteur Rust (commandes Tauri)
import { invoke } from '@tauri-apps/api/core'

export type JobKind = 'torrent' | 'http'
export type JobStatus =
  | 'metadata' | 'checking' | 'connecting' | 'queued' | 'downloading'
  | 'paused' | 'seeding' | 'uploading' | 'done' | 'error'
export type Hold = 'user' | 'queue' | 'game' | 'disk' | 'vpn' | 'seeded'
export type VpnMode = 'off' | 'adapter' | 'wireguard'
export type Priority = 'high' | 'normal' | 'low'
export type Dest = 'local' | 'storageTo'

export interface DeliveryLink { name: string; size: number; key: string; url: string; kind: 'file' | 'collection' }

export interface Delivery {
  state: 'uploading' | 'done' | 'error'
  sentBytes: number
  totalBytes: number
  links: DeliveryLink[]
  expiresAt: number | null
}

/** Statistiques cumulées (octets). `since` : début du décompte en ms, 0 si rien compté. */
export interface TrafficStats {
  downloaded: number
  uploaded: number
  since: number
  sessionDownloaded: number
  sessionUploaded: number
}

export interface Job {
  id: string
  kind: JobKind
  name: string | null
  link: string | null
  status: JobStatus
  hold: Hold | null
  error: string | null
  warning: string | null
  progressBytes: number
  wantedBytes: number
  uploadedBytes: number
  downSpeed: number
  upSpeed: number
  etaSecs: number | null
  peersLive: number
  fileCount: number
  selectedFiles: number
  category: string | null
  priority: Priority
  dest: Dest
  queuePosition: number | null
  addedAt: number
  finishedAt: number | null
  delivery: Delivery | null
  limitKbps: number
  inEngine: boolean
  /** Fichier .torrent local d'origine, s'il existe encore. */
  sourceFile: string | null
  /** Recherche des infos d'un magnet : secondes écoulées et pairs trouvés (sinon `null`). */
  searchSecs: number | null
  peersFound: number | null
}

export type GameModeSwitch = 'auto' | 'on' | 'off'
export type GameAction = 'limit' | 'pause'

export interface Overview {
  jobs: Job[]
  downSpeed: number
  upSpeed: number
  game: { active: boolean; process: string | null; mode: GameModeSwitch; action: GameAction }
  categories: { id: string; name: string }[]
  vpn: VpnState
  /** Arrêt du PC à la fin : option active, et heure prévue (ms) si l'arrêt est programmé. */
  power: { shutdownWhenDone: boolean; shutdownAt: number | null }
}

export interface VpnState {
  mode: VpnMode
  /** off : pas de VPN · protected : tout passe par le VPN · blocked : VPN coupé, tout est bloqué */
  state: 'off' | 'protected' | 'blocked'
  adapter: string | null
  ip: string | null
  /** Protégé mais serveur du tunnel muet. */
  warning: string | null
}

export interface NetworkAdapter {
  name: string
  description: string
  up: boolean
  ipv4: string | null
  ipv6: string | null
  likelyVpn: boolean
}

export type TunnelServiceState = 'notInstalled' | 'stopped' | 'running' | 'outdated'

export interface TunnelInfo {
  service: TunnelServiceState
  config: { addresses: string[]; endpoint: string | null; peers: number; ipv6: boolean } | null
  status: { up: boolean; addresses: string[]; endpoint: string | null; handshakeAgeSecs: number | null; rxBytes: number; txBytes: number } | null
  lastError: string | null
}

export interface TorrentFile {
  index: number
  path: string
  length: number
  included: boolean
  progressBytes: number
  /** Raison de se méfier de ce fichier (extension dangereuse, nom trompeur), ou `null` s'il est inoffensif. */
  danger: string | null
}

export interface Preview {
  infoHash: string
  name: string | null
  totalBytes: number
  files: TorrentFile[]
}

export interface PieceMap {
  totalPieces: number
  /** Empaqueté 8 pièces par octet, poids fort en premier ; 1 = pièce présente. */
  bits: number[]
}

export interface Peer {
  address: string
  client: string | null
  connection: string | null
  downloaded: number
  uploaded: number
  downSpeed: number
  upSpeed: number
}

export interface Tracker {
  url: string
  /** idle : torrent arrêté, pas d'annonce · skipped : jamais contacté (voir `error`) */
  state: 'idle' | 'connecting' | 'working' | 'failing' | 'skipped'
  peers: number
  error: string | null
  lastSuccessAt: number | null
  nextAnnounceAt: number | null
}

export interface Category { id: string; name: string; folder: string }

export interface Settings {
  downloadDir: string
  categories: Category[]
  limits: { downKbps: number; upKbps: number }
  queue: { maxActive: number; ignoreSlow: boolean; slowThresholdKbps: number }
  schedule: { enabled: boolean; startMinutes: number; endMinutes: number; days: number[]; downKbps: number; upKbps: number }
  gameMode: { mode: GameModeSwitch; processes: string[]; action: GameAction; downKbps: number; upKbps: number }
  notifications: { done: boolean; error: boolean }
  torrentPort: number
  utpEnabled: boolean
  maxPeersPerTorrent: number
  uploadSlots: number
  diskMarginMb: number
  seedWhenDone: boolean
  seeding: { ratioLimit: number; timeLimitMinutes: number }
  closeToTray: boolean
  preventSleep: boolean
  scanDownloads: boolean
  checkUpdates: boolean
  magnetPrevious: string | null
  torrentPrevious: string | null
  storageTo: { expiryDays: number; deleteAfterUpload: boolean }
  vpn: { mode: VpnMode; adapter: string; portForwarding: boolean }
  privacy: { encryption: 'off' | 'prefer' | 'require'; hideAnnounces: boolean; hardenedMode: boolean }
  security: { excludedExtensions: string[] }
  blocklist: { enabled: boolean; source: string }
}

export interface SettingsView {
  settings: Settings
  secrets: { storageToToken: boolean }
  startWithWindows: boolean
  magnetHandler: boolean
  listenPort: number | null
  externalPort: number | null
  defenderAvailable: boolean
  /** Réglages d'origine (boutons « valeurs par défaut »). */
  defaults: Settings
}

/** `null` : clé inchangée ; chaîne vide : clé effacée. */
export interface SecretsInput {
  storageToToken: string | null
}

export interface AddOptions {
  category: string | null
  priority: Priority
  dest: Dest
  limitKbps: number
  onlyFiles: number[] | null
}

export interface AddReport { added: number; failed: string[]; errors: string[] }

export const api = {
  overview: () => invoke<Overview>('overview'),
  preview: (link: string) => invoke<Preview>('preview_torrent', { link }),
  addLinks: (links: string[], options: AddOptions) => invoke<AddReport>('add_links', { links, options }),
  pause: (id: string) => invoke<void>('pause_job', { id }),
  resume: (id: string) => invoke<void>('resume_job', { id }),
  startNow: (id: string) => invoke<void>('start_job_now', { id }),
  pauseAll: () => invoke<number>('pause_all'),
  resumeAll: () => invoke<number>('resume_all'),
  setPriority: (id: string, priority: Priority) => invoke<void>('set_job_priority', { id, priority }),
  setLimit: (id: string, limitKbps: number) => invoke<void>('set_job_limit', { id, limitKbps }),
  remove: (id: string, deleteFiles: boolean, deleteOnline: boolean, deleteSource: boolean) =>
    invoke<void>('delete_job', { id, deleteFiles, deleteOnline, deleteSource }),
  clearFinished: () => invoke<number>('clear_finished'),
  files: (id: string) => invoke<TorrentFile[]>('job_files', { id }),
  setFiles: (id: string, files: number[]) => invoke<void>('set_job_files', { id, files }),
  moveFilePriority: (id: string, index: number, up: boolean) => invoke<void>('move_file_priority', { id, index, up }),
  recheck: (id: string) => invoke<void>('recheck_job', { id }),
  peers: (id: string) => invoke<Peer[]>('job_peers', { id }),
  trackers: (id: string) => invoke<Tracker[]>('job_trackers', { id }),
  pieceMap: (id: string) => invoke<PieceMap>('job_piece_map', { id }),
  magnet: (id: string) => invoke<string>('job_magnet', { id }),
  openFolder: (id: string) => invoke<void>('open_job_folder', { id }),
  takeLaunchLinks: () => invoke<string[]>('take_launch_links'),
  /** Redirection de port par le VPN (NAT-PMP) : port obtenu ou dernière erreur. */
  forwardingView: () => invoke<{ enabled: boolean; port: number | null; error: string | null }>('forwarding_view'),
  /** Premier lancement : proposer d'ouvrir les liens magnet et .torrent avec Pocket Torrent ? */
  shouldAskLinkHandler: () => invoke<boolean>('should_ask_link_handler'),
  /** `false` : mode dev, rien n'est changé dans Windows. */
  answerLinkHandler: (enable: boolean) => invoke<boolean>('answer_link_handler', { enable }),
  /** Page GitHub du projet (versions publiées), dans le navigateur. */
  openProjectPage: () => invoke<void>('open_project_page'),
  setGameMode: (mode: GameModeSwitch) => invoke<void>('set_game_mode', { mode }),
  getSettings: () => invoke<SettingsView>('get_settings'),
  saveSettings: (input: { settings: Settings; secrets: SecretsInput; startWithWindows: boolean; magnetHandler: boolean }) =>
    invoke<SettingsView>('save_settings', { input }),
  testNotification: () => invoke<void>('test_notification'),
  /** Renvoie une phrase sur l'état du compte ou le quota restant. */
  testStorageTo: () => invoke<string>('test_storage_to'),
  performanceSnapshot: () => invoke<PerfSnapshot>('performance_snapshot'),
  exportPerformanceSnapshot: (path: string) => invoke<void>('export_performance_snapshot', { path }),
  readLog: (maxLines: number) => invoke<string>('read_log', { maxLines }),
  exportLog: (path: string) => invoke<void>('export_log', { path }),
  openLogFolder: () => invoke<void>('open_log_folder'),
  playFile: (id: string, fileIndex: number) => invoke<void>('play_file', { id, fileIndex }),
  trafficStats: () => invoke<TrafficStats>('traffic_stats'),
  exportList: (path: string) => invoke<number>('export_list', { path }),
  importList: (path: string, includeFinished: boolean) =>
    invoke<{ added: number; skipped: number; errors: string[] }>('import_list', { path, includeFinished }),
  setShutdownWhenDone: (enabled: boolean) => invoke<void>('set_shutdown_when_done', { enabled }),
  cancelShutdown: () => invoke<void>('cancel_shutdown'),
  /** Arrête proprement le moteur (état sauvegardé, redirections de ports retirées) avant l'installeur. */
  prepareForUpdate: () => invoke<void>('prepare_for_update'),
  resetTrafficStats: () => invoke<TrafficStats>('reset_traffic_stats'),
  /** Envoie un .torrent déposé dans la fenêtre (en binaire) ; renvoie le chemin où il est rangé. */
  dropTorrentFile: (name: string, bytes: Uint8Array) =>
    invoke<string>('drop_torrent_file', bytes, { headers: { 'x-file-name': encodeURIComponent(name) } }),
  networkAdapters: () => invoke<NetworkAdapter[]>('network_adapters'),
  checkPublicIp: () => invoke<{ vpn: string | null; normal: string | null; error: string | null }>('check_public_ip'),
  tunnelView: () => invoke<TunnelInfo>('tunnel_view'),
  tunnelImport: (path: string) => invoke<NonNullable<TunnelInfo['config']>>('tunnel_import', { path }),
  tunnelForget: () => invoke<void>('tunnel_forget'),
  tunnelInstallService: () => invoke<void>('tunnel_install_service'),
  tunnelUninstallService: () => invoke<void>('tunnel_uninstall_service'),
  tunnelConnect: () => invoke<void>('tunnel_connect'),
  tunnelDisconnect: () => invoke<void>('tunnel_disconnect')
}

export interface PerfSnapshot {
  uptimeSecs: number
  cpuPercent: number
  memoryMb: number
  torrents: number
  peersLive: number
  downSpeed: number
  upSpeed: number
  listenPort: number | null
  diskFreeMb: number | null
}

export function errorMessage (err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}

export const isFinished = (job: Job) => job.status === 'done' || job.status === 'seeding'

/** Copie dans le presse-papiers (côté Rust : l'API du navigateur exige que la page ait le focus). */
export async function copyText (text: string) {
  await invoke<void>('copy_text', { text })
}
