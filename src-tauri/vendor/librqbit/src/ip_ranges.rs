use anyhow::{Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use intervaltree::IntervalTree;
use std::iter::empty;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;
use std::path::Path;
use std::str::FromStr;
use tokio::io::{AsyncBufRead, AsyncRead};
use tokio::io::AsyncReadExt;
use tokio::{io::AsyncBufReadExt, io::BufReader};
use tracing::trace;
use url::Url;

/// Pocket Torrent : limites de lecture d'une liste (une vraie liste P2P : quelques centaines de
/// milliers de lignes de moins de 200 octets, quelques dizaines de Mio une fois décompressée).
const MAX_LINE_BYTES: usize = 4096;
const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;
const MAX_RANGES: usize = 10_000_000;

struct IntervalTreeWithSize<T> {
    t: IntervalTree<T, ()>,
    len: usize,
}

fn interval_tree<T: Clone + Ord>(it: impl Iterator<Item = Range<T>>) -> IntervalTreeWithSize<T> {
    let mut len = 0;
    let t = IntervalTree::from_iter(it.map(|r| {
        len += 1;
        (r, ())
    }));
    IntervalTreeWithSize { t, len }
}

pub struct IpRanges {
    // We could store only one interval tree, but splitting them takes less memory,
    // as IpAddr is 17 bytes, Ipv4Addr is only 4 bytes (the majority of ranges).
    v4: IntervalTreeWithSize<Ipv4Addr>,
    v6: IntervalTreeWithSize<Ipv6Addr>,
}

impl Default for IpRanges {
    fn default() -> Self {
        Self {
            v4: interval_tree(empty()),
            v6: interval_tree(empty()),
        }
    }
}

impl IpRanges {
    pub fn new(
        v4_ranges: impl IntoIterator<Item = Range<Ipv4Addr>>,
        v6_ranges: impl IntoIterator<Item = Range<Ipv6Addr>>,
    ) -> Self {
        Self {
            v4: interval_tree(v4_ranges.into_iter()),
            v6: interval_tree(v6_ranges.into_iter()),
        }
    }

    pub fn len(&self) -> usize {
        self.v4.len + self.v6.len
    }

    /// Pocket Torrent : l'application télécharge elle-même les listes en ligne (VPN, DNS chiffré)
    /// et passe un fichier local ; cette fonction ne sert plus qu'aux adresses `file:`.
    pub async fn load_from_url(url: &str) -> Result<Self> {
        let parsed_url = Url::parse(url).context("failed to parse URL")?;

        if parsed_url.scheme() == "file" {
            let path = parsed_url
                .to_file_path()
                .ok()
                .context("failed to convert file URL to path")?;
            return Self::load_from_file(path).await;
        }

        // téléchargée ici, la liste partirait par un client par défaut : hors du VPN, même coupé
        anyhow::bail!("only file: URLs are supported, the application downloads online lists itself")
    }

    pub async fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut file = tokio::fs::File::open(path).await?;
        Self::create_from_stream(&mut file).await
    }

    async fn create_from_stream(reader: &mut (dyn AsyncRead + Unpin + Send)) -> Result<Self> {
        let mut peek_bytes = [0u8; 2];
        let mut reader = tokio::io::BufReader::new(reader);

        // Peek the first bytes by filling buffer
        let buffer = reader.fill_buf().await?;
        if buffer.len() >= 2 {
            peek_bytes.copy_from_slice(&buffer[0..2]);
        } else {
            anyhow::bail!("content too short: not enough data to determine compression");
        }

        // Check for Gzip magic bytes
        let is_gzip = peek_bytes == [0x1F, 0x8B];

        if is_gzip {
            trace!("detected gzip stream, decompressing");
            Self::create_from_decoded_stream(&mut BufReader::new(GzipDecoder::new(reader))).await
        } else {
            trace!("plain text file detected.");
            Self::create_from_decoded_stream(&mut reader).await
        }
    }

    async fn create_from_decoded_stream(
        reader: &mut (dyn AsyncBufRead + Unpin + Send),
    ) -> Result<Self> {
        let mut v4 = Vec::new();
        let mut v6 = Vec::new();

        let mut line = String::new();
        let mut total = 0usize;

        // Pocket Torrent : lecture bornée. Sans limite, une ligne sans fin ou une archive gzip
        // piégée (quelques Mio qui en donnent des Gio) remplissait la mémoire.
        loop {
            let read = (&mut *reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_line(&mut line)
                .await?;
            if read == 0 {
                break;
            }
            if read > MAX_LINE_BYTES {
                anyhow::bail!("list line too long");
            }
            total += read;
            if total > MAX_DECODED_BYTES || v4.len() + v6.len() >= MAX_RANGES {
                anyhow::bail!("list too large");
            }
            match parse_ip_range(&line) {
                Some(IpRange::V4(r)) => {
                    v4.push(r);
                }
                Some(IpRange::V6(r)) => {
                    v6.push(r);
                }
                None => {
                    tracing::debug!(line, "couldn't parse line");
                }
            }
            line.clear();
        }

        Ok(Self::new(v4, v6))
    }

    pub fn has(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(a) => self.v4.t.query_point(a).next().is_some(),
            IpAddr::V6(a) => self.v6.t.query_point(a).next().is_some(),
        }
    }
}

enum IpRange {
    V4(Range<Ipv4Addr>),
    V6(Range<Ipv6Addr>),
}

fn parse_ip_range(line: &str) -> Option<IpRange> {
    let line = line.trim();
    if line.starts_with('#') || line.is_empty() {
        return None;
    }

    let (_name, ips) = {
        let is_ipv6 = line.matches(":").count() > 2;
        if is_ipv6 {
            line.split_once(':')?
        } else {
            line.rsplit_once(':')?
        }
    };
    let (start, end) = ips.split_once('-')?;
    match (IpAddr::from_str(start).ok()?, IpAddr::from_str(end).ok()?) {
        (IpAddr::V4(start), IpAddr::V4(end)) => {
            let end = Ipv4Addr::from_bits(end.to_bits().saturating_add(1));
            Some(IpRange::V4(start..end))
        }
        (IpAddr::V6(start), IpAddr::V6(end)) => {
            let end = Ipv6Addr::from_bits(end.to_bits().saturating_add(1));
            Some(IpRange::V6(start..end))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    #[tokio::test]
    async fn hostile_lists_are_bounded() {
        // ligne sans fin
        let endless = vec![b'1'; MAX_LINE_BYTES * 4];
        let err = IpRanges::create_from_stream(&mut Cursor::new(endless)).await.err().unwrap();
        assert!(format!("{err:#}").contains("too long"), "{err:#}");

        // archive gzip piégée : 600 Mio de lignes valides, quelques centaines de Kio compressées
        use async_compression::tokio::write::GzipEncoder;
        use tokio::io::AsyncWriteExt;
        let mut encoder = GzipEncoder::new(Vec::new());
        let line = b"x:1.2.3.0-1.2.3.255\n";
        let block = line.repeat(1024 * 1024 / line.len());
        for _ in 0..600 {
            encoder.write_all(&block).await.unwrap();
        }
        encoder.shutdown().await.unwrap();
        let bomb = encoder.into_inner();
        assert!(bomb.len() < 8 * 1024 * 1024, "{}", bomb.len());
        let err = IpRanges::create_from_stream(&mut Cursor::new(bomb)).await.err().unwrap();
        assert!(format!("{err:#}").contains("too large"), "{err:#}");

        // téléchargement direct par le moteur : refusé (voir `load_from_url`)
        assert!(IpRanges::load_from_url("http://127.0.0.1:9/liste.p2p").await.is_err());
    }

    use crate::tests::test_util::setup_test_logging;

    use super::*;
    use async_compression::tokio::write::GzipEncoder;
    use tokio::io::AsyncWriteExt;

    const LIST: &[u8] = br#"
    # test
    local:192.168.1.1-192.168.1.255
    localv6:2001:db8::1-2001:db8::ffff
    "#;

    #[tokio::test]
    async fn test_list_gzipped() -> Result<()> {
        let mut gzipped_list = Vec::new();
        {
            let mut encoder = GzipEncoder::new(&mut gzipped_list);
            encoder.write_all(LIST).await.unwrap();
            encoder.flush().await.unwrap();
            encoder.shutdown().await.unwrap();
        }
        let list = IpRanges::create_from_stream(&mut Cursor::new(gzipped_list)).await?;
        assert!(list.has("192.168.1.1".parse().unwrap()));
        assert!(!list.has("8.8.8.8".parse().unwrap()));

        Ok(())
    }

    #[tokio::test]
    async fn test_list_plaintext() -> Result<()> {
        let list = IpRanges::create_from_stream(&mut Cursor::new(LIST)).await?;
        assert!(list.has("192.168.1.1".parse().unwrap()));
        assert!(!list.has("8.8.8.8".parse().unwrap()));

        Ok(())
    }

    #[tokio::test]
    async fn test_list_from_plaintext_file() -> Result<()> {
        // Create a temporary file
        let mut temp_file = tokio::fs::File::create("temp_list.txt").await?;
        tokio::io::AsyncWriteExt::write_all(&mut temp_file, LIST).await?;
        drop(temp_file); // Close the file

        // Load the list from the file
        let list = IpRanges::load_from_file("temp_list.txt").await?;

        // Verify the list
        assert!(list.has("192.168.1.1".parse().unwrap()));
        assert!(!list.has("8.8.8.8".parse().unwrap()));
        assert!(list.has("2001:db8::1".parse().unwrap()));
        assert!(!list.has("2001:4860:4860::8888".parse().unwrap()));

        // Clean up the temporary file
        tokio::fs::remove_file("temp_list.txt").await?;

        Ok(())
    }

    #[test]
    fn test_list_empty() {
        let list = IpRanges::default();
        assert!(!list.has("127.0.0.1".parse().unwrap()));
        assert!(!list.has("::1".parse().unwrap()));
    }

    #[test]
    fn test_manual_ranges() {
        // Add IPv4 range
        let start_v4: Ipv4Addr = "192.168.0.0".parse().unwrap();
        let end_v4: Ipv4Addr = "192.168.255.255".parse().unwrap();
        let ipv4_range = start_v4..end_v4;

        // Add IPv6 range
        let start_v6: Ipv6Addr = "2001:db8::".parse().unwrap();
        let end_v6: Ipv6Addr = "2001:db8::ffff".parse().unwrap();
        let ipv6_range = start_v6..end_v6;

        let list = IpRanges::new(Some(ipv4_range), Some(ipv6_range));
        // Test IPv4 addresses
        assert!(list.has("192.168.1.1".parse().unwrap()));
        assert!(!list.has("10.0.0.1".parse().unwrap()));

        // Test IPv6 addresses
        assert!(list.has("2001:db8::1".parse().unwrap()));
        assert!(!list.has("2001:db9::1".parse().unwrap()));
    }

    #[ignore]
    #[tokio::test]
    async fn test_list_real_url() {
        setup_test_logging();
        let _ = IpRanges::load_from_url("https://raw.githubusercontent.com/Naunter/BT_BlockLists/refs/heads/master/bt_blocklists.gz")
            .await
            .unwrap();
    }
}
