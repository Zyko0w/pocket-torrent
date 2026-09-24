import type { Job } from '../api'

/** Téléchargement de test : torrent en cours, à compléter selon le cas. */
export function makeJob (overrides: Partial<Job> = {}): Job {
  return {
    id: 't:4951e70c39f705039d09a8446c99ec955ea4e431',
    kind: 'torrent',
    name: 'Film de test 2026.mkv',
    link: null,
    status: 'downloading',
    hold: null,
    error: null,
    warning: null,
    progressBytes: 100,
    wantedBytes: 1000,
    uploadedBytes: 0,
    downSpeed: 50,
    upSpeed: 0,
    etaSecs: 18,
    peersLive: 3,
    fileCount: 1,
    selectedFiles: 1,
    category: null,
    priority: 'normal',
    dest: 'local',
    queuePosition: null,
    addedAt: 0,
    finishedAt: null,
    delivery: null,
    limitKbps: 0,
    inEngine: true,
    canShare: false,
    sourceFile: null,
    searchSecs: null,
    peersFound: null,
    ...overrides
  }
}
