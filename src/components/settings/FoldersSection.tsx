// Dossiers de téléchargement, catégories et dossiers surveillés.
import { FolderOpen, Plus, Trash } from '@phosphor-icons/react'
import type { Settings } from '../../api'
import { Section, pickFolder, type SectionProps } from './fields'

export function FoldersSection ({ settings, update }: SectionProps) {
  const setCategory = (index: number, patch: Partial<Settings['categories'][number]>) =>
    update({ categories: settings.categories.map((c, i) => (i === index ? { ...c, ...patch } : c)) })

  return (
    <Section id='s-folders' title='Dossiers' intro='Où arrivent les téléchargements, et dossiers par catégorie.'>
      <div className='field'>
        <label htmlFor='download-dir'>Dossier de téléchargement</label>
        <div className='input-row'>
          <input id='download-dir' className='input' value={settings.downloadDir} onChange={(e) => update({ downloadDir: e.target.value })} />
          <button type='button' className='btn btn-ghost' onClick={async () => { const f = await pickFolder(settings.downloadDir); if (f) update({ downloadDir: f }) }}>
            <FolderOpen size={16} /> Parcourir
          </button>
        </div>
        <p className='help'>Les téléchargements déjà commencés restent dans leur dossier.</p>
      </div>
      <div className='field'>
        <span className='label'>Catégories</span>
        {settings.categories.length === 0 && <p className='help' style={{ marginTop: 0 }}>Aucune catégorie : tout va dans le dossier ci-dessus.</p>}
        <ul className='category-list'>
          {settings.categories.map((category, index) => (
            <li key={index} className='category-row'>
              <input className='input' aria-label={`Nom de la catégorie ${index + 1}`} placeholder='Nom (ex. Films)' value={category.name} onChange={(e) => setCategory(index, { name: e.target.value })} />
              <input className='input' aria-label={`Dossier de la catégorie ${index + 1}`} placeholder='Dossier' value={category.folder} onChange={(e) => setCategory(index, { folder: e.target.value })} />
              <button type='button' className='icon-btn' title='Choisir le dossier' aria-label='Choisir le dossier' onClick={async () => { const f = await pickFolder(category.folder); if (f) setCategory(index, { folder: f }) }}>
                <FolderOpen size={18} />
              </button>
              <button type='button' className='icon-btn danger' title='Retirer la catégorie' aria-label='Retirer la catégorie' onClick={() => update({ categories: settings.categories.filter((_, i) => i !== index) })}>
                <Trash size={18} />
              </button>
            </li>
          ))}
        </ul>
        <button type='button' className='btn btn-ghost btn-sm' onClick={() => update({ categories: [...settings.categories, { id: '', name: '', folder: '' }] })}>
          <Plus size={14} /> Ajouter une catégorie
        </button>
      </div>
    </Section>
  )
}

export function WatchedFoldersSection ({ settings, update }: SectionProps) {
  const setWatched = (index: number, patch: Partial<Settings['watchedFolders'][number]>) =>
    update({ watchedFolders: settings.watchedFolders.map((f, i) => (i === index ? { ...f, ...patch } : f)) })

  return (
    <Section id='s-watched' title='Dossiers surveillés' intro='Tout fichier .torrent déposé ici est ajouté automatiquement, puis rangé dans un sous-dossier « .traités ».'>
      {settings.watchedFolders.length === 0 && <p className='help' style={{ marginTop: 0 }}>Aucun dossier surveillé.</p>}
      <ul className='category-list'>
        {settings.watchedFolders.map((folder, index) => (
          <li key={index} className='category-row'>
            <input
              className='input' aria-label={`Dossier surveillé ${index + 1}`} placeholder='Dossier'
              value={folder.path}
              onChange={(e) => setWatched(index, { path: e.target.value })}
            />
            <select
              aria-label={`Catégorie du dossier surveillé ${index + 1}`}
              value={folder.category ?? ''}
              onChange={(e) => setWatched(index, { category: e.target.value || null })}
            >
              <option value=''>Dossier par défaut</option>
              {settings.categories.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
            </select>
            <button
              type='button' className='icon-btn' title='Choisir le dossier' aria-label='Choisir le dossier'
              onClick={async () => { const f = await pickFolder(folder.path); if (f) setWatched(index, { path: f }) }}
            >
              <FolderOpen size={18} />
            </button>
            <button
              type='button' className='icon-btn danger' title='Retirer' aria-label='Retirer'
              onClick={() => update({ watchedFolders: settings.watchedFolders.filter((_, i) => i !== index) })}
            >
              <Trash size={18} />
            </button>
          </li>
        ))}
      </ul>
      <button type='button' className='btn btn-ghost btn-sm' onClick={() => update({ watchedFolders: [...settings.watchedFolders, { path: '', category: null }] })}>
        <Plus size={14} /> Ajouter un dossier
      </button>
    </Section>
  )
}
