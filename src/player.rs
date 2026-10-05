//! Lecture audio : un processus mpv en arrière-plan, piloté par son socket IPC JSON.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

pub enum Event {
    Time(f64),
    Duration(f64),
    Pause(bool),
    Volume(f64),
    /// Morceau terminé normalement : on enchaîne.
    Eof,
    /// Morceau illisible (fichier absent, flux coupé...).
    Error(String),
}

/// Module MPRIS de mpv (paquet `mpv-mpris`) : touches multimédia, widget audio
/// du bureau, écran de verrouillage. Facultatif.
const MPRIS: &[&str] = &[
    "/usr/lib64/mpv/mpris.so",
    "/usr/lib/mpv/mpris.so",
    "/etc/mpv/scripts/mpris.so",
    "/usr/lib/x86_64-linux-gnu/mpv/mpris.so",
];

pub fn mpris_plugin() -> Option<&'static str> {
    MPRIS.iter().copied().find(|p| std::path::Path::new(p).exists())
}

pub struct Player {
    child: Child,
    stream: UnixStream,
    sock: PathBuf,
}

impl Player {
    /// `replaygain` : "track", "album" ou "no".
    pub fn start<M: From<Event> + Send + 'static>(
        tx: Sender<M>,
        volume: f64,
        replaygain: &str,
    ) -> Result<Player, String> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let sock = dir.join(format!("musiq-mpv-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let mut args: Vec<String> = [
            "--idle=yes",
            "--no-terminal",
            "--no-config",
            // Rien que du son : ni vidéo, ni pochette, ni fenêtre.
            "--vid=no",
            "--audio-display=no",
            // Modules intégrés inutiles ici (interface à l'écran, YouTube,
            // console, statistiques…) : autant de mémoire économisée.
            "--osc=no",
            "--ytdl=no",
            "--load-scripts=no",
            "--load-stats-overlay=no",
            "--load-console=no",
            "--load-auto-profiles=no",
            "--load-select=no",
            "--load-commands=no",
            "--load-positioning=no",
            "--load-context-menu=no",
            "--input-default-bindings=no",
            "--input-vo-keyboard=no",
            // Tampon réseau plafonné : par défaut, mpv peut garder des dizaines
            // de Mo d'un FLAC en streaming. 4 Mo = une bonne vingtaine de secondes.
            "--cache=yes",
            "--demuxer-max-bytes=4MiB",
            "--demuxer-max-back-bytes=512KiB",
            // Enchaînement sans blanc : le titre suivant est préchargé.
            "--gapless-audio=yes",
            "--prefetch-playlist=yes",
        ]
        .map(String::from)
        .into();
        args.push(format!("--replaygain={replaygain}"));
        args.push(format!("--input-ipc-server={}", sock.display()));
        args.push(format!("--volume={volume}"));
        if let Some(so) = mpris_plugin() {
            args.push(format!("--script={so}"));
        }
        let child = Command::new("mpv")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("impossible de lancer mpv ({e}) — sudo dnf install mpv"))?;

        let mut stream = None;
        for _ in 0..100 {
            if let Ok(s) = UnixStream::connect(&sock) {
                stream = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        let mut p = Player {
            child,
            stream: stream.ok_or("mpv ne répond pas sur son socket")?,
            sock,
        };
        let reader = p.stream.try_clone().map_err(|e| e.to_string())?;
        thread::spawn(move || read_events(reader, tx));
        for (i, name) in ["time-pos", "duration", "pause", "volume"].iter().enumerate() {
            p.cmd(json!(["observe_property", i + 1, name]));
        }
        Ok(p)
    }

    fn cmd(&mut self, c: Value) {
        let _ = writeln!(self.stream, "{}", json!({ "command": c }));
    }

    /// Joue `target` tout de suite, à la place de tout le reste.
    pub fn load(&mut self, target: &str) {
        self.cmd(json!(["loadfile", target, "replace"]));
        self.cmd(json!(["playlist-clear"]));
        self.cmd(json!(["set_property", "pause", false]));
    }

    /// Charge `target` en pause, positionné à `start` secondes (reprise de session).
    pub fn load_paused(&mut self, target: &str, start: f64) {
        self.cmd(json!(["set_property", "pause", true]));
        self.cmd(json!(["loadfile", target, "replace", -1, format!("start={start:.1}")]));
        self.cmd(json!(["playlist-clear"]));
    }

    /// Remplace le titre préchargé par `target` (None : plus rien après le courant).
    pub fn set_upcoming(&mut self, target: Option<&str>) {
        self.cmd(json!(["playlist-clear"]));
        if let Some(t) = target {
            self.cmd(json!(["loadfile", t, "append"]));
        }
    }

    /// mpv vient d'enchaîner sur le titre préchargé : on oublie celui d'avant.
    pub fn drop_finished(&mut self) {
        self.cmd(json!(["playlist-remove", 0]));
    }

    pub fn toggle_pause(&mut self) {
        self.cmd(json!(["cycle", "pause"]));
    }

    pub fn seek(&mut self, secs: f64) {
        self.cmd(json!(["seek", secs, "relative"]));
    }

    pub fn seek_percent(&mut self, pct: f64) {
        self.cmd(json!(["seek", pct.clamp(0.0, 100.0), "absolute-percent"]));
    }

    pub fn add_volume(&mut self, delta: f64) {
        self.cmd(json!(["add", "volume", delta]));
    }

    pub fn stop(&mut self) {
        self.cmd(json!(["stop"]));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.cmd(json!(["quit"]));
        thread::sleep(Duration::from_millis(50));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

fn read_events<M: From<Event>>(stream: UnixStream, tx: Sender<M>) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ev = match v["event"].as_str() {
            Some("property-change") => {
                let d = &v["data"];
                match v["name"].as_str() {
                    Some("time-pos") => d.as_f64().map(Event::Time),
                    Some("duration") => d.as_f64().map(Event::Duration),
                    Some("pause") => d.as_bool().map(Event::Pause),
                    Some("volume") => d.as_f64().map(Event::Volume),
                    _ => None,
                }
            }
            // reason "stop" = remplacé par un autre morceau : rien à faire.
            Some("end-file") => match v["reason"].as_str() {
                Some("eof") => Some(Event::Eof),
                Some("error") => Some(Event::Error(
                    v["file_error"].as_str().unwrap_or("lecture impossible").to_string(),
                )),
                _ => None,
            },
            _ => None,
        };
        if let Some(ev) = ev {
            if tx.send(ev.into()).is_err() {
                break;
            }
        }
    }
}
