//! Accès à la musique d'un serveur Jellyfin via son API (bibliothèque, playlists, flux audio).

use crate::model::{Source, Track};
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into()
});

#[derive(Clone)]
pub struct Jellyfin {
    pub url: String,
    pub token: String,
    pub user_id: String,
    pub device_id: String,
}

/// Ce qu'on peut lister côté serveur dans la barre de gauche.
#[derive(Clone)]
pub struct Entry {
    pub id: String,
    pub name: String,
}

fn auth_header(device_id: &str, token: Option<&str>) -> String {
    let mut h = format!(
        "MediaBrowser Client=\"musiq\", Device=\"terminal\", DeviceId=\"{device_id}\", Version=\"{}\"",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(t) = token {
        h.push_str(&format!(", Token=\"{t}\""));
    }
    h
}

fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401) => "accès refusé (identifiants ou session expirée)".into(),
        ureq::Error::StatusCode(c) => format!("Jellyfin a répondu HTTP {c}"),
        e => format!("Jellyfin injoignable : {e}"),
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Connexion : renvoie (jeton, id utilisateur). Le mot de passe ne sort pas d'ici.
pub fn login(url: &str, user: &str, pw: &str, device_id: &str) -> Result<(String, String), String> {
    let mut r = AGENT
        .post(format!("{}/Users/AuthenticateByName", url.trim_end_matches('/')))
        .header("Authorization", auth_header(device_id, None))
        .send_json(json!({ "Username": user, "Pw": pw }))
        .map_err(err)?;
    let v: Value = r.body_mut().read_json().map_err(err)?;
    match (v["AccessToken"].as_str(), v["User"]["Id"].as_str()) {
        (Some(t), Some(id)) => Ok((t.to_string(), id.to_string())),
        _ => Err("réponse de connexion inattendue".into()),
    }
}

impl Jellyfin {
    fn base(&self) -> &str {
        self.url.trim_end_matches('/')
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let mut r = AGENT
            .get(format!("{}{}", self.base(), path))
            .header("Authorization", auth_header(&self.device_id, Some(&self.token)))
            .call()
            .map_err(err)?;
        r.body_mut().read_json().map_err(err)
    }

    fn entries(&self, path: &str) -> Result<Vec<Entry>, String> {
        let v = self.get(path)?;
        Ok(items(&v)
            .iter()
            .map(|i| Entry {
                id: i["Id"].as_str().unwrap_or_default().to_string(),
                name: i["Name"].as_str().unwrap_or("?").to_string(),
            })
            .collect())
    }

    fn tracks(&self, path: &str) -> Result<Vec<Track>, String> {
        let v = self.get(path)?;
        Ok(items(&v).iter().map(track).collect())
    }

    pub fn playlists(&self) -> Result<Vec<Entry>, String> {
        self.entries(&format!(
            "/Users/{}/Items?IncludeItemTypes=Playlist&Recursive=true&SortBy=SortName",
            self.user_id
        ))
    }

    pub fn artists(&self) -> Result<Vec<Entry>, String> {
        self.entries(&format!("/Artists/AlbumArtists?UserId={}&SortBy=SortName", self.user_id))
    }

    pub fn albums(&self, artist: Option<&str>) -> Result<Vec<Entry>, String> {
        let mut p = format!(
            "/Users/{}/Items?IncludeItemTypes=MusicAlbum&Recursive=true",
            self.user_id
        );
        match artist {
            Some(a) => p.push_str(&format!("&AlbumArtistIds={a}&SortBy=ProductionYear,SortName")),
            None => p.push_str("&SortBy=SortName"),
        }
        self.entries(&p)
    }

    pub fn playlist_tracks(&self, id: &str) -> Result<Vec<Track>, String> {
        self.tracks(&format!("/Playlists/{id}/Items?UserId={}", self.user_id))
    }

    pub fn album_tracks(&self, id: &str) -> Result<Vec<Track>, String> {
        self.tracks(&format!(
            "/Users/{}/Items?ParentId={id}&IncludeItemTypes=Audio&Recursive=true\
             &SortBy=ParentIndexNumber,IndexNumber,SortName",
            self.user_id
        ))
    }

    pub fn artist_tracks(&self, id: &str) -> Result<Vec<Track>, String> {
        self.tracks(&format!(
            "/Users/{}/Items?ArtistIds={id}&IncludeItemTypes=Audio&Recursive=true\
             &SortBy=ProductionYear,Album,ParentIndexNumber,IndexNumber",
            self.user_id
        ))
    }

    pub fn search(&self, q: &str) -> Result<Vec<Track>, String> {
        self.tracks(&format!(
            "/Users/{}/Items?searchTerm={}&IncludeItemTypes=Audio&Recursive=true&Limit=300",
            self.user_id,
            enc(q)
        ))
    }

    /// Fichier d'origine, sans transcodage (FLAC compris) : pensé pour un réseau local.
    pub fn stream_url(&self, id: &str) -> String {
        format!("{}/Audio/{id}/stream?static=true&api_key={}", self.base(), self.token)
    }
}

fn items(v: &Value) -> Vec<Value> {
    v["Items"]
        .as_array()
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn track(i: &Value) -> Track {
    let artist = i["Artists"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a.as_str())
        .or_else(|| i["AlbumArtist"].as_str())
        .unwrap_or_default();
    Track {
        title: i["Name"].as_str().unwrap_or("?").to_string(),
        artist: artist.to_string(),
        album: i["Album"].as_str().unwrap_or_default().to_string(),
        duration: i["RunTimeTicks"].as_f64().map(|t| t / 10_000_000.0),
        source: Source::Jelly(i["Id"].as_str().unwrap_or_default().to_string()),
    }
}
