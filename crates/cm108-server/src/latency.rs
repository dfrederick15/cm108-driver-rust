use cm108_types::LatencyStats;

/// Power-of-two bucket histogram for dispatch latency in microseconds.
///
/// Bucket k holds samples where `ilog2(us) == k` (i.e. `2^k ≤ us < 2^(k+1)`).
/// Bucket 0 collects values 0 and 1 µs. The upper bound reported for bucket k
/// is `2^(k+1)`, giving log2-scale resolution (acceptable for p99 reporting).
pub struct LatencyHistogram {
    buckets: [u64; 64],
    total: u64,
    min_us: u32,
    max_us: u32,
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self { buckets: [0; 64], total: 0, min_us: u32::MAX, max_us: 0 }
    }

    pub fn record(&mut self, us: u32) {
        let k = us.checked_ilog2().unwrap_or(0) as usize;
        self.buckets[k.min(63)] += 1;
        self.total += 1;
        if us < self.min_us { self.min_us = us; }
        if us > self.max_us { self.max_us = us; }
    }

    /// Snapshot the current histogram into a [`LatencyStats`] value.
    pub fn to_stats(&self) -> LatencyStats {
        LatencyStats {
            min_us: if self.total == 0 { 0 } else { self.min_us },
            max_us: if self.total == 0 { 0 } else { self.max_us },
            p99_us: self.p99_us(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn p99_us(&self) -> u32 {
        if self.total == 0 { return 0; }
        // Ceiling 99th percentile: smallest value k such that ≥ 99% of samples fall in [0,k].
        let threshold = (self.total * 99 + 99) / 100;
        let mut cumulative = 0u64;
        for (i, &count) in self.buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= threshold {
                return 1u32.checked_shl((i + 1) as u32).unwrap_or(u32::MAX); // upper bound of bucket i
            }
        }
        self.max_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_zeros() {
        let s = LatencyHistogram::new().to_stats();
        assert_eq!(s, LatencyStats::default());
    }

    #[test]
    fn single_sample_min_max() {
        let mut h = LatencyHistogram::new();
        h.record(17);
        let s = h.to_stats();
        assert_eq!(s.min_us, 17);
        assert_eq!(s.max_us, 17);
        // ilog2(17) = 4, upper bound = 2^5 = 32
        assert_eq!(s.p99_us, 32);
    }

    #[test]
    fn p99_separates_tail() {
        let mut h = LatencyHistogram::new();
        // 99 samples at 1 µs (ilog2=0, bucket 0), 1 sample at 1000 µs (ilog2=9, bucket 9)
        for _ in 0..99 { h.record(1); }
        h.record(1000);
        let s = h.to_stats();
        // threshold = ceil(100*99/100) = 99; cumulative reaches 99 at bucket 0 → upper bound 2
        assert_eq!(s.p99_us, 2, "p99 should be upper bound of bucket 0 (≤ 2 µs)");
        assert_eq!(s.max_us, 1000);
    }

    #[test]
    fn p99_uniform_distribution() {
        let mut h = LatencyHistogram::new();
        // All 100 samples at 7 µs (ilog2=2, bucket 2, upper bound 8)
        for _ in 0..100 { h.record(7); }
        let s = h.to_stats();
        assert_eq!(s.p99_us, 8);
        assert_eq!(s.min_us, 7);
        assert_eq!(s.max_us, 7);
    }

    #[test]
    fn reset_clears_all() {
        let mut h = LatencyHistogram::new();
        h.record(100);
        h.record(200);
        h.reset();
        assert_eq!(h.to_stats(), LatencyStats::default());
    }

    #[test]
    fn zero_latency_does_not_panic() {
        let mut h = LatencyHistogram::new();
        h.record(0);
        let s = h.to_stats();
        assert_eq!(s.min_us, 0);
    }
}
