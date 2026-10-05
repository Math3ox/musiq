//! État de l'application : arbre des sources, liste de titres, file de lecture, saisies.

use crate::config::{Config, Rng, Session};
use crate::eq;
use crate::jellyfin::{self, ApiError, Entry, Jellyfin, Report};
use crate::local;
use crate::model::{Source, Track};
use crate::player::{self, Player};
use crate::queue::Queue;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{ListState, TableState};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// Durée d'affichage d'un secteur du CD : l'animation avance par crans.
const SECTOR_STEP: f64 = 0.11;
/// Un message d'état reste affiché ce temps-là.
const STATUS_LIFE: Duration = Duration::from_secs(5);
/// Fréquence de la remontée « en cours d'écoute » vers Jellyfin.
const REPORT_EVERY: Duration = Duration::from_secs(10);

pub enum Msg {
    /// Clavier, souris, redimensionnement : lus par un fil dédié.
    Input(Event),
    Player(player::Event),
    Children { path: Vec<usize>, result: Result<Vec<Node>, ApiError> },
    Tracks { req: u64, title: String, result: Result<Vec<Track>, ApiError> },
    Login(Result<(String, String), String>),
}

impl From<player::Event> for Msg {
    fn from(e: player::Event) -> Self {
        Msg::Player(e)
    }
}

#[derive(Clone)]
pub enum Kind {
    Queue,
    PcRoot,
    Dir { root: PathBuf, path: PathBuf },
    ServerRoot,
    Login,
    Playlists,
    Artists,
    Albums,
    Playlist(String),
    Artist(String),
    Album(String),
}

#[derive(Clone)]
pub struct Node {
    pub label: String,
    pub kind: Kind,
    pub expanded: bool,
    pub children: Option<Vec<Node>>,
    pub loading: bool,
}

impl Node {
    fn new(label: impl Into<String>, kind: Kind) -> Node {
        Node { label: label.into(), kind, expanded: false, children: None, loading: false }
    }

    pub fn expandable(&self) -> bool {
        !matches!(self.kind, Kind::Playlist(_) | Kind::Album(_) | Kind::Login | Kind::Queue)
    }
}

/// Une ligne visible de la barre de gauche (chemin d'indices dans l'arbre).
pub struct Row {
    pub path: Vec<usize>,
    pub depth: usize,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Sidebar,
    Tracks,
    Player,
}

pub struct LoginForm {
    pub url: String,
    pub user: String,
    pub pw: String,
    pub field: usize,
    pub busy: bool,
    pub error: Option<String>,
}

pub enum Mode {
    Normal,
    Login(LoginForm),
    Search(String),
    /// Filtre de la liste affichée, appliqué au fil de la frappe.
    Filter,
    /// Égaliseur ouvert, bande sélectionnée.
    Eq { band: usize },
    /// Liste de toutes les touches.
    Help,
}

/// Ce que montre le panneau de droite : il change la réaction à Entrée.
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum View {
    List,
    Search,
    Queue,
}

pub struct App {
    pub cfg: Config,
    tx: Sender<Msg>,
    pub tree: Vec<Node>,
    pub rows: Vec<Row>,
    pub side: ListState,
    pub tracks: Vec<Track>,
    /// Indices de `tracks` visibles avec le filtre ; la sélection du tableau s'y réfère.
    pub view: Vec<usize>,
    pub filter: String,
    pub tracks_title: String,
    pub tracks_loading: bool,
    pub shown: View,
    pub table: TableState,
    req: u64,
    pub focus: Focus,
    pub mode: Mode,
    player: Option<Player>,
    queue: Queue<Track>,
    queue_title: String,
    /// Le titre prévu a bien été confié à mpv (enchaînement sans blanc possible).
    upcoming_sent: bool,
    /// mpv a un fichier chargé (faux après une reprise sans serveur, par exemple).
    loaded: bool,
    /// Titres illisibles d'affilée : au-delà de 3, on arrête au lieu de défiler toute la file.
    errors: u32,
    pub now: Option<Track>,
    pub pos: f64,
    pub dur: f64,
    pub paused: bool,
    pub volume: f64,
    pub shuffle: bool,
    /// Avancée de l'animation du CD, en secteurs (ne progresse qu'en lecture).
    pub spin: f64,
    last_tick: Instant,
    /// Titre Jellyfin signalé « en cours » au serveur, et date de la dernière remontée.
    reported: Option<String>,
    last_report: Instant,
    pub status: Option<(String, Instant)>,
    pub quit: bool,
    rng: Rng,
    session_file: PathBuf,
    /// La chaîne de l'égaliseur est en place dans mpv (on peut la régler à chaud).
    eq_live: bool,
    /// Description de la chaîne telle que mpv la connaît. Les réglages à chaud ne
    /// la modifient pas : si mpv la reconstruisait (changement de format audio),
    /// il reprendrait ces valeurs. On la resynchronise en fermant l'égaliseur.
    eq_graph: Option<String>,
    // Zones mémorisées au rendu, pour savoir où l'on clique.
    pub area_eq_bars: Vec<Rect>,
    pub area_side: Rect,
    pub area_tracks: Rect,
    pub area_player: Rect,
    pub area_gauge: Rect,
}

fn label_of(p: &Path) -> String {
    let home = crate::config::home();
    match p.strip_prefix(&home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => p.display().to_string(),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn entries(v: Vec<Entry>, kind: fn(String) -> Kind) -> Vec<Node> {
    v.into_iter().map(|e| Node::new(e.name, kind(e.id))).collect()
}

fn step(sel: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let i = sel.unwrap_or(0) as isize + delta;
    Some(i.clamp(0, len as isize - 1) as usize)
}

fn matches(t: &Track, q: &str) -> bool {
    [&t.title, &t.artist, &t.album].iter().any(|f| f.to_lowercase().contains(q))
}

impl App {
    pub fn new(cfg: Config, tx: Sender<Msg>) -> App {
        let (volume, replaygain) = (cfg.volume, cfg.replaygain.clone());
        let mut app = App::build(cfg, tx.clone());
        app.session_file = crate::config::session_path();
        match Player::start(tx, volume, &replaygain) {
            Ok(p) => app.player = Some(p),
            Err(e) => app.flash(e),
        }
        app.apply_eq(None);
        app.resume();
        if !app.cfg.logged_in() {
            app.flash("Pas encore connecté à Jellyfin : appuie sur c");
        }
        app
    }

    /// L'application sans lecteur audio ni session (mpv est lancé par `new`) : sert aussi aux tests.
    fn build(cfg: Config, tx: Sender<Msg>) -> App {
        let volume = cfg.volume;
        let mut app = App {
            tx,
            tree: Vec::new(),
            rows: Vec::new(),
            side: ListState::default(),
            tracks: Vec::new(),
            view: Vec::new(),
            filter: String::new(),
            tracks_title: String::new(),
            tracks_loading: false,
            shown: View::List,
            table: TableState::default(),
            req: 0,
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            player: None,
            queue: Queue::default(),
            queue_title: String::new(),
            upcoming_sent: false,
            loaded: false,
            errors: 0,
            now: None,
            pos: 0.0,
            dur: 0.0,
            paused: false,
            volume,
            shuffle: false,
            spin: 0.0,
            last_tick: Instant::now(),
            reported: None,
            last_report: Instant::now(),
            status: None,
            quit: false,
            rng: Rng::new(),
            session_file: PathBuf::new(),
            eq_live: false,
            eq_graph: None,
            area_eq_bars: Vec::new(),
            area_side: Rect::default(),
            area_tracks: Rect::default(),
            area_player: Rect::default(),
            area_gauge: Rect::default(),
            cfg,
        };
        let dirs = app
            .cfg
            .local_dirs
            .iter()
            .map(|d| Node::new(label_of(d), Kind::Dir { root: d.clone(), path: d.clone() }))
            .collect();
        let mut pc = Node::new("Ce PC", Kind::PcRoot);
        pc.children = Some(dirs);
        pc.expanded = true;
        let mut server = Node::new(app.cfg.server_label.clone(), Kind::ServerRoot);
        server.children = Some(app.server_children());
        server.expanded = true;
        app.tree = vec![Node::new("File d'attente", Kind::Queue), pc, server];
        app.side.select(Some(1));
        app.rebuild_rows();
        app
    }

    pub fn shutdown(&mut self) {
        // En quittant, la remontée de fin d'écoute se fait tout de suite (délai court).
        if let (Some(id), Some(j)) = (self.reported.take(), self.jelly()) {
            let _ = j.report(Report::Stop, &id, self.pos, self.paused, self.cfg.mobile_quality);
        }
        self.player = None;
        let _ = self.cfg.save();
        if !self.session_file.as_os_str().is_empty() {
            let s = Session {
                title: self.queue_title.clone(),
                queue: self.queue.items().to_vec(),
                index: self.queue.index(),
                position: if self.now.is_some() { self.pos } else { 0.0 },
            };
            let _ = s.save(&self.session_file);
        }
    }

    /// Reprend la file et la position de la dernière fois, en pause.
    fn resume(&mut self) {
        let Some(s) = Session::load(&self.session_file) else { return };
        self.queue = Queue::new(s.queue, s.index);
        self.queue_title = s.title;
        let Some(t) = self.queue.current().cloned() else { return };
        self.dur = t.duration.unwrap_or(0.0);
        self.pos = s.position;
        self.now = Some(t);
        self.paused = true;
        self.load_resumed();
        self.show_queue();
    }

    /// Charge le titre courant en pause, à la position retenue. Faux si impossible.
    fn load_resumed(&mut self) -> bool {
        let Some(t) = self.now.clone() else { return false };
        let Some(target) = self.target(&t) else { return false };
        let Some(p) = self.player.as_mut() else { return false };
        p.load_paused(&target, self.pos);
        self.loaded = true;
        self.plan_upcoming();
        true
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn playing(&self) -> bool {
        self.now.is_some() && !self.paused
    }

    /// Avance le temps (animation, messages, suivi). Vrai s'il faut redessiner.
    pub fn tick(&mut self) -> bool {
        let dt = self.last_tick.elapsed().as_secs_f64();
        self.last_tick = Instant::now();
        let mut dirty = false;
        if self.playing() {
            let before = self.spin as u64;
            self.spin += dt / SECTOR_STEP;
            dirty |= self.spin as u64 != before;
            if self.reported.is_some() && self.last_report.elapsed() >= REPORT_EVERY {
                self.report(Report::Progress);
            }
        }
        if self.status.as_ref().is_some_and(|(_, t)| t.elapsed() >= STATUS_LIFE) {
            self.status = None;
            dirty = true;
        }
        dirty
    }

    /// Délai avant la prochaine chose à faire sans intervention (None : rien, on attend).
    /// C'est ce qui permet de ne rien redessiner ni réveiller en pause.
    pub fn deadline(&self) -> Option<Duration> {
        let mut next: Option<Duration> = None;
        let mut at = |d: Duration| next = Some(next.map_or(d, |n| n.min(d)));
        if self.playing() {
            let frac = self.spin.fract();
            at(Duration::from_secs_f64(((1.0 - frac) * SECTOR_STEP).max(0.005)));
            if self.reported.is_some() {
                at(REPORT_EVERY.saturating_sub(self.last_report.elapsed()));
            }
        }
        if let Some((_, t)) = &self.status {
            at(STATUS_LIFE.saturating_sub(t.elapsed()));
        }
        next
    }

    fn jelly(&self) -> Option<Jellyfin> {
        self.cfg.logged_in().then(|| Jellyfin {
            url: if self.cfg.jellyfin_url.is_empty() { "http://".into() } else { self.cfg.jellyfin_url.clone() },
            token: self.cfg.token.clone(),
            user_id: self.cfg.user_id.clone(),
            device_id: self.cfg.device_id.clone(),
        })
    }

    /// Ce que mpv doit ouvrir pour ce titre (None : serveur requis mais pas connecté).
    fn target(&self, t: &Track) -> Option<String> {
        match &t.source {
            Source::Local(p) => Some(p.to_string_lossy().into_owned()),
            Source::Jelly(id) => self.jelly().map(|j| j.stream_url(id, self.cfg.mobile_quality)),
        }
    }

    fn server_children(&self) -> Vec<Node> {
        if self.cfg.logged_in() {
            vec![
                Node::new("Playlists", Kind::Playlists),
                Node::new("Artistes", Kind::Artists),
                Node::new("Albums", Kind::Albums),
            ]
        } else {
            vec![Node::new("Se connecter…", Kind::Login)]
        }
    }

    fn reset_server_node(&mut self) {
        let ch = self.server_children();
        if let Some(server) = self.tree.iter_mut().find(|n| matches!(n.kind, Kind::ServerRoot)) {
            server.children = Some(ch);
            server.expanded = true;
        }
        self.rebuild_rows();
    }

    pub fn queue_len(&self) -> usize {
        self.queue.items().len()
    }

    // ── Arbre de gauche ─────────────────────────────────────────────────────

    pub fn node(&self, path: &[usize]) -> Option<&Node> {
        let mut n = self.tree.get(*path.first()?)?;
        for &i in &path[1..] {
            n = n.children.as_ref()?.get(i)?;
        }
        Some(n)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut n = self.tree.get_mut(*path.first()?)?;
        for &i in &path[1..] {
            n = n.children.as_mut()?.get_mut(i)?;
        }
        Some(n)
    }

    fn selected_path(&self) -> Option<Vec<usize>> {
        self.side.selected().and_then(|i| self.rows.get(i)).map(|r| r.path.clone())
    }

    /// Recalcule les lignes visibles en gardant la sélection sur le même nœud.
    fn rebuild_rows(&mut self) {
        fn walk(nodes: &[Node], prefix: &mut Vec<usize>, depth: usize, rows: &mut Vec<Row>) {
            for (i, n) in nodes.iter().enumerate() {
                prefix.push(i);
                rows.push(Row { path: prefix.clone(), depth });
                if n.expanded {
                    if let Some(c) = &n.children {
                        walk(c, prefix, depth + 1, rows);
                    }
                }
                prefix.pop();
            }
        }
        let sel = self.selected_path();
        let mut rows = Vec::new();
        walk(&self.tree, &mut Vec::new(), 0, &mut rows);
        self.rows = rows;
        let i = sel
            .and_then(|p| self.rows.iter().position(|r| r.path == p))
            .unwrap_or_else(|| self.side.selected().unwrap_or(0).min(self.rows.len().saturating_sub(1)));
        self.side.select(Some(i));
    }

    fn expand(&mut self, path: &[usize]) {
        let Some(n) = self.node(path) else { return };
        if !n.expandable() {
            return;
        }
        let kind = n.kind.clone();
        if n.children.is_none() && !n.loading {
            match &kind {
                Kind::Dir { root, path: dir } => {
                    let (dirs, _) = local::list(root, dir);
                    let ch = dirs
                        .into_iter()
                        .map(|d| Node::new(file_name(&d), Kind::Dir { root: root.clone(), path: d }))
                        .collect();
                    self.node_mut(path).unwrap().children = Some(ch);
                }
                Kind::PcRoot | Kind::ServerRoot => {}
                _ => {
                    let Some(j) = self.jelly() else { return self.need_login() };
                    let (tx, p) = (self.tx.clone(), path.to_vec());
                    thread::spawn(move || {
                        let result = match &kind {
                            Kind::Playlists => j.playlists().map(|v| entries(v, Kind::Playlist)),
                            Kind::Artists => j.artists().map(|v| entries(v, Kind::Artist)),
                            Kind::Albums => j.albums(None).map(|v| entries(v, Kind::Album)),
                            Kind::Artist(id) => j.albums(Some(id)).map(|v| entries(v, Kind::Album)),
                            _ => Ok(Vec::new()),
                        };
                        let _ = tx.send(Msg::Children { path: p, result });
                    });
                    self.node_mut(path).unwrap().loading = true;
                }
            }
        }
        self.node_mut(path).unwrap().expanded = true;
        self.rebuild_rows();
    }

    fn collapse(&mut self, path: &[usize]) {
        if let Some(n) = self.node_mut(path) {
            n.expanded = false;
        }
        self.rebuild_rows();
    }

    fn toggle(&mut self, path: &[usize]) {
        match self.node(path) {
            Some(n) if n.expanded => self.collapse(path),
            Some(_) => self.expand(path),
            None => {}
        }
    }

    /// Entrée ou clic sur une ligne de gauche.
    fn activate_side(&mut self, focus_tracks: bool) {
        let Some(path) = self.selected_path() else { return };
        let Some(kind) = self.node(&path).map(|n| n.kind.clone()) else { return };
        match kind {
            Kind::Login => self.open_login(),
            Kind::Queue => {
                self.show_queue();
                if focus_tracks {
                    self.focus = Focus::Tracks;
                }
            }
            Kind::Playlist(_) | Kind::Album(_) => {
                self.open_tracks(&path);
                if focus_tracks {
                    self.focus = Focus::Tracks;
                }
            }
            Kind::Dir { .. } | Kind::Artist(_) => {
                self.toggle(&path);
                self.open_tracks(&path);
            }
            _ => self.toggle(&path),
        }
    }

    // ── Liste de titres ─────────────────────────────────────────────────────

    fn set_tracks(&mut self, title: String, tracks: Vec<Track>, shown: View) {
        self.tracks = tracks;
        self.tracks_title = title;
        self.tracks_loading = false;
        self.shown = shown;
        self.filter.clear();
        self.apply_filter();
        self.table.select(if self.view.is_empty() { None } else { Some(0) });
        *self.table.offset_mut() = 0;
    }

    /// Recalcule les lignes visibles d'après le filtre, en gardant si possible le même titre sélectionné.
    fn apply_filter(&mut self) {
        let keep = self.table.selected().and_then(|i| self.view.get(i)).copied();
        let q = self.filter.to_lowercase();
        self.view = (0..self.tracks.len()).filter(|&i| q.is_empty() || matches(&self.tracks[i], &q)).collect();
        let sel = keep.and_then(|k| self.view.iter().position(|&i| i == k));
        self.table.select(if self.view.is_empty() { None } else { Some(sel.unwrap_or(0)) });
    }

    /// Titre du tableau sous la sélection (ou à la ligne `i` de la vue).
    fn view_track(&self, i: usize) -> Option<&Track> {
        self.view.get(i).and_then(|&k| self.tracks.get(k))
    }

    fn show_queue(&mut self) {
        let items = self.queue.items().to_vec();
        let current = self.queue.index();
        self.set_tracks("File d'attente".into(), items, View::Queue);
        if !self.view.is_empty() {
            self.table.select(Some(current.min(self.view.len() - 1)));
        }
    }

    /// La file a changé : on met à jour son affichage si elle est à l'écran.
    fn refresh_queue_view(&mut self) {
        if self.shown != View::Queue {
            return;
        }
        let (sel, filter) = (self.table.selected(), std::mem::take(&mut self.filter));
        self.tracks = self.queue.items().to_vec();
        self.filter = filter;
        self.apply_filter();
        if let Some(s) = sel {
            self.table.select(step(Some(s), self.view.len(), 0));
        }
    }

    fn open_tracks(&mut self, path: &[usize]) {
        let Some(n) = self.node(path) else { return };
        let (kind, label) = (n.kind.clone(), n.label.clone());
        self.req += 1;
        let req = self.req;
        match kind {
            Kind::Dir { root, path: dir } => {
                let (_, t) = local::list(&root, &dir);
                self.set_tracks(label, t, View::List);
            }
            Kind::Playlist(_) | Kind::Album(_) | Kind::Artist(_) => {
                let Some(j) = self.jelly() else { return self.need_login() };
                self.set_tracks(label.clone(), Vec::new(), View::List);
                self.tracks_loading = true;
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let result = match &kind {
                        Kind::Playlist(id) => j.playlist_tracks(id),
                        Kind::Album(id) => j.album_tracks(id),
                        Kind::Artist(id) => j.artist_tracks(id),
                        _ => Ok(Vec::new()),
                    };
                    let _ = tx.send(Msg::Tracks { req, title: label, result });
                });
            }
            _ => {}
        }
    }

    fn search(&mut self, q: String) {
        self.req += 1;
        let (req, tx, j, dirs) = (self.req, self.tx.clone(), self.jelly(), self.cfg.local_dirs.clone());
        self.set_tracks(format!("Recherche « {q} »"), Vec::new(), View::Search);
        self.tracks_loading = true;
        self.focus = Focus::Tracks;
        thread::spawn(move || {
            let local = local::search(&dirs, &q, 300);
            let result = match j.map(|j| j.search(&q)) {
                Some(Ok(mut t)) => {
                    t.extend(local);
                    Ok(t)
                }
                Some(Err(e)) if local.is_empty() => Err(e),
                _ => Ok(local),
            };
            let _ = tx.send(Msg::Tracks { req, title: format!("Recherche « {q} »"), result });
        });
    }

    // ── Lecture ─────────────────────────────────────────────────────────────

    /// Entrée sur une ligne du tableau : dépend de ce qui est affiché.
    fn activate_track(&mut self, i: usize) {
        let Some(&k) = self.view.get(i) else { return };
        match self.shown {
            // Dans la file : on y saute, sans la remplacer.
            View::Queue => {
                if self.queue.jump(k).is_some() {
                    self.errors = 0;
                    self.play_current();
                }
            }
            // Résultat de recherche pendant une écoute : on le glisse dans la file actuelle.
            View::Search if self.now.is_some() => {
                let t = self.tracks[k].clone();
                let at = self.queue.insert_next(t);
                self.queue.jump(at);
                self.play_current();
            }
            _ => self.play_from(i),
        }
    }

    /// Joue la liste affichée (filtrée) à partir de la ligne `i` : elle devient la file.
    fn play_from(&mut self, i: usize) {
        let list: Vec<Track> = self.view.iter().map(|&k| self.tracks[k].clone()).collect();
        if i >= list.len() {
            return;
        }
        self.queue = Queue::new(list, i);
        self.queue_title = self.tracks_title.clone();
        self.errors = 0;
        self.play_current();
    }

    fn play_current(&mut self) {
        self.report_stop();
        let Some(t) = self.queue.current().cloned() else { return };
        let Some(target) = self.target(&t) else { return self.need_login() };
        let Some(p) = self.player.as_mut() else {
            return self.flash("mpv n'est pas lancé : impossible de lire");
        };
        p.load(&target);
        self.loaded = true;
        self.set_now(t);
        self.plan_upcoming();
    }

    fn set_now(&mut self, t: Track) {
        self.pos = 0.0;
        self.dur = t.duration.unwrap_or(0.0);
        self.paused = false;
        self.now = Some(t);
        self.report(Report::Start);
        self.refresh_queue_view();
    }

    /// Choisit le titre suivant et le confie à mpv, qui le précharge pour l'enchaîner sans blanc.
    fn plan_upcoming(&mut self) {
        let next = self.queue.plan(self.shuffle, &mut self.rng).cloned();
        let target = next.as_ref().and_then(|t| self.target(t));
        self.upcoming_sent = target.is_some();
        if let Some(p) = self.player.as_mut() {
            p.set_upcoming(target.as_deref());
        }
    }

    /// Le morceau s'est terminé (ou n'a pas pu être lu) : mpv a déjà enchaîné si possible.
    fn track_finished(&mut self) {
        if self.now.is_none() {
            return;
        }
        self.report_stop();
        let gapless = self.upcoming_sent;
        match self.queue.advance().cloned() {
            Some(t) if gapless => {
                if let Some(p) = self.player.as_mut() {
                    p.drop_finished();
                }
                self.set_now(t);
                self.plan_upcoming();
            }
            Some(_) => self.play_current(),
            None => self.stop("Fin de la file de lecture"),
        }
    }

    fn next(&mut self) {
        if self.now.is_none() {
            return;
        }
        if self.queue.next(self.shuffle, &mut self.rng).is_some() {
            self.play_current();
        } else {
            self.stop("Fin de la file de lecture");
        }
    }

    fn stop(&mut self, msg: &str) {
        self.report_stop();
        if let Some(p) = self.player.as_mut() {
            p.stop();
        }
        self.loaded = false;
        self.now = None;
        self.pos = 0.0;
        self.dur = 0.0;
        self.flash(msg);
        self.refresh_queue_view();
    }

    fn prev(&mut self) {
        if self.pos > 3.0 {
            if let Some(p) = self.player.as_mut() {
                p.seek_percent(0.0);
            }
        } else if self.queue.prev().is_some() {
            self.play_current();
        }
    }

    fn toggle_pause(&mut self) {
        if self.now.is_none() {
            // Rien en cours : Espace lance le titre sélectionné.
            if let Some(i) = self.table.selected() {
                self.activate_track(i);
            }
        } else if !self.loaded {
            // Reprise de session sans fichier chargé (serveur absent au lancement).
            if self.load_resumed() {
                if let Some(p) = self.player.as_mut() {
                    p.toggle_pause();
                }
            } else {
                self.need_login();
            }
        } else if let Some(p) = self.player.as_mut() {
            p.toggle_pause();
        }
    }

    fn seek(&mut self, s: f64) {
        if self.now.is_some() {
            if let Some(p) = self.player.as_mut() {
                p.seek(s);
            }
        }
    }

    fn add_volume(&mut self, d: f64) {
        if let Some(p) = self.player.as_mut() {
            p.add_volume(d);
        }
    }

    /// « a » : à la fin de la file ; « e » : juste après le titre en cours.
    fn enqueue(&mut self, next: bool) {
        let Some(t) = self.table.selected().and_then(|i| self.view_track(i)).cloned() else { return };
        if self.shown == View::Queue {
            return self.flash("Ce titre est déjà dans la file");
        }
        let title = t.title.clone();
        if self.now.is_none() {
            self.queue = Queue::new(vec![t], 0);
            self.queue_title = "File d'attente".into();
            self.errors = 0;
            return self.play_current();
        }
        if next {
            self.queue.insert_next(t);
            self.flash(format!("Lu ensuite : {title}"));
        } else {
            self.queue.push(t);
            self.flash(format!("Ajouté à la file : {title}"));
        }
        self.plan_upcoming();
        self.refresh_queue_view();
    }

    /// Suppr dans la file : retire le titre (sauf celui en cours).
    fn remove_from_queue(&mut self) {
        if self.shown != View::Queue {
            return;
        }
        let Some(&k) = self.table.selected().and_then(|i| self.view.get(i)) else { return };
        if self.queue.remove(k).is_none() {
            return self.flash("Impossible de retirer le titre en cours");
        }
        self.plan_upcoming();
        self.refresh_queue_view();
    }

    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.flash(if self.shuffle { "Lecture aléatoire activée" } else { "Lecture aléatoire désactivée" });
        if self.loaded {
            self.plan_upcoming();
        }
    }

    fn toggle_mobile(&mut self) {
        self.cfg.mobile_quality = !self.cfg.mobile_quality;
        let _ = self.cfg.save();
        self.flash(if self.cfg.mobile_quality {
            "Qualité mobile (Opus 128 kb/s) à partir du prochain titre"
        } else {
            "Qualité d'origine à partir du prochain titre"
        });
        if self.loaded {
            self.plan_upcoming();
        }
    }

    // ── Égaliseur ───────────────────────────────────────────────────────────

    pub fn eq_active(&self) -> bool {
        self.cfg.eq_enabled && !eq::is_flat(&self.cfg.eq_gains)
    }

    /// Transmet les réglages à mpv au moindre coût : rien à plat, la chaîne
    /// entière seulement à l'activation, sinon la bande modifiée (ou toutes,
    /// avec None) et le préampli, à chaud.
    fn apply_eq(&mut self, band: Option<usize>) {
        let want = self.eq_active();
        let gains = self.cfg.eq_gains.clone();
        let Some(p) = self.player.as_mut() else { return };
        if !want {
            if self.eq_live {
                p.set_eq(None);
                self.eq_live = false;
                self.eq_graph = None;
            }
            return;
        }
        if !self.eq_live {
            self.eq_graph = eq::filter(&gains);
            p.set_eq(self.eq_graph.as_deref());
            self.eq_live = true;
            return;
        }
        match band {
            Some(i) => p.eq_band(i, gains[i]),
            None => gains.iter().enumerate().for_each(|(i, &g)| p.eq_band(i, g)),
        }
        p.eq_preamp(eq::preamp(&gains));
    }

    /// Règle une bande (arrondie au dB). Toucher un réglage active l'égaliseur.
    fn set_gain(&mut self, band: usize, gain: f64) {
        let Some(g) = self.cfg.eq_gains.get_mut(band) else { return };
        *g = eq::clamp(gain.round());
        self.cfg.eq_preset = eq::preset_name(&self.cfg.eq_gains).into();
        self.cfg.eq_enabled = true;
        self.apply_eq(Some(band));
    }

    fn eq_preset(&mut self, delta: isize) {
        let (name, gains) = eq::cycle(&self.cfg.eq_preset, delta);
        self.cfg.eq_gains = gains.to_vec();
        self.cfg.eq_preset = name.into();
        self.cfg.eq_enabled = true;
        self.apply_eq(None);
    }

    fn eq_toggle(&mut self) {
        self.cfg.eq_enabled = !self.cfg.eq_enabled;
        self.apply_eq(None);
    }

    fn key_eq(&mut self, k: KeyEvent) {
        let Mode::Eq { band } = self.mode else { return };
        let g = self.cfg.eq_gains.get(band).copied().unwrap_or(0.0);
        match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('E' | 'é' | 'q') => self.close_eq(),
            KeyCode::Left => self.mode = Mode::Eq { band: band.saturating_sub(1) },
            KeyCode::Right => self.mode = Mode::Eq { band: (band + 1).min(eq::BANDS.len() - 1) },
            KeyCode::Up => self.set_gain(band, g + 1.0),
            KeyCode::Down => self.set_gain(band, g - 1.0),
            KeyCode::PageUp => self.set_gain(band, g + 3.0),
            KeyCode::PageDown => self.set_gain(band, g - 3.0),
            KeyCode::Char('0') => self.set_gain(band, 0.0),
            KeyCode::Char('p') => self.eq_preset(1),
            KeyCode::Char('P') => self.eq_preset(-1),
            KeyCode::Char('o') => self.eq_toggle(),
            KeyCode::Char(' ') => self.toggle_pause(),
            _ => {}
        }
    }

    /// Fermeture : on enregistre, et mpv reçoit une seule fois la chaîne à jour.
    fn close_eq(&mut self) {
        self.mode = Mode::Normal;
        let _ = self.cfg.save();
        let want = eq::filter(&self.cfg.eq_gains);
        if self.eq_live && want != self.eq_graph {
            if let Some(p) = self.player.as_mut() {
                p.set_eq(want.as_deref());
            }
            self.eq_graph = want;
        }
    }

    /// Souris dans l'égaliseur : clic ou glisser sur une barre, molette pour ±1 dB.
    fn mouse_eq(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let Some(band) = self.area_eq_bars.iter().position(|r| r.contains(pos)) else { return };
        self.mode = Mode::Eq { band };
        let g = self.cfg.eq_gains[band];
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                self.set_gain(band, gain_at(self.area_eq_bars[band], m.row));
            }
            MouseEventKind::ScrollUp => self.set_gain(band, g + 1.0),
            MouseEventKind::ScrollDown => self.set_gain(band, g - 1.0),
            _ => {}
        }
    }

    // ── Suivi des écoutes côté Jellyfin ─────────────────────────────────────

    fn report(&mut self, step: Report) {
        let id = match (step, &self.now) {
            (Report::Start, Some(Track { source: Source::Jelly(id), .. })) => {
                self.reported = Some(id.clone());
                id.clone()
            }
            (Report::Start, _) => return,
            _ => match &self.reported {
                Some(id) => id.clone(),
                None => return,
            },
        };
        if step == Report::Stop {
            self.reported = None;
        }
        self.last_report = Instant::now();
        let Some(j) = self.jelly() else { return };
        let (pos, paused, mobile) = (self.pos, self.paused, self.cfg.mobile_quality);
        thread::spawn(move || {
            let _ = j.report(step, &id, pos, paused, mobile);
        });
    }

    fn report_stop(&mut self) {
        self.report(Report::Stop);
    }

    // ── Connexion ───────────────────────────────────────────────────────────

    /// Jellyfin a refusé le jeton : on l'oublie et on propose tout de suite de se reconnecter.
    fn session_expired(&mut self) {
        self.cfg.token.clear();
        self.reported = None;
        self.reset_server_node();
        self.tracks_loading = false;
        if matches!(self.mode, Mode::Login(_)) {
            return; // formulaire déjà ouvert : on ne perd pas la saisie en cours
        }
        self.open_login();
        if let Mode::Login(f) = &mut self.mode {
            f.error = Some("Session expirée : reconnecte-toi.".into());
        }
    }

    fn api_error(&mut self, e: ApiError) {
        match e {
            ApiError::Unauthorized => self.session_expired(),
            ApiError::Other(msg) => self.flash(msg),
        }
    }

    fn need_login(&mut self) {
        self.flash("Pas connecté à Jellyfin : appuie sur c");
    }

    fn open_login(&mut self) {
        self.mode = Mode::Login(LoginForm {
            url: if self.cfg.jellyfin_url.is_empty() { "http://".into() } else { self.cfg.jellyfin_url.clone() },
            user: self.cfg.user_name.clone(),
            pw: String::new(),
            field: if self.cfg.user_name.is_empty() { 1 } else { 2 },
            busy: false,
            error: None,
        });
    }

    fn submit_login(&mut self) {
        let Mode::Login(f) = &mut self.mode else { return };
        if f.busy {
            return;
        }
        let (url, user, pw) = (f.url.trim().to_string(), f.user.trim().to_string(), f.pw.clone());
        if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() < 10 {
            f.error = Some("Adresse du serveur attendue, ex. http://mon-serveur:8096".into());
            return;
        }
        f.busy = true;
        f.error = None;
        let (dev, tx) = (self.cfg.device_id.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::Login(jellyfin::login(&url, &user, &pw, &dev)));
        });
    }

    // ── Messages ────────────────────────────────────────────────────────────

    /// Traite un message. Vrai s'il faut redessiner l'écran.
    pub fn on_msg(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Input(Event::Key(k)) if k.kind == KeyEventKind::Press => self.on_key(k),
            Msg::Input(Event::Mouse(m)) => {
                // Les simples déplacements de la souris n'appellent aucun redessin.
                if matches!(m.kind, MouseEventKind::Moved) {
                    return false;
                }
                self.on_mouse(m);
            }
            Msg::Input(Event::Resize(..)) => {}
            Msg::Input(_) => return false,
            Msg::Player(e) => match e {
                player::Event::Time(t) => {
                    // L'affichage est à la seconde : inutile de redessiner entre deux.
                    let changed = t as u64 != self.pos as u64;
                    self.pos = t;
                    if t > 0.5 {
                        self.errors = 0;
                    }
                    return changed;
                }
                player::Event::Duration(d) => self.dur = d,
                player::Event::Pause(p) => {
                    let was = self.paused;
                    self.paused = p;
                    if was != p && self.now.is_some() {
                        // Première reprise après une session restaurée : on signale le début.
                        if self.reported.is_none() && !p {
                            self.report(Report::Start);
                        } else {
                            self.report(Report::Progress);
                        }
                    }
                }
                player::Event::Volume(v) => self.volume = v,
                player::Event::Eof => self.track_finished(),
                player::Event::Error(e) => {
                    self.errors += 1;
                    if self.errors >= 3 {
                        self.errors = 0;
                        self.stop(&format!("3 titres illisibles d'affilée, lecture arrêtée ({e})"));
                    } else {
                        self.flash(format!("Lecture impossible : {e}"));
                        self.track_finished();
                    }
                }
            },
            Msg::Children { path, result } => {
                // Le nœud doit toujours attendre ce résultat : l'arbre a pu être
                // reconstruit entre-temps (reconnexion), et le chemin viser autre chose.
                let err = match self.node_mut(&path).filter(|n| n.loading) {
                    Some(n) => {
                        n.loading = false;
                        match result {
                            Ok(c) => {
                                n.children = Some(c);
                                None
                            }
                            Err(e) => {
                                n.expanded = false;
                                Some(e)
                            }
                        }
                    }
                    None => None,
                };
                self.rebuild_rows();
                if let Some(e) = err {
                    self.api_error(e);
                }
            }
            Msg::Tracks { req, title, result } => {
                if req != self.req {
                    return false; // une demande plus récente a pris le relais
                }
                match result {
                    Ok(t) => {
                        let shown = self.shown;
                        self.set_tracks(title, t, shown);
                    }
                    Err(e) => {
                        self.tracks_loading = false;
                        self.api_error(e);
                    }
                }
            }
            Msg::Login(result) => {
                let Mode::Login(f) = &mut self.mode else { return false };
                match result {
                    Ok((token, id)) => {
                        self.cfg.jellyfin_url = f.url.trim().to_string();
                        self.cfg.user_name = f.user.trim().to_string();
                        self.cfg.token = token;
                        self.cfg.user_id = id;
                        self.mode = Mode::Normal;
                        self.reset_server_node();
                        match self.cfg.save() {
                            Ok(()) => self.flash("Connecté à Jellyfin"),
                            Err(e) => self.flash(format!("Connecté, mais config non enregistrée : {e}")),
                        }
                    }
                    Err(e) => {
                        f.busy = false;
                        f.error = Some(e);
                    }
                }
            }
        }
        true
    }

    // ── Clavier ─────────────────────────────────────────────────────────────

    pub fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Login(_) => return self.key_login(k),
            Mode::Search(_) => return self.key_search(k),
            Mode::Filter => return self.key_filter(k),
            Mode::Eq { .. } => return self.key_eq(k),
            Mode::Help => {
                self.mode = Mode::Normal; // n'importe quelle touche referme l'aide
                return;
            }
            Mode::Normal => {}
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char('n') => self.next(),
            KeyCode::Char('p') => self.prev(),
            KeyCode::Char(',') => self.seek(-10.0),
            KeyCode::Char('.') => self.seek(10.0),
            KeyCode::Char('+') | KeyCode::Char('=') => self.add_volume(5.0),
            KeyCode::Char('-') => self.add_volume(-5.0),
            KeyCode::Char('s') => self.toggle_shuffle(),
            KeyCode::Char('m') => self.toggle_mobile(),
            KeyCode::Char('E' | 'é') => self.mode = Mode::Eq { band: 0 },
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => self.mode = Mode::Search(String::new()),
            KeyCode::Char('f') => {
                self.mode = Mode::Filter;
                self.focus = Focus::Tracks;
            }
            KeyCode::Char('c') => self.open_login(),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Tracks,
                    Focus::Tracks => Focus::Player,
                    Focus::Player => Focus::Sidebar,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Player,
                    Focus::Tracks => Focus::Sidebar,
                    Focus::Player => Focus::Tracks,
                }
            }
            code => match self.focus {
                Focus::Sidebar => self.key_side(code),
                Focus::Tracks => self.key_tracks(code),
                Focus::Player => self.key_player(code),
            },
        }
    }

    fn key_side(&mut self, code: KeyCode) {
        let len = self.rows.len();
        let sel = self.side.selected();
        match code {
            KeyCode::Up => self.side.select(step(sel, len, -1)),
            KeyCode::Down => self.side.select(step(sel, len, 1)),
            KeyCode::PageUp => self.side.select(step(sel, len, -10)),
            KeyCode::PageDown => self.side.select(step(sel, len, 10)),
            KeyCode::Home => self.side.select(step(Some(0), len, 0)),
            KeyCode::End => self.side.select(step(Some(len), len, 0)),
            KeyCode::Enter => self.activate_side(true),
            KeyCode::Right => {
                let Some(path) = self.selected_path() else { return };
                match self.node(&path) {
                    Some(n) if !n.expandable() => self.activate_side(false),
                    Some(n) if !n.expanded => self.activate_side(false),
                    _ => {}
                }
            }
            KeyCode::Left => {
                let Some(path) = self.selected_path() else { return };
                if self.node(&path).is_some_and(|n| n.expanded) {
                    self.collapse(&path);
                } else if path.len() > 1 {
                    let parent = &path[..path.len() - 1];
                    if let Some(i) = self.rows.iter().position(|r| r.path == parent) {
                        self.side.select(Some(i));
                    }
                }
            }
            _ => {}
        }
    }

    fn key_tracks(&mut self, code: KeyCode) {
        let len = self.view.len();
        let sel = self.table.selected();
        match code {
            KeyCode::Up => self.table.select(step(sel, len, -1)),
            KeyCode::Down => self.table.select(step(sel, len, 1)),
            KeyCode::PageUp => self.table.select(step(sel, len, -15)),
            KeyCode::PageDown => self.table.select(step(sel, len, 15)),
            KeyCode::Home => self.table.select(step(Some(0), len, 0)),
            KeyCode::End => self.table.select(step(Some(len), len, 0)),
            KeyCode::Enter => {
                if let Some(i) = sel {
                    self.activate_track(i);
                }
            }
            KeyCode::Char('a') => self.enqueue(false),
            KeyCode::Char('e') => self.enqueue(true),
            KeyCode::Delete | KeyCode::Char('x') => self.remove_from_queue(),
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.apply_filter();
            }
            KeyCode::Left => self.focus = Focus::Sidebar,
            _ => {}
        }
    }

    fn key_player(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left => self.seek(-5.0),
            KeyCode::Right => self.seek(5.0),
            KeyCode::Up => self.add_volume(5.0),
            KeyCode::Down => self.add_volume(-5.0),
            KeyCode::Enter => self.toggle_pause(),
            _ => {}
        }
    }

    fn key_login(&mut self, k: KeyEvent) {
        let Mode::Login(f) = &mut self.mode else { return };
        let mut submit = false;
        let mut cancel = false;
        match k.code {
            KeyCode::Esc => cancel = true,
            KeyCode::Tab | KeyCode::Down => f.field = (f.field + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => f.field = (f.field + 2) % 3,
            KeyCode::Enter if f.field < 2 => f.field += 1,
            KeyCode::Enter => submit = true,
            KeyCode::Backspace => {
                field(f).pop();
            }
            KeyCode::Char(c) => field(f).push(c),
            _ => {}
        }
        if cancel {
            self.mode = Mode::Normal;
        } else if submit {
            self.submit_login();
        }
    }

    fn key_search(&mut self, k: KeyEvent) {
        let Mode::Search(q) = &mut self.mode else { return };
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                q.pop();
            }
            KeyCode::Char(c) => q.push(c),
            KeyCode::Enter => {
                let q = q.trim().to_string();
                self.mode = Mode::Normal;
                if !q.is_empty() {
                    self.search(q);
                }
            }
            _ => {}
        }
    }

    /// Filtre au fil de la frappe ; Entrée le garde, Échap l'efface.
    fn key_filter(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            KeyCode::Up | KeyCode::Down => {
                return self.key_tracks(k.code);
            }
            _ => return,
        }
        self.apply_filter();
    }

    // ── Souris ──────────────────────────────────────────────────────────────

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if matches!(self.mode, Mode::Eq { .. }) {
            return self.mouse_eq(m);
        }
        if matches!(self.mode, Mode::Help) {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.mode = Mode::Normal;
            }
            return;
        }
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        let pos = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                if self.area_side.contains(pos) {
                    self.side.select(step(self.side.selected(), self.rows.len(), d));
                } else if self.area_tracks.contains(pos) {
                    self.table.select(step(self.table.selected(), self.view.len(), d));
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.area_gauge.contains(pos) && self.now.is_some() {
                    let g = self.area_gauge;
                    let pct = (m.column - g.x) as f64 / g.width.max(1) as f64 * 100.0;
                    if let Some(p) = self.player.as_mut() {
                        p.seek_percent(pct);
                    }
                } else if self.area_side.contains(pos) {
                    self.focus = Focus::Sidebar;
                    if let Some(i) = row_at(self.area_side, m.row, 1, self.side.offset(), self.rows.len()) {
                        self.side.select(Some(i));
                        self.activate_side(false);
                    }
                } else if self.area_tracks.contains(pos) {
                    self.focus = Focus::Tracks;
                    // +2 : bordure du haut et ligne d'en-tête.
                    if let Some(i) = row_at(self.area_tracks, m.row, 2, self.table.offset(), self.view.len()) {
                        self.table.select(Some(i));
                        self.activate_track(i);
                    }
                } else if self.area_player.contains(pos) {
                    self.focus = Focus::Player;
                }
            }
            _ => {}
        }
    }
}

fn field(f: &mut LoginForm) -> &mut String {
    match f.field {
        0 => &mut f.url,
        1 => &mut f.user,
        _ => &mut f.pw,
    }
}

/// Gain (dB) correspondant à la ligne `y` d'une barre de l'égaliseur : le haut
/// vaut +12, la ligne du milieu 0, le bas -12.
fn gain_at(bar: Rect, y: u16) -> f64 {
    let half = (bar.height / 2).max(1) as f64;
    let mid = bar.y + bar.height / 2;
    eq::clamp((mid as f64 - y as f64) / half * eq::MAX_DB)
}

/// Index de l'élément sous la souris dans une liste encadrée.
fn row_at(area: Rect, y: u16, top: u16, offset: usize, len: usize) -> Option<usize> {
    let first = area.y + top;
    if y < first || y >= area.y + area.height.saturating_sub(1) {
        return None;
    }
    let i = offset + (y - first) as usize;
    (i < len).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn app(logged: bool) -> (App, mpsc::Receiver<Msg>) {
        let mut c = Config::default();
        c.local_dirs = Vec::new();
        if logged {
            c.jellyfin_url = "http://127.0.0.1:9".into();
            c.user_name = "moi".into();
            c.user_id = "u".into();
            c.token = "t".into();
        }
        let (tx, rx) = mpsc::channel();
        (App::build(c, tx), rx)
    }

    fn labels(a: &App) -> Vec<String> {
        a.rows.iter().map(|r| a.node(&r.path).unwrap().label.clone()).collect()
    }

    fn track(n: &str) -> Track {
        Track {
            title: n.into(),
            artist: String::new(),
            album: String::new(),
            duration: None,
            source: Source::Local(PathBuf::from(n)),
        }
    }

    fn login_form(a: &App) -> &LoginForm {
        match &a.mode {
            Mode::Login(f) => f,
            _ => panic!("le formulaire de connexion devrait être ouvert"),
        }
    }

    #[test]
    fn arbre_selon_la_connexion() {
        assert_eq!(labels(&app(false).0), ["File d'attente", "Ce PC", "Jellyfin", "Se connecter…"]);
        assert_eq!(labels(&app(true).0), ["File d'attente", "Ce PC", "Jellyfin", "Playlists", "Artistes", "Albums"]);
    }

    #[test]
    fn session_expiree_propose_de_se_reconnecter() {
        let (mut a, _) = app(true);
        a.req = 5;
        a.tracks_loading = true;
        a.on_msg(Msg::Tracks { req: 5, title: "B.Rap".into(), result: Err(ApiError::Unauthorized) });

        assert!(a.cfg.token.is_empty(), "le jeton refusé est oublié");
        assert!(!a.tracks_loading);
        assert_eq!(labels(&a)[3], "Se connecter…");
        let f = login_form(&a);
        assert_eq!((f.user.as_str(), f.field), ("moi", 2), "curseur directement sur le mot de passe");
        assert!(f.error.as_deref().unwrap().contains("Session expirée"));
    }

    #[test]
    fn deuxieme_refus_ne_perd_pas_la_saisie() {
        let (mut a, _) = app(true);
        a.session_expired();
        if let Mode::Login(f) = &mut a.mode {
            f.pw = "en cours".into();
        }
        a.on_msg(Msg::Tracks { req: a.req, title: String::new(), result: Err(ApiError::Unauthorized) });
        assert_eq!(login_form(&a).pw, "en cours");
    }

    #[test]
    fn reconnexion_restaure_le_serveur() {
        let (mut a, _) = app(true);
        let dir = std::env::temp_dir().join(format!("musiq-app-{}", std::process::id()));
        a.cfg.file = dir.join("config.json");
        a.session_expired();
        a.on_msg(Msg::Login(Ok(("nouveau".into(), "u2".into()))));

        assert!(matches!(a.mode, Mode::Normal));
        assert_eq!((a.cfg.token.as_str(), a.cfg.user_id.as_str()), ("nouveau", "u2"));
        assert_eq!(labels(&a)[3..], ["Playlists", "Artistes", "Albums"]);
        assert_eq!(Config::load_from(&a.cfg.file).token, "nouveau", "la nouvelle session est enregistrée");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mauvais_mot_de_passe_reste_dans_le_formulaire() {
        let (mut a, _) = app(false);
        a.open_login();
        a.on_msg(Msg::Login(Err("Identifiant ou mot de passe incorrect".into())));
        let f = login_form(&a);
        assert!(!f.busy);
        assert_eq!(f.error.as_deref(), Some("Identifiant ou mot de passe incorrect"));
    }

    #[test]
    fn adresse_invalide_refusee_sans_bloquer_le_formulaire() {
        let (mut a, _) = app(false);
        a.open_login();
        a.submit_login();
        let f = login_form(&a);
        assert!(!f.busy, "on peut corriger et réessayer");
        assert!(f.error.as_deref().unwrap().contains("Adresse du serveur"));
    }

    #[test]
    fn autre_erreur_affichee_sans_deconnecter() {
        let (mut a, _) = app(true);
        a.on_msg(Msg::Tracks { req: a.req, title: String::new(), result: Err(ApiError::Other("HTTP 500".into())) });
        assert_eq!(a.cfg.token, "t");
        assert!(matches!(a.mode, Mode::Normal));
        assert!(a.status.as_ref().unwrap().0.contains("500"));
    }

    #[test]
    fn resultat_d_une_ancienne_demande_ignore() {
        let (mut a, _) = app(true);
        a.req = 2;
        a.on_msg(Msg::Tracks { req: 1, title: "vieux".into(), result: Ok(vec![track("x")]) });
        assert!(a.tracks.is_empty());
        a.on_msg(Msg::Tracks { req: 2, title: "récent".into(), result: Ok(vec![track("y")]) });
        assert_eq!((a.tracks_title.as_str(), a.tracks.len()), ("récent", 1));
    }

    #[test]
    fn enfants_charges_puis_resultat_perime_ignore() {
        let (mut a, _) = app(true);
        let playlists = [2, 0];
        a.node_mut(&playlists).unwrap().loading = true;
        a.node_mut(&playlists).unwrap().expanded = true;
        let pl = |n: &str| Node::new(n, Kind::Playlist(n.into()));
        a.on_msg(Msg::Children { path: playlists.to_vec(), result: Ok(vec![pl("B.Rap"), pl("Chill")]) });
        assert_eq!(labels(&a)[4..6], ["B.Rap", "Chill"]);

        // Pendant un chargement, la session expire et l'arbre est reconstruit :
        // le même chemin désigne désormais « Se connecter… ».
        a.node_mut(&[2, 1]).unwrap().loading = true;
        a.session_expired();
        a.on_msg(Msg::Children { path: vec![2, 0], result: Ok(vec![pl("intrus")]) });
        assert!(a.node(&[2, 0]).unwrap().children.is_none());
        assert_eq!(labels(&a), ["File d'attente", "Ce PC", "Jellyfin", "Se connecter…"]);
    }

    /// Une file de 10 titres locaux, le premier « en cours », le suivant prévu.
    fn en_lecture(a: &mut App) {
        a.queue = Queue::new((0..10).map(|i| track(&i.to_string())).collect(), 0);
        a.now = Some(track("0"));
        a.loaded = true;
        a.plan_upcoming();
    }

    fn now(a: &App) -> &str {
        a.now.as_ref().map(|t| t.title.as_str()).unwrap_or("-")
    }

    #[test]
    fn trois_titres_illisibles_arretent_la_lecture() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        for _ in 0..2 {
            a.on_msg(Msg::Player(player::Event::Error("fichier absent".into())));
            assert!(a.now.is_some());
        }
        a.on_msg(Msg::Player(player::Event::Error("fichier absent".into())));
        assert!(a.now.is_none());
        assert!(a.status.as_ref().unwrap().0.starts_with("3 titres illisibles"));
    }

    #[test]
    fn une_lecture_reussie_remet_le_compteur_a_zero() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        a.on_msg(Msg::Player(player::Event::Error("x".into())));
        a.on_msg(Msg::Player(player::Event::Error("x".into())));
        a.on_msg(Msg::Player(player::Event::Time(12.0)));
        a.on_msg(Msg::Player(player::Event::Error("x".into())));
        assert!(a.now.is_some());
    }

    #[test]
    fn deplacement_borne() {
        assert_eq!(step(None, 0, 1), None);
        assert_eq!(step(None, 5, 1), Some(1));
        assert_eq!(step(Some(4), 5, 1), Some(4));
        assert_eq!(step(Some(1), 5, -10), Some(0));
        assert_eq!(step(Some(5), 5, 0), Some(4));
    }

    #[test]
    fn clic_sur_une_ligne() {
        let zone = Rect::new(0, 10, 30, 8); // bordure en y=10 et y=17
        assert_eq!(row_at(zone, 10, 1, 0, 50), None, "bordure du haut");
        assert_eq!(row_at(zone, 11, 1, 0, 50), Some(0));
        assert_eq!(row_at(zone, 13, 1, 20, 50), Some(22), "décalage du défilement");
        assert_eq!(row_at(zone, 17, 1, 0, 50), None, "bordure du bas");
        assert_eq!(row_at(zone, 12, 2, 0, 50), Some(0), "ligne d'en-tête du tableau");
        assert_eq!(row_at(zone, 15, 1, 0, 3), None, "sous le dernier élément");
    }

    #[test]
    fn enchainement_sur_le_titre_precharge_puis_fin_de_file() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        assert!(a.upcoming_sent, "le suivant est confié à mpv à l'avance");
        a.on_msg(Msg::Player(player::Event::Eof));
        assert_eq!(now(&a), "1");
        a.on_msg(Msg::Player(player::Event::Eof));
        assert_eq!(now(&a), "2");
        for _ in 2..9 {
            a.on_msg(Msg::Player(player::Event::Eof));
        }
        assert_eq!(now(&a), "9");
        assert!(!a.upcoming_sent, "plus rien après le dernier");
        a.on_msg(Msg::Player(player::Event::Eof));
        assert_eq!(now(&a), "-");
        assert!(a.status.as_ref().unwrap().0.contains("Fin de la file"));
    }

    #[test]
    fn filtre_au_fil_de_la_frappe() {
        let (mut a, _) = app(false);
        let mut t = vec![track("Tchikita"), track("Bande organisée"), track("Tout va bien")];
        t[1].artist = "Jul".into();
        a.set_tracks("B.Rap".into(), t, View::List);
        let touche = |a: &mut App, c: KeyCode| a.on_key(KeyEvent::from(c));
        touche(&mut a, KeyCode::Char('f'));
        for c in "jul".chars() {
            touche(&mut a, KeyCode::Char(c));
        }
        assert_eq!(a.view, [1], "l'artiste compte aussi, sans tenir compte des majuscules");
        touche(&mut a, KeyCode::Backspace);
        touche(&mut a, KeyCode::Backspace);
        touche(&mut a, KeyCode::Backspace);
        touche(&mut a, KeyCode::Char('t'));
        assert_eq!(a.view, [0, 2]);
        touche(&mut a, KeyCode::Enter);
        assert!(matches!(a.mode, Mode::Normal));
        assert_eq!(a.filter, "t", "Entrée garde le filtre");
        // Lire depuis la liste filtrée : la file ne contient que ce qui est visible.
        a.play_from(1);
        assert_eq!(a.queue.items().iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Tchikita", "Tout va bien"]);
        assert_eq!(a.queue.index(), 1);
        touche(&mut a, KeyCode::Char('f'));
        touche(&mut a, KeyCode::Esc);
        assert_eq!(a.view, [0, 1, 2], "Échap efface le filtre");
    }

    #[test]
    fn ajouter_et_lire_ensuite() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        a.set_tracks("Album".into(), vec![track("A"), track("B")], View::List);
        a.focus = Focus::Tracks;
        a.table.select(Some(0));
        a.on_key(KeyEvent::from(KeyCode::Char('a')));
        a.table.select(Some(1));
        a.on_key(KeyEvent::from(KeyCode::Char('e')));
        let titres: Vec<&str> = a.queue.items().iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titres[..2], ["0", "B"], "« e » : juste après le titre en cours");
        assert_eq!(titres.last(), Some(&"A"), "« a » : en fin de file");
        assert_eq!(a.queue.planned().unwrap().title, "B", "le préchargé suit la file modifiée");
        a.on_msg(Msg::Player(player::Event::Eof));
        assert_eq!(now(&a), "B");
    }

    #[test]
    fn la_file_d_attente_se_consulte_et_se_modifie() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        a.show_queue();
        assert_eq!((a.shown, a.tracks.len(), a.table.selected()), (View::Queue, 10, Some(0)));
        a.focus = Focus::Tracks;
        a.table.select(Some(4));
        a.on_key(KeyEvent::from(KeyCode::Delete));
        assert_eq!(a.queue_len(), 9);
        assert_eq!(a.tracks.len(), 9, "l'affichage suit");
        assert!(a.tracks.iter().all(|t| t.title != "4"));
        a.table.select(Some(0));
        a.remove_from_queue();
        assert!(a.status.as_ref().unwrap().0.contains("en cours"));
        // Entrée dans la file : on saute au titre sans remplacer la file.
        a.table.select(Some(6));
        a.activate_track(6);
        assert_eq!(a.queue.index(), 6);
        assert_eq!(a.queue_len(), 9);
    }

    #[test]
    fn un_resultat_de_recherche_se_glisse_dans_la_file() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        a.set_tracks("Recherche « x »".into(), vec![track("trouvé")], View::Search);
        a.activate_track(0);
        assert_eq!(a.queue_len(), 11, "la file d'origine est gardée");
        assert_eq!(a.queue.current().unwrap().title, "trouvé");
        assert_eq!(a.queue.items()[2].title, "1", "le reste de la file suit");
    }

    #[test]
    fn reprise_de_session() {
        let (mut a, _) = app(false);
        let dir = std::env::temp_dir().join(format!("musiq-session-{}", std::process::id()));
        a.session_file = dir.join("session.json");
        en_lecture(&mut a);
        a.queue.jump(3);
        a.queue_title = "B.Rap".into();
        a.pos = 83.0;
        a.shutdown();

        let (mut b, _) = app(false);
        b.session_file = a.session_file.clone();
        b.resume();
        assert_eq!((now(&b), b.pos, b.paused), ("3", 83.0, true));
        assert_eq!((b.shown, b.queue_len(), b.queue_title.as_str()), (View::Queue, 10, "B.Rap"));
        assert!(!b.loaded, "sans mpv, rien n'est chargé : Espace rechargera");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn aucun_reveil_ni_redessin_en_pause() {
        let (mut a, _) = app(false);
        assert_eq!(a.deadline(), None, "à l'arrêt : on attend sans rien faire");
        en_lecture(&mut a);
        assert!(a.deadline().unwrap() <= Duration::from_secs_f64(SECTOR_STEP), "en lecture : l'animation");
        a.paused = true;
        assert_eq!(a.deadline(), None);
        assert!(!a.tick());
        a.flash("message");
        assert!(a.deadline().unwrap() <= STATUS_LIFE, "un message doit s'effacer à temps");
    }

    #[test]
    fn redessin_seulement_quand_l_affichage_change() {
        let (mut a, _) = app(false);
        en_lecture(&mut a);
        assert!(a.on_msg(Msg::Player(player::Event::Time(1.2))));
        assert!(!a.on_msg(Msg::Player(player::Event::Time(1.7))), "même seconde affichée");
        assert!(a.on_msg(Msg::Player(player::Event::Time(2.0))));
        let bouge = MouseEvent { kind: MouseEventKind::Moved, column: 3, row: 3, modifiers: KeyModifiers::NONE };
        assert!(!a.on_msg(Msg::Input(Event::Mouse(bouge))));
        assert!(a.on_msg(Msg::Input(Event::Resize(80, 24))));
    }

    #[test]
    fn suivi_des_ecoutes_seulement_pour_jellyfin() {
        let (mut a, _) = app(true);
        a.set_now(track("local"));
        assert_eq!(a.reported, None);
        let mut t = track("serveur");
        t.source = Source::Jelly("t1".into());
        a.set_now(t);
        assert_eq!(a.reported.as_deref(), Some("t1"));
        a.report_stop();
        assert_eq!(a.reported, None);
    }

    #[test]
    fn qualite_mobile_et_aleatoire_se_basculent() {
        let (mut a, _) = app(true);
        a.on_key(KeyEvent::from(KeyCode::Char('m')));
        assert!(a.cfg.mobile_quality);
        let mut t = track("x");
        t.source = Source::Jelly("t1".into());
        assert!(a.target(&t).unwrap().contains("AudioCodec=opus"));
        a.on_key(KeyEvent::from(KeyCode::Char('m')));
        assert!(a.target(&t).unwrap().contains("static=true"));
        a.on_key(KeyEvent::from(KeyCode::Char('s')));
        assert!(a.shuffle);
    }

    #[test]
    fn egaliseur_au_clavier() {
        let (mut a, _) = app(false);
        let touche = |a: &mut App, c: KeyCode| a.on_key(KeyEvent::from(c));
        touche(&mut a, KeyCode::Char('é'));
        assert!(matches!(a.mode, Mode::Eq { band: 0 }));
        assert!(!a.eq_active(), "à plat : rien n'est appliqué");
        touche(&mut a, KeyCode::Right);
        touche(&mut a, KeyCode::Up);
        touche(&mut a, KeyCode::PageUp);
        assert_eq!(a.cfg.eq_gains[1], 4.0);
        assert!(a.cfg.eq_enabled && a.eq_active(), "toucher un réglage active l'égaliseur");
        assert_eq!(a.cfg.eq_preset, eq::CUSTOM);
        for _ in 0..10 {
            touche(&mut a, KeyCode::PageUp);
        }
        assert_eq!(a.cfg.eq_gains[1], eq::MAX_DB, "borné à +12 dB");
        touche(&mut a, KeyCode::Char('0'));
        assert_eq!(a.cfg.eq_gains[1], 0.0);
        assert_eq!(a.cfg.eq_preset, "Plat", "tout à zéro : reconnu comme le préréglage Plat");
        touche(&mut a, KeyCode::Char('p'));
        assert_eq!((a.cfg.eq_preset.as_str(), a.cfg.eq_gains[0]), ("Graves", 6.0));
        touche(&mut a, KeyCode::Char('o'));
        assert!(!a.eq_active(), "« o » coupe sans perdre les réglages");
        assert_eq!(a.cfg.eq_gains[0], 6.0);
        touche(&mut a, KeyCode::Esc);
        assert!(matches!(a.mode, Mode::Normal));
    }

    #[test]
    fn egaliseur_a_la_souris() {
        let (mut a, _) = app(false);
        a.mode = Mode::Eq { band: 0 };
        a.area_eq_bars = (0..10).map(|i| Rect::new(10 + i * 4, 5, 2, 9)).collect();
        let clic = |col, row, kind| MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE };
        a.on_mouse(clic(22, 5, MouseEventKind::Down(MouseButton::Left)));
        assert!(matches!(a.mode, Mode::Eq { band: 3 }));
        assert_eq!(a.cfg.eq_gains[3], 12.0, "haut de la barre = +12 dB");
        a.on_mouse(clic(22, 13, MouseEventKind::Drag(MouseButton::Left)));
        assert_eq!(a.cfg.eq_gains[3], -12.0, "bas de la barre = -12 dB");
        a.on_mouse(clic(22, 9, MouseEventKind::Down(MouseButton::Left)));
        assert_eq!(a.cfg.eq_gains[3], 0.0, "milieu = 0 dB");
        a.on_mouse(clic(22, 9, MouseEventKind::ScrollUp));
        assert_eq!(a.cfg.eq_gains[3], 1.0);
        a.on_mouse(clic(0, 0, MouseEventKind::Down(MouseButton::Left)));
        assert_eq!(a.cfg.eq_gains[3], 1.0, "hors des barres : rien ne change");
    }
}
