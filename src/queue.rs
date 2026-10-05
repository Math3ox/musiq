//! File de lecture : ordre normal ou aléatoire, historique pour « précédent ».
//! Logique pure, sans mpv, pour pouvoir la tester.

use crate::config::Rng;

pub struct Queue<T> {
    items: Vec<T>,
    idx: usize,
    /// Titres joués avant le courant, pour revenir en arrière (y compris en aléatoire).
    history: Vec<usize>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue { items: Vec::new(), idx: 0, history: Vec::new() }
    }
}

impl<T> Queue<T> {
    pub fn new(items: Vec<T>, start: usize) -> Queue<T> {
        let idx = start.min(items.len().saturating_sub(1));
        Queue { items, idx, history: Vec::new() }
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.idx)
    }

    /// Passe au titre suivant. None = fin de la file (rien ne change).
    pub fn next(&mut self, shuffle: bool, rng: &mut Rng) -> Option<&T> {
        let n = self.items.len();
        let i = if shuffle && n > 1 {
            // Jamais deux fois le même titre d'affilée.
            let j = rng.below(n - 1);
            if j >= self.idx { j + 1 } else { j }
        } else if self.idx + 1 < n {
            self.idx + 1
        } else {
            return None;
        };
        self.history.push(self.idx);
        self.idx = i;
        self.current()
    }

    /// Revient au titre joué juste avant. None = déjà au début.
    pub fn prev(&mut self) -> Option<&T> {
        if let Some(i) = self.history.pop() {
            self.idx = i;
        } else if self.idx > 0 && !self.items.is_empty() {
            self.idx -= 1;
        } else {
            return None;
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: usize, start: usize) -> Queue<usize> {
        Queue::new((0..n).collect(), start)
    }

    #[test]
    fn ordre_normal_puis_fin_de_file() {
        let mut rng = Rng::with_seed(1);
        let mut q = q(3, 0);
        assert_eq!(q.next(false, &mut rng), Some(&1));
        assert_eq!(q.next(false, &mut rng), Some(&2));
        assert_eq!(q.next(false, &mut rng), None);
        assert_eq!(q.current(), Some(&2), "la fin de file ne change pas le titre courant");
    }

    #[test]
    fn precedent_suit_l_historique() {
        let mut rng = Rng::with_seed(1);
        let mut q = q(5, 2);
        q.next(false, &mut rng);
        q.next(false, &mut rng);
        assert_eq!(q.prev(), Some(&3));
        assert_eq!(q.prev(), Some(&2));
        // Historique épuisé : on recule simplement dans la liste.
        assert_eq!(q.prev(), Some(&1));
        assert_eq!(q.prev(), Some(&0));
        assert_eq!(q.prev(), None);
    }

    #[test]
    fn aleatoire_jamais_deux_fois_le_meme_et_reste_dans_la_liste() {
        let mut rng = Rng::with_seed(42);
        let mut q = q(4, 0);
        let mut vus = [false; 4];
        for _ in 0..500 {
            let avant = *q.current().unwrap();
            let apres = *q.next(true, &mut rng).expect("l'aléatoire ne s'arrête jamais");
            assert_ne!(avant, apres);
            vus[apres] = true;
        }
        assert!(vus.iter().all(|&v| v), "tous les titres finissent par passer");
    }

    #[test]
    fn aleatoire_puis_precedent_revient_au_meme_titre() {
        let mut rng = Rng::with_seed(7);
        let mut q = q(10, 3);
        let parcours: Vec<usize> = (0..5).map(|_| *q.next(true, &mut rng).unwrap()).collect();
        for &attendu in parcours.iter().rev().skip(1) {
            assert_eq!(q.prev(), Some(&attendu));
        }
        assert_eq!(q.prev(), Some(&3));
    }

    #[test]
    fn file_vide_ou_a_un_seul_titre() {
        let mut rng = Rng::with_seed(1);
        let mut vide: Queue<usize> = Queue::new(Vec::new(), 0);
        assert_eq!(vide.current(), None);
        assert_eq!(vide.next(false, &mut rng), None);
        assert_eq!(vide.next(true, &mut rng), None);
        assert_eq!(vide.prev(), None);
        let mut un = q(1, 0);
        assert_eq!(un.next(true, &mut rng), None, "aléatoire sur un seul titre = fin de file");
    }

    #[test]
    fn depart_hors_limites_ramene_au_dernier() {
        assert_eq!(q(3, 99).current(), Some(&2));
    }
}
