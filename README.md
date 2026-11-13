<p align="center">
  <img src="app-icon.png" width="120" alt="Logo Pocket Torrent">
</p>

<h1 align="center">Pocket Torrent</h1>

<p align="center">
  Client BitTorrent pour Windows, avec VPN intégré et téléchargements directs.<br>
  Rapide, privé, sans compte ni télémétrie.
</p>

<p align="center">
  <a href="https://github.com/Zyko0w/pocket-torrent-releases/releases/latest"><b>⬇️ Télécharger la dernière version</b></a>
</p>

<p align="center">
  <img alt="Version" src="https://img.shields.io/badge/version-0.5.3-7C3AED">
  <img alt="Windows" src="https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows&logoColor=white">
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-moteur-000000?logo=rust&logoColor=white">
  <img alt="React 19" src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black">
</p>

---

## Présentation

Pocket Torrent est un client BitTorrent léger pour Windows, construit sur un moteur Rust. Il gère les liens magnet,
les fichiers `.torrent` et les liens directs, protège la connexion avec un VPN WireGuard intégré, et peut même mettre
les fichiers terminés en ligne avec un lien de partage.

## Fonctionnalités

### ⬇️ Téléchargement
- Liens magnet, fichiers `.torrent` (glisser-déposer compris) et **liens directs HTTP(S)** avec reprise.
- Sélection et priorité des fichiers, avant comme pendant le téléchargement.
- File d'attente à trois niveaux de priorité, limites de vitesse globales et par torrent, planificateur horaire.
- Catégories avec dossier dédié et dossier des téléchargements en cours.
- **Flux RSS de séries** : ajout automatique des nouveaux épisodes (RSS, Atom, Jackett, Prowlarr, filtres regex).
- Partage avec ratio et durée maximum.

### 🛡️ Confidentialité
- **Tunnel WireGuard intégré** (import du `.conf` du fournisseur), limité aux téléchargements.
- Liaison stricte à l'interface du VPN, avec arrêt immédiat en cas de coupure et reprise automatique.
- Proxy SOCKS5 sans fuite, redirection de port ProtonVPN (NAT-PMP).
- Chiffrement entre pairs (MSE) et DNS over HTTPS.
- Liste de blocage d'IP, détection des fichiers dangereux, analyse Microsoft Defender en option.
- **Aucune télémétrie, aucun compte.**

### ☁️ Mise en ligne
- Lien de partage en un clic sur **storage.to, GoFile, VikingFile** ou ton propre hébergeur (import des `.sxcu` de ShareX).
- Automatique à la fin du téléchargement, ou à la demande ; durée de vie du lien et limite de téléchargements au choix.

### 🎮 Confort
- **Mode jeu** : ralentissement ou pause automatique au lancement d'un jeu.
- Graphique du débit en direct, statistiques et état des trackers.
- Arrêt du PC à la fin, démarrage avec Windows, notifications, association des liens magnet et `.torrent`.
- Mises à jour signées, proposées au démarrage et installées seulement après accord.

### ⚙️ Moteur
- Basé sur [librqbit](https://github.com/ikatson/rqbit) (Rust), étendu pour Pocket Torrent.
- DHT, PEX, trackers HTTP/UDP, µTP, sources web (BEP 19), extension Fast (BEP 6), partage partiel (BEP 21),
  priorité des pairs (BEP 40), DHT sécurisée (BEP 42), perçage de NAT (BEP 55), UPnP et NAT-PMP.

## Installation

1. Télécharge `PocketTorrent_<version>_x64-setup.exe` depuis la
   [dernière version](https://github.com/Zyko0w/pocket-torrent-releases/releases/latest).
2. Lance l'installateur (aucun droit administrateur requis).
3. Si SmartScreen affiche un avertissement : **Informations complémentaires**, puis **Exécuter quand même**.

Configuration requise : Windows 10 ou 11, 64 bits.

## Développement

| Partie | Techno |
|---|---|
| Interface | React 19, TypeScript, Vite |
| Application | Tauri 2 |
| Moteur | Rust, librqbit |
| Tests | Vitest, Testing Library |

```bash
npm install
npm run tauri dev      # lance l'application en développement
npm run tauri build    # produit l'installateur
npm test               # tests de l'interface
```

Prérequis : [Rust](https://rustup.rs) et les Microsoft C++ Build Tools.

## Sécurité

Le projet a fait l'objet d'un audit et d'un test d'intrusion complets : voir [`docs/`](docs) et [`pentest/`](pentest).

## Utilisation responsable

Pocket Torrent est un outil de partage de fichiers. Son utilisation doit se limiter aux contenus que tu es autorisé
à télécharger et à partager.
