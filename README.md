# musiq

Lecteur de musique minimaliste **en mode terminal**, qui se pilote au clavier
et à la souris. Il lit la musique stockée sur le PC et celle d'un serveur
[Jellyfin](https://jellyfin.org) (playlists, artistes, albums, recherche).

Pendant la lecture, un CD vu de trois quarts tourne façon vieil autoradio :
ses reflets s'allument secteur par secteur.

- Écrit en Rust avec [ratatui](https://ratatui.rs) ; la lecture est confiée à
  [mpv](https://mpv.io), piloté en arrière-plan.
- Les morceaux Jellyfin sont lus dans leur format d'origine (FLAC compris),
  sans transcodage, ou en **qualité mobile** (Opus 128 kb/s converti par le
  serveur) pour écouter hors de chez soi.
- Enchaînement **sans blanc** entre les titres, **volume homogène**
  (ReplayGain), **file d'attente**, reprise là où on s'était arrêté, et
  remontée des écoutes vers Jellyfin.
- **Égaliseur 10 bandes** avec préréglages, réglable à chaud sans coupure.
- Interface qui s'adapte à la taille de la fenêtre : colonnes masquées,
  panneau compact, commandes repliées sur plusieurs lignes (**?** les liste toutes).
- Léger : ≈ 3 Mo de mémoire pour l'interface (mpv ≈ 35 Mo pour le son),
  aucun réveil ni redessin quand la lecture est en pause ; l'égaliseur actif
  coûte moins de 0,1 % de CPU, et rien du tout quand il est coupé.

## Prérequis

- Linux, un terminal qui affiche les couleurs 24 bits (Konsole, GNOME Terminal,
  kitty, Alacritty…)
- `mpv` : `sudo dnf install mpv` (Fedora) ou `sudo apt install mpv` (Debian/Ubuntu)
- facultatif, pour les **touches multimédia** du clavier et le widget audio du
  bureau (MPRIS) : `sudo dnf install mpv-mpris` ou `sudo apt install mpv-mpris`
  — détecté automatiquement

## Installer

Binaire prêt à l'emploi (Linux x86_64) : page
[Releases](https://github.com/Math3ox/musiq/releases), archive
`musiq-vX.Y.Z-linux-x86_64.tar.gz`, puis :

    tar -xzf musiq-*-linux-x86_64.tar.gz
    install -m 755 musiq-*-linux-x86_64/musiq ~/.local/bin/

Ou depuis les sources, avec Rust (<https://rustup.rs>) :

    git clone https://github.com/Math3ox/musiq.git
    cd musiq
    cargo build --release
    install -m 755 target/release/musiq ~/.local/bin/

Puis lancer `musiq`.

Tests : `cargo test` (aucun serveur ni mpv requis : un faux serveur Jellyfin
est lancé en local).

## Configurer

Rien à modifier dans le code. Au premier lancement :

1. appuyer sur **c** et saisir l'**adresse du serveur Jellyfin**
   (par exemple `http://mon-serveur:8096`), l'identifiant et le mot de passe ;
2. mettre sa musique dans `~/Musique`, ou indiquer d'autres dossiers dans le
   fichier de configuration.

Le fichier de configuration est créé au premier lancement :
`~/.config/musiq/config.json` (droits 600).

| Clé | Rôle | Par défaut |
|---|---|---|
| `jellyfin_url` | adresse du serveur Jellyfin (renseignée à la connexion) | vide |
| `server_label` | nom du serveur dans la barre de gauche | `Jellyfin` |
| `local_dirs` | dossiers de musique du PC | `["~/Musique"]` (chemin complet) |
| `volume` | volume au démarrage, en % | `50` |
| `replaygain` | égalisation du volume : `track`, `album` ou `no` | `track` |
| `mobile_quality` | qualité réduite (Opus 128 kb/s), basculable avec **m** | `false` |
| `eq_enabled`, `eq_gains`, `eq_preset` | égaliseur (réglé depuis **E**) | coupé, à plat |
| `user_name`, `user_id`, `token` | session Jellyfin, remplis à la connexion | — |

Seul un **jeton de session** Jellyfin est enregistré, jamais le mot de passe.
Pour se déconnecter, vider `token` (ou supprimer le fichier). Si la session
expire, musiq propose directement de se reconnecter.

En quittant, la file de lecture et la position sont enregistrées dans
`~/.local/state/musiq/session.json` : au lancement suivant, on reprend au même
endroit, en pause.

## Touches

| Touche | Action |
|---|---|
| ↑ ↓ / molette | naviguer |
| → / Entrée | ouvrir (dossier, playlist, album…) — Entrée sur un titre : le lire |
| a | ajouter le titre à la fin de la file |
| e | lire le titre juste après celui en cours |
| Suppr | retirer un titre de la file (dans « File d'attente ») |
| f | filtrer la liste affichée, au fil de la frappe (Échap efface) |
| ← | fermer / remonter |
| Tab | changer de panneau |
| Espace | pause (depuis n'importe où) |
| n / p | titre suivant / précédent |
| , / . | reculer / avancer de 10 s (← → dans le panneau Lecture) |
| + / - | volume |
| s | lecture aléatoire |
| / | rechercher (serveur + PC) — Entrée sur un résultat le glisse dans la file en cours |
| m | qualité mobile / qualité d'origine |
| E (ou é) | égaliseur : ←→ bande, ↑↓ ±1 dB, p préréglage, o activer/couper, 0 remise à zéro ; clic ou glisser sur une barre |
| ? | liste de toutes les touches |
| c | se connecter à Jellyfin |
| q | quitter |

À la souris : un clic sur un titre le lance, un clic sur une source l'ouvre,
un clic sur la barre de progression déplace la lecture.

## Lanceur (KDE Plasma)

Pour lancer musiq depuis le menu des applications ou le bureau, sans taper de
commande :

    install -m 644 packaging/musiq.svg ~/.local/share/icons/hicolor/scalable/apps/
    install -m 644 packaging/musiq.desktop ~/.local/share/applications/
    install -m 755 packaging/musiq.desktop "$(xdg-user-dir DESKTOP)/"

Le lanceur ouvre musiq dans une fenêtre Konsole sans menu ni onglets, et
suppose que `~/.local/bin` est dans le `PATH`. Avec un autre terminal, adapter
la ligne `Exec=` de `packaging/musiq.desktop`.
