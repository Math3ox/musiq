use std::path::PathBuf;

#[derive(Clone, PartialEq)]
pub enum Source {
    Local(PathBuf),
    /// Identifiant d'élément Jellyfin.
    Jelly(String),
}

#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
    pub source: Source,
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
