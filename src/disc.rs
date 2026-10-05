//! Le CD façon vieil autoradio : un disque vu de trois quarts, dont les secteurs
//! s'allument à tour de rôle (changement de phase) pour simuler la rotation.
//!
//! Dessin en demi-blocs « ▀ » : chaque cellule du terminal porte deux pixels
//! (haut = couleur du texte, bas = couleur de fond), ce qui donne des pixels
//! à peu près carrés.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use std::f64::consts::TAU;

/// Nombre de secteurs du disque : 8, comme les afficheurs segmentés d'époque.
pub const SECTORS: usize = 8;

/// Écrasement vertical (disque vu de biais) et inclinaison de l'ellipse.
const SQUASH: f64 = 0.42;
const TILT: f64 = -0.30;
/// Rayon relatif du trou central.
const HOLE: f64 = 0.17;

pub struct Disc {
    /// Secteur « de tête » (le plus lumineux), qui avance quand on joue.
    pub head: usize,
    /// Faux : disque à l'arrêt, gris, sans traînée lumineuse.
    pub active: bool,
    pub accent: Color,
}

fn mix(a: Color, b: Color, t: f64) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => a,
    }
}

impl Disc {
    /// Coordonnées dans le plan du disque (ellipse « redressée ») : (u, v, rayon relatif).
    fn plane(x: f64, y: f64, radius: f64) -> (f64, f64, f64) {
        let (s, c) = TILT.sin_cos();
        let u = (x * c + y * s) / radius;
        let v = (-x * s + y * c) / (radius * SQUASH);
        (u, v, u.hypot(v))
    }

    /// Part (0 à 1) d'un pixel couverte par la zone r ≤ limite : 4×4 échantillons.
    /// Le contour suit mieux l'ellipse inclinée qu'un simple test au centre.
    fn coverage(px: f64, py: f64, radius: f64, limit: f64) -> f64 {
        let mut n = 0;
        for i in 0..4 {
            for j in 0..4 {
                let x = px + (i as f64 + 0.5) / 4.0;
                let y = py + (j as f64 + 0.5) / 4.0;
                if Self::plane(x, y, radius).2 <= limit {
                    n += 1;
                }
            }
        }
        n as f64 / 16.0
    }

    /// Couleur de l'intérieur du disque au point (x, y) relatif au centre.
    fn surface(&self, x: f64, y: f64, radius: f64) -> Color {
        let (u, v, r) = Self::plane(x, y, radius);
        let base = Color::Rgb(58, 60, 68);
        if r < 0.36 {
            return HUB;
        }
        let angle = v.atan2(u).rem_euclid(TAU);
        let sector = (angle / TAU * SECTORS as f64) as usize % SECTORS;
        // Secteurs pairs/impairs légèrement différents : le disque reste
        // lisible même à l'arrêt.
        let resting = if sector % 2 == 0 { base } else { mix(base, RIM, 0.12) };
        if !self.active {
            return resting;
        }
        // Deux reflets opposés, comme la lumière sur un vrai CD, chacun
        // suivi d'une courte traînée qui indique le sens de rotation.
        let behind = (self.head + SECTORS - sector) % (SECTORS / 2);
        let glow = match behind {
            0 => 1.0,
            1 => 0.4,
            _ => 0.0,
        };
        let bright = mix(self.accent, Color::Rgb(255, 225, 200), 0.35);
        mix(resting, bright, glow)
    }
}

const RIM: Color = Color::Rgb(150, 152, 160);
const HUB: Color = Color::Rgb(95, 97, 105);

impl Widget for Disc {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let (w, h) = (area.width as f64, area.height as f64 * 2.0);
        // Plus grand rayon qui tient dans la zone une fois écrasé et incliné.
        let (s, c) = TILT.sin_cos();
        let half_w = (c * c + SQUASH * SQUASH * s * s).sqrt();
        let half_h = (s * s + SQUASH * SQUASH * c * c).sqrt();
        let radius = ((w / 2.0 - 0.5) / half_w).min((h / 2.0 - 0.5) / half_h);

        // Masques en pixels (deux par cellule) : disque plein, et trou central.
        let (pw, ph) = (area.width as usize, area.height as usize * 2);
        let ox = |px: usize| px as f64 - w / 2.0;
        let oy = |py: usize| py as f64 - h / 2.0;
        let mut disc = vec![vec![false; pw]; ph];
        let mut hole = vec![vec![false; pw]; ph];
        for py in 0..ph {
            for px in 0..pw {
                disc[py][px] = Self::coverage(ox(px), oy(py), radius, 1.0) >= 0.5;
                hole[py][px] = Self::coverage(ox(px), oy(py), radius, HOLE) >= 0.5;
            }
        }
        let at = |m: &Vec<Vec<bool>>, x: isize, y: isize| {
            x >= 0 && y >= 0 && (x as usize) < pw && (y as usize) < ph && m[y as usize][x as usize]
        };
        // Pixels isolés aux pointes de l'ellipse (moins de deux voisins) : on les retire.
        let spikes: Vec<(usize, usize)> = (0..ph)
            .flat_map(|py| (0..pw).map(move |px| (px, py)))
            .filter(|&(px, py)| {
                disc[py][px]
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .filter(|&&(dx, dy)| at(&disc, px as isize + dx, py as isize + dy))
                        .count()
                        < 2
            })
            .collect();
        for (px, py) in spikes {
            disc[py][px] = false;
        }
        // Bords au pixel près : un voisin hors du disque, ou dans le trou.
        let color = |px: usize, py: usize| -> Option<Color> {
            if !disc[py][px] || hole[py][px] {
                return None;
            }
            let n = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dy)| (px as isize + dx, py as isize + dy));
            if n.iter().any(|&(x, y)| !at(&disc, x, y) || at(&hole, x, y)) {
                return Some(RIM);
            }
            Some(self.surface(ox(px) + 0.5, oy(py) + 0.5, radius))
        };

        for cy in 0..area.height {
            for cx in 0..area.width {
                let (px, py) = (cx as usize, cy as usize * 2);
                let cell = &mut buf[(area.x + cx, area.y + cy)];
                match (color(px, py), color(px, py + 1)) {
                    (None, None) => {}
                    (Some(t), None) => {
                        cell.set_char('▀').set_fg(t);
                    }
                    (None, Some(b)) => {
                        cell.set_char('▄').set_fg(b);
                    }
                    (Some(t), Some(b)) => {
                        cell.set_char('▀').set_fg(t).set_bg(b);
                    }
                }
            }
        }
    }
}
