# Plan d'upgrade et d'optimisation d'un client BitTorrent existant

> Document de référence pour auditer, mettre à niveau et sécuriser un client torrent **déjà en production**.
> Sources : spécifications officielles BEP (bittorrent.org), documentation de libtorrent-rasterbar, qBittorrent, Transmission, Deluge, BiglyBT et rqbit.
> Dernière mise à jour : 25/09/2026 (Pocket Torrent 0.2.2 et suivantes).

---

## 0. Récapitulatif de l'audit et de la mise à niveau (22–23/09/2026)

Client : Tauri + React, moteur `librqbit` 9.0.1 (vendorisé dans `src-tauri/vendor`, voir `PATCHES.md` pour les correctifs appliqués).

| Point | Statut | Détail |
|---|---|---|
| **§2.2** Mesures de référence | ✅ | Instantané CPU/RAM/débit/pairs à la demande + export JSON (Réglages → Diagnostics) |
| **§3** Socle protocolaire | ✅ *(voir tableau §3, audit complété le 23/09)* | Quasi complet : extension Fast (BEP 6) et partage partiel (BEP 21) ajoutés, web seeds (BEP 19) aussi ; magnet v2 (`urn:btmh:`) : **25/09** hybrides acceptés (empreinte v1), v2 seul refusé avec un message clair (le moteur ne gère pas BitTorrent v2) |
| **§4** µTP réglable | ✅ | Désactivé par défaut (module jeune côté moteur), activable dans Réglages → Réseau |
| **§4.1** Piece picker : pièces déjà entamées | ✅ | Correctif moteur : proposées en priorité **et** un vrai gaspillage corrigé au passage (les chunks déjà reçus d'une pièce interrompue étaient effacés, pas seulement en cas d'échec de somme de contrôle) |
| **§4.1** Piece picker : rarest-first | ✅ | Correctif moteur : disponibilité de chaque pièce suivie sur les pairs connectés ; dans le fichier prioritaire en cours, la pièce la plus rare d'abord (première/dernière pièce du fichier toujours en tête pour les aperçus). Priorité des fichiers, pièces entamées et streaming inchangés |
| **§4.1** Mode fin de partie | ✅ *(25/09)* | Écarté à tort : **mesuré**, une source lente faisait attendre la fin (5,0 s pour 2 Mio avec une source à 16 Kio/s). Les pairs inactifs reprennent maintenant les pièces en retard chez les pairs nettement plus lents : **0,27 s** |
| **§4.1 / §5** Priorité par fichier | ✅ | Réglable pendant le téléchargement (était figée par ordre alphabétique) ; boutons ▲▼ dans le panneau Fichiers. Enregistrée dans `jobs.json` (pas dans les données de reprise du moteur, §12) et réappliquée après une pause ou un redémarrage. |
| **§4.2** Étranglement des pairs (choking) | ✅ | Correctif moteur : 8 places d'envoi par torrent par défaut (réglable, 0 = ancien comportement), réciprocité + place tournante toutes les 30 s, déblocage immédiat s'il reste une place. Garde-fou : pas plus de pairs intéressés que de places → tous servis. Validé par un essaim local sous contrainte (1 partageur à une place, 4 clients) |
| **§4.4** UPnP réparé | ✅ | Échouait toujours sur Freebox (IPv4 partagée : ports externes 32768–49151 seulement) : autre port externe demandé en cas de refus, port externe annoncé (trackers, DHT, pairs), TCP + UDP, box trouvée en 44 ms au lieu de ~4 min ; **24/09** : redirections retirées de la box à la fermeture (UPnP et NAT-PMP), y compris à l'arrêt de Windows et avec « éteindre à la fin », bail ramené à 20 min |
| **§4.3** Plafond de pairs par torrent | ✅ | Réglage déjà supporté par librqbit, juste pas exposé avant |
| **§4.3** Délais RTT adaptatifs, pipelining adaptatif | ❌ *écarté* | Inutile en pratique : la fenêtre de 128 requêtes × 16 Kio = 2 Mio en vol par pair permet ~20 Mio/s par pair à 100 ms de latence (~7 Mio/s à 300 ms), et on télécharge depuis plusieurs pairs à la fois ; les pairs lents sont déjà relayés par `try_steal` |
| **§4 / §10.6** Bannissement intelligent | ✅ | Correctif moteur : bannit une IP après plusieurs blocs corrompus, pas dès le premier |
| **§5** Empêcher la mise en veille | ✅ | Pendant un transfert actif, réglable |
| **§5** Programme externe à l'ajout/fin | ❌ *retiré (0.2.0)* | Fait puis retiré avec l'onglet Automatisation : plus aucune commande lancée par l'appli |
| **§5** Dossiers surveillés | ❌ *retiré (0.2.0)* | Fait puis retiré : fonctionnalité jugée inutile |
| **§5** File d'attente : ignorer les téléchargements lents | ✅ | Seuil configurable |
| **§5** Planificateur de limites (plages horaires) | ✅ | Heures + jours de la semaine, se combine avec le mode jeu |
| **§5** Revérification forcée | ✅ | Retiré du moteur puis rajouté depuis les octets déjà en mémoire, sans retélécharger les métadonnées |
| **§5** Carte des pièces | ✅ | Correctif moteur d'un mot-clé (accesseur interne déjà existant, juste pas exposé) |
| **§5** Déplacement automatique à la fin / dossier temporaire | ✅ *(25/09)* | Dossier « en cours » facultatif : torrents et liens directs y sont téléchargés puis déplacés dans leur dossier final (celui de la catégorie). Torrent retiré du moteur le temps du déplacement (fichiers fermés), puis rajouté et revérifié (rapide). Tout ou rien, jamais d'écrasement, copie puis suppression d'un disque à l'autre ; en cas d'échec, les fichiers restent en place et « Reprendre » retente. Tests en bac à sable |
| **§5** Détails : vue trackers | ✅ | État de chaque tracker (connexion, succès, échec et raison, pairs reçus, prochaine annonce) ; **25/09** : sources et clients (seeders/leechers) rapportés par chaque tracker HTTP et UDP |
| **§5** Journal consultable et exportable | ✅ | Fichier tournant (7 jours), 300 dernières lignes affichables, export |
| **§5 / §6.1** Création de torrent avancée | ❌ *retirée (0.2.1)* | Faite (taille de pièce, trackers, option privée) puis retirée avec « Partager en torrent » : une commande de moins exposée |
| **§6.1** BitTorrent v2 / hybride | ❌ *mis de côté* | Rien d'utilisable en amont chez rqbit (juste des fondations) ; on surveille l'issue #546 |
| **§6.3** Sélection de fichiers dans un magnet (BEP 53) | ✅ *déjà présent* | Fonctionnait déjà nativement dans librqbit, non exploité avant vérification |
| **§6.2 / §6.3** DHT avancé, holepunch, pont WebTorrent | ❌ *refusé* | Non supporté par le moteur ; le holepunch demanderait du code réseau non testable ici (pas de vrai NAT à disposition) |
| **§7.1-7.5** Démon/API séparés, REST v2, plugins, config TOML, Prometheus | ❌ *refusé* | Refonte d'architecture, hors périmètre d'une amélioration incrémentale. **RSS fait depuis** (0.2.1 : flux de séries, filtres, épisodes, voir §7.3) |
| **§7.6** Serveur de streaming | ❌ *retiré (25/09)* | Fait (serveur HTTP local, bouton « Lire ») puis retiré à la demande de l'utilisateur : jugé inutile, et il lisait le fichier tel quel sur le disque (morceaux manquants lus comme du vide) |
| **§7.6** Priorité automatique aux sous-titres | ❌ *sans objet* | Lecture en flux retirée le 25/09 |
| **§7.7** Thème clair/sombre | ✅ | Suit le réglage Windows automatiquement |
| **§7.7** Accessibilité | ⚠️ *audit statique fait* | Noms des boutons, libellés, focus visible, animations réduites : conformes. Corrigés : contrastes du thème clair (4 couleurs sous 4,5:1) et onglets au clavier (motif ARIA). Reste un test réel au lecteur d'écran (NVDA/Narrateur) |
| **§7.7** Traductions | ❌ *refusé* | Effort continu hors périmètre |
| **§8** Performances bas niveau | ❌ *refusé* | Dépend de l'intérieur du moteur, rien d'actionnable sans le patcher en profondeur |
| **§9.1-9.3** VPN, arrêt automatique, chiffrement, vérification d'IP | ✅ *déjà en place* | Avant cette série de sessions |
| **§9.3** Redirection de port automatique côté VPN | ✅ *(0.2.0, ProtonVPN)* | NAT-PMP auprès de la passerelle du VPN, renouvelé avant expiration, port d'écoute changé à chaud ; ports système refusés. Les autres fournisseurs (API propres) ne sont pas gérés |
| **§9.4** Proxy manuel | ✅ *(25/09)* | SOCKS5 avec identifiants (mot de passe dans le Gestionnaire d'identification) : pairs, trackers HTTP, sources web, liens directs, RSS et storage.to passent par lui, noms résolus par lui (`socks5h`) ; DHT, trackers UDP, µTP, écoute, UPnP et découverte locale coupés (ils ne passeraient pas par le proxy) ; proxy injoignable = rien ne sort. Test en bac à sable avec un faux proxy |
| **§9.5** I2P | ❌ *refusé* | Chantier à part entière (moteur à réécrire pour des adresses I2P) |
| **§9.5** Détection Tor | *N/A* | Pas de réglage de proxy manuel dans l'app : aucun chemin pour router par Tor sans le savoir |
| **§9.6** Mode confidentialité renforcée | ✅ | Peer ID masqué, UPnP coupé, `yourip` (BEP 10) supprimé (correctif moteur ; **corrigé le 23/09** : ne s'appliquait qu'à la récupération des métadonnées, pas aux pairs du torrent) |
| **§9.7** Listes de blocage d'IP | ✅ | Format P2P (texte/gzip), avertissement d'efficacité limitée affiché comme demandé |
| **§10.3** Fuzzing des messages entre pairs | ✅ | `librqbit-peer-protocol` copiée dans `vendor` et fuzzée : 65 millions de messages forgés, aucune panique |
| **§10.3** Fuzzing bencode/réseau | ✅ | Fuzzing par mutation sans dépendance, lancé avec `cargo test` (torrents, magnets, DHT, trackers HTTP/UDP). **2 vraies failles trouvées et corrigées** : entier bencode trop grand (panique / valeur tronquée) et magnet `so=0-999999999999` qui arrêtait l'application (des To de mémoire demandés) |
| **§10.3** Plafond du nombre de fichiers | ✅ | 100 000 entrées par torrent au plus (correctif `librqbit-core`) |
| **Hors plan** Glisser-déposer | ✅ | Fichiers .torrent et liens magnet/http déposés dans la fenêtre → zone d'ajout (catégorie, priorité, fichiers restent au choix) |
| **Hors plan** Recherche et tri | ✅ | Recherche sans accents sur le nom et le lien ; tri par date, nom, taille, progression, vitesse (mémorisé) |
| **Hors plan** Statistiques cumulées | ✅ | Reçu, envoyé, ratio global depuis le début et pour la session (Réglages → Statistiques), remise à zéro |
| **Hors plan** Éteindre le PC à la fin | ✅ | Pour la session seulement ; `shutdown /s /t 60` avec avertissement Windows et bouton d'annulation |
| **Hors plan** Export / import de la liste | ✅ | Fichier JSON avec les .torrent d'origine, catégories, priorités, fichiers choisis ; terminés importés seulement sur demande |
| **Audit de sécurité** (23/09) | ✅ | Serveur de streaming protégé (jeton secret + contrôle `Host` contre le DNS rebinding), .torrent par lien plafonné à 100 Mio (flux sans fin = arrêt de l'appli), liste de lecture .m3u protégée contre l'injection, programme externe lancé via `cmd /D /S /C` sans ré-échappement, dépendance `time` mise à jour (RUSTSEC-2026-0009). Tunnel WireGuard, secrets, suppression de fichiers, CSP : sains |
| **Audit de sécurité** (25/09) | ✅ | Test d'intrusion de toute l'appli (voir `docs/audit-securite-2026-09-25.md`) : liste de blocage téléchargée hors du VPN même VPN coupé (**élevée**, corrigée), moteur bloqué par une liste injoignable, métadonnées de magnets (4 Gio réservables), inondation PEX, UPnP détourné par un appareil du réseau local, fuite NTLM par une liste importée, réponses de trackers sans fin, 10 000 trackers par torrent — tous corrigés avec tests en bac à sable |
| **§10.2** Mises à jour signées | ✅ | 0.1.1 : GitHub Releases + signature minisign vérifiée avant installation, proposées dans un bandeau (jamais installées d'office). Clé de signature protégée par mot de passe (même paire de clés, rechiffrée) ; build signé via `scripts/build-release.ps1`, qui demande le mot de passe sans le stocker |
| **§10.5** Secrets masqués dans le journal | ✅ | Paramètres d'URL, `user:pass@`, segments de type passkey et liens magnet masqués à l'écriture, à l'affichage et à l'export du journal |
| **§10.3** Profondeur bencode limitée | ✅ | Correctif moteur (`librqbit-bencode`, vendorisée pour l'occasion) : rejette le bencode imbriqué à outrance reçu d'un pair/tracker/DHT (DoS par débordement de pile) |
| **§10.4** Filtre de fichiers dangereux | ✅ | Extensions dangereuses, « fausses vidéos », RTLO — décochés par défaut, jamais bloqués |
| **§10.4** Analyse antivirus optionnelle | ✅ | Via `MpCmdRun.exe` de Microsoft Defender (déjà sur Windows, pas de nouvelle dépendance) ; comportement vérifié avec un fichier test EICAR standard avant d'écrire le code ; rapport seulement, rien n'est supprimé automatiquement |
| **§10.5** Bac à sable système, build reproductible/SBOM | ⚠️ *en partie* | CI GitHub Actions en place (types, tests interface/Rust/tunnel/trackers/µTP, clippy strict, `npm audit`, `cargo audit`) ; bac à sable système, build reproductible et SBOM non faits |
| **§10.6** Signature de torrents (BEP 35) | ❌ *refusé* | Brouillon de spécification, P2 explicite dans le plan |

Détail des correctifs appliqués au moteur vendorisé : voir `src-tauri/vendor/PATCHES.md`.

---

## Sommaire

1. [Méthode : comment utiliser ce document](#1-méthode--comment-utiliser-ce-document)
2. [Phase 0 : audit de l'existant](#2-phase-0--audit-de-lexistant)
3. [Socle protocolaire à vérifier (P0)](#3-socle-protocolaire-à-vérifier-p0)
4. [Moteur de transfert : optimisations](#4-moteur-de-transfert--optimisations)
5. [Gestion et interface : fonctions attendues](#5-gestion-et-interface--fonctions-attendues)
6. [Modernisation du protocole (P1/P2)](#6-modernisation-du-protocole-p1p2)
7. [Configurabilité et usages multiples](#7-configurabilité-et-usages-multiples)
8. [Performances](#8-performances)
9. [Sécurité : anonymat, chiffrement, VPN](#9-sécurité--anonymat-chiffrement-vpn)
10. [Durcissement du client lui-même](#10-durcissement-du-client-lui-même)
11. [Architecture cible et refactoring](#11-architecture-cible-et-refactoring)
12. [Stratégie de migration sans casse](#12-stratégie-de-migration-sans-casse)
13. [Tests et qualité](#13-tests-et-qualité)
14. [Feuille de route priorisée](#14-feuille-de-route-priorisée)
15. [Annexes](#15-annexes)

---

## 1. Méthode : comment utiliser ce document

**Principe** : on ne réécrit pas le client, on le fait évoluer par petites livraisons compatibles.

**Niveaux de priorité**

| Code | Signification |
|---|---|
| **P0** | Indispensable. Un manque ici est un bug ou une faille. À corriger en premier. |
| **P1** | Standard actuel du marché. L'utilisateur s'attend à le trouver. |
| **P2** | Différenciant ou tourné vers l'avenir. |

**Statuts à renseigner** dans les tableaux au fil de l'audit : `✅ présent` · `⚠️ partiel` · `❌ absent` · `🔧 en cours`.

**Règles d'or de l'upgrade**

- Chaque nouveauté arrive derrière un **drapeau de fonctionnalité** (*feature flag*), désactivé par défaut tant qu'elle n'est pas stable.
- **Aucune régression** sur le format de reprise (*resume data*) ni sur la configuration existante : il faut une migration automatique et versionnée.
- Toute nouvelle fonction est **exposée dans l'API** en même temps que dans l'interface.
- Rien n'est fusionné sans tests d'interopérabilité avec au moins qBittorrent et Transmission.

---

## 2. Phase 0 : audit de l'existant

### 2.1 Inventaire technique

| Question | À relever |
|---|---|
| Moteur utilisé | libtorrent-rasterbar (quelle version, 1.2 ou 2.x ?), libtransmission, moteur maison, anacrolix (Go), librqbit (Rust)… |
| Langage et framework de l'interface | Qt, GTK, Electron, Tauri, web… |
| Architecture | Monolithe avec interface intégrée, ou démon et interface séparés ? |
| Stockage de l'état | Fichiers `.fastresume`, SQLite, fichiers maison ? |
| Format de configuration | INI, JSON, TOML, registre Windows ? Est-il versionné ? |
| API existante | Existe-t-elle ? Est-elle documentée ? Comment s'authentifie-t-on ? |
| Plateformes | Windows, macOS, Linux, BSD, Docker, NAS, mobile |
| Dépendances | Versions d'OpenSSL, de Boost, de Qt, CVE connues |

### 2.2 Mesures de référence (*baseline*) à relever **avant** toute modification

- Débit maximal atteint en local (essaim de test sur réseau 1 et 10 Gbit/s).
- Consommation CPU et RAM avec 10, 1 000 et 10 000 torrents chargés.
- Temps de démarrage avec 1 000 torrents.
- Temps de vérification d'un torrent de 50 Go (SSD NVMe, puis disque dur).
- Nombre de pairs connectés, et temps écoulé avant le premier octet reçu depuis un lien magnet.
- Nombre d'écritures disque par seconde et amplification d'écriture.

> Ces chiffres servent à prouver les gains (ou à détecter les régressions) après chaque étape.

### 2.3 Audit de sécurité initial

- [ ] Interface web ou API accessible sans authentification ? Identifiants par défaut ?
- [ ] Certificats TLS réellement vérifiés (mises à jour, RSS, GeoIP, trackers HTTPS) ?
- [x] Parseur bencode déjà soumis à du fuzzing ? — oui depuis le 23/09/2026 (§10.3).
- [ ] Noms de fichiers comme `../`, chemins absolus ou noms réservés Windows (`CON`, `NUL`…) bien gérés ?
- [ ] Le client tourne-t-il avec des droits administrateur ou root ?
- [ ] Des fuites d'IP sont-elles possibles hors VPN ou proxy ?

---

## 3. Socle protocolaire à vérifier (P0)

| Fonction | Réf. | Priorité | Statut | Remarques d'implémentation |
|---|---|---|---|---|
| Parsing `.torrent` (bencode) | BEP 3 | P0 | ✅ | Limites gérées par librqbit ; correctif `safe_component` contre la traversée de chemin (voir `PATCHES.md`). |
| Liens magnet, téléchargement des métadonnées | BEP 9 | P0 | ⚠️ | `xt=urn:btih:` (v1) accepté ; `urn:btmh:` (v2) absent, lié au BitTorrent v2 non supporté (§6.1). |
| Protocole d'extension | BEP 10 | P0 | ✅ | Base de PEX, des métadonnées ; `yourip` rendu optionnel (mode confidentialité renforcée, §9.6). |
| Tracker HTTP(S) | BEP 3 | P0 | ✅ | TLS vérifié (certificats épinglés), DNS chiffré. |
| Tracker UDP | BEP 15 | P0 | ✅ | Résolution par DNS chiffré (correctif `librqbit-tracker-comms`). |
| Plusieurs trackers | BEP 12 | P0 | ✅ | Torrents publics : tous à la fois. Torrents privés (**25/09**) : un seul à la fois dans l'ordre du .torrent, les suivants en secours (avant : seul le premier, tiré au hasard après une pause). |
| DHT | BEP 5 | P0 | ✅ | Table de routage persistante (`dht.json`) entre deux sessions. |
| DHT IPv6 | BEP 32 | P0 | ✅ | `librqbit-dht` génère et répond aux `nodes6` (`generate_compact_nodes_both`, `bprotocol.rs`) ; sockets IPv6 gérées par `librqbit-dualstack-sockets`. |
| Tracker IPv6 | BEP 7 | P0 | ✅ | `tracker_comms_http.rs` (`peers6: Peers<'a, SocketAddrV6>`) parse la réponse `peers6` des trackers HTTP. |
| PEX | BEP 11 | P0 | ✅ | Coupé automatiquement sur les torrents privés (géré par librqbit). |
| Découverte locale (LSD) | BEP 14 | P1 | ✅ | Désactivable (mode discret / confidentialité renforcée, §9.6). |
| Extension Fast | BEP 6 | P0 | ✅ | Ajoutée (correctif moteur, `librqbit-peer-protocol` vendorisée) : HaveAll/HaveNone/Reject gérés dans les deux sens, Suggest/AllowedFast acceptés et ignorés (facultatifs). |
| Listes de pairs compactes | BEP 23 | P0 | ✅ | `librqbit-core/src/compact_ip.rs` encode/décode le format compact IPv4 **et** IPv6, utilisé par le DHT et les trackers. |
| Torrents privés | BEP 27 | P0 | ✅ | Coupe automatiquement DHT, PEX et LSD ; **création** de torrent privé ajoutée (correctif moteur, §5/§6.1). |
| Web seeds (type GetRight) | BEP 19 | P1 | ✅ | Ajoutés (correctif moteur) : `url-list` des .torrent, téléchargement par plages HTTP, même vérification d'empreinte que les pairs ; audit 23/09 : réseau local refusé (anti-SSRF, vérifié aussi après résolution DNS et à chaque redirection le 24/09), HTTPS seul en mode discret, pièce > 32 Mio refusée. Pas pour `ws=` des magnets. |
| µTP (LEDBAT) | BEP 29 | P0 | ✅ | Réglable, désactivé par défaut (module jeune côté moteur), voir §4. |
| MSE/PE (obfuscation) | Vuze | P0 | ✅ | Modes désactivé/préféré/obligatoire (correctif moteur déjà en place avant cette série de sessions). |
| Partage partiel (*upload_only*) | BEP 21 | P1 | ✅ | Annoncé quand tout ce qui est choisi est téléchargé ; un pair en partage seul est déconnecté si on a fini aussi. |
| Priorité canonique des pairs | BEP 40 | P1 | ✅ | **25/09** (correctif moteur) : adresse publique estimée d'après les pairs, pairs en attente choisis par priorité canonique. |

---

## 4. Moteur de transfert : optimisations

### 4.1 Choix des pièces (*piece picker*)

- [ ] **Les plus rares d'abord** (*rarest first*) — confirmé absent : l'ordre est fichier-priorité puis première/dernière/milieu (voir plus bas), sans pondérer par le nombre de pairs qui ont chaque pièce. Demanderait un nouveau suivi de disponibilité par pièce sur tout l'essaim (mise à jour à chaque `Bitfield`/`Have`/déconnexion) : mis de côté, mérite sa propre session (ampleur comparable à I2P, §6.3).
- [ ] **Mode fin de partie** (*endgame*) : demander les derniers blocs en double, puis envoyer `cancel` dès qu'ils arrivent — confirmé absent en tant que tel, mais librqbit a un mécanisme différent qui répond au même besoin : `PieceTracker::try_steal` réattribue (sans dupliquer la requête) une pièce à un pair 3 à 10× plus rapide qu'un pair en cours trop lent. Pas de gaspillage de bande passante par les doubles requêtes ; recommandation : garder ce mécanisme plutôt que d'en ajouter un second en parallèle.
- [x] **Mode séquentiel** et « premières et dernières pièces en priorité » (streaming) — déjà en place : `TorrentStreams::iter_next_pieces` (`torrent_state/streaming.rs`) suit la position de lecture de chaque flux actif avec 32 Mio d'avance, entrelacés équitablement ; l'ordre de base (`iter_queued_pieces`) visite aussi chaque fichier première/dernière/milieu.
- [x] **Priorité par fichier**, y compris ne pas télécharger un fichier — l'infrastructure (`file_priorities: Vec<usize>`, un ordre de visite des fichiers) existait déjà côté moteur mais était figée par ordre alphabétique (`// TODO: make it configurable`) ; réglable maintenant pendant le téléchargement (`ManagedTorrent::update_file_priorities`, correctif moteur, voir `PATCHES.md`), exposé jusque dans l'interface (boutons ▲▼ dans le panneau Fichiers). Ne pas télécharger un fichier : déjà en place (`only_files`/§5). Priorité **par pièce** dans un fichier : pas de primitive dédiée au-delà de l'ordre première/dernière/milieu.
- [x] Priorité **aux pièces déjà entamées**, pour limiter les pièces incomplètes en mémoire — ajouté (correctif moteur) : `ChunkTracker::iter_started_pieces` propose en premier les pièces dont au moins un chunk est déjà reçu. **Corrige au passage un vrai gaspillage** : `mark_piece_broken_if_not_have` effaçait les chunks déjà reçus dès qu'un pair mourait en cours de pièce (correct seulement pour un échec de somme de contrôle réel) ; une pièce interrompue repartait donc toujours de zéro. Nouveau `requeue_piece_keep_partial` les garde, et la boucle de requêtes ne redemande plus que les chunks manquants.
- [ ] Pour le streaming : **délais par pièce** (*deadlines*), comme `set_piece_deadline` dans libtorrent — pas d'équivalent exact (pas de minuterie par pièce), mais le lookahead de `iter_next_pieces` (point précédent) couvre le même besoin pratique pour ce client.

### 4.2 Étranglement des pairs (*choking*)

**Confirmé absent en entier** : librqbit envoie `Unchoke` à tout pair dès la poignée de main (`peer_connection.rs`, `peer_info_reader/mod.rs`) et ne renvoie jamais `Choke` — tout le monde est toujours autorisé à demander, sans aucune réciprocité. Ce n'est pas un manque partiel, c'est l'absence totale de l'algorithme :

- [ ] Réciprocité (*tit-for-tat*) et pair aléatoire (*optimistic unchoke*) toutes les 30 s.
- [ ] Algorithme de partage dédié : tourniquet, ou priorité aux téléchargements les plus rapides.
- [ ] Nombre de places d'envoi **automatique**, calculé selon le débit disponible.
- [ ] Anti-leech optionnel pour les trackers privés (bonus aux pairs qui rendent).

*Mis de côté cette session* : implémenter un vrai choking change le comportement protocolaire envers **tous** les pairs de **tous** les torrents (équité de l'essaim, pas seulement une case de réglage) ; ça demande une tâche périodique, un calcul de réciprocité par pair et des tests contre un vrai essaim avant de merger — mérite sa propre session dédiée, comme I2P (§6.3) ou BitTorrent v2 (§6.1). Risque si fait à la hâte : casser l'upload pour de vrais utilisateurs.

### 4.3 Gestion des pairs

- [x] Plafond de connexions **par torrent** — ajouté (réglage déjà supporté par librqbit). Global et connexions semi-ouvertes : non vérifiés.
- [x] **Bannissement intelligent** : repérer quel pair a envoyé le bloc corrompu, et ne pas bannir toute une IP pour une seule erreur — correctif moteur, bannit après plusieurs incidents (voir §10.6).
- [ ] Délais d'attente adaptatifs selon le temps de réponse (RTT) de chaque pair — confirmé absent : `peer_connection.rs` n'utilise que des délais fixes (`connect_timeout`, `read_write_timeout`), aucune mesure de RTT par pair.
- [ ] Nombre de requêtes en attente par pair (*pipelining*) ajusté selon son débit — confirmé quasi absent : la fenêtre (`request_window`) part de `DEFAULT_PEER_REQUEST_WINDOW` et n'est réduite que si le pair annonce un `reqq` plus petit dans sa poignée de main étendue (une seule fois, pas un ajustement continu selon le débit mesuré).

### 4.4 Réseau

- [x] **UPnP IGD**, avec renouvellement des baux — confirmé : `librqbit-upnp` relance `forward_port` toutes les `lease_duration / 2` (`tokio::time::interval`). Coupé automatiquement en mode confidentialité renforcée (§9.6) et en VPN. **NAT-PMP** ajouté (correctif, RFC 6886, sans effet sur une Freebox qui ne parle qu'UPnP) ; **PCP** non géré.
- [x] Port d'écoute configurable, et option « port aléatoire au démarrage » — déjà en place (0 = automatique).
- [x] Choix de l'**interface réseau** et de l'adresse IP d'écoute (voir §9, VPN) — déjà en place.
- [ ] Marquage **DSCP/ToS** optionnel — confirmé absent : aucune référence à `IP_TOS`/`traffic_class` dans le moteur vendorisé.

---

## 5. Gestion et interface : fonctions attendues

| Fonction | Priorité | Statut | Remarques |
|---|---|---|---|
| File d'attente (téléchargements, partages, torrents actifs) | P0 | ✅ | Option pour ignorer les torrents lents dans le décompte ajoutée. |
| Limites de débit globales et par torrent | P0 | ✅ | Débits en envoi et en réception, déjà en place. |
| **Planificateur** de limites (plages horaires) | P1 | ✅ | Heures + jours de la semaine, se combine avec le mode jeu. |
| Objectifs de ratio et de temps de partage, avec action à la clé | P0 | ✅ | Pause ou arrêt du partage, déjà en place. |
| Catégories, étiquettes, chemin de sauvegarde par catégorie | P1 | ✅ | Déjà en place. |
| Dossiers surveillés | P1 | ❌ | Faits puis retirés en 0.2.0 (voir §0). |
| Déplacement automatique à la fin, dossier temporaire | P1 | ✅ | Fait le 25/09 (voir §0). |
| Programme externe à l'ajout ou à la fin | P1 | ❌ | Fait puis retiré en 0.2.0 (voir §0). |
| Création de torrents (v1, v2, hybride) | P1 | ❌ | v1 faite puis retirée en 0.2.1 (voir §0). |
| Revérification forcée | P0 | ✅ | Retiré du moteur puis rajouté depuis les octets déjà en mémoire, sans retélécharger les métadonnées. |
| Détails : pairs, trackers, fichiers, carte des pièces | P0 | ✅ | Pairs ✅, trackers ✅ (liste seule, pas d'état), fichiers ✅, carte des pièces ✅ (correctif moteur d'un mot-clé). |
| Journal consultable et exportable | P1 | ✅ | Fichier tournant, 300 dernières lignes, export. |
| Liens `magnet:` et fichiers `.torrent` associés au client | P0 | ✅ | Déjà en place. |
| Notifications système | P1 | ✅ | Déjà en place. |
| Mise en veille bloquée pendant les transferts | P1 | ✅ | Ajoutée, réglable. |

---

## 6. Modernisation du protocole (P1/P2)

### 6.1 BitTorrent v2 et torrents hybrides (P1, prioritaire)

| Élément | Réf. | Détail |
|---|---|---|
| Empreintes SHA-256 et arbre de Merkle par fichier | BEP 52 | Remplace SHA-1, qui est affaibli (collision SHAttered, 2017). |
| Fichiers de bourrage | BEP 47 | Chaque fichier commence au début d'une pièce, ce qui permet la déduplication. |
| **Torrents hybrides v1+v2** | BEP 52 | Indispensables pendant la transition : un même torrent rejoint les deux essaims. |
| Magnet `urn:btmh:` | BEP 52 | |

**Pour l'implémenter** :
- Si le moteur est libtorrent, **passer à la 2.x** (qBittorrent l'a fait à partir de la 4.4). Attention, le format de reprise change : il faut une migration (voir §12).
- Si le moteur est maison, prévoir deux espaces de noms pour les infohashes (v1 en 20 octets, v2 en 32 octets). Un torrent hybride a deux identités. C'est le changement le plus profond : il touche les index, la base de données et l'API.

**Bénéfices** : intégrité renforcée, vérification plus fine à l'échelle du bloc de 16 Kio, **déduplication** entre torrents qui partagent des fichiers identiques.

### 6.2 DHT avancé

| Fonction | Réf. | Priorité | Intérêt |
|---|---|---|---|
| Identifiant de nœud lié à l'IP | BEP 42 | P1 | Protège contre les attaques Sybil. |
| Stockage de données (immuables et modifiables) | BEP 44 | P2 | Base des torrents modifiables. |
| **Torrents modifiables** | BEP 46 | P2 | « Flux » de torrents mis à jour par leur auteur, sans tracker. |
| Échantillonnage d'infohashes | BEP 51 | P2 | Recherche décentralisée. |

### 6.3 Connectivité

| Fonction | Réf. | Priorité | Intérêt |
|---|---|---|---|
| Perçage du NAT (*holepunch*) | BEP 55 | P1 | Connecte deux pairs situés tous deux derrière un NAT. |
| Sélection dans un magnet (`so=`) | BEP 53 | P1 | Télécharger seulement une partie d'un torrent. |
| Signalement des pièces supprimées (`lt_donthave`) | BEP 54 | P2 | |
| **Pont WebTorrent (WebRTC)** | — | P2 | Échanger avec les pairs qui tournent dans un navigateur (BiglyBT, WebTorrent Desktop). Demande une pile WebRTC (libdatachannel) et des trackers WebSocket. |
| **I2P natif (SAM v3)** | — | P2 | Anonymat réel (voir §9). Disponible dans libtorrent 2.x. |

---

## 7. Configurabilité et usages multiples

### 7.1 Séparer le démon et l'interface (P1, structurant)

Si le client est un monolithe, c'est **le chantier le plus rentable** : il permet le mode serveur, NAS, Docker et mobile.

```
[Interface graphique]  [Interface web]  [CLI]  [Application mobile]
        \                   |             |           /
         └──── API REST + WebSocket (jeton, TLS) ────┘
                              |
                          [Démon]
```

- Exécution sans interface (*headless*) : service systemd, service Windows, launchd.
- L'interface graphique locale devient **un client de l'API comme les autres**, ce qui oblige à garder l'API complète.

### 7.2 API

- [ ] **REST versionnée** (`/api/v2/...`), décrite par un schéma **OpenAPI**.
- [ ] **WebSocket** ou Server-Sent Events pour les mises à jour en temps réel (pas de scrutation périodique).
- [ ] Authentification par **jetons à portée limitée** (lecture seule, ajout seulement, administration), qu'on peut révoquer.
- [ ] Limitation du nombre de requêtes par client.
- [ ] **Compatibilité avec l'API qBittorrent** en option : Sonarr, Radarr et Prowlarr l'utilisent déjà, donc l'intégration serait immédiate.

### 7.3 Automatisation

- [x] **RSS** (0.2.1) : filtres « contient / ne contient pas », détection des épisodes (S01E02, 1x02) sans doublon de qualité, mémorisation des éléments déjà vus, première lecture sans rien télécharger, intervalle global (5 min à 24 h). Pas de regex. Sécurité : liens vers le réseau local refusés, .torrent téléchargés par un client limité à Internet (contre le DNS rebinding, **25/09**).
- [ ] **Recherche par greffons** (sur le modèle des plugins de recherche de qBittorrent), dans un bac à sable.
- [ ] **Système de greffons** : **WASM** de préférence (isolé, multi-langage, sûr), ou Lua. Points d'accroche : ajout d'un torrent, fin d'un téléchargement, changement d'état, minuterie.
- [ ] **Règles automatiques** : « si le tracker est X, alors catégorie Y, limite Z et ratio R ».
- [ ] Webhooks sortants (vers Discord, Gotify, ntfy, Home Assistant).

### 7.4 Configuration

- [ ] Fichier de configuration **lisible et versionné** (TOML ou YAML), avec schéma et validation.
- [ ] **Chaque option est surchargeable par variable d'environnement** (indispensable pour Docker).
- [ ] **Profils** (maison, VPN, mobile, seedbox) avec bascule rapide.
- [ ] Export et import de la configuration **et** de l'état (torrents et données de reprise).
- [ ] Option « avancé » pour masquer les réglages d'expert.
- [ ] Bouton « valeurs par défaut » par section.

### 7.5 Observabilité

- [ ] Point d'accès **Prometheus** `/metrics` : débits, pairs, nœuds DHT, erreurs de tracker, file d'écriture disque.
- [ ] Journaux **structurés en JSON**, avec niveau réglable à chaud.
- [ ] Vérification de santé (*health check*) pour Docker et Kubernetes.

### 7.6 Streaming — **retiré (25/09/2026)**, à la demande de l'utilisateur

- [x] Serveur HTTP local qui gère les requêtes partielles (*Range*) — `127.0.0.1` uniquement ; sans les délais par pièce (§4.1), non exposés par librqbit.
- [x] Bouton « ouvrir dans VLC ou mpv », génération d'une liste de lecture M3U.
- [ ] Sous-titres : priorité automatique aux fichiers `.srt` — refusé, librqbit n'a pas de priorité par fichier ; gain marginal de toute façon (fichiers minuscules, arrivent vite).

### 7.7 Expérience utilisateur

- [x] Thème sombre et clair qui suit le système.
- [ ] Traductions (Weblate, Crowdin) — hors périmètre, effort continu.
- [ ] Accessibilité : lecteurs d'écran, navigation complète au clavier, contrastes suffisants — **audit statique fait (23/09/2026)** : tous les boutons et champs ont un nom accessible, focus visible partout, `prefers-reduced-motion` respecté, menus au clavier, `<dialog>` modaux, toasts annoncés (`role=status`). Corrigé : contrastes du thème clair (succès 2,87→6,21:1, info 3,57→5,17:1, avertissement et erreur remontés au-dessus de 4,5:1) et onglets « En cours / Terminés » (flèches, Début/Fin, un seul arrêt de tabulation, `aria-controls`). Reste à faire : un passage réel au lecteur d'écran (NVDA ou Narrateur).
- [ ] Interface web **adaptée au mobile** ou application progressive (PWA) — non applicable : Pocket Torrent est une appli de bureau, pas d'interface web.

---

## 8. Performances

| Axe | Action | Gain attendu |
|---|---|---|
| **Entrées-sorties disque** | io_uring (Linux ≥ 5.6), IOCP (Windows), plutôt que des threads bloquants | Moins de latence, débit disque plus élevé |
| mmap et pread/pwrite | libtorrent 2.0 est passé à mmap (résultats variables selon le système et le disque). Proposer **le choix du mode** dans la configuration. | Stabilité de la RAM et du cache |
| **Calcul des empreintes** | Réparti sur plusieurs threads, avec les instructions SHA-NI et ARMv8 du processeur | Vérification et création beaucoup plus rapides |
| Copies mémoire | Tampons partagés entre le réseau et le disque, `sendfile` et `splice` quand c'est possible | Moins de CPU à 10 Gbit/s |
| Préallocation | Fichiers creux par défaut, `fallocate` en option | Moins de fragmentation |
| Cache | Cache d'écriture regroupé et aligné, taille automatique | Moins d'accès aléatoires (important sur disque dur) |
| **Montée en charge** | Structures prévues pour plus de 10 000 torrents : pas de parcours complet de la liste à chaque tic | Seedboxes |
| Démarrage | Reprise **à la demande** (charger l'état au démarrage de chaque torrent, pas tout d'un coup) et base SQLite plutôt que des milliers de fichiers | Démarrage bien plus rapide |
| Réseau | Taille des tampons de socket réglable, envoi groupé en UDP (`sendmmsg`, GSO) pour µTP | Débit µTP |
| Interface | Diffusion des **seules différences** par WebSocket, listes virtualisées dans l'interface | Interface fluide avec 10 000 torrents |

> Toujours comparer aux mesures de référence de la phase 0 (§2.2).

---

## 9. Sécurité : anonymat, chiffrement, VPN

### 9.1 Rappel fondamental (à afficher dans l'aide du client)

**BitTorrent n'est pas anonyme.** Tout pair d'un essaim voit votre adresse IP. Les sociétés de surveillance (et l'Arcom en France) s'appuient justement là-dessus : elles rejoignent les essaims et relèvent les IP.

### 9.2 Chiffrement du protocole (MSE/PE) : ce qu'il faut dire à l'utilisateur

- Il repose sur RC4 avec un échange de clés Diffie-Hellman, **sans authentification**.
- Il sert **uniquement** à empêcher les FAI de détecter et brider le trafic (inspection DPI).
- Il n'apporte **ni anonymat ni confidentialité réelle**.
- **Réglage par défaut recommandé : « préféré »**. Le mode « forcé » fait perdre beaucoup de pairs.
- **Dans l'interface** : renommer l'option en « Obfuscation du protocole (anti-bridage FAI) » pour ne pas donner une fausse impression de sécurité.

### 9.3 Intégration VPN (P0 à P1)

| Fonction à implémenter | Priorité | Détail |
|---|---|---|
| **Liaison à une interface réseau** | P0 | Le client n'écoute et ne se connecte **que** par l'interface choisie (`wg0`, `tun0`…). Si l'interface disparaît, tout le trafic s'arrête. C'est plus fiable qu'un kill switch système. |
| Détection de l'interface VPN | P1 | Liste des interfaces avec leur nom, avertissement si l'interface par défaut est choisie alors qu'un VPN est détecté. |
| **Arrêt d'urgence intégré** | P1 | Mise en pause de tous les torrents si l'IP publique ou l'interface change. |
| Vérification de l'IP publique | P1 | Afficher l'IP vue de l'extérieur et alerter si elle correspond à l'IP du FAI (liste d'adresses de référence configurable). |
| **Redirection de port automatique** | P2 | ✅ NAT-PMP côté VPN (ProtonVPN) fait en 0.2.0, port d'écoute mis à jour à chaud. API fournisseur (AirVPN, PIA) non faites. |
| Blocage d'IPv6 hors tunnel | P1 | Option « IPv6 seulement si le VPN le route ». |
| DNS | P1 | Les requêtes DNS des trackers passent par l'interface choisie, ou par DNS-over-HTTPS. |
| **Documentation Docker et gluetun** | P1 | Exemple `docker-compose` fourni : le client n'a accès qu'au réseau du VPN. |

**Critères de choix d'un VPN à mentionner dans la documentation** : redirection de port (ProtonVPN, AirVPN, PIA la proposent ; **Mullvad l'a supprimée en 2023**), politique sans journaux **auditée par un tiers**, WireGuard, serveurs qui acceptent le P2P. Rappel : un VPN déplace la confiance du FAI vers le fournisseur VPN, il ne rend pas anonyme au sens strict.

### 9.4 Proxy

| Point | Action |
|---|---|
| SOCKS5 **ne chiffre rien** | À indiquer clairement dans l'interface. |
| Fuites | Option « **tout passer par le proxy** » : pairs, trackers, DHT, µTP. Si une fonction ne passe pas par le proxy, elle est **désactivée automatiquement**, pas contournée. |
| UDP | SOCKS5 `UDP ASSOCIATE` pour µTP, le DHT et les trackers UDP, sinon on les désactive. |
| Authentification | Identifiants chiffrés au repos. |

### 9.5 Tor et I2P

- **Tor : bloquer ou avertir.** Le projet Tor déconseille explicitement BitTorrent : l'IP réelle fuit (DHT, IP annoncée aux trackers, UDP) et le trafic surcharge le réseau. Si un proxy Tor est détecté (port 9050 ou 9150), **afficher un avertissement**.
- **I2P : la seule vraie solution d'anonymat** pour le pair-à-pair (P2).
  - Implémentation par **SAM v3**, avec un routeur i2pd ou Java I2P externe (libtorrent 2.x le fait déjà).
  - Torrents I2P **isolés** du réseau classique, ou **mode mixte** explicite comme BiglyBT, avec avertissement.
  - Réglages : longueur des tunnels, nombre de tunnels.
  - Inconvénients à annoncer : débits plus faibles, essaims plus petits.

### 9.6 Mode « confidentialité renforcée » (P1)

Un seul interrupteur qui active tout ceci — **fait** (Réglages → Confidentialité → « Mode confidentialité renforcée ») :
- [x] Pas d'UPnP, pas de LSD (pas de NAT-PMP dans ce client de toute façon).
- [ ] Toutes les connexions passent par le proxy, le VPN ou I2P, sinon rien — pas de réglage de proxy manuel ni d'I2P dans ce client (§9.4, §9.5, mis de côté) ; combiner avec le VPN pour cette garantie.
- [x] Identifiant et version du client masqués (*peer ID* générique) — correctif moteur, `SessionOptions::peer_id`/`client_name_and_version` étaient déjà réglables mais inexploités.
- [x] L'IP réelle n'est pas envoyée aux trackers (pas de paramètre `ip=`) — déjà le cas nativement, librqbit ne l'envoie pas.
- [x] Pas d'échange d'IP externe dans l'extension BEP 10 (`yourip`) — correctif moteur (`PeerConnectionOptions::disable_yourip`).
- [x] Aucune télémétrie, aucune vérification de mise à jour automatique — aucune télémétrie n'existe ; depuis la 0.1.1, la recherche de mise à jour au démarrage (§10.2) est sautée en mode confidentialité renforcée (le bouton manuel reste disponible).

### 9.7 Listes de blocage d'IP — **fait**

- [x] Garder la fonction (format P2P texte/gzip ; CIDR et DAT non couverts) et la mise à jour automatique (rechargée à chaque démarrage du moteur, pas de vérification en tâche de fond).
- [x] **Mais** indiquer dans l'interface que son efficacité est faible, parce que les surveillants changent régulièrement d'IP. C'est une fausse impression de sécurité si on la présente comme une protection.

---

## 10. Durcissement du client lui-même

C'est là que les incidents réels se produisent. Ce sont souvent des P0.

### 10.1 Interface web et API

| Mesure | Priorité | Référence ou incident |
|---|---|---|
| **Aucun identifiant par défaut** : mot de passe aléatoire au premier lancement | P0 | qBittorrent utilisait `admin/adminadmin` jusqu'à la version 4.6.1. |
| Écoute sur `127.0.0.1` par défaut | P0 | |
| Vérification de l'en-tête `Host` et du CORS, protection CSRF | P0 | Faille de *DNS rebinding* sur le RPC de Transmission en 2018 (Tavis Ormandy). |
| Stockage des mots de passe avec Argon2id | P0 | |
| TLS intégré, ou documentation d'un reverse proxy | P1 | |
| Double authentification (TOTP) et bannissement après échecs répétés | P1 | |
| En-têtes de sécurité : CSP, `X-Frame-Options`, `SameSite` | P1 | |
| Avertissement si l'interface web est exposée sur une IP publique | P1 | |

### 10.2 Réseau et TLS

- [ ] **Vérification systématique des certificats** (mises à jour, RSS, GeoIP, trackers HTTPS). qBittorrent ne les vérifiait pas depuis des années, corrigé en **5.0.1 (octobre 2024)**.
- [x] Mises à jour **signées** (minisign ou Sigstore) et vérifiées avant d'être installées — fait en 0.1.1 : `tauri-plugin-updater` (signature minisign vérifiée avec la clé publique intégrée à l'appli), versions publiées sur GitHub Releases (`Zyko0w/torrent-cloud-releases`, dépôt public ne contenant que les installateurs). Recherche au démarrage désactivable, **proposée dans un bandeau, jamais installée d'office**. Clé privée hors du projet (`%USERPROFILE%\.tauri\torrent-cloud.key`), à sauvegarder : la perdre coupe les mises à jour des appareils déjà installés.
- [ ] OpenSSL, BoringSSL ou rustls maintenus à jour et surveillés (Dependabot, Renovate).

### 10.3 Données non fiables (réseau et fichiers)

- [x] **Fuzzing** : bencode, métadonnées de torrent, magnets, messages DHT, réponses des trackers HTTP et UDP — fuzzing par mutation sans dépendance (`librqbit-core/src/fuzz_mutator.rs`), lancé par `cargo test` (20 000 essais par cible ; `FUZZ_ITERS`/`FUZZ_SEED` pour des campagnes plus longues : 2 millions × 3 graines passés, contrôles de débordement activés). **Deux failles trouvées et corrigées** : entier bencode trop grand (panique en debug, valeur tronquée en release) et magnet `so=` à plage énorme (arrêt de l'application — critique, un simple clic sur un lien suffisait). Messages entre pairs : dans `librqbit-peer-protocol`, non vendorisée, non couverts. Pas de libFuzzer/OSS-Fuzz (couverture guidée) : possible plus tard, nightly est installé.
- [x] Limites strictes — **profondeur bencode** : correctif moteur ajouté (`librqbit-bencode`, vendorisé pour l'occasion, voir `PATCHES.md`) : un bencode reçu d'un pair/tracker/DHT dont les dictionnaires/listes s'imbriquent au-delà de 200 niveaux est rejeté proprement au lieu de risquer un débordement de pile. Taille des métadonnées et longueur des messages : déjà bornées ailleurs (`UtMetadataBufLargerThanTotalSize`/`UtMetadataSizeMismatch` pour BEP 9, longueur de message du protocole pair limitée à un `u32`). Nombre de fichiers : plafonné à 100 000 entrées par torrent (`MAX_FILES`, `librqbit-core`).
- [ ] **Traversée de chemin** : normaliser et rejeter les noms `..`, les chemins absolus, les liens symboliques, les noms réservés Windows et les caractères interdits.
- [ ] Si le code est en C ou C++ : compiler avec ASan et UBSan en CI, et **envisager d'écrire en Rust** les nouveaux modules qui analysent des données (via FFI).

### 10.4 Contenu malveillant

- [x] **Liste de noms et d'extensions exclus** (`.exe`, `.scr`, `.lnk`, `.bat`, `.vbs`, `.msi`…), comme dans qBittorrent 5.x — décochés par défaut à l'ajout, jamais bloqués (l'utilisateur garde la main).
- [x] Alerte « **fausse vidéo** » : torrent présenté comme un film mais contenant un exécutable ou un `.lnk`.
- [x] Détection des **doubles extensions** (`film.mkv.exe`) et des caractères Unicode trompeurs (RTLO).
- [x] Analyse antivirus optionnelle à la fin — via `MpCmdRun.exe` de Microsoft Defender (déjà sur Windows, pas de nouvelle dépendance), rapport seulement. Comportement (codes de sortie, format de sortie) vérifié avec un fichier test EICAR standard avant d'écrire le code. AMSI et ClamAV non utilisés.
- [x] Ne jamais lancer ni ouvrir automatiquement un fichier téléchargé — déjà le cas, aucun code de l'application n'ouvre un fichier téléchargé sans action explicite de l'utilisateur.

### 10.5 Système

- [x] Ne **jamais** tourner en administrateur ou root — confirmé : pas de manifeste `requestedExecutionLevel` (donc `asInvoker` par défaut), permissions Tauri minimales (`src-tauri/capabilities/default.json`). Seule l'**installation** du service WireGuard déclenche un UAC ponctuel (`tunnel.rs`), comme le client WireGuard officiel — pas de mode service dédié.
- [ ] Bac à sable : AppArmor, SELinux, Flatpak, profil seccomp, AppContainer sous Windows — non traité.
- [x] Identifiants stockés dans le **trousseau du système**, jamais en clair — confirmé : `secrets.rs` range les jetons storage.to et la config WireGuard (qui contient la clé privée du tunnel) dans le Gestionnaire d'identification Windows (`CredWriteW`/`CredReadW`, `CRED_PERSIST_LOCAL_MACHINE`). Pas de passkeys de tracker privé ni de mot de passe de proxy dans ce client (aucune des deux fonctions n'existe, §9.4).
- [x] Passkeys **masquées dans les journaux** et les exports — audité et corrigé (23/09/2026) : le journal écrivait les erreurs réseau avec l'URL complète (jetons des liens directs, signatures R2, et potentiellement passkeys de trackers). Un filtre (`logging.rs`) masque maintenant paramètres d'URL, identifiants `user:pass@`, segments de chemin de type passkey et liens magnet, à l'écriture comme à l'affichage et à l'export. L'instantané de performances ne contient que des chiffres.
- [ ] Construction reproductible et SBOM publiés.

### 10.6 Intégrité

- [ ] Passage à la v2 (SHA-256), voir §6.1.
- [ ] Bannissement des pairs qui envoient des données corrompues.
- [ ] Signature de torrents (BEP 35, brouillon) : fonction P2, pour les distributeurs officiels.

---

## 11. Architecture cible et refactoring

```
┌──────────────────────── Interfaces ─────────────────────────┐
│ GUI native (existante, adaptée) · Web UI SPA · CLI · Mobile │
└──────────────────────────┬──────────────────────────────────┘
                           │ API REST v2 + WebSocket (OpenAPI, jetons, TLS)
┌──────────────────────────▼──────────────────────────────────┐
│ DÉMON                                                        │
│  ├─ Session manager (queue, règles, scheduler)               │
│  ├─ Automatisation : RSS · Recherche · Plugins WASM · Hooks  │
│  ├─ Config (TOML versionné + env vars + profils)             │
│  ├─ État (SQLite : torrents, resume, stats)                  │
│  ├─ Sécurité : auth, secrets (keychain), filtres fichiers    │
│  └─ Observabilité : /metrics, logs JSON, health              │
├──────────────────────────────────────────────────────────────┤
│ MOTEUR BitTorrent (libtorrent 2.x / moteur maison)           │
│  ├─ Peer wire TCP/µTP · MSE · extensions BEP 10              │
│  ├─ Piece picker · Choker · Rate limiter                     │
│  ├─ Trackers HTTP/UDP · DHT (BEP 5/32/42/44) · PEX · LSD     │
│  ├─ v1 + v2 + hybride · Merkle · padding                     │
│  ├─ Disk I/O async (io_uring/IOCP) · hashing multi-thread    │
│  └─ Réseau : bind interface · proxy · I2P SAM · NAT/holepunch│
└──────────────────────────────────────────────────────────────┘
```

**Refactoring conseillé, par ordre** :
1. Isoler le moteur derrière une **interface interne** (couche d'abstraction), pour pouvoir le mettre à jour ou le remplacer.
2. Sortir l'état et la configuration de l'interface graphique pour les placer dans le démon.
3. Exposer l'API, puis migrer l'interface graphique existante vers cette API.
4. Ajouter l'interface web, les greffons et les métriques par-dessus l'API.

---

## 12. Stratégie de migration sans casse

| Risque | Parade |
|---|---|
| Changement du format de reprise (libtorrent 1.2 vers 2.x, passage à SQLite) | Migration automatique au premier lancement, **sauvegarde de l'ancien état**, possibilité de revenir en arrière pendant N versions. |
| Configuration renommée ou restructurée | Clé `config_version` et migrations enchaînées (v1 → v2 → v3). |
| Changement d'API | Garder `/api/v1` (déprécié et documenté) au moins 2 versions majeures, avec l'en-tête `Deprecation`. |
| Infohashes v2 (32 octets) | Les index et l'API acceptent les deux longueurs. Un torrent hybride est cherchable par ses deux identifiants. |
| Nouveaux réglages par défaut plus stricts (sécurité) | Appliqués aux **nouvelles** installations. Pour les installations existantes, afficher un **assistant** qui explique le changement plutôt que de l'imposer en silence. |
| Nouvelle fonction instable | Drapeau de fonctionnalité, canal bêta ou nightly. |

**Canaux de publication** : stable, bêta et nightly, avec télémétrie des plantages **uniquement si l'utilisateur l'accepte** (Sentry auto-hébergé, sans données personnelles).

---

## 13. Tests et qualité

- **Tests unitaires** : bencode, choix des pièces, étranglement, arbres de Merkle, filtres RSS.
- **Fuzzing** en continu en CI (§10.3).
- **Essaims de test** en local : docker-compose avec un tracker (opentracker), N clients du projet et N clients de référence (qBittorrent, Transmission, rqbit).
- **Interopérabilité** : v1, v2, hybride, µTP, MSE, trackers privés, magnets sans tracker (DHT seul).
- **Simulation réseau** : perte de paquets, latence, NAT (`tc netem`, namespaces réseau).
- **Tests de fuite** : on coupe l'interface VPN en plein transfert et on vérifie qu'**aucun paquet** ne part par l'interface physique (capture tcpdump ou Wireshark automatisée).
- **Tests de charge** : 10 000 torrents, 1 000 pairs, 10 Gbit/s.
- **Tests de sécurité** : scan de l'interface web (OWASP ZAP), analyse statique (CodeQL, clang-tidy, clippy), audit des dépendances (`cargo audit`, `osv-scanner`).
- **Tests de régression de performance** : comparaison automatique aux mesures de référence (§2.2) sur chaque release.

---

## 14. Feuille de route priorisée

### Étape 1 : sécurité et socle (P0)
- [x] Audit de la phase 0 et mesures de référence — instantané de performance à la demande (§2.2), plus l'audit complet de ce document.
- [ ] Interface web et API : pas d'identifiants par défaut, écoute locale, CSRF, vérification de `Host` — **non applicable**, Pocket Torrent n'expose ni interface web ni API (client de bureau, communication Tauri interne seulement).
- [x] Vérification TLS partout (déjà en place). [x] Mises à jour signées — fait en 0.1.1 (§10.2).
- [x] Fuzzing du parseur bencode et des messages réseau — fait (§10.3), 2 failles corrigées. [x] Protection contre la traversée de chemin — déjà en place (`safe_component`, voir `PATCHES.md`). [x] Profondeur bencode limitée — ajoutée (`librqbit-bencode`, voir `PATCHES.md`).
- [x] **Liaison à une interface réseau** (VPN) avec arrêt total si l'interface tombe — déjà en place.
- [x] Filtre d'extensions dangereuses — fait (§10.4).
- [x] Combler les manques du tableau §3 — tous les statuts vérifiés (23/09/2026) ; magnet v2 : hybrides acceptés, v2 seul refusé proprement (25/09) ; web seeds, upload_only et extension Fast ajoutés par correctifs moteur.

### Étape 2 : modernisation (P1)
- [ ] Migration vers libtorrent 2.x ou implémentation de **v2 et hybride** — mis de côté, rien d'utilisable en amont (§6.1).
- [ ] Séparation démon et interface, **API REST v2 + WebSocket + OpenAPI** — refusé, refonte d'architecture hors périmètre incrémental.
- [ ] Configuration TOML versionnée, variables d'environnement, profils — refusé, casserait la promesse de migration du §12 si fait sans precaution.
- [x] Mode confidentialité renforcée (fait, §9.6). [ ] Proxy sans fuite — pas de proxy manuel dans ce client (§9.4). [ ] Avertissement Tor — non applicable (§9.5).
- [x] BEP 42 — fait le 25/09 (correctif moteur, voir `vendor/PATCHES.md`). [ ] BEP 55 — voir plus bas. [x] BEP 53 (sélection de fichiers dans un magnet) — déjà présent nativement dans librqbit.
- [x] RSS (0.2.1, voir §7.3). [ ] Règles regex, webhooks — refusés, hors périmètre incrémental.
- [ ] Métriques Prometheus, journaux JSON — refusé (journal texte lisible fait à la place, §5) ; image Docker — non applicable, client de bureau.
- [x] Piece picker : priorité aux pièces déjà entamées, priorité par fichier réglable (§4.1) — faits, avec un vrai gaspillage corrigé au passage. [x] Rarest-first et choking réel (§4.1/§4.2) — faits depuis par correctifs moteur (voir `vendor/PATCHES.md`).

### Étape 3 : performances (P1)
- [x] Calcul d'empreintes : lecture et calcul en parallèle pendant la vérification (**25/09**, +24 % mesuré, plus sur SSD ; `vendor/PATCHES.md`). [ ] Entrées-sorties asynchrones (IOCP) — non fait : cœur du stockage du moteur, gain incertain une fois la lecture recouverte par le calcul.
- [ ] Reprise à la demande, état dans SQLite — **jugé inutile (25/09)** : le moteur garde déjà les pièces de chaque torrent dans un fichier à part, projeté en mémoire, et `jobs.json` n'est réécrit qu'après une action (jamais en boucle). SQLite ajouterait une dépendance native sans gain mesurable.
- [ ] Interface web virtualisée, diffusion des seules différences — non applicable, pas d'interface web.

#### Audit du 24/09/2026 (bugs, sécurité, performances)

- **Bugs corrigés** :
  - blocage définitif possible de l'appli (verrous pris dans un ordre différent par la boucle et l'interface) ;
  - revérification qui pouvait effacer catégorie, priorité et destination ;
  - dossier surveillé qui déplaçait le .torrent avant sa lecture ;
  - sélection de fichiers écrasée pendant un rechargement, et bouton « Annuler » sans effet ;
  - taille de pièce < 16 Kio acceptée au partage ;
  - boucle qui continuait après l'arrêt ;
  - port importé des réglages web non appliqué.
- **Sécurité** :
  - jobs.json écrit sur le disque avant d'être remplacé (coupure de courant) ;
  - fichier de résultat du programme administrateur créé neuf uniquement (pas de lien préparé d'avance) ;
  - clés R2 plus relues chaque seconde dans le Gestionnaire d'identification ;
  - `cargo audit` : 3 avis dans des dépendances, aucun atteignable (quick-xml à la compilation seulement, ringbuf avec des octets) ;
  - `npm audit` : 0.
- **Performances** :
  - fichiers choisis vérifiés en temps linéaire (au lieu de quadratique, chaque seconde) ;
  - rafraîchissements en pause quand la fenêtre est cachée ;
  - liste de fichiers affichée par pages de 300 et rechargée toutes les 10 s au-delà de 2 000 fichiers ;
  - rarest-first arrêté au minimum possible ;
  - jobs.json plus réécrit sans changement.

### Étape 4 : différenciation (P2)
- [ ] **I2P natif (SAM v3)** — mis de côté, mérite sa propre session (ampleur comparable au tunnel WireGuard déjà en place).
- [ ] Pont WebTorrent (WebRTC) — refusé, faible intérêt pour un client de bureau Windows.
- [ ] Greffons WASM — refusé, hors périmètre incrémental.
- [ ] Torrents modifiables (BEP 46), déduplication v2 entre torrents — refusé, non supporté par librqbit.
- [x] Redirection de port automatique côté VPN — NAT-PMP (ProtonVPN) en 0.2.0 ; API des autres fournisseurs non faites.
- [ ] Serveur de streaming — fait puis retiré le 25/09 (jugé inutile).
- [ ] Application mobile — non applicable, client de bureau Windows.

---

## 15. Annexes

### 15.1 Récapitulatif des BEP cités

| BEP | Objet |
|---|---|
| 3 | Protocole BitTorrent de base |
| 5 | DHT |
| 6 | Extension Fast |
| 7 | Tracker IPv6 |
| 9 | Métadonnées et liens magnet |
| 10 | Protocole d'extension |
| 11 | PEX |
| 12 | Plusieurs trackers |
| 14 | Découverte locale des pairs |
| 15 | Tracker UDP |
| 19 | Web seeds (GetRight) |
| 21 | Partage partiel |
| 23 | Listes de pairs compactes |
| 27 | Torrents privés |
| 29 | µTP |
| 32 | DHT IPv6 |
| 35 | Signature de torrents (brouillon) |
| 40 | Priorité canonique des pairs |
| 42 | Extension de sécurité du DHT |
| 44 | Stockage de données dans le DHT |
| 46 | Torrents modifiables via le DHT |
| 47 | Fichiers de bourrage |
| 51 | Échantillonnage d'infohashes du DHT |
| 52 | BitTorrent v2 |
| 53 | Sélection de fichiers dans un magnet |
| 54 | `lt_donthave` |
| 55 | Perçage du NAT |

### 15.2 Moteurs et bibliothèques de référence

| Bibliothèque | Langage | Utilisée par |
|---|---|---|
| libtorrent-rasterbar | C++ | qBittorrent, Deluge |
| libtransmission | C/C++ | Transmission |
| anacrolix/torrent | Go | divers |
| librqbit | Rust | rqbit |
| WebTorrent | JavaScript | navigateurs, Node.js |
| Cœur de BiglyBT | Java | BiglyBT (I2P, Tor, WebTorrent) |

### 15.3 Réglages par défaut recommandés (nouvelles installations)

```toml
[network]
listen_interface   = ""          # à définir : "wg0" si VPN
listen_port        = 0           # 0 = aléatoire au 1er lancement
upnp               = true
natpmp             = true
ipv6               = "auto"      # "vpn_only" en mode confidentialité

[protocol]
encryption         = "prefer"    # disabled | prefer | require
utp                = true
dht                = true
pex                = true
lsd                = true

[privacy]
hardened_mode      = false
anonymous_peer_id  = false
announce_real_ip   = false
block_tor_proxy_warning = true

[webui]
enabled            = false
bind               = "127.0.0.1"
port               = 8080
password_hash      = ""          # généré au 1er lancement (Argon2id)
csrf_protection    = true
host_header_check  = true

[security]
excluded_extensions = [".exe", ".scr", ".lnk", ".bat", ".cmd", ".vbs", ".msi", ".ps1"]
warn_double_extension = true
verify_tls         = true

[storage]
preallocation     = "sparse"     # sparse | full
io_backend        = "auto"       # auto | pread | mmap | io_uring
hashing_threads   = 0            # 0 = nombre de cœurs
```

### 15.4 Aspect légal

Le protocole est légal : distributions Linux, mises à jour de jeux, jeux de données scientifiques, Internet Archive. Partager des œuvres protégées sans autorisation expose, en France, aux procédures de l'**Arcom** (anciennement Hadopi), qui repère les adresses IP dans les essaims. Prévoir une mention dans la documentation et l'assistant de premier lancement.
