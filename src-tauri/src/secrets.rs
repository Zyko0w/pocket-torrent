//! Secrets (jetons storage.to, configuration WireGuard) rangés dans le Gestionnaire
//! d'identification de Windows, jamais dans les fichiers de réglages ni renvoyés à l'interface.

#[derive(Clone, Copy, Debug)]
pub enum Secret {
    /// Jeton de compte storage.to (facultatif : lève les limites des envois anonymes).
    StorageToToken,
    /// Jeton de visiteur storage.to : identifie les envois anonymes de ce PC.
    StorageToVisitor,
    /// Fichier de configuration WireGuard (contient la clé privée du tunnel).
    WireguardConfig,
    /// Mot de passe du proxy SOCKS5.
    ProxyPassword,
}

impl Secret {
    fn target(self) -> &'static str {
        match self {
            Secret::WireguardConfig => "PocketTorrent/wireguard-conf",
            Secret::StorageToToken => "PocketTorrent/storage-to-token",
            Secret::StorageToVisitor => "PocketTorrent/storage-to-visitor",
            Secret::ProxyPassword => "PocketTorrent/proxy-password",
        }
    }
}

pub fn get(secret: Secret) -> Option<String> {
    imp::read(secret.target()).filter(|value| !value.is_empty())
}

pub fn is_set(secret: Secret) -> bool {
    get(secret).is_some()
}

/// Enregistre la valeur ; une chaîne vide efface la clé.
pub fn set(secret: Secret, value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    if value.is_empty() {
        imp::delete(secret.target());
        return Ok(());
    }
    imp::write(secret.target(), value)
}

#[cfg(windows)]
mod imp {
    use std::ptr;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read(target: &str) -> Option<String> {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: `target` est terminée par un zéro ; `credential` est libéré par CredFree.
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                return None;
            }
            let blob =
                std::slice::from_raw_parts((*credential).CredentialBlob, (*credential).CredentialBlobSize as usize);
            let value = String::from_utf8(blob.to_vec()).ok();
            CredFree(credential.cast());
            value
        }
    }

    pub fn write(target: &str, value: &str) -> anyhow::Result<()> {
        let mut target = wide(target);
        let mut user = wide("Pocket Torrent");
        let mut blob = value.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            CredentialBlobSize: u32::try_from(blob.len())?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: user.as_mut_ptr(),
        };
        // SAFETY: tous les pointeurs restent valides pendant l'appel.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            anyhow::bail!(
                "impossible d'enregistrer la clé dans le Gestionnaire d'identification Windows ({})",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    pub fn delete(target: &str) {
        let target = wide(target);
        // SAFETY: `target` est terminée par un zéro.
        unsafe {
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn read(_target: &str) -> Option<String> {
        None
    }
    pub fn write(_target: &str, _value: &str) -> anyhow::Result<()> {
        anyhow::bail!("stockage des clés disponible seulement sous Windows")
    }
    pub fn delete(_target: &str) {}
}
