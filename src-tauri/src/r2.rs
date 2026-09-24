//! Envoi des fichiers terminés sur Cloudflare R2 (API compatible S3) et liens de téléchargement.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use rusty_s3::actions::{CreateMultipartUpload, ObjectIdentifier};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use tokio::io::AsyncReadExt;

use crate::secrets::{self, Secret};
use crate::settings::R2Settings;

/// Durée de validité des requêtes signées d'envoi (pas celle des liens de téléchargement).
const SIGN_FOR: Duration = Duration::from_secs(3600);
const MB: u64 = 1024 * 1024;
/// Caractères gardés tels quels dans un chemin d'URL publique.
const PATH_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

pub struct R2 {
    bucket: Bucket,
    credentials: Credentials,
    public_url: String,
    link_expiry: Duration,
}

impl R2 {
    /// `None` si R2 n'est pas configuré (identifiant, bucket et clés).
    pub fn from_settings(settings: &R2Settings) -> Option<anyhow::Result<Self>> {
        if settings.account_id.is_empty() || settings.bucket.is_empty() {
            return None;
        }
        let (Some(key), Some(secret)) = (secrets::get(Secret::R2AccessKeyId), secrets::get(Secret::R2SecretAccessKey))
        else {
            return None;
        };
        Some((|| {
            let endpoint = format!("https://{}.r2.cloudflarestorage.com", settings.account_id).parse()?;
            let bucket = Bucket::new(endpoint, UrlStyle::Path, settings.bucket.clone(), "auto")
                .map_err(|e| anyhow!("adresse R2 invalide : {e:?}"))?;
            Ok(Self {
                bucket,
                credentials: Credentials::new(key, secret),
                public_url: settings.public_url.clone(),
                link_expiry: Duration::from_secs(u64::from(settings.link_expiry_days) * 86_400),
            })
        })())
    }

    pub fn has_public_links(&self) -> bool {
        !self.public_url.is_empty()
    }

    pub fn link_expiry(&self) -> Duration {
        self.link_expiry
    }

    /// Vérifie l'accès au bucket (bouton « Tester » des réglages).
    pub async fn check(&self, client: &reqwest::Client) -> anyhow::Result<()> {
        let url = self.bucket.head_bucket(Some(&self.credentials)).sign(SIGN_FOR);
        let response = send(client.head(url), "Cloudflare R2 injoignable").await?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => bail!("accès refusé : vérifie les clés et leurs droits sur le bucket"),
            404 => bail!("bucket introuvable : vérifie son nom et l'identifiant de compte"),
            code => bail!("réponse inattendue de R2 (HTTP {code})"),
        }
    }

    /// Envoie un fichier ; `on_progress` reçoit le nombre d'octets déjà envoyés.
    pub async fn upload(
        &self,
        client: &reqwest::Client,
        key: &str,
        path: &Path,
        mut on_progress: impl FnMut(u64),
    ) -> anyhow::Result<()> {
        let size =
            tokio::fs::metadata(path).await.with_context(|| format!("fichier introuvable : {}", path.display()))?.len();
        let mut file = tokio::fs::File::open(path).await?;

        if size <= 64 * MB {
            let mut body = Vec::with_capacity(size as usize);
            file.read_to_end(&mut body).await?;
            let url = self.bucket.put_object(Some(&self.credentials), key).sign(SIGN_FOR);
            let response = send(client.put(url).body(body), "envoi vers R2 impossible").await?;
            ensure_ok(response, "envoi du fichier").await?;
            on_progress(size);
            return Ok(());
        }

        // envoi en plusieurs parties (10 000 parties maximum chez R2)
        let part_size = (32 * MB).max(size.div_ceil(9_000));
        let create = self.bucket.create_multipart_upload(Some(&self.credentials), key).sign(SIGN_FOR);
        let response = send(client.post(create), "envoi vers R2 impossible").await?;
        let body = ensure_ok(response, "début de l'envoi").await?.text().await?;
        let upload = CreateMultipartUpload::parse_response(&body).map_err(|e| anyhow!("réponse R2 illisible : {e}"))?;
        let upload_id = upload.upload_id().to_owned();

        let result = async {
            let mut etags = Vec::new();
            let mut sent = 0u64;
            let mut part_number: u16 = 1;
            while sent < size {
                let len = part_size.min(size - sent) as usize;
                let mut buffer = vec![0u8; len];
                file.read_exact(&mut buffer).await?;
                // Bytes : un nouvel essai réutilise la même mémoire sans la recopier
                let buffer = bytes::Bytes::from(buffer);
                let url = self.bucket.upload_part(Some(&self.credentials), key, part_number, &upload_id).sign(SIGN_FOR);
                let response = send_with_retry(|| client.put(url.clone()).body(buffer.clone())).await?;
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow!("R2 n'a pas confirmé la partie {part_number}"))?
                    .to_owned();
                etags.push(etag);
                sent += len as u64;
                part_number += 1;
                on_progress(sent);
            }
            let complete = self.bucket.complete_multipart_upload(
                Some(&self.credentials),
                key,
                &upload_id,
                etags.iter().map(String::as_str),
            );
            let url = complete.sign(SIGN_FOR);
            let response = send(client.post(url).body(complete.body()), "fin de l'envoi impossible").await?;
            ensure_ok(response, "fin de l'envoi").await?;
            anyhow::Ok(())
        }
        .await;

        if result.is_err() {
            let abort = self.bucket.abort_multipart_upload(Some(&self.credentials), key, &upload_id).sign(SIGN_FOR);
            let _ = client.delete(abort).send().await;
        }
        result
    }

    /// Lien de téléchargement : permanent avec une URL publique, sinon signé et temporaire.
    pub fn link(&self, key: &str, file_name: &str) -> String {
        if self.has_public_links() {
            let path: Vec<String> =
                key.split('/').map(|part| utf8_percent_encode(part, PATH_SET).to_string()).collect();
            return format!("{}/{}", self.public_url, path.join("/"));
        }
        let mut action = self.bucket.get_object(Some(&self.credentials), key);
        let disposition = format!("attachment; filename*=UTF-8''{}", utf8_percent_encode(file_name, PATH_SET));
        action.query_mut().insert("response-content-disposition", disposition);
        action.sign(self.link_expiry).to_string()
    }

    /// Supprime des objets du bucket. Renvoie le nombre d'objets supprimés.
    pub async fn delete(&self, client: &reqwest::Client, keys: &[String]) -> anyhow::Result<usize> {
        let mut deleted = 0;
        for batch in keys.chunks(1000) {
            let objects: Vec<ObjectIdentifier> = batch.iter().cloned().map(ObjectIdentifier::new).collect();
            let action = self.bucket.delete_objects(Some(&self.credentials), objects.iter());
            let url = action.sign(SIGN_FOR);
            let (body, md5) = action.body_with_md5();
            let response =
                send(client.post(url).header("Content-MD5", md5).body(body), "Cloudflare R2 injoignable").await?;
            ensure_ok(response, "suppression sur R2").await?;
            deleted += batch.len();
        }
        Ok(deleted)
    }
}

/// Envoie la requête ; en cas d'échec réseau, le message n'inclut pas l'URL signée (elle contient la clé d'accès).
async fn send(request: reqwest::RequestBuilder, context: &str) -> anyhow::Result<reqwest::Response> {
    request.send().await.map_err(|err| anyhow::Error::from(err.without_url()).context(context.to_owned()))
}

async fn ensure_ok(response: reqwest::Response, step: &str) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let code = body
        .split("<Code>")
        .nth(1)
        .and_then(|rest| rest.split("</Code>").next())
        .map(|code| format!(" : {code}"))
        .unwrap_or_default();
    bail!("R2 a refusé l'étape « {step} » (HTTP {status}{code})")
}

async fn send_with_retry(request: impl Fn() -> reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = match send(request(), "Cloudflare R2 injoignable").await {
            Ok(response) => ensure_ok(response, "envoi d'une partie").await,
            Err(err) => Err(err),
        };
        match result {
            Ok(response) => return Ok(response),
            Err(err) if attempt >= 4 => return Err(err),
            Err(err) => {
                tracing::warn!(attempt, "partie R2 à renvoyer : {err:#}");
                tokio::time::sleep(Duration::from_secs(2 * attempt)).await;
            }
        }
    }
}

/// « Films/Mon film-3f2a1b9c/fichier.mkv » : nom lisible et sans caractères problématiques.
pub fn key_prefix(category: Option<&str>, name: &str, unique: &str) -> String {
    let clean = |text: &str| -> String {
        let mut out = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() || "._-".contains(c) {
                out.push(c);
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        let out: String = out.trim_matches('-').chars().take(80).collect();
        if out.is_empty() { "fichier".into() } else { out }
    };
    let base = format!("{}-{}", clean(name), &unique[..unique.len().min(8)]);
    match category {
        Some(category) => format!("{}/{base}", clean(category)),
        None => base,
    }
}
