//! Dessin de l'interface (ratatui).

use crate::app::{App, Focus, Kind, LoginForm, Mode, View};
use crate::disc::{Disc, SECTORS};
use crate::eq;
use crate::model::fmt_time;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, LineGauge, List, ListItem, Padding, Paragraph, Row, Table,
};

const ACCENT: Color = Color::Rgb(217, 119, 87);
const DIM: Color = Color::DarkGray;

/// En dessous de ces dimensions, le panneau Lecture passe en version compacte (sans le CD).
const FULL_PLAYER_MIN_W: u16 = 60;
const FULL_PLAYER_MIN_H: u16 = 24;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Les commandes passent à la ligne plutôt que d'être coupées (4 lignes au plus).
    let help_lines = help_lines(app, area.width);
    let help_h = help_lines.len().clamp(1, 4) as u16;
    let player_h = if area.width >= FULL_PLAYER_MIN_W && area.height >= FULL_PLAYER_MIN_H { 9 } else { 5 };
    let [main, player, help] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(player_h), Constraint::Length(help_h)])
            .areas(area);
    let side_w = (main.width * 30 / 100).clamp(18, 34);
    let [side, list] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(10)]).areas(main);
    draw_side(f, app, side);
    draw_tracks(f, app, list);
    draw_player(f, app, player);
    f.render_widget(Paragraph::new(help_lines), help);
    if let Mode::Login(form) = &app.mode {
        draw_login(f, form);
    }
    if let Mode::Help = app.mode {
        draw_keys(f);
    }
    if let Mode::Eq { band } = app.mode {
        draw_eq(f, app, band);
    } else {
        app.area_eq_bars.clear();
    }
}

fn panel(title: String, focused: bool) -> Block<'static> {
    let color = if focused { ACCENT } else { DIM };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Span::styled(format!(" {title} "), Style::new().bold().fg(if focused { ACCENT } else { Color::Reset })))
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().bg(ACCENT).fg(Color::Black)
    } else {
        Style::new().bg(Color::Rgb(60, 60, 60))
    }
}

fn draw_side(f: &mut Frame, app: &mut App, area: Rect) {
    app.area_side = area;
    let focused = app.focus == Focus::Sidebar;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .filter_map(|r| app.node(&r.path).map(|n| (r, n)))
        .map(|(r, n)| {
            let marker = if !n.expandable() {
                "  "
            } else if n.loading {
                "◌ "
            } else if n.expanded {
                "▾ "
            } else {
                "▸ "
            };
            let style = match &n.kind {
                Kind::PcRoot | Kind::ServerRoot | Kind::Queue => Style::new().bold(),
                Kind::Login => Style::new().fg(ACCENT).italic(),
                _ => Style::new(),
            };
            ListItem::new(Line::from(vec![
                Span::raw(" ".repeat(r.depth * 2 + 1)),
                Span::styled(marker, Style::new().fg(DIM)),
                Span::styled(n.label.clone(), style),
                Span::styled(
                    match n.kind {
                        Kind::Queue if app.queue_len() > 0 => format!("  {}", app.queue_len()),
                        _ => String::new(),
                    },
                    Style::new().fg(DIM),
                ),
            ]))
        })
        .collect();
    let list = List::new(items).block(panel("Sources".into(), focused)).highlight_style(highlight(focused));
    f.render_stateful_widget(list, area, &mut app.side);
}

fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect) {
    app.area_tracks = area;
    let focused = app.focus == Focus::Tracks;
    let title = if app.tracks_title.is_empty() {
        "Titres".to_string()
    } else if app.tracks_loading {
        format!("{} · chargement…", app.tracks_title)
    } else if app.filter.is_empty() {
        format!("{} · {} titres", app.tracks_title, app.tracks.len())
    } else {
        format!("{} · filtre « {} » : {} / {}", app.tracks_title, app.filter, app.view.len(), app.tracks.len())
    };
    let block = panel(title, focused);
    if app.view.is_empty() {
        let msg = if app.tracks_loading {
            "Chargement…"
        } else if !app.tracks.is_empty() {
            "Aucun titre ne correspond au filtre (Échap pour l'effacer)."
        } else if app.shown == View::Queue {
            "La file est vide : « a » ajoute un titre, « e » le place juste après celui en cours."
        } else if app.tracks_title.is_empty() {
            "Choisis une playlist, un album ou un dossier à gauche."
        } else {
            "Aucun titre ici."
        };
        let p = Paragraph::new(format!("\n{msg}")).style(Style::new().fg(DIM)).centered().block(block);
        f.render_widget(p, area);
        return;
    }
    let playing = app.now.as_ref().map(|t| t.source.clone());
    let rows: Vec<(Vec<Cell>, Style)> = app
        .view
        .iter()
        .map(|&i| (i, &app.tracks[i]))
        .map(|(i, t)| {
            let is_now = playing.as_ref() == Some(&t.source);
            let num = match (is_now, app.paused) {
                (true, true) => "⏸".to_string(),
                (true, false) => "▶".to_string(),
                _ => (i + 1).to_string(),
            };
            let cells = vec![
                Cell::from(num),
                Cell::from(t.title.clone()),
                Cell::from(t.artist.clone()),
                Cell::from(t.album.clone()),
                Cell::from(t.duration.map(fmt_time).unwrap_or_default()),
            ];
            (cells, if is_now { Style::new().fg(ACCENT).bold() } else { Style::new() })
        })
        .collect();
    // Colonnes masquées quand la place manque : l'album d'abord, puis l'artiste.
    let (show_artist, show_album) = (area.width >= 56, area.width >= 80);
    let keep = |i: usize| match i {
        2 => show_artist,
        3 => show_album,
        _ => true,
    };
    let rows: Vec<Row> = rows
        .into_iter()
        .map(|(cells, style)| Row::new(cells.into_iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, c)| c)).style(style))
        .collect();
    let header = Row::new(["#", "Titre", "Artiste", "Album", "Durée"].into_iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, c)| c))
        .style(Style::new().fg(DIM).bold());
    let widths: Vec<Constraint> = [
        Constraint::Length(4),
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Fill(2),
        Constraint::Length(6),
    ]
    .into_iter()
    .enumerate()
    .filter(|(i, _)| keep(*i))
    .map(|(_, c)| c)
    .collect();
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(highlight(focused));
    f.render_stateful_widget(table, area, &mut app.table);
}

fn draw_player(f: &mut Frame, app: &mut App, area: Rect) {
    app.area_player = area;
    let block = panel("Lecture".into(), app.focus == Focus::Player);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    // Fenêtre petite : pas de CD, et l'essentiel sur trois lignes.
    let compact = inner.height < 7 || inner.width < FULL_PLAYER_MIN_W - 4;
    let (title_l, artist_l, album_l, gauge_l, status_l) = if compact {
        let [t, g, st] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);
        (t, Rect::default(), Rect::default(), g, st)
    } else {
        let [disc_area, _, info] =
            Layout::horizontal([Constraint::Length(24), Constraint::Length(3), Constraint::Min(10)]).areas(inner);
        f.render_widget(
            Disc { head: app.spin as usize % SECTORS, active: app.now.is_some() && !app.paused, accent: ACCENT },
            disc_area,
        );
        let [_, t, a, al, _, g, st] = Layout::vertical([Constraint::Length(1); 7]).areas(info);
        (t, a, al, g, st)
    };
    match &app.now {
        Some(t) => {
            let mut line = vec![
                Span::styled(if app.paused { "⏸  " } else { "▶  " }, Style::new().fg(ACCENT)),
                Span::styled(t.title.clone(), Style::new().bold()),
            ];
            if compact && !t.artist.is_empty() {
                line.push(Span::styled(format!(" — {}", t.artist), Style::new().fg(DIM)));
            }
            f.render_widget(Paragraph::new(Line::from(line)), title_l);
            f.render_widget(Paragraph::new(format!("   {}", t.artist)), artist_l);
            f.render_widget(Paragraph::new(format!("   {}", t.album)).style(Style::new().fg(DIM)), album_l);
        }
        None => f.render_widget(Paragraph::new("■  Rien en lecture").style(Style::new().fg(DIM)), title_l),
    }

    let label = format!(
        "{} / {}",
        fmt_time(app.pos),
        if app.dur > 0.0 { fmt_time(app.dur) } else { "--:--".into() }
    );
    let [time_area, gauge_area] =
        Layout::horizontal([Constraint::Length(label.chars().count() as u16 + 4), Constraint::Min(1)]).areas(gauge_l);
    f.render_widget(Paragraph::new(format!("   {label}")).style(Style::new().fg(DIM)), time_area);
    let ratio = if app.dur > 0.0 { (app.pos / app.dur).clamp(0.0, 1.0) } else { 0.0 };
    let gauge = LineGauge::default()
        .ratio(ratio)
        .label("")
        .filled_style(Style::new().fg(ACCENT))
        .unfilled_style(Style::new().fg(DIM))
        .filled_symbol("━")
        .unfilled_symbol("━");
    f.render_widget(gauge, gauge_area);
    app.area_gauge = gauge_area;

    let mut status = vec![Span::styled(format!("   vol {:.0} %", app.volume), Style::new().fg(DIM))];
    if app.eq_active() {
        status.push(Span::styled(format!("   ·   égaliseur {}", app.cfg.eq_preset), Style::new().fg(ACCENT)));
    }
    if app.cfg.mobile_quality {
        status.push(Span::styled("   ·   qualité mobile", Style::new().fg(ACCENT)));
    }
    if app.shuffle {
        status.push(Span::styled("   ·   aléatoire", Style::new().fg(ACCENT)));
    }
    f.render_widget(Paragraph::new(Line::from(status)), status_l);
}

/// Commandes réparties sur autant de lignes que nécessaire pour tenir dans `width`.
fn wrap_hints(list: &[(&str, &str)], width: u16) -> Vec<Line<'static>> {
    // Séparateur plus court quand la place manque.
    let sep = if width < 90 { " · " } else { "  ·  " };
    let sep_w = Line::from(sep).width();
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![Span::raw(" ")]];
    let mut used = 1;
    for (k, d) in list {
        let item = Line::from(format!("{k} {d}")).width();
        let line = lines.last_mut().unwrap();
        let first = line.len() == 1;
        if !first && used + sep_w + item > width as usize {
            lines.push(vec![Span::raw(" ")]);
            used = 1;
        }
        let line = lines.last_mut().unwrap();
        if line.len() > 1 {
            line.push(Span::styled(sep, Style::new().fg(DIM)));
            used += sep_w;
        }
        line.push(Span::styled(k.to_string(), Style::new().fg(ACCENT).bold()));
        line.push(Span::styled(format!(" {d}"), Style::new().fg(DIM)));
        used += item;
    }
    lines.into_iter().map(Line::from).collect()
}

/// Ligne(s) du bas : saisie en cours, message, ou commandes repliées sur la largeur.
fn help_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if let Mode::Filter = &app.mode {
        return vec![Line::from(vec![
            Span::styled(" Filtrer : ", Style::new().fg(ACCENT).bold()),
            Span::raw(app.filter.clone()),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled("     Entrée garder · Échap effacer", Style::new().fg(DIM)),
        ])];
    }
    if let Mode::Search(q) = &app.mode {
        return vec![Line::from(vec![
            Span::styled(" Rechercher : ", Style::new().fg(ACCENT).bold()),
            Span::raw(q.clone()),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled("     Entrée chercher · Échap annuler", Style::new().fg(DIM)),
        ])];
    }
    if let Some((m, _)) = &app.status {
        return vec![Line::styled(format!(" {m}"), Style::new().fg(ACCENT))];
    }
    // Les commandes propres au panneau d'abord : ce sont elles qui restent
    // visibles si la fenêtre est vraiment trop petite.
    let mut h: Vec<(&str, &str)> = match app.focus {
        Focus::Sidebar => vec![("↑↓", "naviguer"), ("→ Entrée", "ouvrir"), ("←", "fermer")],
        Focus::Tracks if app.shown == View::Queue => {
            vec![("↑↓", "naviguer"), ("Entrée", "aller à"), ("Suppr", "retirer"), ("f", "filtrer")]
        }
        Focus::Tracks => vec![
            ("↑↓", "naviguer"),
            ("Entrée", "lire"),
            ("a", "ajouter à la file"),
            ("e", "lire ensuite"),
            ("f", "filtrer"),
        ],
        Focus::Player => vec![("←→", "±5 s"), ("↑↓", "volume")],
    };
    h.extend([
        ("Espace", "pause"),
        ("n/p", "suivant/préc."),
        (",/.", "±10 s"),
        ("+/-", "volume"),
        ("s", "aléatoire"),
        ("/", "chercher"),
        ("Tab", "panneau"),
        ("E", "égaliseur"),
        ("m", "qualité mobile"),
        ("c", "connexion"),
        ("q", "quitter"),
    ]);
    fit_hints(&h, width, 4)
}

/// Commandes en `max` lignes au plus. Si elles ne tiennent pas, on garde les
/// premières (les plus utiles) et la dernière ligne finit par « ? aide », qui
/// ouvre la liste complète : aucune commande ne devient introuvable.
fn fit_hints(list: &[(&str, &str)], width: u16, max: usize) -> Vec<Line<'static>> {
    let all = wrap_hints(list, width);
    if all.len() <= max {
        return all;
    }
    for keep in (0..list.len()).rev() {
        let mut items = list[..keep].to_vec();
        items.push(("?", "aide"));
        let lines = wrap_hints(&items, width);
        if lines.len() <= max {
            return lines;
        }
    }
    wrap_hints(&[("?", "aide")], width)
}

/// Toutes les touches, regroupées (touche « ? »).
const KEYS: &[(&str, &[(&str, &str)])] = &[
    ("Navigation", &[
        ("↑ ↓ / molette", "se déplacer"),
        ("→ / Entrée", "ouvrir · lire"),
        ("←", "fermer · remonter"),
        ("Tab", "changer de panneau"),
        ("clic", "ouvrir · lire · se placer"),
    ]),
    ("Lecture", &[
        ("Espace", "pause"),
        ("n / p", "titre suivant / précédent"),
        (", / .", "-10 s / +10 s"),
        ("+ / -", "volume"),
        ("s", "lecture aléatoire"),
        ("m", "qualité mobile"),
        ("E / é", "égaliseur"),
    ]),
    ("Liste et file", &[
        ("a", "ajouter à la file"),
        ("e", "lire ensuite"),
        ("Suppr", "retirer de la file"),
        ("f", "filtrer la liste"),
        ("/", "rechercher partout"),
    ]),
    ("Divers", &[("c", "connexion Jellyfin"), ("?", "cette aide"), ("q", "quitter")]),
];

fn draw_keys(f: &mut Frame) {
    let r = f.area();
    let w = 48.min(r.width);
    let mut lines = Vec::new();
    for (group, keys) in KEYS {
        lines.push(Line::styled(format!(" {group}"), Style::new().fg(ACCENT).bold()));
        for (k, d) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("   {k:<15}"), Style::new().bold()),
                Span::styled(d.to_string(), Style::new().fg(DIM)),
            ]));
        }
    }
    // Pas assez de hauteur : chaque groupe sur une ou deux lignes, puis, si ça ne
    // suffit pas, toutes les touches à la suite, sans titres de groupe.
    let fits = |lines: &Vec<Line>| lines.len() as u16 + 2 <= r.height;
    if !fits(&lines) {
        lines.clear();
        for (group, keys) in KEYS {
            lines.push(Line::styled(format!(" {group}"), Style::new().fg(ACCENT).bold()));
            lines.extend(wrap_hints(keys, w.saturating_sub(2)));
        }
    }
    if !fits(&lines) {
        let all: Vec<(&str, &str)> = KEYS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
        lines = wrap_hints(&all, w.saturating_sub(2));
    }
    let close = Line::styled(" n'importe quelle touche pour fermer", Style::new().fg(DIM).italic());
    if lines.len() as u16 + 3 <= r.height {
        lines.push(close);
    }
    let h = (lines.len() as u16 + 2).min(r.height);
    let area = Rect::new(r.x + (r.width - w) / 2, r.y + (r.height - h) / 2, w, h);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(panel("Touches".into(), true)), area);
}

fn draw_login(f: &mut Frame, form: &LoginForm) {
    let r = f.area();
    let (w, h) = (66.min(r.width), 12.min(r.height));
    let area = Rect::new(r.x + (r.width - w) / 2, r.y + (r.height - h) / 2, w, h);
    f.render_widget(Clear, area);
    let block = panel("Connexion à Jellyfin".into(), true).padding(Padding::horizontal(1));
    let field = |i: usize, label: &str, value: String| {
        let active = form.field == i;
        let mut v = vec![
            Span::styled(
                format!("{label:<14}"),
                if active { Style::new().fg(ACCENT).bold() } else { Style::new().fg(DIM) },
            ),
            Span::raw(value),
        ];
        if active {
            v.push(Span::styled("▌", Style::new().fg(ACCENT)));
        }
        Line::from(v)
    };
    let mut lines = vec![
        Line::raw(""),
        field(0, "Adresse", form.url.clone()),
        field(1, "Utilisateur", form.user.clone()),
        field(2, "Mot de passe", "•".repeat(form.pw.chars().count())),
        Line::raw(""),
    ];
    lines.push(if form.busy {
        Line::styled("Connexion…", Style::new().fg(DIM))
    } else if let Some(e) = &form.error {
        Line::styled(e.clone(), Style::new().fg(Color::Red))
    } else {
        Line::styled("Seul un jeton de session est enregistré, jamais le mot de passe.", Style::new().fg(DIM))
    });
    lines.push(Line::raw(""));
    lines.extend(wrap_hints(&[("Entrée", "valider"), ("Tab", "champ suivant"), ("Échap", "annuler")], w.saturating_sub(4)));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Égaliseur façon autoradio : 10 barres verticales en demi-blocs (1,5 dB par
/// demi-ligne), ligne du zéro au milieu.
fn draw_eq(f: &mut Frame, app: &mut App, band: usize) {
    const ROWS: u16 = 9; // 4 lignes au-dessus de zéro, la ligne du zéro, 4 en dessous
    let r = f.area();
    // Colonnes de 5 à 3 caractères selon la largeur disponible.
    let col: u16 = ((r.width.saturating_sub(10)) / 10).clamp(3, 5);
    let bar = col - 2;
    let w = (6 + col * 10 + 4).min(r.width);
    let help = vec![
        ("←→", "bande"),
        ("↑↓", "±1 dB"),
        ("0", "zéro"),
        ("souris", "cliquer, glisser"),
        ("p", "préréglage"),
        ("o", if app.cfg.eq_enabled { "couper" } else { "activer" }),
        ("Échap", "fermer"),
    ];
    let help_lines = wrap_hints(&help, w.saturating_sub(4));
    // Colonnes trop étroites pour « 125 », « 250 »… côte à côte : fréquences en quinconce.
    let staggered = col < 4;
    let label_rows: u16 = if staggered { 2 } else { 1 };
    // bordures (2) + ligne vide + barres + fréquences + valeurs + ligne vide + aide
    let h = (2 + 1 + ROWS + label_rows + 1 + 1 + help_lines.len() as u16).min(r.height);
    let area = Rect::new(r.x + (r.width - w) / 2, r.y + (r.height - h) / 2, w, h);
    f.render_widget(Clear, area);
    let state = if !app.cfg.eq_enabled {
        "désactivé"
    } else if eq::is_flat(&app.cfg.eq_gains) {
        "à plat"
    } else {
        "actif"
    };
    let block = panel(format!("Égaliseur · {} · {state}", app.cfg.eq_preset), true);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let [_, bars, freqs, values, _, help_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(ROWS),
        Constraint::Length(label_rows),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(help_lines.len() as u16),
    ])
    .areas(inner);

    let on = app.cfg.eq_enabled;
    let buf = f.buffer_mut();
    let dim = Style::new().fg(DIM);
    for (row, label) in [(0, "+12"), (ROWS / 2, "  0"), (ROWS - 1, "-12")] {
        buf.set_string(bars.x, bars.y + row, label, dim);
    }
    let mut rects = Vec::new();
    for (i, &g) in app.cfg.eq_gains.iter().enumerate() {
        let x = bars.x + 5 + i as u16 * col;
        if x + bar > bars.x + bars.width {
            break;
        }
        let selected = i == band;
        let color = match (on, selected) {
            (false, _) => DIM,
            (true, true) => ACCENT,
            (true, false) => Color::Rgb(150, 85, 65),
        };
        // Nombre de demi-lignes à remplir, de part et d'autre du zéro.
        let halves = (g.abs() / (eq::MAX_DB / ((ROWS / 2) * 2) as f64)).round() as i32;
        for row in 0..ROWS {
            let y = bars.y + row;
            let mid = ROWS / 2;
            if row == mid {
                let zero = if selected { Style::new().fg(ACCENT) } else { dim };
                buf.set_string(x, y, "─".repeat(bar as usize), zero);
                continue;
            }
            let (dist, side_ok, half) = if row < mid {
                ((mid - 1 - row) as i32, g > 0.0, '▄')
            } else {
                ((row - mid - 1) as i32, g < 0.0, '▀')
            };
            let cover = if side_ok { (halves - 2 * dist).clamp(0, 2) } else { 0 };
            let cell = match cover {
                2 => '█',
                1 => half,
                _ => continue,
            };
            buf.set_string(x, y, cell.to_string().repeat(bar as usize), Style::new().fg(color));
        }
        let label_style = if selected { Style::new().fg(ACCENT).bold() } else { dim };
        let center = |t: &str| format!("{t:^w$}", w = col as usize);
        let fy = if staggered { freqs.y + (i % 2) as u16 } else { freqs.y };
        let label = if staggered { format!("{:<3}", eq::LABELS[i]) } else { center(eq::LABELS[i]) };
        buf.set_string(if staggered { x } else { x - 1 }, fy, label, label_style);
        let v = if g.abs() < 0.05 { "0".to_string() } else { format!("{g:+.0}") };
        buf.set_string(x - 1, values.y, center(&v), label_style);
        // Zone cliquable : toute la largeur de la colonne.
        rects.push(Rect::new(x - 1, bars.y, col, ROWS));
    }
    app.area_eq_bars = rects;
    f.render_widget(Paragraph::new(help_lines), help_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &[(&str, &str)] = &[
        ("↑↓", "naviguer"),
        ("Entrée", "lire"),
        ("a", "ajouter à la file"),
        ("Espace", "pause"),
        ("n/p", "suivant/préc."),
        ("/", "chercher"),
        ("E", "égaliseur"),
        ("q", "quitter"),
    ];

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn chaque_ligne_tient_dans_la_largeur() {
        for w in [30u16, 44, 60, 80, 120] {
            for l in wrap_hints(H, w) {
                assert!(l.width() <= w as usize, "« {} » dépasse {w} colonnes", text(&l));
            }
        }
    }

    #[test]
    fn tout_tient_quand_il_y_a_la_place() {
        let lines = fit_hints(H, 200, 4);
        assert_eq!(lines.len(), 1);
        assert!(text(&lines[0]).contains("q quitter"));
        assert!(!text(&lines[0]).contains("? aide"));
    }

    #[test]
    fn sinon_aide_en_fin_de_derniere_ligne() {
        let lines = fit_hints(H, 30, 2);
        assert!(lines.len() <= 2);
        assert!(text(lines.last().unwrap()).trim_end().ends_with("? aide"));
        assert!(text(&lines[0]).contains("↑↓ naviguer"), "les commandes du panneau passent en premier");
    }

    #[test]
    fn la_liste_complete_contient_chaque_commande_de_la_barre() {
        let all: Vec<&str> = KEYS.iter().flat_map(|(_, k)| k.iter().map(|(_, d)| *d)).collect();
        for needed in ["pause", "égaliseur", "qualité mobile", "quitter", "filtrer la liste", "retirer de la file"] {
            assert!(all.contains(&needed), "{needed} manque dans l'aide");
        }
    }
}
