//! Configuration persistante : ~/.config/musiq/config.json (droits 600, contient le jeton Jellyfin).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    /// Adresse du serveur Jellyfin, saisie à la première connexion.
    #[serde(default)]
    pub jellyfin_url: String,
    /// Nom du serveur dans la barre de gauche.
    #[serde(default = "default_label")]
    pub server_label: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_id: String,
    /// Jeton de session Jellyfin. Le mot de passe, lui, n'est jamais stocké.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default = "default_dirs")]
    pub local_dirs: Vec<PathBuf>,
    /// Volume au démarrage (en %). Ce que l'on règle pendant l'écoute n'est pas retenu.
    #[serde(default = "default_volume")]
    pub volume: f64,
}

fn default_dirs() -> Vec<PathBuf> {
    vec![home().join("Musique")]
}

fn default_label() -> String {
    "Jellyfin".into()
}

fn default_volume() -> f64 {
    50.0
}

impl Default for Config {
    fn default() -> Self {
        Config {
            jellyfin_url: String::new(),
            server_label: default_label(),
            user_name: String::new(),
            user_id: String::new(),
            token: String::new(),
            device_id: String::new(),
            local_dirs: default_dirs(),
            volume: default_volume(),
        }
    }
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("musiq").join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let mut c: Config = fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if c.device_id.is_empty() {
            c.device_id = random_hex(16);
        }
        c
    }

    pub fn save(&self) -> io::Result<()> {
        let p = path();
        fs::create_dir_all(p.parent().unwrap())?;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&p)?;
        // mode() ne joue qu'à la création : on resserre aussi un fichier existant.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600))?;
        let s = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        f.write_all(s.as_bytes())
    }

    pub fn logged_in(&self) -> bool {
        !self.token.is_empty() && !self.user_id.is_empty()
    }
}

/// Petit générateur pseudo-aléatoire (xorshift) : suffisant pour un identifiant
/// d'appareil et le mode aléatoire, sans dépendance supplémentaire.
pub struct Rng(u64);

impl Rng {
    pub fn new() -> Rng {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Rng(t ^ ((std::process::id() as u64) << 32) | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn random_hex(bytes: usize) -> String {
    let mut r = Rng::new();
    (0..bytes).map(|_| format!("{:02x}", r.next() as u8)).collect()
}
