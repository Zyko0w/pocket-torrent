//! Déplacement des fichiers terminés du dossier « en cours » vers leur dossier final.
//!
//! Tout ou rien : si un fichier ne peut pas être déplacé (fichier du même nom déjà à l'arrivée,
//! disque plein, droits), ceux déjà déplacés reviennent à leur place et l'erreur est renvoyée.
//! Rien n'est jamais écrasé. Sur le même disque, un simple renommage (instantané) ; d'un disque à
//! l'autre, copie puis suppression de l'original une fois la copie complète.

use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

/// Déplace `from` vers `to` (dossiers créés au besoin). Erreur si `to` existe déjà.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} existe déjà", to.display())));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // le moteur vient de fermer le fichier : Windows peut le garder verrouillé un court instant
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => return copy_then_remove(from, to),
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => return Err(err),
        }
    }
}

/// D'un disque à l'autre : copie vers un fichier temporaire, mis sur le disque puis renommé ;
/// l'original n'est supprimé qu'après. Une coupure en route laisse l'original intact.
fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".deplacement");
    let tmp = PathBuf::from(tmp);
    let copied = (|| {
        std::fs::copy(from, &tmp)?;
        // écriture demandée : Windows refuse de vider sur le disque un fichier ouvert en lecture seule
        std::fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, to)
    })();
    if let Err(err) = copied {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    std::fs::remove_file(from)
}

fn plain(rel: &Path) -> bool {
    rel.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Déplace les fichiers `rels` (chemins relatifs) de `old` vers `new`, tout ou rien. Les fichiers
/// absents de `old` (non choisis, jamais créés) sont ignorés.
pub fn move_tree(old: &Path, new: &Path, rels: &[PathBuf]) -> anyhow::Result<()> {
    if rels.iter().any(|rel| !plain(rel)) {
        bail!("chemin de fichier invalide dans le torrent");
    }
    let present: Vec<&PathBuf> = rels.iter().filter(|rel| old.join(rel).is_file()).collect();
    // conflit vérifié avant de toucher à quoi que ce soit
    if let Some(taken) = present.iter().find(|rel| new.join(rel).exists()) {
        bail!("« {} » existe déjà dans le dossier final", new.join(taken).display());
    }
    let mut moved: Vec<&PathBuf> = Vec::new();
    for rel in &present {
        if let Err(err) = move_file(&old.join(rel), &new.join(rel)) {
            for back in moved.iter().rev() {
                let _ = move_file(&new.join(back), &old.join(back));
            }
            return Err(err).with_context(|| format!("déplacement de « {} » impossible", rel.display()));
        }
        moved.push(rel);
    }
    Ok(())
}

/// Retire les dossiers devenus vides sous `old` après un déplacement (jamais `keep` lui-même :
/// le dossier « en cours » reste).
pub fn remove_empty_dirs(old: &Path, rels: &[PathBuf], keep: &Path) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for rel in rels.iter().filter(|rel| plain(rel)) {
        let mut dir = old.join(rel);
        while dir.pop() && dir.starts_with(old) {
            if !dirs.contains(&dir) {
                dirs.push(dir.clone());
            }
            if dir == old {
                break;
            }
        }
    }
    // les plus profonds d'abord
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        if dir != keep {
            let _ = std::fs::remove_dir(&dir); // échoue s'il n'est pas vide : voulu
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn write(&self, rel: &str, content: &[u8]) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rels(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn finished_files_move_and_empty_folders_go() {
        let sandbox = Sandbox::new();
        sandbox.write("en-cours/Série S01/e01.mkv", b"un");
        sandbox.write("en-cours/Série S01/sous-titres/e01.srt", b"deux");
        let old = sandbox.0.join("en-cours").join("Série S01");
        let new = sandbox.0.join("Séries").join("Série S01");
        let files = rels(&["e01.mkv", "sous-titres/e01.srt", "non-choisi.mkv"]);
        move_tree(&old, &new, &files).unwrap();
        assert_eq!(std::fs::read(new.join("e01.mkv")).unwrap(), b"un");
        assert_eq!(std::fs::read(new.join("sous-titres/e01.srt")).unwrap(), b"deux");
        remove_empty_dirs(&old, &files, &sandbox.0.join("en-cours"));
        assert!(!old.exists(), "dossier du torrent vidé puis retiré");
        assert!(sandbox.0.join("en-cours").exists(), "le dossier « en cours » reste");
    }

    #[test]
    fn nothing_is_overwritten_and_nothing_moves_on_conflict() {
        let sandbox = Sandbox::new();
        sandbox.write("en-cours/film/a.mkv", b"nouveau a");
        sandbox.write("en-cours/film/b.mkv", b"nouveau b");
        sandbox.write("final/film/b.mkv", b"ancien b, a l'utilisateur");
        let (old, new) = (sandbox.0.join("en-cours/film"), sandbox.0.join("final/film"));
        let err = move_tree(&old, &new, &rels(&["a.mkv", "b.mkv"])).unwrap_err();
        assert!(format!("{err:#}").contains("existe déjà"), "{err:#}");
        assert_eq!(std::fs::read(new.join("b.mkv")).unwrap(), b"ancien b, a l'utilisateur");
        assert!(old.join("a.mkv").exists() && !new.join("a.mkv").exists(), "rien de déplacé");
    }

    #[test]
    fn a_failure_midway_puts_everything_back() {
        let sandbox = Sandbox::new();
        sandbox.write("en-cours/film/a.mkv", b"a");
        sandbox.write("en-cours/film/sous/b.mkv", b"b");
        let (old, new) = (sandbox.0.join("en-cours/film"), sandbox.0.join("final/film"));
        // à l'arrivée, un fichier « sous » là où il faudrait un dossier : le 2e déplacement
        // échoue après le 1er
        sandbox.write("final/film/sous", b"un fichier, pas un dossier");
        let err = move_tree(&old, &new, &rels(&["a.mkv", "sous/b.mkv"]));
        assert!(err.is_err());
        assert!(old.join("a.mkv").exists(), "a remis à sa place");
        assert!(!new.join("a.mkv").exists());
        assert!(old.join("sous/b.mkv").exists(), "b jamais parti");
    }

    #[test]
    fn copy_between_disks_keeps_the_original_until_done() {
        let sandbox = Sandbox::new();
        sandbox.write("a.bin", &vec![7u8; 100_000]);
        copy_then_remove(&sandbox.0.join("a.bin"), &sandbox.0.join("b.bin")).unwrap();
        assert!(!sandbox.0.join("a.bin").exists());
        assert_eq!(std::fs::read(sandbox.0.join("b.bin")).unwrap().len(), 100_000);
        assert!(!sandbox.0.join("b.bin.deplacement").exists(), "aucun fichier temporaire laissé");
        // source absente : erreur, rien de créé
        assert!(copy_then_remove(&sandbox.0.join("absent"), &sandbox.0.join("c.bin")).is_err());
        assert!(!sandbox.0.join("c.bin").exists() && !sandbox.0.join("c.bin.deplacement").exists());
    }

    #[test]
    fn hostile_paths_are_refused() {
        let sandbox = Sandbox::new();
        assert!(move_tree(&sandbox.0, &sandbox.0.join("x"), &rels(&["../evil"])).is_err());
    }
}
