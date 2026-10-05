# musiq

Lecteur de musique minimaliste **en mode terminal**, qui se pilote au clavier
et à la souris. Il lit la musique stockée sur le PC et celle d'un serveur
[Jellyfin](https://jellyfin.org) (playlists, artistes, albums, recherche).

Pendant la lecture, un CD vu de trois quarts tourne façon vieil autoradio :
ses reflets s'allument secteur par secteur.

- Écrit en Rust avec [ratatui](https://ratatui.rs) ; la lecture est confiée à
  [mpv](https://mpv.io), piloté en arrière-plan.
- Les morceaux Jellyfin sont lus dans leur format d'origine (FLAC compris),
  sans transcodage : pensé pour un serveur sur le réseau local.

## Prérequis

- Linux, un terminal qui affiche les couleurs 24 bits (Konsole, GNOME Terminal,
  kitty, Alacritty…)
- `mpv` : `sudo dnf install mpv` (Fedora) ou `sudo apt install mpv` (Debian/Ubuntu)
- Rust, pour compiler : <https://rustup.rs>

## Installer

    git clone https://github.com/Math3ox/musiq.git
    cd musiq
    cargo build --release
    install -m 755 target/release/musiq ~/.local/bin/

Puis lancer `musiq`.

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
| `user_name`, `user_id`, `token` | session Jellyfin, remplis à la connexion | — |

Seul un **jeton de session** Jellyfin est enregistré, jamais le mot de passe.
Pour se déconnecter, vider `token` (ou supprimer le fichier).

## Touches

| Touche | Action |
|---|---|
| ↑ ↓ / molette | naviguer |
| → / Entrée | ouvrir (dossier, playlist, album…) — Entrée sur un titre : le lire |
| ← | fermer / remonter |
| Tab | changer de panneau |
| Espace | pause (depuis n'importe où) |
| n / p | titre suivant / précédent |
| , / . | reculer / avancer de 10 s (← → dans le panneau Lecture) |
| + / - | volume |
| s | lecture aléatoire |
| / | rechercher (serveur + PC) |
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
