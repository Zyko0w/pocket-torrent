//! Journal de l'application : fichier tournant, consultable et exportable depuis les
//! Réglages (§5 du plan d'upgrade). Un fichier, pas la seule console : en mode fenêtré
//! (lancé avec `--cache`), il n'y a pas de console attachée et les journaux seraient perdus.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::MakeWriter;

const DIR: &str = "logs";
const PREFIX: &str = "pocket-torrent";

fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// Le classeur renvoyé doit rester en vie (`app.manage(..)`) pendant toute la durée de
/// l'application : le laisser tomber interrompt l'écriture des journaux sur le disque.
pub fn init(data_dir: &Path) -> anyhow::Result<WorkerGuard> {
    let dir = log_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(PREFIX)
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(Redacting(writer))
        .init();
    Ok(guard)
}

// ------------------------------------------------------------ masquage des secrets

/// Le journal est fait pour être exporté et partagé (§10.5) : passkeys de trackers privés,
/// jetons et signatures d'URL (liens directs, envois en ligne) et identifiants n'y sont jamais écrits.
/// Appliqué à chaque ligne, quel que soit le module qui l'émet (moteur compris).
#[derive(Clone)]
struct Redacting<W>(W);

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for Redacting<W> {
    type Writer = RedactingWriter<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter(self.0.make_writer())
    }
}

struct RedactingWriter<W>(W);

impl<W: Write> Write for RedactingWriter<W> {
    // le formateur écrit chaque évènement d'un seul bloc : les URL ne sont jamais coupées
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_all(clean_event(&String::from_utf8_lossy(buf)).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Un évènement = une ligne. Un nom de torrent ou une réponse de serveur ne peut pas inventer
/// de fausses lignes (« VPN connecté ») par un retour à la ligne, ni piéger un terminal qui
/// afficherait le journal (séquences d'échappement), ni s'afficher à l'envers (RLO).
fn clean_event(event: &str) -> String {
    let (body, newline) = match event.strip_suffix('\n') {
        Some(body) => (body.strip_suffix('\r').unwrap_or(body), "\n"),
        None => (event, ""),
    };
    let body: String = body
        .chars()
        .map(|c| match c {
            '\n' | '\r' => '⏎',
            '\t' => '\t',
            c if c.is_control() || librqbit_core::torrent_metainfo::is_bidi_control(c) => '�',
            c => c,
        })
        .collect();
    redact(&body) + newline
}

fn is_url_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '`')
}

/// Remplace dans `text` les parties secrètes des URL et des liens magnet.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let url_at = rest.find("://").map(|i| {
            // début du schéma (« https », « udp »…)
            rest[..i]
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                .last()
                .map_or(i, |(start, _)| start)
        });
        let magnet_at = rest.find("magnet:?");
        let start = match (url_at, magnet_at) {
            (Some(u), Some(m)) => u.min(m),
            (Some(u), None) => u,
            (None, Some(m)) => m,
            (None, None) => break,
        };
        let end = rest[start..].find(is_url_end).map_or(rest.len(), |e| start + e);
        out.push_str(&rest[..start]);
        let link = &rest[start..end];
        if link.starts_with("magnet:?") {
            out.push_str("magnet:?…");
        } else {
            out.push_str(&redact_url(link));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn redact_url(url: &str) -> String {
    let Some((scheme, after)) = url.split_once("://") else { return url.to_owned() };
    let (without_fragment, fragment) = match after.split_once('#') {
        Some((before, _)) => (before, "#…"),
        None => (after, ""),
    };
    let (location, query) = match without_fragment.split_once('?') {
        Some((before, _)) => (before, "?…"),
        None => (without_fragment, ""),
    };
    let (authority, path) = location.split_at(location.find('/').unwrap_or(location.len()));
    let host = match authority.rsplit_once('@') {
        Some((_, host)) => format!("***@{host}"),
        None => authority.to_owned(),
    };
    let path: Vec<&str> =
        path.split('/').map(|segment| if looks_like_token(segment) { "…" } else { segment }).collect();
    format!("{scheme}://{host}{}{query}{fragment}", path.join("/"))
}

/// Segment de chemin qui ressemble à une passkey ou à un jeton (ex. `/0f3a…9c/announce`).
fn looks_like_token(segment: &str) -> bool {
    segment.len() >= 16
        && segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && segment.chars().any(|c| c.is_ascii_digit())
        && segment.chars().any(|c| c.is_ascii_alphabetic())
}

/// Fichier du jour, le plus récemment modifié dans le dossier des journaux.
fn latest_file(data_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dir = log_dir(data_dir);
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    Ok(files.pop())
}

/// Fin du journal lue par `tail`.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Dernières lignes du journal du jour, les plus récentes en dernier.
pub fn tail(data_dir: &Path, max_lines: usize) -> anyhow::Result<String> {
    let Some(path) = latest_file(data_dir)? else { return Ok(String::new()) };
    // seulement la fin du fichier : un journal d'une journée chargée n'est pas relu en entier
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let skipped = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(skipped))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut raw = String::from_utf8_lossy(&bytes).into_owned();
    if skipped > 0 {
        // première ligne coupée en route : retirée
        raw = raw.split_once('\n').map_or_else(String::new, |(_, rest)| rest.to_owned());
    }
    // masqué aussi à la lecture : lignes écrites par une version sans masquage
    let text = redact(&raw);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines[start..].join("\n"))
}

/// Copie le journal du jour vers `dest`, choisi par l'utilisateur.
pub fn export(data_dir: &Path, dest: &Path) -> anyhow::Result<()> {
    let Some(path) = latest_file(data_dir)? else {
        anyhow::bail!("aucun journal pour l'instant");
    };
    std::fs::write(dest, redact(&std::fs::read_to_string(path)?))?;
    Ok(())
}

pub fn folder(data_dir: &Path) -> PathBuf {
    log_dir(data_dir)
}

#[cfg(test)]
mod tests {
    use super::{clean_event, redact};

    /// Nom de torrent piégé : une seule ligne, sans séquence d'échappement ni inversion.
    #[test]
    fn hostile_text_cannot_forge_log_lines() {
        let event = "2026-09-25 INFO ajout : film\n2026-09-25 INFO VPN connecté\u{1b}[2J\u{202e}exe.mp4\r\n";
        let cleaned = clean_event(event);
        assert_eq!(cleaned.matches('\n').count(), 1, "{cleaned:?}");
        assert!(cleaned.ends_with('\n'));
        assert!(!cleaned.contains('\u{1b}') && !cleaned.contains('\u{202e}') && !cleaned.contains('\r'));
        assert_eq!(clean_event("a\tb\n"), "a\tb\n");
    }

    /// Bac à sable : journal de 10 Mio, seule la fin est lue, lignes entières.
    #[test]
    fn tail_reads_only_the_end_of_a_big_log() {
        let data = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        let dir = data.join(super::DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::new();
        let mut n = 0;
        while text.len() < 10 * 1024 * 1024 {
            text.push_str(&format!("ligne {n} ................................................\n"));
            n += 1;
        }
        std::fs::write(dir.join("pocket-torrent.2026-09-25.log"), &text).unwrap();
        let tail = super::tail(&data, 3).unwrap();
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[2].starts_with(&format!("ligne {} ", n - 1)));
        let many = super::tail(&data, usize::MAX).unwrap();
        assert!(many.lines().all(|l| l.starts_with("ligne ")), "première ligne coupée retirée");
        assert!(many.len() <= 4 * 1024 * 1024);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn secrets_are_masked() {
        let cases = [
            (
                "annonce https://tracker.example/announce.php?passkey=0123456789abcdef échouée",
                "annonce https://tracker.example/announce.php?… échouée",
            ),
            (
                "error sending request for url (http://t.example:2710/0123456789abcdef0123456789abcdef/announce)",
                "error sending request for url (http://t.example:2710/…/announce)",
            ),
            ("udp://user:secret@tracker.example:6969/announce", "udp://***@tracker.example:6969/announce"),
            (
                "r2 https://bucket.r2.example/f.mkv?X-Amz-Signature=abc#x fin",
                "r2 https://bucket.r2.example/f.mkv?…#… fin",
            ),
            ("ajout magnet:?xt=urn:btih:abc&tr=http%3A%2F%2Ft%2Fpk ok", "ajout magnet:?… ok"),
            ("https://releases.example/ubuntu-24.04-desktop.iso", "https://releases.example/ubuntu-24.04-desktop.iso"),
            ("aucune URL ici : 500 Internal Server Error", "aucune URL ici : 500 Internal Server Error"),
        ];
        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "{input}");
        }
    }
}
