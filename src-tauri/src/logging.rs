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
/// Préfixe des journaux écrits sous l'ancien nom (Torrent Cloud) : supprimés après 7 jours.
const OLD_PREFIX: &str = "torrent-cloud.";

fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// Le classeur renvoyé doit rester en vie (`app.manage(..)`) pendant toute la durée de
/// l'application : le laisser tomber interrompt l'écriture des journaux sur le disque.
pub fn init(data_dir: &Path) -> anyhow::Result<WorkerGuard> {
    let dir = log_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    remove_old_name_logs(&dir);
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

/// Journaux de l'ancien nom : plus gérés par la rotation (autre préfixe), retirés après 7 jours.
fn remove_old_name_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    for path in entries.flatten().map(|e| e.path()) {
        let old_name = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(OLD_PREFIX));
        let expired = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old_name && expired {
            let _ = std::fs::remove_file(path);
        }
    }
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
        self.0.write_all(redact(&String::from_utf8_lossy(buf)).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
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

/// Dernières lignes du journal du jour, les plus récentes en dernier.
pub fn tail(data_dir: &Path, max_lines: usize) -> anyhow::Result<String> {
    let Some(path) = latest_file(data_dir)? else { return Ok(String::new()) };
    // masqué aussi à la lecture : lignes écrites par une version sans masquage
    let text = redact(&std::fs::read_to_string(path)?);
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
    use super::redact;

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
