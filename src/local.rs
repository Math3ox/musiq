//! Musique stockée sur le PC : simple parcours de dossiers, sans base de données.
//! Les tags (titre, artiste, album, durée, piste) sont lus à l'ouverture d'un
//! dossier ; les noms de dossiers servent de secours.

use crate::model::{Source, Track};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
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
    // Ordre des pistes (disque, numéro) quand les tags le donnent, sinon par nom.
    let mut tracks: Vec<(Option<(u32, u32)>, Track)> = files.iter().map(|f| tagged(root, f)).collect();
    if tracks.iter().all(|(n, _)| n.is_some()) {
        tracks.sort_by_key(|(n, _)| *n);
    }
    (dirs, tracks.into_iter().map(|(_, t)| t).collect())
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
                    out.push(tagged(root, &p).1);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// Morceau décrit par ses tags, complétés par `track` pour ce qui manque.
/// Renvoie aussi (disque, piste) si le numéro de piste est connu.
fn tagged(root: &Path, file: &Path) -> (Option<(u32, u32)>, Track) {
    let mut t = track(root, file);
    let Ok(f) = lofty::read_from_path(file) else { return (None, t) };
    let d = f.properties().duration().as_secs_f64();
    if d > 0.0 {
        t.duration = Some(d);
    }
    let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) else { return (None, t) };
    let set = |field: &mut String, v: Option<std::borrow::Cow<str>>| {
        if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
            *field = v.trim().to_string();
        }
    };
    set(&mut t.title, tag.title());
    set(&mut t.artist, tag.artist());
    set(&mut t.album, tag.album());
    (tag.track().map(|n| (tag.disk().unwrap_or(1), n)), t)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Dossier temporaire supprimé à la fin du test.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("musiq-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }

        fn file(&self, rel: &str) {
            let f = self.0.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, b"").unwrap();
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn titles(t: &[Track]) -> Vec<&str> {
        t.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn reconnait_les_extensions_audio() {
        assert!(is_audio(Path::new("a.FLAC")));
        assert!(is_audio(Path::new("x/b.mp3")));
        assert!(is_audio(Path::new("c.opus")));
        assert!(!is_audio(Path::new("cover.jpg")));
        assert!(!is_audio(Path::new("sans_extension")));
    }

    #[test]
    fn liste_triee_sans_fichiers_caches_ni_autres_fichiers() {
        let t = Tmp::new("liste");
        for f in ["B/x.mp3", "A/y.mp3", "2.flac", "1.mp3", "cover.jpg", ".cache.mp3", ".cache/z.mp3"] {
            t.file(f);
        }
        let (dirs, tracks) = list(&t.0, &t.0);
        let names: Vec<_> = dirs.iter().map(|d| d.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(titles(&tracks), ["1", "2"]);
    }

    #[test]
    fn deduit_artiste_et_album_des_dossiers() {
        let t = Tmp::new("tags");
        t.file("Daft Punk/Discovery/01 One More Time.flac");
        t.file("Discovery/02 Aerodynamic.mp3");
        t.file("seul.mp3");

        let (_, a) = list(&t.0, &t.0.join("Daft Punk/Discovery"));
        assert_eq!((a[0].title.as_str(), a[0].artist.as_str(), a[0].album.as_str()),
                   ("01 One More Time", "Daft Punk", "Discovery"));
        assert_eq!(a[0].source, Source::Local(t.0.join("Daft Punk/Discovery/01 One More Time.flac")));

        let (_, b) = list(&t.0, &t.0.join("Discovery"));
        assert_eq!((b[0].artist.as_str(), b[0].album.as_str()), ("", "Discovery"));

        let (_, c) = list(&t.0, &t.0);
        assert_eq!((c[0].artist.as_str(), c[0].album.as_str()), ("", ""));
    }

    #[test]
    fn recherche_insensible_a_la_casse_et_limitee() {
        let t = Tmp::new("recherche");
        for f in ["Jul/Album/Tchikita.mp3", "Jul/Album/Autre.mp3", "PNL/Deux frères/Au DD.flac", "PNL/notes.txt"] {
            t.file(f);
        }
        let roots = [t.0.clone()];
        assert_eq!(search(&roots, "jul", 10).len(), 2, "le nom du dossier compte aussi");
        assert_eq!(titles(&search(&roots, "au dd", 10)), ["Au DD"]);
        assert_eq!(search(&roots, "jul", 1).len(), 1);
        assert!(search(&roots, "notes", 10).is_empty(), "seulement les fichiers audio");
        assert!(search(&roots, "introuvable", 10).is_empty());
    }

    #[test]
    fn dossier_absent_ne_plante_pas() {
        let absent = Path::new("/nexiste/vraiment/pas");
        let (dirs, tracks) = list(absent, absent);
        assert!(dirs.is_empty() && tracks.is_empty());
        assert!(search(&[absent.to_path_buf()], "x", 10).is_empty());
    }
}
