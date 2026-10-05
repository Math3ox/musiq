//! État de l'application : arbre des sources, liste de titres, file de lecture, saisies.

use crate::config::{Config, Rng};
use crate::jellyfin::{self, Entry, Jellyfin};
use crate::local;
use crate::model::{Source, Track};
use crate::player::{self, Player};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{ListState, TableState};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// Durée d'affichage d'un secteur du CD : l'animation avance par crans.
const SECTOR_STEP: f64 = 0.11;

pub enum Msg {
    Player(player::Event),
    Children { path: Vec<usize>, result: Result<Vec<Node>, String> },
    Tracks { req: u64, title: String, result: Result<Vec<Track>, String> },
    Login(Result<(String, String), String>),
}

impl From<player::Event> for Msg {
    fn from(e: player::Event) -> Self {
        Msg::Player(e)
    }
}

#[derive(Clone)]
pub enum Kind {
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
        !matches!(self.kind, Kind::Playlist(_) | Kind::Album(_) | Kind::Login)
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
}

pub struct App {
    pub cfg: Config,
    tx: Sender<Msg>,
    pub tree: Vec<Node>,
    pub rows: Vec<Row>,
    pub side: ListState,
    pub tracks: Vec<Track>,
    pub tracks_title: String,
    pub tracks_loading: bool,
    pub table: TableState,
    req: u64,
    pub focus: Focus,
    pub mode: Mode,
    player: Option<Player>,
    queue: Vec<Track>,
    qidx: usize,
    history: Vec<usize>,
    pub now: Option<Track>,
    pub pos: f64,
    pub dur: f64,
    pub paused: bool,
    pub volume: f64,
    pub shuffle: bool,
    /// Avancée de l'animation du CD, en secteurs (ne progresse qu'en lecture).
    pub spin: f64,
    last_tick: Instant,
    pub status: Option<(String, Instant)>,
    pub quit: bool,
    rng: Rng,
    // Zones mémorisées au rendu, pour savoir où l'on clique.
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

impl App {
    pub fn new(cfg: Config, tx: Sender<Msg>) -> App {
        let volume = cfg.volume;
        let mut app = App {
            tx: tx.clone(),
            tree: Vec::new(),
            rows: Vec::new(),
            side: ListState::default(),
            tracks: Vec::new(),
            tracks_title: String::new(),
            tracks_loading: false,
            table: TableState::default(),
            req: 0,
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            player: None,
            queue: Vec::new(),
            qidx: 0,
            history: Vec::new(),
            now: None,
            pos: 0.0,
            dur: 0.0,
            paused: false,
            volume,
            shuffle: false,
            spin: 0.0,
            last_tick: Instant::now(),
            status: None,
            quit: false,
            rng: Rng::new(),
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
        app.tree = vec![pc, server];
        app.side.select(Some(0));
        app.rebuild_rows();

        match Player::start(tx, volume) {
            Ok(p) => app.player = Some(p),
            Err(e) => app.flash(e),
        }
        if !app.cfg.logged_in() {
            app.flash("Pas encore connecté à Jellyfin : appuie sur c");
        }
        app
    }

    pub fn shutdown(&mut self) {
        self.player = None;
        let _ = self.cfg.save();
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    pub fn tick(&mut self) {
        let dt = self.last_tick.elapsed().as_secs_f64();
        self.last_tick = Instant::now();
        if self.now.is_some() && !self.paused {
            self.spin += dt / SECTOR_STEP;
        }
        if self.status.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(5)) {
            self.status = None;
        }
    }

    fn jelly(&self) -> Option<Jellyfin> {
        self.cfg.logged_in().then(|| Jellyfin {
            url: if self.cfg.jellyfin_url.is_empty() { "http://".into() } else { self.cfg.jellyfin_url.clone() },
            token: self.cfg.token.clone(),
            user_id: self.cfg.user_id.clone(),
            device_id: self.cfg.device_id.clone(),
        })
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

    fn set_tracks(&mut self, title: String, tracks: Vec<Track>) {
        self.table.select(if tracks.is_empty() { None } else { Some(0) });
        *self.table.offset_mut() = 0;
        self.tracks = tracks;
        self.tracks_title = title;
        self.tracks_loading = false;
    }

    fn open_tracks(&mut self, path: &[usize]) {
        let Some(n) = self.node(path) else { return };
        let (kind, label) = (n.kind.clone(), n.label.clone());
        self.req += 1;
        let req = self.req;
        match kind {
            Kind::Dir { root, path: dir } => {
                let (_, t) = local::list(&root, &dir);
                self.set_tracks(label, t);
            }
            Kind::Playlist(_) | Kind::Album(_) | Kind::Artist(_) => {
                let Some(j) = self.jelly() else { return self.need_login() };
                self.tracks.clear();
                self.tracks_title = label.clone();
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
        self.tracks.clear();
        self.tracks_title = format!("Recherche « {q} »");
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

    fn play_from(&mut self, i: usize) {
        self.queue = self.tracks.clone();
        self.history.clear();
        self.play_index(i);
    }

    fn play_index(&mut self, i: usize) {
        let Some(t) = self.queue.get(i).cloned() else { return };
        let target = match &t.source {
            Source::Local(p) => p.to_string_lossy().into_owned(),
            Source::Jelly(id) => match self.jelly() {
                Some(j) => j.stream_url(id),
                None => return self.need_login(),
            },
        };
        let Some(p) = self.player.as_mut() else {
            return self.flash("mpv n'est pas lancé : impossible de lire");
        };
        p.load(&target);
        self.qidx = i;
        self.pos = 0.0;
        self.dur = t.duration.unwrap_or(0.0);
        self.paused = false;
        self.now = Some(t);
    }

    fn next(&mut self) {
        if self.queue.is_empty() || self.now.is_none() {
            return;
        }
        let n = self.queue.len();
        let i = if self.shuffle && n > 1 {
            let mut j = self.qidx;
            while j == self.qidx {
                j = self.rng.below(n);
            }
            j
        } else if self.qidx + 1 < n {
            self.qidx + 1
        } else {
            if let Some(p) = self.player.as_mut() {
                p.stop();
            }
            self.now = None;
            self.pos = 0.0;
            self.dur = 0.0;
            return self.flash("Fin de la file de lecture");
        };
        self.history.push(self.qidx);
        self.play_index(i);
    }

    fn prev(&mut self) {
        if self.pos > 3.0 {
            if let Some(p) = self.player.as_mut() {
                p.seek_percent(0.0);
            }
        } else if let Some(i) = self.history.pop() {
            self.play_index(i);
        } else if self.qidx > 0 && !self.queue.is_empty() {
            self.play_index(self.qidx - 1);
        }
    }

    fn toggle_pause(&mut self) {
        if self.now.is_none() {
            // Rien en cours : Espace lance le titre sélectionné.
            if let Some(i) = self.table.selected() {
                self.play_from(i);
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

    // ── Connexion ───────────────────────────────────────────────────────────

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

    // ── Messages des fils d'arrière-plan ────────────────────────────────────

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Player(e) => match e {
                player::Event::Time(t) => self.pos = t,
                player::Event::Duration(d) => self.dur = d,
                player::Event::Pause(p) => self.paused = p,
                player::Event::Volume(v) => self.volume = v,
                player::Event::Eof => self.next(),
                player::Event::Error(e) => {
                    self.flash(format!("Lecture impossible : {e}"));
                    self.next();
                }
            },
            Msg::Children { path, result } => {
                let err = match self.node_mut(&path) {
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
                if let Some(e) = err {
                    self.flash(e);
                }
                self.rebuild_rows();
            }
            Msg::Tracks { req, title, result } => {
                if req != self.req {
                    return; // une demande plus récente a pris le relais
                }
                match result {
                    Ok(t) => self.set_tracks(title, t),
                    Err(e) => {
                        self.tracks_loading = false;
                        self.flash(e);
                    }
                }
            }
            Msg::Login(result) => {
                let Mode::Login(f) = &mut self.mode else { return };
                match result {
                    Ok((token, id)) => {
                        self.cfg.jellyfin_url = f.url.trim().to_string();
                        self.cfg.user_name = f.user.trim().to_string();
                        self.cfg.token = token;
                        self.cfg.user_id = id;
                        self.mode = Mode::Normal;
                        let ch = self.server_children();
                        if let Some(server) = self.tree.get_mut(1) {
                            server.children = Some(ch);
                            server.expanded = true;
                        }
                        self.rebuild_rows();
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
            KeyCode::Char('s') => {
                self.shuffle = !self.shuffle;
                self.flash(if self.shuffle { "Lecture aléatoire activée" } else { "Lecture aléatoire désactivée" });
            }
            KeyCode::Char('/') => self.mode = Mode::Search(String::new()),
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
        let len = self.tracks.len();
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
                    self.play_from(i);
                }
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

    // ── Souris ──────────────────────────────────────────────────────────────

    pub fn on_mouse(&mut self, m: MouseEvent) {
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
                    self.table.select(step(self.table.selected(), self.tracks.len(), d));
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
                    if let Some(i) = row_at(self.area_tracks, m.row, 2, self.table.offset(), self.tracks.len()) {
                        self.table.select(Some(i));
                        self.play_from(i);
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

/// Index de l'élément sous la souris dans une liste encadrée.
fn row_at(area: Rect, y: u16, top: u16, offset: usize, len: usize) -> Option<usize> {
    let first = area.y + top;
    if y < first || y >= area.y + area.height.saturating_sub(1) {
        return None;
    }
    let i = offset + (y - first) as usize;
    (i < len).then_some(i)
}
