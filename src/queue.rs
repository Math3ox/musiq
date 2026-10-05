//! File de lecture : ordre normal ou aléatoire, historique pour « précédent ».
//! Le titre suivant est choisi à l'avance (`plan`) pour que mpv puisse le
//! précharger et l'enchaîner sans blanc. Logique pure, sans mpv, testable.

use crate::config::Rng;

pub struct Queue<T> {
    items: Vec<T>,
    idx: usize,
    /// Titres joués avant le courant, pour revenir en arrière (y compris en aléatoire).
    history: Vec<usize>,
    /// Titre choisi pour passer après le courant (déjà transmis à mpv).
    planned: Option<usize>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue { items: Vec::new(), idx: 0, history: Vec::new(), planned: None }
    }
}

impl<T> Queue<T> {
    pub fn new(items: Vec<T>, start: usize) -> Queue<T> {
        let idx = start.min(items.len().saturating_sub(1));
        Queue { items, idx, history: Vec::new(), planned: None }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn index(&self) -> usize {
        self.idx
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.idx)
    }

    /// Choisit le titre qui suivra le courant, sans y passer. None = fin de file.
    pub fn plan(&mut self, shuffle: bool, rng: &mut Rng) -> Option<&T> {
        let n = self.items.len();
        self.planned = if shuffle && n > 1 {
            // Jamais deux fois le même titre d'affilée.
            let j = rng.below(n - 1);
            Some(if j >= self.idx { j + 1 } else { j })
        } else if self.idx + 1 < n {
            Some(self.idx + 1)
        } else {
            None
        };
        self.planned.and_then(|i| self.items.get(i))
    }

    #[cfg(test)]
    pub fn planned(&self) -> Option<&T> {
        self.planned.and_then(|i| self.items.get(i))
    }

    /// Passe au titre prévu. None = rien de prévu (fin de file).
    pub fn advance(&mut self) -> Option<&T> {
        let i = self.planned.take()?;
        self.history.push(self.idx);
        self.idx = i;
        self.current()
    }

    /// Passe au titre suivant (en le choisissant s'il ne l'est pas encore).
    pub fn next(&mut self, shuffle: bool, rng: &mut Rng) -> Option<&T> {
        if self.planned.is_none() {
            self.plan(shuffle, rng);
        }
        self.advance()
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
        self.planned = None;
        self.current()
    }

    /// Saute directement à un titre de la file.
    pub fn jump(&mut self, i: usize) -> Option<&T> {
        if i >= self.items.len() {
            return None;
        }
        self.history.push(self.idx);
        self.idx = i;
        self.planned = None;
        self.current()
    }

    /// Ajoute à la fin de la file.
    pub fn push(&mut self, item: T) {
        self.items.push(item);
        self.planned = None;
    }

    /// Insère juste après le titre courant (« lire ensuite »). Renvoie sa position.
    pub fn insert_next(&mut self, item: T) -> usize {
        let at = if self.items.is_empty() { 0 } else { self.idx + 1 };
        self.items.insert(at, item);
        for h in &mut self.history {
            if *h >= at {
                *h += 1;
            }
        }
        self.planned = None;
        at
    }

    /// Retire un titre de la file. Le titre en cours ne peut pas être retiré.
    pub fn remove(&mut self, i: usize) -> Option<T> {
        if i >= self.items.len() || i == self.idx {
            return None;
        }
        let item = self.items.remove(i);
        if i < self.idx {
            self.idx -= 1;
        }
        self.history.retain(|&h| h != i);
        for h in &mut self.history {
            if *h > i {
                *h -= 1;
            }
        }
        self.planned = None;
        Some(item)
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

    #[test]
    fn le_titre_prevu_est_celui_qui_passe() {
        let mut rng = Rng::with_seed(9);
        let mut q = q(20, 0);
        for _ in 0..50 {
            let prevu = *q.plan(true, &mut rng).unwrap();
            assert_eq!(q.planned(), Some(&prevu));
            assert_eq!(q.advance(), Some(&prevu));
            assert_eq!(q.planned(), None, "un titre prévu ne sert qu'une fois");
        }
        assert_eq!(q.advance(), None, "rien de prévu : on ne bouge pas");
    }

    #[test]
    fn lire_ensuite_et_ajouter() {
        let mut rng = Rng::with_seed(1);
        let mut q = q(3, 1);
        q.plan(false, &mut rng);
        assert_eq!(q.insert_next(9), 2);
        assert_eq!(q.items(), [0, 1, 9, 2]);
        assert_eq!(q.planned(), None, "la file a changé : le suivant est à recalculer");
        q.push(7);
        assert_eq!(q.next(false, &mut rng), Some(&9));
        assert_eq!(q.next(false, &mut rng), Some(&2));
        assert_eq!(q.next(false, &mut rng), Some(&7));
        let mut vide: Queue<usize> = Queue::default();
        assert_eq!(vide.insert_next(5), 0);
        assert_eq!(vide.current(), Some(&5));
    }

    #[test]
    fn insertion_garde_l_historique_coherent() {
        let mut rng = Rng::with_seed(1);
        let mut q = q(5, 0);
        q.next(false, &mut rng);
        q.next(false, &mut rng); // historique : 0, 1 ; courant : 2
        q.insert_next(99);
        assert_eq!(q.prev(), Some(&1));
        assert_eq!(q.prev(), Some(&0));
    }

    #[test]
    fn retirer_un_titre() {
        let mut rng = Rng::with_seed(1);
        let mut q = q(5, 0);
        q.next(false, &mut rng);
        q.next(false, &mut rng); // courant : 2
        assert_eq!(q.remove(2), None, "pas le titre en cours");
        assert_eq!(q.remove(0), Some(0));
        assert_eq!(q.items(), [1, 2, 3, 4]);
        assert_eq!(q.current(), Some(&2));
        assert_eq!(q.remove(3), Some(4));
        assert_eq!(q.prev(), Some(&1), "l'historique suit les décalages");
        assert_eq!(q.prev(), None);
        assert_eq!(q.remove(42), None);
    }

    #[test]
    fn sauter_a_un_titre() {
        let mut q = q(5, 0);
        assert_eq!(q.jump(3), Some(&3));
        assert_eq!(q.prev(), Some(&0));
        assert_eq!(q.jump(9), None);
    }
}
