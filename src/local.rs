//! Musique stockée sur le PC : simple parcours de dossiers, sans base de données.

use crate::model::{Source, Track};
use std::fs;
use std::path::{Path, PathBuf};

const AUDIO: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "oga", "opus", "wav", "aiff", "aif", "wma", "alac", "ape",
];

pub fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
}

/// Sous-dossiers et morceaux d'un dossier, triés par nom.
pub fn list(root: &Path, dir: &Path) -> (Vec<PathBuf>, Vec<Track>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')) {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else if is_audio(&p) {
                files.push(p);
            }
        }
    }
    dirs.sort();
    files.sort();
    (dirs, files.iter().map(|f| track(root, f)).collect())
}

/// Recherche par nom de fichier (chemin complet) dans tous les dossiers locaux.
pub fn search(roots: &[PathBuf], query: &str, limit: usize) -> Vec<Track> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if is_audio(&p)
                    && p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_lowercase().contains(&q)
                {
                    out.push(track(root, &p));
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// Titre = nom du fichier ; pour une arborescence Artiste/Album/morceau,
/// on déduit l'artiste et l'album des dossiers parents.
fn track(root: &Path, file: &Path) -> Track {
    let name = |p: Option<&Path>| {
        p.and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let depth = file.strip_prefix(root).map(|r| r.components().count()).unwrap_or(1);
    let parent = file.parent();
    Track {
        title: file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        album: if depth >= 2 { name(parent) } else { String::new() },
        artist: if depth >= 3 { name(parent.and_then(|p| p.parent())) } else { String::new() },
        duration: None,
        source: Source::Local(file.to_path_buf()),
    }
}
