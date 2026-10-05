//! Configuration persistante : ~/.config/musiq/config.json (droits 600, contient le jeton Jellyfin).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
    /// Égalisation du volume entre les titres : "track", "album" ou "no".
    #[serde(default = "default_replaygain")]
    pub replaygain: String,
    /// Qualité réduite (Opus 128 kb/s, converti par le serveur) pour écouter hors de chez soi.
    #[serde(default)]
    pub mobile_quality: bool,
    /// Fichier d'où vient cette configuration, et où elle est réenregistrée.
    #[serde(skip)]
    pub file: PathBuf,
}

fn default_dirs() -> Vec<PathBuf> {
    vec![home().join("Musique")]
}

fn default_label() -> String {
    "Jellyfin".into()
}

fn default_replaygain() -> String {
    "track".into()
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
            replaygain: default_replaygain(),
            mobile_quality: false,
            file: PathBuf::new(),
        }
    }
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Où l'on reprend au lancement : ~/.local/state/musiq/session.json.
pub fn session_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("state"));
    base.join("musiq").join("session.json")
}

/// File de lecture et position, enregistrées en quittant.
#[derive(Serialize, Deserialize, Default)]
pub struct Session {
    pub title: String,
    pub queue: Vec<crate::model::Track>,
    pub index: usize,
    pub position: f64,
}

impl Session {
    pub fn load(p: &Path) -> Option<Session> {
        let s: Session = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
        (!s.queue.is_empty()).then_some(s)
    }

    pub fn save(&self, p: &Path) -> io::Result<()> {
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(p, serde_json::to_string(self).map_err(io::Error::other)?)
    }
}

fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("musiq").join("config.json")
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&path())
    }

    pub fn save(&self) -> io::Result<()> {
        if self.file.as_os_str().is_empty() {
            return Err(io::Error::other("aucun fichier de configuration"));
        }
        self.save_to(&self.file)
    }

    /// Fichier absent ou illisible : configuration par défaut.
    pub fn load_from(p: &Path) -> Config {
        let mut c: Config = fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        c.file = p.to_path_buf();
        if c.device_id.is_empty() {
            c.device_id = random_hex(16);
        }
        c
    }

    pub fn save_to(&self, p: &Path) -> io::Result<()> {
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&p)?;
        // mode() ne joue qu'à la création : on resserre aussi un fichier existant.
        fs::set_permissions(p, fs::Permissions::from_mode(0o600))?;
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

    #[cfg(test)]
    pub fn with_seed(seed: u64) -> Rng {
        Rng(seed | 1)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("musiq-cfg-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p.join("config.json")
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn aller_retour_en_droits_600() {
        let p = tmp("aller-retour");
        let mut c = Config::load_from(&p);
        c.token = "jeton".into();
        c.user_id = "u1".into();
        c.volume = 30.0;
        c.save().unwrap();
        assert_eq!(mode(&p), 0o600);
        let l = Config::load_from(&p);
        assert_eq!((l.token.as_str(), l.user_id.as_str(), l.volume), ("jeton", "u1", 30.0));
        assert_eq!(l.device_id, c.device_id, "l'identifiant d'appareil reste stable");
        assert!(l.logged_in());
        fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn fichier_trop_ouvert_est_resserre() {
        let p = tmp("droits");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{}").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        Config::load_from(&p).save().unwrap();
        assert_eq!(mode(&p), 0o600);
        fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn ancienne_config_complete_par_defaut() {
        let p = tmp("ancienne");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, r#"{"jellyfin_url":"http://x:8096","token":"t"}"#).unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.jellyfin_url, "http://x:8096");
        assert_eq!(c.server_label, "Jellyfin");
        assert_eq!(c.volume, 50.0);
        assert_eq!(c.local_dirs, [home().join("Musique")]);
        assert_eq!(c.device_id.len(), 32);
        assert!(!c.logged_in(), "un jeton sans utilisateur ne suffit pas");
        fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn fichier_absent_ou_corrompu_donne_les_valeurs_par_defaut() {
        let p = tmp("corrompu");
        assert!(Config::load_from(&p).jellyfin_url.is_empty());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "pas du json {").unwrap();
        let c = Config::load_from(&p);
        assert_eq!((c.volume, c.server_label.as_str()), (50.0, "Jellyfin"));
        fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn session_aller_retour() {
        let p = tmp("session");
        let t = crate::model::Track {
            title: "Tchikita".into(),
            artist: "Jul".into(),
            album: String::new(),
            duration: Some(221.0),
            source: crate::model::Source::Jelly("t1".into()),
        };
        let s = Session { title: "B.Rap".into(), queue: vec![t.clone(), t], index: 1, position: 42.5 };
        s.save(&p).unwrap();
        let l = Session::load(&p).unwrap();
        assert_eq!((l.title.as_str(), l.queue.len(), l.index, l.position), ("B.Rap", 2, 1, 42.5));
        assert_eq!(l.queue[0].source, crate::model::Source::Jelly("t1".into()));
        Session::default().save(&p).unwrap();
        assert!(Session::load(&p).is_none(), "une file vide ne se reprend pas");
        fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn nouveaux_reglages_par_defaut() {
        let c = Config::default();
        assert_eq!((c.replaygain.as_str(), c.mobile_quality), ("track", false));
    }

    #[test]
    fn sans_fichier_on_n_ecrit_nulle_part() {
        assert!(Config::default().save().is_err());
    }

    #[test]
    fn hasard_dans_les_bornes() {
        let mut r = Rng::with_seed(3);
        assert!((0..1000).all(|_| r.below(7) < 7));
        assert_eq!(r.below(0), 0);
    }
}
