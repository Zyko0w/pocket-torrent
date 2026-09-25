//! Chiffrement des connexions entre pairs : Message Stream Encryption / Protocol Encryption (MSE/PE),
//! le chiffrement « Vuze » compatible qBittorrent, Transmission, Deluge, µTorrent et libtorrent.
//!
//! Échange Diffie-Hellman 768 bits, puis RC4 (les 1024 premiers octets du flux sont jetés).
//! Aucune signature BitTorrent en clair ne transite : ni « BitTorrent protocol », ni l'empreinte du torrent
//! (seule son empreinte hachée avec le secret partagé circule).
//!
//! Ajouté par Pocket Torrent.

use std::io::IoSliceMut;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, bail};
use librqbit_core::hash_id::Id20;
use num_bigint::BigUint;
use sha1w::ISha1;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::type_aliases::{BoxAsyncReadVectored, BoxAsyncWrite};
use crate::vectored_traits::AsyncReadVectored;

/// Politique de chiffrement de la session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncryptionMode {
    /// Connexions en clair uniquement.
    Disabled,
    /// Chiffre si le pair sait le faire, sinon en clair.
    #[default]
    Prefer,
    /// Refuse toute connexion en clair.
    Require,
}

const PRIME_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563";
const KEY_LEN: usize = 96;
const MAX_PAD: usize = 512;
const VC: [u8; 8] = [0; 8];
const CRYPTO_PLAINTEXT: u32 = 0x01;
const CRYPTO_RC4: u32 = 0x02;

fn prime() -> BigUint {
    BigUint::parse_bytes(PRIME_HEX.as_bytes(), 16).expect("nombre premier MSE")
}

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut hash = sha1w::Sha1::new();
    for part in parts {
        hash.update(part);
    }
    hash.finish()
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 32);
    while out.len() < len {
        out.extend_from_slice(&rand::random::<[u8; 32]>());
    }
    out.truncate(len);
    out
}

fn random_pad() -> Vec<u8> {
    let len = usize::from(rand::random::<u16>()) % (MAX_PAD + 1);
    random_bytes(len)
}

/// Clé Diffie-Hellman éphémère.
struct KeyPair {
    private: BigUint,
    public: [u8; KEY_LEN],
}

fn to_96(value: &BigUint) -> [u8; KEY_LEN] {
    let bytes = value.to_bytes_be();
    let mut out = [0u8; KEY_LEN];
    out[KEY_LEN - bytes.len()..].copy_from_slice(&bytes);
    out
}

impl KeyPair {
    fn generate() -> Self {
        // 160 bits d'aléa, comme recommandé par la spécification
        let private = BigUint::from_bytes_be(&random_bytes(20));
        let public = BigUint::from(2u32).modpow(&private, &prime());
        Self {
            private,
            public: to_96(&public),
        }
    }

    fn shared_secret(&self, remote_public: &[u8]) -> anyhow::Result<[u8; KEY_LEN]> {
        let p = prime();
        let remote = BigUint::from_bytes_be(remote_public);
        if remote <= BigUint::from(1u32) || remote >= &p - 1u32 {
            bail!("clé publique MSE invalide");
        }
        Ok(to_96(&remote.modpow(&self.private, &p)))
    }
}

/// RC4, avec les 1024 premiers octets jetés (exigé par MSE).
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (v, i) in s.iter_mut().zip(0..=u8::MAX) {
            *v = i;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        let mut rc4 = Self { s, i: 0, j: 0 };
        let mut discard = [0u8; 1024];
        rc4.apply(&mut discard);
        rc4
    }

    fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k =
                self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
            *byte ^= k;
        }
    }
}

fn keys(secret: &[u8], skey: &[u8]) -> (Rc4, Rc4) {
    let key_a = sha1(&[b"keyA", secret, skey]);
    let key_b = sha1(&[b"keyB", secret, skey]);
    (Rc4::new(&key_a), Rc4::new(&key_b))
}

// ------------------------------------------------------------------ flux

/// Lecture : octets déjà reçus (en clair) puis le reste du flux, déchiffré si RC4.
struct CryptReader {
    inner: BoxAsyncReadVectored,
    prefix: Vec<u8>,
    prefix_pos: usize,
    rc4: Option<Rc4>,
}

impl AsyncRead for CryptReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.prefix_pos < this.prefix.len() {
            let n = buf.remaining().min(this.prefix.len() - this.prefix_pos);
            buf.put_slice(&this.prefix[this.prefix_pos..this.prefix_pos + n]);
            this.prefix_pos += n;
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        std::task::ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        if let Some(rc4) = this.rc4.as_mut() {
            rc4.apply(&mut buf.filled_mut()[before..]);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncReadVectored for CryptReader {
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        vec: &mut [IoSliceMut<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.prefix_pos < this.prefix.len() {
            let mut copied = 0;
            for slice in vec.iter_mut() {
                let n = slice.len().min(this.prefix.len() - this.prefix_pos);
                slice[..n].copy_from_slice(&this.prefix[this.prefix_pos..this.prefix_pos + n]);
                this.prefix_pos += n;
                copied += n;
                if this.prefix_pos == this.prefix.len() {
                    break;
                }
            }
            return Poll::Ready(Ok(copied));
        }
        let n = std::task::ready!(Pin::new(&mut this.inner).poll_read_vectored(cx, vec))?;
        if let Some(rc4) = this.rc4.as_mut() {
            let mut left = n;
            for slice in vec.iter_mut() {
                if left == 0 {
                    break;
                }
                let take = slice.len().min(left);
                rc4.apply(&mut slice[..take]);
                left -= take;
            }
        }
        Poll::Ready(Ok(n))
    }
}

/// Écriture chiffrée : le tampon est chiffré d'un bloc puis envoyé en entier avant le suivant.
///
/// RC4 est un chiffrement de flux : des octets chiffrés ne peuvent pas être « rendus ». Si l'envoi
/// n'est pas terminé, on répond « en attente » (le réveil vient du flux sous-jacent) et on retient
/// le nombre d'octets déjà acceptés, confirmé à l'appel suivant (write_all rappelle avec les mêmes octets).
struct CryptWriter {
    inner: BoxAsyncWrite,
    rc4: Rc4,
    pending: Vec<u8>,
    pending_pos: usize,
    /// Octets du dernier appel déjà chiffrés mais pas encore confirmés à l'appelant.
    claimed: Option<usize>,
}

impl CryptWriter {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        while self.pending_pos < self.pending.len() {
            let n = std::task::ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.pending[self.pending_pos..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(std::io::ErrorKind::WriteZero.into()));
            }
            self.pending_pos += n;
        }
        self.pending.clear();
        self.pending_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for CryptWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_drain(cx))?;
        if let Some(claimed) = this.claimed.take() {
            // octets de l'appel précédent enfin partis
            return Poll::Ready(Ok(claimed.min(buf.len())));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        this.pending.extend_from_slice(buf);
        this.rc4.apply(&mut this.pending);
        match this.poll_drain(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(buf.len())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                this.claimed = Some(buf.len());
                Poll::Pending
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

fn wrap(
    reader: BoxAsyncReadVectored,
    writer: BoxAsyncWrite,
    prefix: Vec<u8>,
    rc4: Option<(Rc4, Rc4)>,
) -> (BoxAsyncReadVectored, BoxAsyncWrite) {
    match rc4 {
        Some((decrypt, encrypt)) => (
            Box::new(CryptReader {
                inner: reader,
                prefix,
                prefix_pos: 0,
                rc4: Some(decrypt),
            }),
            Box::new(CryptWriter {
                inner: writer,
                rc4: encrypt,
                pending: Vec::new(),
                pending_pos: 0,
                claimed: None,
            }),
        ),
        None => (
            Box::new(CryptReader {
                inner: reader,
                prefix,
                prefix_pos: 0,
                rc4: None,
            }),
            writer,
        ),
    }
}

/// Lit des octets jusqu'à trouver `pattern` dans les `max_scan` octets suivant `buffer`.
/// Renvoie les octets reçus après le motif.
async fn read_until(
    reader: &mut BoxAsyncReadVectored,
    buffer: &mut Vec<u8>,
    pattern: &[u8],
    max_scan: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = buffer.windows(pattern.len()).position(|w| w == pattern) {
            if pos > max_scan {
                bail!("synchronisation MSE introuvable");
            }
            return Ok(buffer.split_off(pos + pattern.len()));
        }
        if buffer.len() > max_scan + pattern.len() {
            bail!("synchronisation MSE introuvable");
        }
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            bail!("le pair a fermé la connexion pendant le chiffrement");
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
}

/// Lit exactement `len` octets, en consommant d'abord `buffer`.
async fn take_exact(
    reader: &mut BoxAsyncReadVectored,
    buffer: &mut Vec<u8>,
    len: usize,
) -> anyhow::Result<Vec<u8>> {
    while buffer.len() < len {
        let mut chunk = [0u8; 1024];
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            bail!("le pair a fermé la connexion pendant le chiffrement");
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let rest = buffer.split_off(len);
    Ok(std::mem::replace(buffer, rest))
}

// ------------------------------------------------------------ initiateur

/// Chiffre une connexion sortante (nous appelons le pair). Exige RC4.
pub async fn initiate(
    mut reader: BoxAsyncReadVectored,
    mut writer: BoxAsyncWrite,
    info_hash: Id20,
    timeout: Duration,
) -> anyhow::Result<(BoxAsyncReadVectored, BoxAsyncWrite)> {
    tokio::time::timeout(timeout.max(Duration::from_secs(10)), async move {
        let keys_pair = KeyPair::generate();
        let skey = info_hash.0;

        // 1. A → B : Ya, PadA
        let mut hello = keys_pair.public.to_vec();
        hello.extend_from_slice(&random_pad());
        writer.write_all(&hello).await?;

        // 2. B → A : Yb, PadB
        let mut buffer = Vec::new();
        let yb = take_exact(&mut reader, &mut buffer, KEY_LEN).await?;
        let secret = keys_pair.shared_secret(&yb)?;
        let (mut encrypt, decrypt) = keys(&secret, &skey);

        // 3. A → B : HASH(req1, S), HASH(req2, SKEY) xor HASH(req3, S), ENCRYPT(VC, provide, len(PadC), PadC, len(IA))
        let req1 = sha1(&[b"req1", &secret]);
        let req2 = sha1(&[b"req2", &skey]);
        let req3 = sha1(&[b"req3", &secret]);
        let mut message = req1.to_vec();
        message.extend(req2.iter().zip(req3.iter()).map(|(a, b)| a ^ b));
        let mut encrypted = VC.to_vec();
        encrypted.extend_from_slice(&CRYPTO_RC4.to_be_bytes());
        encrypted.extend_from_slice(&0u16.to_be_bytes()); // PadC vide
        encrypted.extend_from_slice(&0u16.to_be_bytes()); // pas de charge initiale
        encrypt.apply(&mut encrypted);
        message.extend_from_slice(&encrypted);
        writer.write_all(&message).await?;

        // 4. B → A : ENCRYPT(VC, select, len(PadD), PadD) — synchronisation sur le VC chiffré
        let mut vc_encrypted = VC;
        let mut probe = decrypt.clone();
        probe.apply(&mut vc_encrypted);
        let mut decrypt = probe;
        let mut rest = read_until(&mut reader, &mut buffer, &vc_encrypted, MAX_PAD).await?;
        let mut header = take_exact(&mut reader, &mut rest, 6).await?;
        decrypt.apply(&mut header);
        let select = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let pad_len = usize::from(u16::from_be_bytes([header[4], header[5]]));
        if pad_len > MAX_PAD {
            bail!("PadD MSE trop long");
        }
        if select != CRYPTO_RC4 {
            bail!("le pair n'a pas choisi le chiffrement RC4");
        }
        let mut pad = take_exact(&mut reader, &mut rest, pad_len).await?;
        decrypt.apply(&mut pad);

        // la suite du flux (déjà reçue) est chiffrée
        decrypt.apply(&mut rest);
        Ok(wrap(reader, writer, rest, Some((decrypt, encrypt))))
    })
    .await
    .context("délai dépassé pendant le chiffrement")?
}

// ------------------------------------------------------------ répondeur

/// Résultat de l'examen d'une connexion entrante.
pub enum Incoming {
    /// Poignée de main BitTorrent en clair : flux rendu tel quel (octets déjà lus inclus).
    Plaintext(BoxAsyncReadVectored, BoxAsyncWrite),
    /// Connexion chiffrée (ou négociée en clair par MSE) établie.
    Encrypted(BoxAsyncReadVectored, BoxAsyncWrite),
}

/// Détecte le type de connexion entrante et négocie MSE si besoin.
/// `info_hashes` : empreintes des torrents de la session (pour reconnaître SKEY).
pub async fn accept(
    mut reader: BoxAsyncReadVectored,
    mut writer: BoxAsyncWrite,
    mode: EncryptionMode,
    info_hashes: &[Id20],
    timeout: Duration,
) -> anyhow::Result<Incoming> {
    tokio::time::timeout(timeout.max(Duration::from_secs(10)), async move {
        let mut buffer = Vec::new();
        let first = take_exact(&mut reader, &mut buffer, 1).await?;
        // une poignée de main en clair commence par 19 puis « BitTorrent protocol »
        if first[0] == 19 {
            let mut head = first;
            head.extend(take_exact(&mut reader, &mut buffer, 19).await?);
            if &head[1..20] == b"BitTorrent protocol" {
                if mode == EncryptionMode::Require {
                    bail!("connexion en clair refusée (chiffrement obligatoire)");
                }
                head.append(&mut buffer);
                let (r, w) = wrap(reader, writer, head, None);
                return Ok(Incoming::Plaintext(r, w));
            }
            buffer.splice(0..0, head);
        } else {
            buffer.splice(0..0, first);
        }
        if mode == EncryptionMode::Disabled {
            bail!("connexion chiffrée refusée (chiffrement désactivé)");
        }

        // 1. A → B : Ya, PadA
        let ya = take_exact(&mut reader, &mut buffer, KEY_LEN).await?;
        let keys_pair = KeyPair::generate();
        let secret = keys_pair.shared_secret(&ya)?;

        // 2. B → A : Yb, PadB
        let mut hello = keys_pair.public.to_vec();
        hello.extend_from_slice(&random_pad());
        writer.write_all(&hello).await?;

        // 3. synchronisation sur HASH(req1, S), puis identification du torrent
        let req1 = sha1(&[b"req1", &secret]);
        let mut rest = read_until(&mut reader, &mut buffer, &req1, MAX_PAD).await?;
        let obfuscated = take_exact(&mut reader, &mut rest, 20).await?;
        let req3 = sha1(&[b"req3", &secret]);
        let req2: Vec<u8> = obfuscated
            .iter()
            .zip(req3.iter())
            .map(|(a, b)| a ^ b)
            .collect();
        let skey = info_hashes
            .iter()
            .find(|hash| sha1(&[b"req2", &hash.0]) == req2.as_slice())
            .context("torrent inconnu dans la connexion chiffrée")?
            .0;
        // côté répondeur : on déchiffre avec keyA, on chiffre avec keyB
        let (mut decrypt, mut encrypt) = keys(&secret, &skey);

        let mut header = take_exact(&mut reader, &mut rest, 14).await?;
        decrypt.apply(&mut header);
        if header[..8] != VC {
            bail!("vérification MSE invalide");
        }
        let provide = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        let pad_len = usize::from(u16::from_be_bytes([header[12], header[13]]));
        if pad_len > MAX_PAD {
            bail!("PadC MSE trop long");
        }
        let mut pad_and_len = take_exact(&mut reader, &mut rest, pad_len + 2).await?;
        decrypt.apply(&mut pad_and_len);
        let ia_len = usize::from(u16::from_be_bytes([
            pad_and_len[pad_len],
            pad_and_len[pad_len + 1],
        ]));
        let mut initial = take_exact(&mut reader, &mut rest, ia_len).await?;
        decrypt.apply(&mut initial);

        let select = if provide & CRYPTO_RC4 != 0 {
            CRYPTO_RC4
        } else if provide & CRYPTO_PLAINTEXT != 0 && mode == EncryptionMode::Prefer {
            CRYPTO_PLAINTEXT
        } else {
            bail!("aucune méthode de chiffrement commune avec le pair");
        };

        // 4. B → A : ENCRYPT(VC, select, len(PadD)=0)
        let mut answer = VC.to_vec();
        answer.extend_from_slice(&select.to_be_bytes());
        answer.extend_from_slice(&0u16.to_be_bytes());
        encrypt.apply(&mut answer);
        writer.write_all(&answer).await?;

        if select == CRYPTO_RC4 {
            decrypt.apply(&mut rest);
            initial.append(&mut rest);
            let (r, w) = wrap(reader, writer, initial, Some((decrypt, encrypt)));
            Ok(Incoming::Encrypted(r, w))
        } else {
            initial.append(&mut rest);
            let (r, w) = wrap(reader, writer, initial, None);
            Ok(Incoming::Encrypted(r, w))
        }
    })
    .await
    .context("délai dépassé pendant le chiffrement")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initiator_and_responder_agree() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let info_hash = Id20::new([7u8; 20]);

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, w) = stream.into_split();
            match accept(
                Box::new(r),
                Box::new(w),
                EncryptionMode::Require,
                &[Id20::new([1; 20]), info_hash],
                Duration::from_secs(5),
            )
            .await
            .unwrap()
            {
                Incoming::Encrypted(mut r, mut w) => {
                    let mut buf = [0u8; 5];
                    r.read_exact(&mut buf).await.unwrap();
                    assert_eq!(&buf, b"hello");
                    w.write_all(b"world!").await.unwrap();
                    w.flush().await.unwrap();
                }
                Incoming::Plaintext(..) => panic!("attendu chiffré"),
            }
        });

        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (r, w) = stream.into_split();
        let (mut r, mut w) = initiate(Box::new(r), Box::new(w), info_hash, Duration::from_secs(5))
            .await
            .unwrap();
        w.write_all(b"hello").await.unwrap();
        w.flush().await.unwrap();
        let mut buf = [0u8; 6];
        r.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world!");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn require_rejects_plaintext() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let mut hs = vec![19u8];
            hs.extend_from_slice(b"BitTorrent protocol");
            hs.extend_from_slice(&[0u8; 48]);
            let _ = stream.write_all(&hs).await;
        });
        let (stream, _) = listener.accept().await.unwrap();
        let (r, w) = stream.into_split();
        let result = accept(
            Box::new(r),
            Box::new(w),
            EncryptionMode::Require,
            &[],
            Duration::from_secs(5),
        )
        .await;
        assert!(result.is_err());
        client.await.unwrap();
    }

    #[tokio::test]
    async fn large_write_under_backpressure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // tube minuscule : l'écriture est sans cesse « en attente »
        let (client, server) = tokio::io::duplex(64);
        let (_, client_w) = tokio::io::split(client);
        let (mut server_r, _) = tokio::io::split(server);
        let key = [9u8; 20];
        let mut writer = CryptWriter {
            inner: Box::new(client_w),
            rc4: Rc4::new(&key),
            pending: Vec::new(),
            pending_pos: 0,
            claimed: None,
        };
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        let expected = data.clone();
        let sender = tokio::spawn(async move {
            for chunk in data.chunks(16_384) {
                writer.write_all(chunk).await.unwrap();
            }
        });
        let mut received = vec![0u8; expected.len()];
        tokio::time::timeout(Duration::from_secs(20), server_r.read_exact(&mut received))
            .await
            .expect("envoi bloqué")
            .unwrap();
        Rc4::new(&key).apply(&mut received);
        assert!(received == expected, "données altérées");
        sender.await.unwrap();
    }

    #[test]
    fn rc4_known_vector() {
        // RC4 sans le rejet des 1024 octets : vecteur « Key » / « Plaintext »
        let mut s = [0u8; 256];
        for (v, i) in s.iter_mut().zip(0..=u8::MAX) {
            *v = i;
        }
        let key = b"Key";
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        let mut rc4 = Rc4 { s, i: 0, j: 0 };
        let mut data = *b"Plaintext";
        rc4.apply(&mut data);
        assert_eq!(data, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
    }
}
