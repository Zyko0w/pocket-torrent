//! Lecture d'un fichier de configuration WireGuard (.conf) fourni par un VPN.

use std::net::IpAddr;

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Config {
    pub private_key: [u8; 32],
    pub listen_port: Option<u16>,
    /// Adresses de la carte (IP + longueur du préfixe).
    pub addresses: Vec<(IpAddr, u8)>,
    pub mtu: Option<u16>,
    pub peers: Vec<Peer>,
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    /// « hôte:port » tel qu'écrit dans le fichier.
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<(IpAddr, u8)>,
    pub persistent_keepalive: Option<u16>,
}

/// Résumé sans secret, affiché dans l'application.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub addresses: Vec<String>,
    pub endpoint: Option<String>,
    pub peers: usize,
    pub ipv6: bool,
}

impl Config {
    pub fn summary(&self) -> Summary {
        Summary {
            addresses: self.addresses.iter().map(|(ip, cidr)| format!("{ip}/{cidr}")).collect(),
            endpoint: self.peers.iter().find_map(|p| p.endpoint.clone()),
            peers: self.peers.len(),
            ipv6: self.addresses.iter().any(|(ip, _)| ip.is_ipv6()),
        }
    }
}

fn key(value: &str, what: &str) -> anyhow::Result<[u8; 32]> {
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(value.trim()).map_err(|_| anyhow!("{what} illisible"))?;
    bytes.try_into().map_err(|_| anyhow!("{what} : 32 octets attendus"))
}

fn cidr_list(value: &str, what: &str) -> anyhow::Result<Vec<(IpAddr, u8)>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (ip, prefix) = item.split_once('/').unwrap_or((item, ""));
            let ip: IpAddr = ip.trim().parse().with_context(|| format!("{what} : adresse invalide « {item} »"))?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let prefix = if prefix.is_empty() {
                max
            } else {
                prefix
                    .trim()
                    .parse::<u8>()
                    .ok()
                    .filter(|p| *p <= max)
                    .with_context(|| format!("{what} : préfixe invalide « {item} »"))?
            };
            Ok((ip, prefix))
        })
        .collect()
}

pub fn parse(text: &str) -> anyhow::Result<Config> {
    let mut section = "";
    let mut private_key = None;
    let mut listen_port = None;
    let mut addresses = Vec::new();
    let mut mtu = None;
    let mut peers: Vec<Peer> = Vec::new();

    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match line[1..line.len() - 1].trim().to_lowercase().as_str() {
                "interface" => "interface",
                "peer" => {
                    peers.push(Peer {
                        public_key: [0; 32],
                        preshared_key: None,
                        endpoint: None,
                        allowed_ips: Vec::new(),
                        persistent_keepalive: None,
                    });
                    "peer"
                }
                other => bail!("ligne {} : section inconnue [{other}]", number + 1),
            };
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .map(|(n, v)| (n.trim().to_lowercase(), v.trim()))
            .with_context(|| format!("ligne {} : « nom = valeur » attendu", number + 1))?;

        match (section, name.as_str()) {
            ("interface", "privatekey") => private_key = Some(key(value, "PrivateKey")?),
            ("interface", "listenport") => listen_port = Some(value.parse().context("ListenPort invalide")?),
            ("interface", "address") => addresses.extend(cidr_list(value, "Address")?),
            ("interface", "mtu") => mtu = Some(value.parse().context("MTU invalide")?),
            // DNS, scripts… : le tunnel ne sert qu'aux téléchargements, ces lignes sont ignorées
            ("interface", "dns" | "table" | "preup" | "postup" | "predown" | "postdown" | "saveconfig") => {}
            ("peer", field) => {
                let peer = peers.last_mut().expect("section peer ouverte");
                match field {
                    "publickey" => peer.public_key = key(value, "PublicKey")?,
                    "presharedkey" => peer.preshared_key = Some(key(value, "PresharedKey")?),
                    "endpoint" => peer.endpoint = Some(value.to_owned()),
                    "allowedips" => peer.allowed_ips.extend(cidr_list(value, "AllowedIPs")?),
                    "persistentkeepalive" => {
                        peer.persistent_keepalive = match value {
                            "off" => None,
                            v => Some(v.parse().context("PersistentKeepalive invalide")?),
                        }
                    }
                    other => bail!("ligne {} : paramètre inconnu « {other} »", number + 1),
                }
            }
            ("", _) => bail!("ligne {} : en dehors de toute section", number + 1),
            (_, other) => bail!("ligne {} : paramètre inconnu « {other} »", number + 1),
        }
    }

    let private_key = private_key.context("PrivateKey manquante dans [Interface]")?;
    if addresses.is_empty() {
        bail!("Address manquante dans [Interface]");
    }
    if peers.is_empty() {
        bail!("aucune section [Peer]");
    }
    for peer in &peers {
        if peer.public_key == [0; 32] {
            bail!("PublicKey manquante dans une section [Peer]");
        }
        if peer.endpoint.is_none() {
            bail!("Endpoint manquant dans une section [Peer]");
        }
    }
    Ok(Config { private_key, listen_port, addresses, mtu, peers })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Interface]
# commentaire
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.2.0.2/32, fd00::2/128
DNS = 10.2.0.1

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.org:51820
PersistentKeepalive = 25
";

    #[test]
    fn parses_provider_config() {
        let config = parse(SAMPLE).unwrap();
        assert_eq!(config.addresses.len(), 2);
        assert_eq!(config.peers[0].allowed_ips.len(), 2);
        assert_eq!(config.peers[0].persistent_keepalive, Some(25));
        let summary = config.summary();
        assert_eq!(summary.endpoint.as_deref(), Some("vpn.example.org:51820"));
        assert!(summary.ipv6);
    }

    #[test]
    fn rejects_missing_key() {
        assert!(parse("[Interface]\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nEndpoint = 1.2.3.4:1").is_err());
    }
}
