//! Dessin de l'interface (ratatui).

use crate::app::{App, Focus, Kind, LoginForm, Mode, View};
use crate::disc::{Disc, SECTORS};
use crate::model::fmt_time;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, LineGauge, List, ListItem, Padding, Paragraph, Row, Table,
};

const ACCENT: Color = Color::Rgb(217, 119, 87);
const DIM: Color = Color::DarkGray;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, player, help] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(9), Constraint::Length(1)]).areas(f.area());
    let [side, list] = Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).areas(main);
    draw_side(f, app, side);
    draw_tracks(f, app, list);
    draw_player(f, app, player);
    draw_help(f, app, help);
    if let Mode::Login(form) = &app.mode {
        draw_login(f, form);
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
    let rows: Vec<Row> = app
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
            let row = Row::new(vec![
                Cell::from(num),
                Cell::from(t.title.clone()),
                Cell::from(t.artist.clone()),
                Cell::from(t.album.clone()),
                Cell::from(t.duration.map(fmt_time).unwrap_or_default()),
            ]);
            if is_now { row.style(Style::new().fg(ACCENT).bold()) } else { row }
        })
        .collect();
    let header = Row::new(["#", "Titre", "Artiste", "Album", "Durée"]).style(Style::new().fg(DIM).bold());
    let widths = [
        Constraint::Length(4),
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Fill(2),
        Constraint::Length(6),
    ];
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
    let [disc_area, _, info] =
        Layout::horizontal([Constraint::Length(24), Constraint::Length(3), Constraint::Min(10)]).areas(inner);
    f.render_widget(
        Disc { head: app.spin as usize % SECTORS, active: app.now.is_some() && !app.paused, accent: ACCENT },
        disc_area,
    );

    let [_, title_l, artist_l, album_l, _, gauge_l, status_l] = Layout::vertical([Constraint::Length(1); 7]).areas(info);
    match &app.now {
        Some(t) => {
            let line = Line::from(vec![
                Span::styled(if app.paused { "⏸  " } else { "▶  " }, Style::new().fg(ACCENT)),
                Span::styled(t.title.clone(), Style::new().bold()),
            ]);
            f.render_widget(Paragraph::new(line), title_l);
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
    if app.cfg.mobile_quality {
        status.push(Span::styled("   ·   qualité mobile", Style::new().fg(ACCENT)));
    }
    if app.shuffle {
        status.push(Span::styled("   ·   aléatoire", Style::new().fg(ACCENT)));
    }
    f.render_widget(Paragraph::new(Line::from(status)), status_l);
}

fn hints(list: &[(&str, &str)]) -> Line<'static> {
    let mut v = vec![Span::raw(" ")];
    for (i, (k, d)) in list.iter().enumerate() {
        if i > 0 {
            v.push(Span::styled("  ·  ", Style::new().fg(DIM)));
        }
        v.push(Span::styled(k.to_string(), Style::new().fg(ACCENT).bold()));
        v.push(Span::styled(format!(" {d}"), Style::new().fg(DIM)));
    }
    Line::from(v)
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Mode::Filter = &app.mode {
        Line::from(vec![
            Span::styled(" Filtrer : ", Style::new().fg(ACCENT).bold()),
            Span::raw(app.filter.clone()),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled("     Entrée garder · Échap effacer", Style::new().fg(DIM)),
        ])
    } else if let Mode::Search(q) = &app.mode {
        Line::from(vec![
            Span::styled(" Rechercher : ", Style::new().fg(ACCENT).bold()),
            Span::raw(q.clone()),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled("     Entrée chercher · Échap annuler", Style::new().fg(DIM)),
        ])
    } else if let Some((m, _)) = &app.status {
        Line::styled(format!(" {m}"), Style::new().fg(ACCENT))
    } else {
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
            ("m", "qualité mobile"),
            ("c", "connexion"),
            ("q", "quitter"),
        ]);
        hints(&h)
    };
    f.render_widget(Paragraph::new(line), area);
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
    lines.push(hints(&[("Entrée", "valider"), ("Tab", "champ suivant"), ("Échap", "annuler")]));
    f.render_widget(Paragraph::new(lines).block(block), area);
}
