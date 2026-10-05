//! Égaliseur 10 bandes, appliqué par mpv (filtres « equalizer » de FFmpeg).
//!
//! Le moins coûteux possible : à plat ou désactivé, aucun filtre n'est chargé ;
//! actif, ce sont 10 filtres biquad (bien moins de 0,1 % d'un cœur), réglés à
//! chaud sans reconstruire la chaîne audio.

/// Fréquences centrales (Hz) : une bande par octave, comme les égaliseurs classiques.
pub const BANDS: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const LABELS: [&str; 10] = ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
/// Plage de réglage, en dB.
pub const MAX_DB: f64 = 12.0;

pub const PRESETS: &[(&str, [f64; 10])] = &[
    ("Plat", [0.0; 10]),
    ("Graves", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Aigus", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0]),
    ("Voix", [-2.0, -2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0]),
    ("Rock", [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0]),
    ("Électro", [5.0, 4.0, 1.0, 0.0, -2.0, 0.0, 1.0, 2.0, 4.0, 5.0]),
    ("Rap", [5.0, 4.0, 2.0, 1.0, -1.0, -1.0, 1.0, 0.0, 1.0, 2.0]),
    ("Classique", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -2.0, -3.0]),
];
pub const CUSTOM: &str = "Personnalisé";

pub fn clamp(g: f64) -> f64 {
    g.clamp(-MAX_DB, MAX_DB)
}

pub fn is_flat(gains: &[f64]) -> bool {
    gains.iter().all(|g| g.abs() < 0.05)
}

/// Préampli : on baisse d'autant que la bande la plus poussée, pour ne jamais saturer.
pub fn preamp(gains: &[f64]) -> f64 {
    -gains.iter().copied().fold(0.0, f64::max)
}

/// Chaîne de filtres mpv, ou None s'il n'y a rien à appliquer (coût nul).
pub fn filter(gains: &[f64]) -> Option<String> {
    if is_flat(gains) {
        return None;
    }
    let bands: Vec<String> = BANDS
        .iter()
        .zip(gains)
        .enumerate()
        .map(|(i, (f, g))| format!("equalizer@b{i}=f={f}:t=o:w=1:g={g:.1}"))
        .collect();
    Some(format!("@eq:lavfi=[volume@pre=volume={:.1}dB,{}]", preamp(gains), bands.join(",")))
}

/// Préréglage suivant (ou précédent) dans la liste ; depuis « Personnalisé », on repart du début.
pub fn cycle(current: &str, delta: isize) -> (&'static str, [f64; 10]) {
    let n = PRESETS.len() as isize;
    let i = PRESETS.iter().position(|(name, _)| *name == current).map(|i| i as isize).unwrap_or(-1);
    let j = if i < 0 && delta < 0 { n - 1 } else { (i + delta).rem_euclid(n) };
    PRESETS[j as usize]
}

/// Nom du préréglage qui correspond exactement à ces réglages, sinon « Personnalisé ».
pub fn preset_name(gains: &[f64]) -> &'static str {
    PRESETS
        .iter()
        .find(|(_, p)| p.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05))
        .map(|(name, _)| *name)
        .unwrap_or(CUSTOM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plat_aucun_filtre() {
        assert_eq!(filter(&[0.0; 10]), None);
        assert_eq!(filter(&[0.01; 10]), None);
    }

    #[test]
    fn chaine_de_filtres_et_preampli() {
        let mut g = [0.0; 10];
        g[0] = 6.0;
        g[9] = -3.0;
        let f = filter(&g).unwrap();
        assert!(f.starts_with("@eq:lavfi=[volume@pre=volume=-6.0dB,equalizer@b0=f=31:t=o:w=1:g=6.0,"));
        assert!(f.ends_with("equalizer@b9=f=16000:t=o:w=1:g=-3.0]"));
        assert_eq!(f.matches("equalizer@").count(), 10);
        assert_eq!(preamp(&[-4.0; 10]), 0.0, "on ne remonte jamais le volume");
    }

    #[test]
    fn prereglages_complets_et_dans_la_plage() {
        for (name, p) in PRESETS {
            assert!(p.iter().all(|g| g.abs() <= MAX_DB), "{name}");
            assert_eq!(preset_name(p), *name);
        }
        assert_eq!(preset_name(&[1.0; 10]), CUSTOM);
        assert_eq!(clamp(20.0), MAX_DB);
        assert_eq!(clamp(-20.0), -MAX_DB);
    }

    #[test]
    fn faire_defiler_les_prereglages() {
        assert_eq!(cycle("Plat", 1).0, "Graves");
        assert_eq!(cycle("Plat", -1).0, "Classique");
        assert_eq!(cycle("Classique", 1).0, "Plat");
        assert_eq!(cycle(CUSTOM, 1).0, "Plat");
        assert_eq!(cycle(CUSTOM, -1).0, "Classique");
    }
}
