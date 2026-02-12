//! Deterministic random number generation for simulation.
//!
//! Uses ChaCha8Rng for reproducible randomness across platforms.
//! See spec §17.2 - RNG Specification.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::clock::SimDuration;

/// Deterministic RNG wrapper for simulation.
///
/// Wraps `ChaCha8Rng` with convenient methods for simulation use cases.
#[derive(Debug, Clone)]
pub struct SimRng {
    inner: ChaCha8Rng,
}

impl SimRng {
    /// Create a new RNG from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            inner: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Fork a new independent RNG.
    ///
    /// Useful for giving each component its own RNG stream while maintaining
    /// reproducibility (the order of forks must be deterministic).
    #[must_use]
    pub fn fork(&mut self) -> Self {
        Self {
            inner: ChaCha8Rng::seed_from_u64(self.inner.gen()),
        }
    }

    /// Generate a random boolean with given probability of being true.
    pub fn bool_with_probability(&mut self, probability: f64) -> bool {
        debug_assert!(
            (0.0..=1.0).contains(&probability),
            "Probability must be in [0.0, 1.0]"
        );
        self.inner.gen::<f64>() < probability
    }

    /// Generate a random duration in range [min, max].
    pub fn duration_range(&mut self, min: SimDuration, max: SimDuration) -> SimDuration {
        let min_micros = min.as_micros();
        let max_micros = max.as_micros();
        if min_micros >= max_micros {
            return min;
        }
        SimDuration::from_micros(self.inner.gen_range(min_micros..=max_micros))
    }

    /// Generate a random duration with base ± jitter.
    ///
    /// Returns a duration in range [base - jitter, base + jitter], clamped to >= 0.
    /// Uses saturating arithmetic to avoid overflow with large duration values.
    pub fn duration_with_jitter(&mut self, base: SimDuration, jitter: SimDuration) -> SimDuration {
        let base_micros = base.as_micros();
        let jitter_micros = jitter.as_micros();

        // Use saturating operations to prevent underflow/overflow
        let min = base_micros.saturating_sub(jitter_micros);
        let max = base_micros.saturating_add(jitter_micros);

        SimDuration::from_micros(self.inner.gen_range(min..=max))
    }

    /// Generate a random u64 in range [0, max).
    pub fn gen_range_u64(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        self.inner.gen_range(0..max)
    }

    /// Generate a random usize in range [0, max).
    pub fn gen_range_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        self.inner.gen_range(0..max)
    }

    /// Shuffle a slice in place.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        use rand::seq::SliceRandom;
        slice.shuffle(&mut self.inner);
    }

    /// Pick a random element from a slice.
    pub fn choose<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        use rand::seq::SliceRandom;
        slice.choose(&mut self.inner)
    }

    /// Generate random bytes.
    pub fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.inner.fill(dest);
    }

    /// Generate a random u64.
    pub fn gen_u64(&mut self) -> u64 {
        self.inner.gen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rng_deterministic() {
        let mut rng1 = SimRng::new(42);
        let mut rng2 = SimRng::new(42);

        for _ in 0..100 {
            assert_eq!(rng1.gen_u64(), rng2.gen_u64());
        }
    }

    #[test]
    fn test_rng_different_seeds() {
        let mut rng1 = SimRng::new(42);
        let mut rng2 = SimRng::new(43);

        // Very unlikely to match with different seeds
        let vals1: Vec<u64> = (0..10).map(|_| rng1.gen_u64()).collect();
        let vals2: Vec<u64> = (0..10).map(|_| rng2.gen_u64()).collect();

        assert_ne!(vals1, vals2);
    }

    #[test]
    fn test_rng_fork() {
        let mut rng1 = SimRng::new(42);
        let mut rng2 = SimRng::new(42);

        let fork1 = rng1.fork();
        let fork2 = rng2.fork();

        // Forked RNGs should be identical if forked in same order
        let mut f1 = fork1;
        let mut f2 = fork2;
        for _ in 0..10 {
            assert_eq!(f1.gen_u64(), f2.gen_u64());
        }
    }

    #[test]
    fn test_bool_with_probability() {
        let mut rng = SimRng::new(42);

        // 0% should always be false
        for _ in 0..100 {
            assert!(!rng.bool_with_probability(0.0));
        }

        // 100% should always be true
        for _ in 0..100 {
            assert!(rng.bool_with_probability(1.0));
        }

        // 50% should be roughly half (statistically)
        let mut trues = 0;
        for _ in 0..1000 {
            if rng.bool_with_probability(0.5) {
                trues += 1;
            }
        }
        assert!(
            trues > 400 && trues < 600,
            "Expected ~500 trues, got {trues}"
        );
    }

    #[test]
    fn test_duration_range() {
        let mut rng = SimRng::new(42);

        let min = SimDuration::from_millis(10);
        let max = SimDuration::from_millis(100);

        for _ in 0..100 {
            let d = rng.duration_range(min, max);
            assert!(d >= min && d <= max);
        }
    }

    #[test]
    fn test_duration_with_jitter() {
        let mut rng = SimRng::new(42);

        let base = SimDuration::from_millis(50);
        let jitter = SimDuration::from_millis(10);

        for _ in 0..100 {
            let d = rng.duration_with_jitter(base, jitter);
            assert!(d.as_millis() >= 40 && d.as_millis() <= 60);
        }
    }

    #[test]
    fn test_shuffle() {
        let mut rng = SimRng::new(42);

        let original = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut shuffled = original.clone();
        rng.shuffle(&mut shuffled);

        // Should be different order (very unlikely to be same with 10 elements)
        assert_ne!(shuffled, original);

        // Should contain same elements
        let mut sorted = shuffled.clone();
        sorted.sort();
        assert_eq!(sorted, original);
    }
}
