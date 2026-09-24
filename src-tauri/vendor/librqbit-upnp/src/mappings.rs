//! Redirections de ports ouvertes par ce programme (ajouté pour Torrent Cloud) : tenues à jour
//! par les gestionnaires UPnP et NAT-PMP, pour les retirer de la box à la fermeture au lieu de
//! les laisser expirer seules. Seules les redirections créées ici sont retirées, jamais celles
//! faites à la main.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use tracing::{debug, info};
use url::Url;

use crate::SERVICE_TYPE_WAN_IP_CONNECTION;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Active {
    /// Redirection UPnP d'un port externe (TCP et UDP) sur la box `control_url`.
    Upnp { control_url: Url, external: u16 },
    /// Redirection NAT-PMP du port interne (TCP et UDP) sur la passerelle `gateway`.
    NatPmp { gateway: SocketAddr, internal: u16 },
}

#[derive(Default)]
pub struct ActiveMappings(Mutex<Vec<Active>>);

impl ActiveMappings {
    fn record(&self, mapping: Active, same_place: impl Fn(&Active) -> bool) {
        let mut list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // un nouveau port externe sur la même box remplace l'ancien (qui expirera seul)
        list.retain(|m| !same_place(m));
        list.push(mapping);
    }

    pub(crate) fn upnp(&self, control_url: &Url, external: u16) {
        self.record(
            Active::Upnp {
                control_url: control_url.clone(),
                external,
            },
            |m| matches!(m, Active::Upnp { control_url: u, .. } if u == control_url),
        );
    }

    pub(crate) fn natpmp(&self, gateway: SocketAddr, internal: u16) {
        self.record(
            Active::NatPmp { gateway, internal },
            |m| matches!(m, Active::NatPmp { gateway: g, internal: i } if *g == gateway && *i == internal),
        );
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Retire toutes les redirections de la box, en parallèle et en `timeout` au plus : une box
    /// qui ne répond pas ne doit jamais bloquer la fermeture (la redirection expirera seule).
    pub async fn remove_all(&self, timeout: Duration) {
        let list = std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()));
        if list.is_empty() {
            return;
        }
        let count = list.len();
        let removals = list.into_iter().map(|m| async move {
            let result = match &m {
                Active::Upnp {
                    control_url,
                    external,
                } => {
                    let tcp = delete_upnp_mapping(control_url, *external, "TCP").await;
                    // l'UDP a pu échouer à l'ouverture : son retrait peut échouer sans gravité
                    let _ = delete_upnp_mapping(control_url, *external, "UDP").await;
                    tcp
                }
                Active::NatPmp { gateway, internal } => {
                    let tcp = crate::natpmp::unmap_port(*gateway, true, *internal).await;
                    let _ = crate::natpmp::unmap_port(*gateway, false, *internal).await;
                    tcp
                }
            };
            match result {
                Ok(()) => true,
                Err(e) => {
                    debug!(?m, "port forwarding removal failed: {e:#}");
                    false
                }
            }
        });
        match tokio::time::timeout(timeout, futures::future::join_all(removals)).await {
            Ok(results) => {
                let removed = results.iter().filter(|ok| **ok).count();
                info!(
                    removed,
                    failed = count - removed,
                    "port forwardings removed from the router"
                );
            }
            Err(_) => debug!("router too slow, port forwardings left to expire"),
        }
    }
}

/// `DeletePortMapping` (UPnP IGD) : retire la redirection d'un port externe.
async fn delete_upnp_mapping(
    control_url: &Url,
    external: u16,
    protocol: &str,
) -> anyhow::Result<()> {
    let body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:DeletePortMapping xmlns:u="{SERVICE_TYPE_WAN_IP_CONNECTION}">
<NewRemoteHost></NewRemoteHost>
<NewExternalPort>{external}</NewExternalPort>
<NewProtocol>{protocol}</NewProtocol>
</u:DeletePortMapping>
</s:Body>
</s:Envelope>"#
    );
    let response = reqwest::Client::new()
        .post(control_url.clone())
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPAction",
            format!("\"{SERVICE_TYPE_WAN_IP_CONNECTION}#DeletePortMapping\""),
        )
        .body(body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("DeletePortMapping answered {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Fausse box UPnP : note le corps de chaque requête SOAP et répond 200.
    async fn fake_router() -> (Url, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/ctl", listener.local_addr().unwrap())).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let sink = sink.clone();
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    // jusqu'à la fin de l'enveloppe SOAP
                    while !String::from_utf8_lossy(&data).contains("</s:Envelope>") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                        }
                    }
                    sink.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&data).into_owned());
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn upnp_mappings_are_deleted_once() {
        let (url, seen) = fake_router().await;
        let mappings = ActiveMappings::default();
        mappings.upnp(&url, 40001);
        mappings.upnp(&url, 40002); // nouveau port externe : remplace le précédent
        mappings.remove_all(Duration::from_secs(5)).await;
        let bodies = seen.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2, "TCP et UDP du dernier port seulement");
        for proto in ["TCP", "UDP"] {
            assert!(bodies.iter().any(|b| b.contains("#DeletePortMapping")
                && b.contains("<NewExternalPort>40002</NewExternalPort>")
                && b.contains(&format!("<NewProtocol>{proto}</NewProtocol>"))));
        }
        assert!(mappings.is_empty());
        mappings.remove_all(Duration::from_secs(5)).await;
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "rien à retirer une seconde fois"
        );
    }

    #[tokio::test]
    async fn silent_router_does_not_block_shutdown() {
        // port qui accepte la connexion mais ne répond jamais
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/ctl", listener.local_addr().unwrap())).unwrap();
        let mappings = ActiveMappings::default();
        mappings.upnp(&url, 40001);
        let started = std::time::Instant::now();
        mappings.remove_all(Duration::from_millis(300)).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(listener);
    }
}
