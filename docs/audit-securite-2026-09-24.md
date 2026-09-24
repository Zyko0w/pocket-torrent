# Audit de sécurité — 24/09/2026 (Pocket Torrent 0.1.2)

Tests d'intrusion exécutés dans des bacs à sable : dossiers temporaires dédiés, moteur hors ligne
(`Binding::Offline`), canaux nommés et serveurs de test propres à chaque test. Rien ne touche aux
vraies données de l'appli ni au vrai service du tunnel. Relancer : `cargo test --lib` (src-tauri et
src-tauri/tunnel) et `npm test`.

## Corrigé

| Gravité | Problème | Correctif | Test |
| --- | --- | --- | --- |
| **Élevée** | Tunnel : le client envoyait la configuration WireGuard (clé privée) à quiconque tenait le canal `\\.\pipe\TorrentCloudTunnel`. N'importe quel programme peut le créer quand le service est arrêté, et le serveur d'un canal peut, par défaut, emprunter l'identité du client. | Le client exige que le serveur du canal soit le processus du service (identifiant donné par le gestionnaire de services), et ouvre le canal en `SECURITY_IDENTIFICATION`. | `tunnel/src/client.rs` : `fake_pipe_server_never_receives_the_wireguard_key` (le faux service ne reçoit aucun octet), `genuine_server_process_is_accepted` |
| Faible | Dépôt de fichiers : le message WebView2 n'était pas filtré selon son origine. | Seule la page de l'appli (`tauri.localhost`, ou Vite en dev) est écoutée. | `filedrop.rs` : `only_the_app_page_may_send_dropped_files` |
| Faible | « Supprimer le .torrent d'origine » : `document.txt:x.torrent` (flux NTFS caché d'un autre fichier) passait le contrôle d'extension. | Nom contenant `:` refusé. | `actions.rs` : `only_torrent_files_are_removed_as_sources` |
| Faible | Deux nettoyeurs de noms de fichiers aux règles différentes (celui de l'appli ignorait `CONIN$`, `CONOUT$`). | `direct::safe_file_name` s'appuie sur `librqbit_core::safe_component`. | `direct.rs` : `hostile_names_stay_plain_file_names` |
| Dépendance (dev) | vitest 3 : GHSA-82fw-gwwq-j7x9 (lecture de fichiers via @vitest/mocker, outils de test seulement). | vitest 4. `npm audit` : 0. | — |

## Vérifié, résiste

- **Serveur de streaming** (`stream.rs`, test `stream::pentest`) : sans jeton, jeton faux ou tronqué, `Host` étranger (DNS rebinding), autre téléchargement, index hors limites, `..` encodé ou non → 404. Lecture partielle (`Range`) correcte.
- **Torrents piégés** (`engine::sandbox_tests::hostile_torrents_cannot_write_outside_the_download_folder`) : `..`, séparateurs dans un nom et nom de torrent `..` refusés ; `C:`, `D:evil`, flux `a:b`, `CON`, `nul.txt`, `COM1.log`, points et espaces finaux neutralisés ; aucune écriture hors du dossier de téléchargement.
- **Service du tunnel** : canal réservé à SYSTEM, aux administrateurs et au compte installateur, clients distants refusés, première instance exclusive, requête limitée à 64 Ko ; scripts `PreUp`/`PostUp` des .conf ignorés. Fichier de résultat du programme lancé en administrateur créé en `create_new` (un lien placé d'avance ne détourne pas l'écriture).
- **Programmes externes** (`hooks.rs`) : caractères spéciaux de `cmd.exe` retirés des noms ; `cmd /D` (pas d'AutoRun).
- **Secrets** : clés R2 et configuration WireGuard dans le gestionnaire d'identification Windows ; secrets masqués dans le journal (test `logging::tests`).
- **Interface** : aucune insertion de HTML brut (`dangerouslySetInnerHTML`, `innerHTML`, `eval`) ; CSP stricte (`script-src 'self'`, aucun cadre) ; permissions Tauri minimales.
- **Import de liste, dossiers surveillés, .torrent déposés** : taille plafonnée, contenu validé avant usage.

## Risques acceptés (documentés)

- `cargo audit` : `quick-xml` 0.38 (RUSTSEC-2026-0194/0195) n'arrive que par `plist` (format macOS de Tauri), qui ne lit aucune donnée venue d'Internet sous Windows. `ringbuf` 0.4 (RUSTSEC-2026-0293) dans le µTP de librqbit : la faille exige un élément dont la destruction panique ; le tampon ne contient que des octets (`u8`). Les autres alertes sont des crates « non maintenues » sans faille connue. À revoir à la prochaine mise à jour de Tauri et de librqbit-utp.
- Programmes externes : un modèle qui met `%N` **sans guillemets** laisse un nom de torrent ajouter des arguments au programme appelé (pas de commande supplémentaire). L'exemple affiché dans les réglages utilise des guillemets.
- Torrents ajoutés depuis un dossier surveillé : le .torrent est ensuite déplacé, la case « supprimer le .torrent d'origine » n'est donc pas proposée pour eux.

## Optimisation et refactoring

- Ligne de téléchargement redessinée seulement si ses données changent (la liste est relue chaque seconde).
- Serveur de streaming découplé du gestionnaire (`StreamSource`) : testable sans lancer l'appli.
- Nettoyage des noms de fichiers unifié (voir plus haut).
