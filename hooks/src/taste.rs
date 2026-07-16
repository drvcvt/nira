//! Shared taste-profile primitives for recommendations, radio and shuffle.
//!
//! Two building blocks: an exponential time-decay weight for plays, and
//! weighted random sampling without replacement (Efraimidis–Spirakis).
//! Everything takes `&mut impl Rng` so tests run seeded and deterministic.

use chrono::{DateTime, Utc};
use rand::Rng;

/// Half-life of a play's influence on the taste profile. Two weeks means a
/// binge from this morning nudges the profile instead of owning it, while a
/// month-old favourite still carries real weight.
const HALF_LIFE_DAYS: f64 = 14.0;

/// Weight of one play, decayed by age. 1.0 for "just now", 0.5 after two
/// weeks, and so on. Future timestamps clamp to 1.0.
pub(crate) fn play_weight(now: DateTime<Utc>, played_at: DateTime<Utc>) -> f64 {
    let days = (now - played_at).num_seconds().max(0) as f64 / 86_400.0;
    0.5f64.powf(days / HALF_LIFE_DAYS)
}

/// Weighted sampling without replacement (Efraimidis–Spirakis): each item
/// gets key `u^(1/w)` with `u ~ U(0,1)`; the `n` largest keys win. Heavier
/// items are more likely but the long tail always has a chance — exactly the
/// "random stuff in the general direction of my taste" behaviour we want.
pub(crate) fn weighted_sample<T>(items: Vec<(f64, T)>, n: usize, rng: &mut impl Rng) -> Vec<T> {
    let mut keyed: Vec<(f64, T)> = items
        .into_iter()
        .map(|(w, item)| {
            let u: f64 = rng.random::<f64>().max(f64::MIN_POSITIVE);
            (u.powf(1.0 / w.max(1e-9)), item)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    keyed.into_iter().take(n).map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn play_weight_decays_with_age() {
        let now = Utc::now();
        let fresh = play_weight(now, now);
        let two_weeks = play_weight(now, now - chrono::Duration::days(14));
        let old = play_weight(now, now - chrono::Duration::days(60));
        assert!((fresh - 1.0).abs() < 1e-6);
        assert!((two_weeks - 0.5).abs() < 0.01);
        assert!(old < 0.1);
        // Future timestamps clamp instead of exploding.
        assert!((play_weight(now, now + chrono::Duration::days(3)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn weighted_sample_prefers_heavy_items() {
        let mut rng = StdRng::seed_from_u64(7);
        let mut heavy_first = 0;
        for _ in 0..200 {
            let picked = weighted_sample(vec![(100.0, "heavy"), (1.0, "light")], 1, &mut rng);
            if picked == vec!["heavy"] {
                heavy_first += 1;
            }
        }
        assert!(heavy_first > 180, "heavy won only {heavy_first}/200 draws");
    }

    #[test]
    fn weighted_sample_never_repeats_and_caps_at_len() {
        let mut rng = StdRng::seed_from_u64(1);
        let items: Vec<(f64, usize)> = (0..5).map(|i| (1.0, i)).collect();
        let mut picked = weighted_sample(items, 10, &mut rng);
        picked.sort();
        assert_eq!(picked, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn weighted_sample_long_tail_gets_drawn_sometimes() {
        let mut rng = StdRng::seed_from_u64(42);
        let mut tail_hits = 0;
        for _ in 0..300 {
            let picked = weighted_sample(vec![(5.0, "head"), (1.0, "tail")], 1, &mut rng);
            if picked == vec!["tail"] {
                tail_hits += 1;
            }
        }
        assert!(tail_hits > 15, "tail never surfaced ({tail_hits}/300)");
    }
}
