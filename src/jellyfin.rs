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
#[derive(Clone, Debug)]
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

/// Erreur d'API : le refus de session (401) est traité à part, pour proposer
/// de se reconnecter au lieu d'afficher une erreur à chaque action.
#[derive(Debug, PartialEq)]
pub enum ApiError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => f.write_str("session Jellyfin refusée"),
            ApiError::Other(s) => f.write_str(s),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

fn err(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::StatusCode(401) => ApiError::Unauthorized,
        ureq::Error::StatusCode(c) => ApiError::Other(format!("Jellyfin a répondu HTTP {c}")),
        e => ApiError::Other(format!("Jellyfin injoignable : {e}")),
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
        .map_err(|e| match err(e) {
            ApiError::Unauthorized => "Identifiant ou mot de passe incorrect".to_string(),
            ApiError::Other(s) => s,
        })?;
    let v: Value = r.body_mut().read_json().map_err(|e| err(e).to_string())?;
    match (v["AccessToken"].as_str(), v["User"]["Id"].as_str()) {
        (Some(t), Some(id)) => Ok((t.to_string(), id.to_string())),
        _ => Err("réponse de connexion inattendue".into()),
    }
}

impl Jellyfin {
    fn base(&self) -> &str {
        self.url.trim_end_matches('/')
    }

    fn get(&self, path: &str) -> ApiResult<Value> {
        let mut r = AGENT
            .get(format!("{}{}", self.base(), path))
            .header("Authorization", auth_header(&self.device_id, Some(&self.token)))
            .call()
            .map_err(err)?;
        r.body_mut().read_json().map_err(err)
    }

    fn entries(&self, path: &str) -> ApiResult<Vec<Entry>> {
        let v = self.get(path)?;
        Ok(items(&v)
            .iter()
            .map(|i| Entry {
                id: i["Id"].as_str().unwrap_or_default().to_string(),
                name: i["Name"].as_str().unwrap_or("?").to_string(),
            })
            .collect())
    }

    fn tracks(&self, path: &str) -> ApiResult<Vec<Track>> {
        let v = self.get(path)?;
        Ok(items(&v).iter().map(track).collect())
    }

    pub fn playlists(&self) -> ApiResult<Vec<Entry>> {
        self.entries(&format!(
            "/Users/{}/Items?IncludeItemTypes=Playlist&Recursive=true&SortBy=SortName",
            self.user_id
        ))
    }

    pub fn artists(&self) -> ApiResult<Vec<Entry>> {
        self.entries(&format!("/Artists/AlbumArtists?UserId={}&SortBy=SortName", self.user_id))
    }

    pub fn albums(&self, artist: Option<&str>) -> ApiResult<Vec<Entry>> {
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

    pub fn playlist_tracks(&self, id: &str) -> ApiResult<Vec<Track>> {
        self.tracks(&format!("/Playlists/{id}/Items?UserId={}", self.user_id))
    }

    pub fn album_tracks(&self, id: &str) -> ApiResult<Vec<Track>> {
        self.tracks(&format!(
            "/Users/{}/Items?ParentId={id}&IncludeItemTypes=Audio&Recursive=true\
             &SortBy=ParentIndexNumber,IndexNumber,SortName",
            self.user_id
        ))
    }

    pub fn artist_tracks(&self, id: &str) -> ApiResult<Vec<Track>> {
        self.tracks(&format!(
            "/Users/{}/Items?ArtistIds={id}&IncludeItemTypes=Audio&Recursive=true\
             &SortBy=ProductionYear,Album,ParentIndexNumber,IndexNumber",
            self.user_id
        ))
    }

    pub fn search(&self, q: &str) -> ApiResult<Vec<Track>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<String>>>;

    /// Faux serveur Jellyfin : (préfixe du chemin, code HTTP, corps JSON).
    /// Renvoie son adresse et le journal des requêtes reçues.
    fn fake(routes: Vec<(&'static str, u16, &'static str)>) -> (String, Log) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let log: Log = Arc::default();
        let journal = log.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let (mut req, mut len) = (String::new(), 0);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    req.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                let _ = r.read_exact(&mut body);
                req.push_str(&String::from_utf8_lossy(&body));
                let path = req.split_whitespace().nth(1).unwrap_or_default().to_string();
                let (code, json) = routes
                    .iter()
                    .find(|(p, _, _)| path.starts_with(p))
                    .map(|&(_, c, j)| (c, j))
                    .unwrap_or((404, "{}"));
                journal.lock().unwrap().push(req);
                let _ = write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        (url, log)
    }

    fn client(url: &str) -> Jellyfin {
        Jellyfin { url: url.into(), token: "JETON".into(), user_id: "U1".into(), device_id: "D1".into() }
    }

    #[test]
    fn connexion_renvoie_jeton_et_utilisateur() {
        let (url, log) = fake(vec![("/Users/AuthenticateByName", 200, r#"{"AccessToken":"abc","User":{"Id":"u42"}}"#)]);
        assert_eq!(login(&url, "moi", "secret", "dev").unwrap(), ("abc".to_string(), "u42".to_string()));
        let req = &log.lock().unwrap()[0];
        assert!(req.starts_with("POST /Users/AuthenticateByName"));
        let body: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({ "Username": "moi", "Pw": "secret" }));
        assert!(req.contains(r#"DeviceId="dev""#) && !req.contains("Token="), "pas de jeton avant d'en avoir un");
    }

    #[test]
    fn mauvais_identifiants() {
        let (url, _) = fake(vec![("/Users/AuthenticateByName", 401, "{}")]);
        assert_eq!(login(&url, "moi", "faux", "dev").unwrap_err(), "Identifiant ou mot de passe incorrect");
    }

    #[test]
    fn reponse_de_connexion_incomplete() {
        let (url, _) = fake(vec![("/Users/AuthenticateByName", 200, r#"{"AccessToken":"abc"}"#)]);
        assert!(login(&url, "moi", "x", "dev").is_err());
    }

    #[test]
    fn jeton_refuse_donne_unauthorized() {
        let (url, _) = fake(vec![("/Users/U1/Items", 401, "{}")]);
        assert_eq!(client(&url).playlists().unwrap_err(), ApiError::Unauthorized);
    }

    #[test]
    fn autre_erreur_http_et_serveur_injoignable() {
        let (url, _) = fake(vec![("/Users/U1/Items", 500, "{}")]);
        assert!(matches!(client(&url).playlists(), Err(ApiError::Other(s)) if s.contains("500")));
        // Port fermé : refus de connexion immédiat.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        assert!(matches!(client(&url).playlists(), Err(ApiError::Other(s)) if s.starts_with("Jellyfin injoignable")));
    }

    #[test]
    fn playlists_avec_le_jeton_dans_l_en_tete() {
        let (url, log) = fake(vec![(
            "/Users/U1/Items",
            200,
            r#"{"Items":[{"Id":"p1","Name":"B.Rap"},{"Id":"p2","Name":"Chill"}]}"#,
        )]);
        let p = client(&url).playlists().unwrap();
        assert_eq!(p.iter().map(|e| (e.id.as_str(), e.name.as_str())).collect::<Vec<_>>(), [("p1", "B.Rap"), ("p2", "Chill")]);
        let req = &log.lock().unwrap()[0];
        assert!(req.contains(r#"Token="JETON""#));
        assert!(req.contains("IncludeItemTypes=Playlist"));
    }

    #[test]
    fn titres_d_une_playlist() {
        let (url, _) = fake(vec![(
            "/Playlists/p1/Items",
            200,
            r#"{"Items":[
                {"Id":"t1","Name":"Tchikita","Artists":["Jul"],"Album":"Album","RunTimeTicks":2210000000},
                {"Id":"t2","Name":"Sans artiste","AlbumArtist":"PNL"},
                {"Id":"t3"}
            ]}"#,
        )]);
        let t = client(&url).playlist_tracks("p1").unwrap();
        assert_eq!((t[0].title.as_str(), t[0].artist.as_str(), t[0].album.as_str()), ("Tchikita", "Jul", "Album"));
        assert_eq!(t[0].duration, Some(221.0));
        assert_eq!(t[0].source, Source::Jelly("t1".into()));
        assert_eq!((t[1].artist.as_str(), t[1].duration), ("PNL", None), "repli sur l'artiste de l'album");
        assert_eq!(t[2].title, "?", "un titre sans nom ne plante pas");
    }

    #[test]
    fn recherche_encode_la_requete() {
        let (url, log) = fake(vec![("/Users/U1/Items", 200, r#"{"Items":[]}"#)]);
        assert!(client(&url).search("Deux frères & co").unwrap().is_empty());
        assert!(log.lock().unwrap()[0].contains("searchTerm=Deux%20fr%C3%A8res%20%26%20co&"));
    }

    #[test]
    fn adresse_du_flux() {
        assert_eq!(
            client("http://srv:8096/").stream_url("t1"),
            "http://srv:8096/Audio/t1/stream?static=true&api_key=JETON"
        );
    }
}
