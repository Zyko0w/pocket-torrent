# Audit de sécurité et test d'intrusion — 25/09/2026 (Pocket Torrent 0.2.2 et suivantes)

Portée : l'application entière, pas seulement les derniers ajouts. Posture d'attaquant : pair
BitTorrent hostile, nœud DHT, tracker, serveur de liste de blocage, flux RSS, appareil du réseau
local (Wi-Fi public), fichier envoyé à l'utilisateur (liste à importer, .torrent), page web qui
ouvre un lien magnet.

Méthode : chaque attaque est rejouée dans un bac à sable — dossiers temporaires dédiés, moteur
hors ligne ou relié à 127.0.0.1, faux serveurs (tracker, liste de blocage, appareil UPnP, flux)
lancés par le test lui-même. Rien ne touche aux vraies données de l'appli, au vrai service du
tunnel, ni à Internet. Relancer : `cargo test --lib` (src-tauri, src-tauri/tunnel,
vendor/librqbit-upnp), la commande de `vendor/PATCHES.md` pour le moteur, et `npm test`.
Tous ces tests tournent aussi dans la CI.

## Corrigé

| Gravité | Problème | Correctif | Test |
| --- | --- | --- | --- |
| **Élevée** | Liste de blocage : téléchargée par le moteur avec un client HTTP par défaut, donc **hors du VPN, même VPN coupé** (arrêt automatique contourné, adresse réelle révélée au serveur de la liste, DNS du fournisseur d'accès). | L'appli télécharge la liste par son propre client (VPN, DNS chiffré ; VPN coupé : rien ne sort) et passe une copie locale au moteur, qui refuse désormais tout téléchargement lui-même. | `engine::tests::sandbox_tests::blocklist_is_never_fetched_outside_the_vpn` — **contre-test** : avant le correctif, le faux serveur recevait la requête. |
| Moyenne | Liste de blocage injoignable (ou fichier local effacé) : le moteur **refusait de démarrer**, plus aucun téléchargement. | Dernière copie de la même adresse utilisée, sinon démarrage sans liste (avertissement dans le journal). | `unreachable_blocklist_does_not_stop_the_engine` — échouait avant. |
| Moyenne | Liste de blocage : ligne sans fin ou archive gzip piégée (quelques Mio → des Gio) remplissaient la mémoire. | Téléchargement plafonné à 128 Mio ; lecture bornée (ligne de 4 Kio, 512 Mio décompressés, 10 millions de plages). | `ip_ranges::tests::hostile_lists_are_bounded` (bombe gzip de 600 Mio). |
| Moyenne | Magnets : chaque pair annonce la taille des métadonnées (32 Mio au plus), réservée aussitôt, pour 128 pairs à la fois. Un attaquant annoncé sur la DHT pour un magnet courant, avec de nombreux ports, faisait réserver **4 Gio**. | Budget mémoire commun à toutes les lectures de métadonnées : 256 Mio. | `peer_info_reader::budget_tests::metadata_memory_is_bounded_across_peers` |
| Moyenne | PEX : un pair pouvait envoyer en boucle des messages remplis d'adresses inventées : liste des pairs sans limite (mémoire) et connexions tentées vers toutes ces adresses (le client servait à **bombarder une cible** choisie par l'attaquant). Les pairs annoncés « partis » étaient ajoutés aussi. | 10 000 adresses connues au plus par torrent ; 50 pairs ajoutés au plus par message PEX ; « partis » ignorés. | `peers::tests::known_peers_are_bounded` |
| Moyenne | UPnP : n'importe quel appareil du réseau local pouvait répondre à la recherche de box et faire **lire à l'appli l'adresse de son choix** (autre machine, Internet), lui envoyer une réponse sans fin, puis **rediriger ses commandes SOAP** ailleurs ; une rafale de fausses réponses lançait autant de requêtes. | Description et commandes acceptées seulement sur l'appareil qui a répondu (même adresse IP) ; réponses de 1 Mio et 10 s au plus ; 32 appareils par recherche. | `librqbit-upnp` : `ssdp_answers_must_point_to_the_answering_device`, `endless_device_description_is_cut` (faux appareil). |
| Moyenne | Import de liste : une liste piégée contenant `\\serveur\partage\x.torrent` faisait se connecter Windows en SMB au serveur de l'attaquant — **empreinte NTLM du mot de passe de la session envoyée**. | Liens d'une liste importée : magnets et http(s) seulement. | `transfer::tests::imported_lists_cannot_point_to_files_or_network_shares` |
| Moyenne | Tracker HTTP hostile : réponse lue entièrement en mémoire, sans limite (flux sans fin = arrêt de l'appli). | Lecture par morceaux, 4 Mio au plus. | `tracker_comms::tests::endless_http_answer_is_cut_instead_of_filling_memory` (64 Mio envoyés). |
| Faible | .torrent listant des dizaines de milliers de trackers : autant d'annonces et de requêtes DNS ; cache DNS jamais vidé. | 100 trackers au plus par torrent (annonces et affichage) ; cache DNS borné à 4 096 noms. | `tracker_comms::tests::trackers_per_torrent_are_bounded` (10 000 trackers). |
| Faible | Flux RSS : un nom public qui se résout vers la box (« DNS rebinding ») passait la vérification du lien (risque résiduel de l'audit du 24/09). | .torrent des flux téléchargés par un client limité à Internet (adresse réellement jointe contrôlée, redirections comprises), sauf flux local (Jackett, Prowlarr). | `feeds::tests::torrent_of_a_public_name_resolving_to_the_local_network_is_refused` — contre-test inclus. |
| Faible | Explorateur : chemin passé sans guillemets ; Explorer découpe sa ligne de commande aux virgules (nom de torrent « a,b.exe »). | Chemin toujours entre guillemets (format documenté `/select,"…"`). | `explorer_tests::explorer_always_gets_one_quoted_path` |
| Faible (dépendances) | Un simple `cargo update` redescendait `time` en 0.3.45 (RUSTSEC-2026-0009) : le tunnel déclarait Rust 1.85. | Tunnel aligné sur Rust 1.88 ; dépendances mises à jour. `cargo audit` : 0 faille (7 avertissements « non maintenu » à la compilation, `glib` Linux seulement) ; `npm audit` : 0. | — |

## Vérifié, résiste

- **Lien magnet ouvert par une page web** : il remplit seulement la zone d'ajout ; rien n'est contacté avant le clic « Ajouter ».
- **Service du tunnel** : canal réservé à SYSTEM, aux administrateurs et au compte installateur, clients distants refusés, requête de 64 Ko au plus, scripts des .conf ignorés, pilote vérifié par empreinte à l'installation.
- **Serveur de streaming**, **torrents piégés**, **faux service du tunnel**, **noms de fichiers** : tests d'intrusion du 24/09 toujours réussis.
- **Lecture** : seule une liste `.m3u` générée est ouverte, jamais le fichier du torrent (pas d'exécution d'un `.exe` déguisé).
- **Interface** : aucun lien cliquable ni HTML brut construit à partir de données d'un torrent ; CSP stricte.
- **DHT** : pairs et jetons retenus bornés (1 000). **DNS chiffré** : décodage borné, types et longueurs contrôlés.
- **Messages entre pairs** : taille bornée (`MAX_MSG_LEN`), fuzzing du 23/09 toujours valable.

## Risques acceptés

- Un magnet ou un .torrent **ajouté volontairement** peut contenir un tracker ou un pair du réseau local : comme dans tous les clients, l'appli le contacte (requête d'annonce, poignée de main BitTorrent).
- UPnP : les correctifs n'ont été essayés que contre un faux appareil. À revérifier une fois sur la vraie box (dans `src-tauri/vendor/librqbit-upnp` : `cargo test --lib -- --ignored probe_forwarder_finds_allowed_port`).

## Optimisation et refactoring (même journée)

- Revérification : lecture et calcul des empreintes en parallèle, +24 % mesuré (voir `vendor/PATCHES.md`).
- `engine.rs` (1 330 lignes) découpé en module : session, vues, ajout, tests.
- Boucle d'une seconde et rafraîchissements de l'interface relus : rien de coûteux sans raison (la priorité des fichiers n'est réappliquée que si elle a changé, le volet Fichiers se rafraîchit moins souvent au-delà de 2 000 fichiers, et tout se met en pause fenêtre cachée).
- Paquet de l'interface : 510 Ko (143 Ko compressés), chargé depuis le disque ; pas de gain notable à le découper.

## Deuxième passe : test d'intrusion complet (soir du 25/09)

Toutes les attaques sont regroupées et rejouables d'une commande : `pwsh pentest/run.ps1`
(index des attaques : `pentest/README.md`). Chaque faille corrigée a son contre-test : correctif
retiré, l'attaque réussit.

| Gravité | Problème trouvé | Correctif |
| --- | --- | --- |
| **Élevée** | Un seul datagramme UDP de plus de 16 Kio, envoyé par n'importe qui, **arrêtait toute la DHT** (et, séparément, tout le µTP) jusqu'au redémarrage. | Tampon de 64 Kio, erreur de lecture non fatale. |
| **Élevée** | settings.json abîmé (coupure de courant, valeur d'une autre version) : tous les réglages revenaient par défaut, **VPN obligatoire compris** — les torrents repartaient sans protection. | Copie gardée, chaque réglage valide repris ; idem pour jobs.json (seuls les téléchargements illisibles sont perdus). |
| Moyenne | Boucle d'acceptation des connexions entrantes : panique possible (plus aucun pair entrant) ; une seule adresse IP occupait toutes les places de vérification. | Plus de panique ; 16 connexions en attente au plus par adresse. |
| Moyenne | PEX : un pair remplissait en une seconde toute la liste des pairs connus de fausses adresses (client isolé des vrais pairs). | Un message PEX par minute et par pair, 250 adresses par connexion. |
| Moyenne | DHT : amorçage raté au démarrage (réseau pas prêt) = nœud arrêté sans message. | Nouvel essai chaque minute. |
| Moyenne | Lien direct : reprise acceptée même quand le serveur renvoyait une autre plage (fichier corrompu) ; flux sans taille annoncée pouvant remplir le disque ; site public pouvant rediriger vers la box. | Plage vérifiée ; place revérifiée tous les 64 Mio ; client limité à Internet pour les liens publics. |
| Moyenne | Nom « film\u{202e}4pm.exe » : l'Explorateur l'affiche « filmexe.mp4 » (exécutable déguisé) ; seul U+202E était signalé, le nom restait tel quel sur le disque. | Tous les caractères d'inversion d'affichage remplacés et signalés ; COM¹–³ / LPT¹–³ réservés. |
| Faible | Planificateur : une plage de nuit (23 h → 7 h) s'arrêtait à minuit si le lendemain n'était pas coché. | La plage compte pour le jour où elle commence ; début = fin couvre la journée. |
| Faible | Journal : un nom de torrent avec retours à la ligne ou séquences d'échappement inventait de fausses lignes. | Un évènement = une ligne, caractères de contrôle neutralisés ; lecture limitée aux 4 derniers Mio. |
| Faible | Perçage de NAT : aucune limite de fréquence par pair. | Un message par seconde et par pair. |
| Défense en profondeur | Exports (liste, journal, mesures) vers n'importe quel type de fichier : un script injecté (improbable, CSP stricte) aurait pu déposer un `.bat` dans Démarrage. | .json/.log/.txt seulement. Import .conf : taille vérifiée avant lecture. |

**Vérifié, résiste** (voir `pentest/README.md`) : pairs hostiles en masse, fuzzing des messages
entre pairs, de la DHT, des .torrent et des .conf, faux trackers, fausse box UPnP, flux RSS
piégés, listes piégées, faux service du tunnel, arrêt d'urgence du VPN (carte absente, éteinte,
sans adresse), interface (ni HTML brut, ni `eval`, ni lien construit à partir de données).

**Fonctionnalités désormais testées** : file d'attente (ordre, limite, « démarrer maintenant »,
téléchargements lents, disque plein), limites (normales, planifiées, mode jeu), choix de la
liaison réseau, validation de tous les réglages, actions d'un torrent (priorité des fichiers,
choix, pause/reprise, lien magnet), reprise des liens directs, journal. Aucune fonctionnalité ni
option retirée.
