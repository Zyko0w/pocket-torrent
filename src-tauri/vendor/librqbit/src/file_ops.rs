use std::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use anyhow::{Context, bail};
use buffers::{ByteBuf, ByteBufOwned};
use librqbit_core::{
    lengths::{ChunkInfo, ValidPieceIndex},
    torrent_metainfo::ValidatedTorrentMetaV1Info,
};
use peer_binary_protocol::{DoubleBufHelper, Piece};
use sha1w::{ISha1, Sha1};
use tracing::{debug, trace, warn};

use crate::{
    file_info::FileInfo,
    storage::TorrentStorage,
    type_aliases::{BF, FileInfos, PeerHandle},
};

pub fn update_hash_from_file<Sha1: ISha1>(
    file_id: usize,
    file_info: &FileInfo,
    mut pos: u64,
    files: &dyn TorrentStorage,
    hash: &mut Sha1,
    buf: &mut [u8],
    mut bytes_to_read: usize,
) -> anyhow::Result<()> {
    let mut read = 0;
    while bytes_to_read > 0 {
        let chunk = std::cmp::min(buf.len(), bytes_to_read);
        if file_info.attrs.padding {
            buf[..chunk].fill(0);
        } else {
            files
                .pread_exact(file_id, pos, &mut buf[..chunk])
                .with_context(|| {
                    format!("failed reading chunk of size {chunk}, read so far {read}")
                })?;
        }
        bytes_to_read -= chunk;
        read += chunk;
        pos += chunk as u64;
        hash.update(&buf[..chunk]);
    }
    Ok(())
}

/// Taille d'un bloc lu pendant la vérification initiale, et nombre de blocs en attente de calcul
/// au plus (4 Mio en vol).
const PIPELINE_BLOCK: usize = 256 * 1024;
const PIPELINE_BLOCKS: usize = 16;

enum CheckMessage {
    /// Bloc lu (tampon, octets utiles) : fait partie de la pièce en cours.
    Data(Vec<u8>, usize),
    /// Fin d'une pièce ; `broken` si un de ses fichiers n'a pas pu être lu.
    PieceEnd {
        piece_index: u32,
        len: u64,
        broken: bool,
    },
}

pub(crate) struct FileOps<'a> {
    torrent: &'a ValidatedTorrentMetaV1Info<ByteBufOwned>,
    files: &'a dyn TorrentStorage,
    file_infos: &'a FileInfos,
    phantom_data: PhantomData<Sha1>,
}

impl<'a> FileOps<'a> {
    pub fn new(
        torrent: &'a ValidatedTorrentMetaV1Info<ByteBufOwned>,
        files: &'a dyn TorrentStorage,
        file_infos: &'a FileInfos,
    ) -> Self {
        Self {
            torrent,
            files,
            file_infos,
            phantom_data: PhantomData,
        }
    }

    // Returns the bitvector with pieces we have.
    //
    // Pocket Torrent : lecture et calcul des empreintes en parallèle. Un fil lit les fichiers dans
    // l'ordre (toujours séquentiel : pas de va-et-vient sur un disque dur) et passe les blocs lus
    // à celui-ci, qui calcule les empreintes pendant que la suite est lue. Avant, un seul fil
    // faisait les deux à tour de rôle : le temps de lecture et le temps de calcul s'ajoutaient.
    // Mémoire bornée : `PIPELINE_BLOCKS` blocs de `PIPELINE_BLOCK` octets en vol.
    pub fn initial_check(
        &self,
        progress: &AtomicU64,
        pause_requested: &AtomicBool,
    ) -> anyhow::Result<BF> {
        let mut have_pieces =
            BF::from_boxed_slice(vec![0u8; self.torrent.lengths().piece_bitfield_bytes()].into());
        let (tx, rx) = std::sync::mpsc::sync_channel::<CheckMessage>(PIPELINE_BLOCKS);
        let (free_tx, free_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        std::thread::scope(|scope| {
            let reader = std::thread::Builder::new()
                .name("initial-check-reader".into())
                .spawn_scoped(scope, move || {
                    self.read_for_check(tx, free_rx, pause_requested)
                })
                .context("error spawning the initial check reader thread")?;

            let hashed = (|| -> anyhow::Result<()> {
                let mut computed_hash = Sha1::new();
                for message in rx {
                    match message {
                        CheckMessage::Data(buf, len) => {
                            computed_hash.update(&buf[..len]);
                            // le lecteur a peut-être déjà fini : bloc simplement libéré
                            let _ = free_tx.send(buf);
                        }
                        CheckMessage::PieceEnd {
                            piece_index,
                            len,
                            broken,
                        } => {
                            progress.fetch_add(len, Ordering::Relaxed);
                            let hash = std::mem::replace(&mut computed_hash, Sha1::new());
                            if broken {
                                trace!("piece {} had errors, marking as needed", piece_index);
                                continue;
                            }
                            if self
                                .torrent
                                .info()
                                .compare_hash(piece_index, hash.finish())
                                .context("bug: either torrent info broken or we have a bug - piece index invalid")?
                            {
                                have_pieces.set(piece_index as usize, true);
                            }
                        }
                    }
                }
                Ok(())
            })();
            // en cas d'erreur ici, `rx` est déjà libéré : le lecteur s'arrête à son prochain envoi
            let read = reader
                .join()
                .map_err(|_| anyhow::anyhow!("initial check reader thread panicked"))?;
            hashed?;
            read
        })?;

        Ok(have_pieces)
    }

    /// Côté lecture de `initial_check` : chaque pièce, fichier par fichier, envoyée par blocs,
    /// suivie de `PieceEnd`. Un fichier illisible marque ses pièces comme à télécharger.
    fn read_for_check(
        &self,
        tx: std::sync::mpsc::SyncSender<CheckMessage>,
        free: std::sync::mpsc::Receiver<Vec<u8>>,
        pause_requested: &AtomicBool,
    ) -> anyhow::Result<()> {
        #[derive(Debug)]
        struct CurrentFile<'a> {
            index: usize,
            fi: &'a FileInfo,
            processed_bytes: u64,
            is_broken: bool,
        }
        impl CurrentFile<'_> {
            fn remaining(&self) -> u64 {
                self.fi.len - self.processed_bytes
            }
            fn mark_processed_bytes(&mut self, bytes: u64) {
                self.processed_bytes += bytes
            }
        }
        let mut file_iterator = self
            .file_infos
            .iter()
            .enumerate()
            .map(|(idx, fi)| CurrentFile {
                index: idx,
                fi,
                processed_bytes: 0,
                is_broken: false,
            });

        let mut current_file = file_iterator.next().context("empty input file list")?;
        // le calcul des empreintes a abandonné (erreur) : inutile de continuer
        let stopped = || anyhow::anyhow!("initial check hasher stopped");

        for piece_info in self.torrent.lengths().iter_piece_infos() {
            if pause_requested.load(Ordering::Relaxed) {
                bail!("initial check paused");
            }

            let mut piece_remaining = piece_info.len as usize;
            let mut some_files_broken = false;

            while piece_remaining > 0 {
                let mut to_read_in_file: usize =
                    std::cmp::min(current_file.remaining(), piece_remaining as u64).try_into()?;

                // Keep changing the current file to next until we find a file that has greater than 0 length.
                while to_read_in_file == 0 {
                    current_file = file_iterator.next().context("broken torrent metadata")?;

                    to_read_in_file =
                        std::cmp::min(current_file.remaining(), piece_remaining as u64)
                            .try_into()?;
                }

                let mut pos = current_file.processed_bytes;
                piece_remaining -= to_read_in_file;
                current_file.mark_processed_bytes(to_read_in_file as u64);

                if current_file.is_broken {
                    // no need to read.
                    some_files_broken = true;
                    continue;
                }

                let mut read = 0;
                while to_read_in_file > 0 {
                    let chunk = std::cmp::min(PIPELINE_BLOCK, to_read_in_file);
                    let mut buf = free
                        .try_recv()
                        .unwrap_or_else(|_| vec![0u8; PIPELINE_BLOCK]);
                    if current_file.fi.attrs.padding {
                        buf[..chunk].fill(0);
                    } else if let Err(err) =
                        self.files
                            .pread_exact(current_file.index, pos, &mut buf[..chunk])
                    {
                        debug!(
                            "error reading from file {} ({:?}) at {}: {:#}",
                            current_file.index,
                            current_file.fi.relative_filename,
                            pos,
                            err.context(format!(
                                "failed reading chunk of size {chunk}, read so far {read}"
                            ))
                        );
                        current_file.is_broken = true;
                        some_files_broken = true;
                        break;
                    }
                    tx.send(CheckMessage::Data(buf, chunk))
                        .map_err(|_| stopped())?;
                    to_read_in_file -= chunk;
                    read += chunk;
                    pos += chunk as u64;
                }
            }

            tx.send(CheckMessage::PieceEnd {
                piece_index: piece_info.piece_index.get(),
                len: piece_info.len as u64,
                broken: some_files_broken,
            })
            .map_err(|_| stopped())?;
        }
        Ok(())
    }

    pub fn check_piece(&self, piece_index: ValidPieceIndex) -> anyhow::Result<bool> {
        if cfg!(feature = "_disable_disk_write_net_benchmark") {
            return Ok(true);
        }

        let mut h = Sha1::new();
        let piece_length = self.torrent.lengths().piece_length(piece_index);
        let mut absolute_offset = self.torrent.lengths().piece_offset(piece_index);
        let mut buf = vec![0u8; std::cmp::min(65536, piece_length as usize)];

        let mut piece_remaining_bytes = piece_length as usize;

        for (file_idx, fi) in self.file_infos.iter().enumerate() {
            let file_len = fi.len;
            if absolute_offset > file_len {
                absolute_offset -= file_len;
                continue;
            }
            let file_remaining_len = file_len - absolute_offset;

            let to_read_in_file: usize =
                std::cmp::min(file_remaining_len, piece_remaining_bytes as u64).try_into()?;
            trace!(
                "piece={}, file_idx={}, seeking to {}",
                piece_index, file_idx, absolute_offset,
            );
            update_hash_from_file(
                file_idx,
                fi,
                absolute_offset,
                self.files,
                &mut h,
                &mut buf,
                to_read_in_file,
            )
            .with_context(|| {
                format!(
                    "error reading {to_read_in_file} bytes, file_id: {file_idx} (\"{:?}\")",
                    fi.relative_filename
                )
            })?;

            piece_remaining_bytes -= to_read_in_file;

            if piece_remaining_bytes == 0 {
                break;
            }

            absolute_offset = 0;
        }

        match self
            .torrent
            .info()
            .compare_hash(piece_index.get(), h.finish())
        {
            Some(true) => {
                trace!("piece={} hash matches", piece_index);
                Ok(true)
            }
            Some(false) => {
                let piece_length = self.torrent.lengths().piece_length(piece_index);
                let absolute_offset = self.torrent.lengths().piece_offset(piece_index);
                warn!(
                    piece_length,
                    absolute_offset, "the piece={} hash does not match", piece_index
                );
                Ok(false)
            }
            None => {
                // this is probably a bug?
                warn!("compare_hash() did not find the piece");
                anyhow::bail!("compare_hash() did not find the piece");
            }
        }
    }

    pub fn read_chunk(
        &self,
        who_sent: PeerHandle,
        chunk_info: &ChunkInfo,
        result_buf: &mut [u8],
    ) -> anyhow::Result<()> {
        if result_buf.len() < chunk_info.size as usize {
            anyhow::bail!("read_chunk(): not enough capacity in the provided buffer")
        }
        let mut absolute_offset = self.torrent.lengths().chunk_absolute_offset(chunk_info);
        let mut buf = result_buf;

        for (file_idx, file_info) in self.file_infos.iter().enumerate() {
            let file_len = file_info.len;
            if absolute_offset > file_len {
                absolute_offset -= file_len;
                continue;
            }
            let file_remaining_len = file_len - absolute_offset;
            let to_read_in_file = std::cmp::min(file_remaining_len, buf.len() as u64).try_into()?;

            trace!(
                "piece={}, handle={}, file_idx={}, seeking to {}. To read chunk: {:?}",
                chunk_info.piece_index, who_sent, file_idx, absolute_offset, &chunk_info
            );
            if file_info.attrs.padding {
                buf[..to_read_in_file].fill(0);
            } else {
                self.files
                    .pread_exact(file_idx, absolute_offset, &mut buf[..to_read_in_file])
                    .with_context(|| {
                        format!("error reading {file_idx} bytes, file_id: {to_read_in_file}")
                    })?;
            }

            buf = &mut buf[to_read_in_file..];

            if buf.is_empty() {
                break;
            }

            absolute_offset = 0;
        }

        Ok(())
    }

    pub fn write_chunk(
        &self,
        who_sent: PeerHandle,
        data: &Piece<ByteBuf<'a>>,
        chunk_info: &ChunkInfo,
    ) -> anyhow::Result<()> {
        let mut absolute_offset = self.torrent.lengths().chunk_absolute_offset(chunk_info);
        let mut data = DoubleBufHelper::new(data.data().0, data.data().1);

        for (file_idx, file_info) in self.file_infos.iter().enumerate() {
            let file_len = file_info.len;
            if absolute_offset > file_len {
                absolute_offset -= file_len;
                continue;
            }

            let remaining_len = file_len - absolute_offset;
            let to_write = std::cmp::min(data.len() as u64, remaining_len).try_into()?;

            trace!(
                "piece={}, chunk={:?}, handle={}, begin={}, file={}, writing {} bytes at {}",
                chunk_info.piece_index,
                chunk_info,
                who_sent,
                chunk_info.offset,
                file_idx,
                to_write,
                absolute_offset
            );
            let slices = data.as_ioslices(to_write);
            debug_assert_eq!(slices[0].len() + slices[1].len(), to_write);
            if !file_info.attrs.padding {
                let written = self
                    .files
                    .pwrite_all_vectored(file_idx, absolute_offset, slices)
                    .with_context(|| {
                        format!(
                            "error writing to file {file_idx} (\"{:?}\")",
                            file_info.relative_filename
                        )
                    })?;
                debug_assert_eq!(written, to_write);
            }
            data.advance(to_write);
            if data.is_empty() {
                break;
            }

            absolute_offset = 0;
        }

        Ok(())
    }
}
