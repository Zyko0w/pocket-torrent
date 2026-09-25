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
- **Torrents piégés** (`engine::tests::sandbox_tests::hostile_torrents_cannot_write_outside_the_download_folder`) : `..`, séparateurs dans un nom et nom de torrent `..` refusés ; `C:`, `D:evil`, flux `a:b`, `CON`, `nul.txt`, `COM1.log`, points et espaces finaux neutralisés ; aucune écriture hors du dossier de téléchargement.
- **Service du tunnel** : canal réservé à SYSTEM, aux administrateurs et au compte installateur, clients distants refusés, première instance exclusive, requête limitée à 64 Ko ; scripts `PreUp`/`PostUp` des .conf ignorés. Fichier de résultat du programme lancé en administrateur créé en `create_new` (un lien placé d'avance ne détourne pas l'écriture).
- **Secrets** : clés R2 et configuration WireGuard dans le gestionnaire d'identification Windows ; secrets masqués dans le journal (test `logging::tests`).
- **Interface** : aucune insertion de HTML brut (`dangerouslySetInnerHTML`, `innerHTML`, `eval`) ; CSP stricte (`script-src 'self'`, aucun cadre) ; permissions Tauri minimales.
- **Import de liste, .torrent déposés** : taille plafonnée, contenu validé avant usage.

## Risques acceptés (documentés)

- `cargo audit` : `quick-xml` 0.38 (RUSTSEC-2026-0194/0195) n'arrive que par `plist` (format macOS de Tauri), qui ne lit aucune donnée venue d'Internet sous Windows. `ringbuf` 0.4 (RUSTSEC-2026-0293) dans le µTP de librqbit : la faille exige un élément dont la destruction panique ; le tampon ne contient que des octets (`u8`). Les autres alertes sont des crates « non maintenues » sans faille connue. À revoir à la prochaine mise à jour de Tauri et de librqbit-utp.

## Optimisation et refactoring

- Ligne de téléchargement redessinée seulement si ses données changent (la liste est relue chaque seconde).
- Serveur de streaming découplé du gestionnaire (`StreamSource`) : testable sans lancer l'appli.
- Nettoyage des noms de fichiers unifié (voir plus haut).

---

# Audit complémentaire — 24/09/2026 (après les ajouts du jour)

Portée : répartition de la bande passante, dépendances, CI, valeurs par défaut et mode avancé,
redirection de port NAT-PMP, question des liens magnet au premier lancement.

## Corrigé

| Gravité | Problème | Correctif | Test |
| --- | --- | --- | --- |
| Moyenne | Redémarrage du moteur : l'ancien était arrêté avant que le nouveau démarre ; un port déjà pris laissait les torrents sans moteur. Déclenchable par un serveur VPN hostile (port imposé par NAT-PMP). | Repli automatique sur un port libre ; la redirection est abandonnée si le port donné par le VPN est pris. | — (nécessiterait un vrai port réseau hors bac à sable) |
| Moyenne | NAT-PMP : un serveur VPN hostile pouvait imposer un port système (445, 80…). | Ports < 1024 refusés. | `natpmp::tests::responses_are_checked_strictly` |
| Vérifié | NAT-PMP : fausse réponse envoyée par un autre programme. | Socket reliée à la passerelle : le système écarte tout autre expéditeur. | `answers_from_anyone_but_the_gateway_are_ignored` — **contre-test** : protection retirée, le test échoue (fausse réponse acceptée) ; protection remise, il passe. |
| Faible | CI : actions GitHub référencées par un tag déplaçable. | Figées sur un commit précis, jeton du dépôt non conservé (`persist-credentials: false`), droits en lecture seule. | — |
| Dépendances | quick-xml 0.38 (RUSTSEC-2026-0194/0195), ringbuf 0.4 (RUSTSEC-2026-0293). | plist 1.10 (quick-xml 0.42) ; librqbit-utp vendorisé avec ringbuf 0.5.2 (76 tests µTP réussis). `cargo audit` et `npm audit` : 0 vulnérabilité. | — |

## Vérifié, sans défaut

- Répartition de la bande passante : pas de division par zéro (poids ≥ 1), calculs sur 128 bits, plancher de 16 Kio/s par torrent, limite bornée à `u32::MAX`.
- Question des liens magnet : en mode dev, rien n'est écrit dans le registre (les liens pointeraient vers l'exe de test).
- Mode avancé : simple préférence d'affichage (stockage local), les réglages masqués restent appliqués.
- Tests d'intrusion précédents (streaming, torrents piégés, faux service du tunnel, noms de fichiers) : toujours réussis.

## Optimisation et refactoring

- Répartition de la bande passante : sortie immédiate quand au plus un torrent télécharge (cas courant), sans reconstruire la table des priorités chaque seconde.
- Redémarrage du moteur, relance des recherches et des limites : une seule fonction (`restart_engine`) au lieu de trois copies.
- Réglages d'origine : fournis par Rust (`Settings::default`), avec un test qui garde le jeu d'essai de l'interface synchronisé.

---

# Remplacement de Cloudflare R2 par storage.to — 24/09/2026

- R2 retiré (code, réglages, dépendance `rusty-s3`) ; les clés R2 des anciennes versions sont effacées du Gestionnaire d'identification au démarrage.
- Destination « r2 » d'anciennes listes : lue comme « garder sur le PC », jamais d'envoi non choisi (`store::dest_tests`).
- Requêtes limitées à `https://storage.to/api` ; adresses d'envoi signées acceptées en HTTPS seulement ; jamais écrites dans les erreurs ni le journal.
- Identifiants renvoyés par storage.to validés (lettres et chiffres) avant d'être mis dans une adresse ; suppression refusée pour un identifiant ou un type piégé (`storage_to::tests::deletion_needs_the_owner_token_and_a_plain_id`).
- Preuves de propriété (suppression à distance) gardées côté Rust, jamais envoyées à l'interface (`overview::tests::storage_owner_tokens_never_reach_the_interface`) ; jeton de compte et jeton de visiteur dans le Gestionnaire d'identification.
- Envoi testé contre un faux serveur storage.to en bac à sable : envoi simple, envoi en plusieurs parties reconstitué à l'identique, collection, durée de vie.

---

# Audit des ajouts depuis la 0.2.0 — 24/09/2026

Portée : limite de vitesse par torrent, liens magnet du presse-papiers, graphique de débit,
flux RSS, storage.to (voir section précédente), retrait de la création de torrents.

## Corrigé

| Gravité | Problème | Correctif | Test |
| --- | --- | --- | --- |
| **Moyenne** | Flux RSS : un flux hostile pouvait faire suivre des liens `.torrent` vers le réseau local (box, NAS : `http://192.168.1.1/…`), c'est-à-dire des requêtes émises par l'appli dans le réseau de l'utilisateur. | Lien web vers une adresse locale (privée, boucle, lien local, CGNAT, `localhost`, `.local`…, IPv6 comprise) refusé, sauf sur le même serveur que le flux (Jackett / Prowlarr sur ce PC). Magnets toujours acceptés. | `rss::tests::hostile_links_to_the_local_network_are_refused` — **contre-test** : protection retirée, le test échoue ; remise, il passe. |
| Faible | Enregistrement des réglages : fichier renommé sans être d'abord écrit sur le disque ; une coupure de courant pouvait laisser un `reglages.json` vide. | Écriture sûre commune (`store::write_atomic` : écriture, `sync_all`, renommage). | `store::write_tests::atomic_write_replaces_the_file_and_leaves_no_temporary` |

## Vérifié, résiste

- **Flux RSS** : entité externe (XXE) jamais chargée ; flux de plus de 5 Mio coupé ; 300 éléments au plus ; adresses `file:`, `ftp:`, `javascript:` refusées ; lien d'un élément vers une page web jamais téléchargé (magnet ou `.torrent` seulement) ; flux lus par le même chemin réseau que les téléchargements (VPN). Faux serveur de flux en bac à sable (`feeds::tests::feed_is_fetched_and_hostile_answers_are_refused`).
- **Presse-papiers** : lu seulement au retour sur la fenêtre, côté Rust, taille bornée ; seul un lien magnet valide (une ligne, 8 000 caractères au plus) est renvoyé à l'interface, une seule fois (`add::tests::only_a_real_magnet_is_taken_from_the_clipboard`) ; désactivable.
- **Limite par torrent** : bornée (4 000 000 Ko/s), jamais dépassée par la répartition par priorité, part libérée redistribuée (`bandwidth::tests::manual_limit_is_never_exceeded_and_frees_the_rest`).
- **Graphique** : historique en mémoire seulement, borné (300 points par téléchargement), oublié pour un téléchargement retiré.
- **Création de torrents retirée** : commande `share_job` supprimée, une surface de moins.

## Risque résiduel (documenté)

- ~~Flux RSS : un nom de domaine public qui se résout vers une adresse locale (rebinding DNS) n'est pas bloqué par la vérification du lien, faite sur l'adresse écrite.~~ **Corrigé le 25/09** : les .torrent d'un flux sont téléchargés par un client limité à Internet (adresses locales retirées après résolution, redirections contrôlées), sauf les liens d'un flux lui-même local (Jackett / Prowlarr sur ce PC). Test : `feeds::tests::torrent_of_a_public_name_resolving_to_the_local_network_is_refused` (contre-test inclus : le client ordinaire atteint bien le serveur local).

## Optimisation et refactoring

- Vérification des flux : lit deux valeurs des réglages chaque seconde au lieu de copier toute la liste des flux.
- Écriture sûre des fichiers mise en commun (liste des téléchargements, réglages, mémoire des flux).
- Mise en forme `rustfmt` appliquée au code ajouté.
