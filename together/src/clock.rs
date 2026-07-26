//! Clock offset estimation between two peers.
//!
//! Both sides run their own monotonic clock with no shared epoch, so we never
//! exchange absolute times — only the offset between them, measured NTP-style:
//!
//! ```text
//! me:   t1 ──────────────▶            ◀────────────── t3
//! peer:            t2 (receive + reply)
//!
//! offset = t2 - (t1 + t3) / 2      // add to my clock to get peer's
//! rtt    = t3 - t1
//! ```
//!
//! The estimate is only as good as the path is symmetric, and a queued packet
//! skews it by half the delay. So we keep a window of samples and use the one
//! with the *lowest* RTT rather than averaging: the fastest round trip is the
//! one that spent the least time being delayed asymmetrically. Averaging mixes
//! the good sample back into the bad ones.

use std::collections::VecDeque;

/// How many probes to keep. At one probe every 2 s this is ~30 s of history —
/// long enough to catch a quiet moment on the link, short enough that a
/// route change ages out.
const WINDOW: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Nanoseconds to add to our clock to land on the peer's.
    offset_ns: i64,
    rtt_ns: u64,
}

#[derive(Debug, Default)]
pub struct ClockSync {
    samples: VecDeque<Sample>,
}

impl ClockSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one completed probe. `t1`/`t3` are our clock at send/receive,
    /// `t2` is the peer's clock when it replied.
    ///
    /// A reply that arrives before it was sent is a bug or a rolled clock, not
    /// a measurement — dropped rather than poisoning the window.
    pub fn record(&mut self, t1: u64, t2: u64, t3: u64) {
        if t3 < t1 {
            return;
        }
        let rtt_ns = t3 - t1;
        let midpoint = t1 + rtt_ns / 2;
        let offset_ns = t2 as i64 - midpoint as i64;
        if self.samples.len() == WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample { offset_ns, rtt_ns });
    }

    /// Best offset estimate, or `None` until the first probe lands.
    pub fn offset_ns(&self) -> Option<i64> {
        self.best().map(|s| s.offset_ns)
    }

    /// RTT of the sample the current offset came from. Surfaced so the UI can
    /// show link quality and the sync loop can widen its tolerance on a slow
    /// link instead of correcting against noise.
    pub fn rtt_ns(&self) -> Option<u64> {
        self.best().map(|s| s.rtt_ns)
    }

    fn best(&self) -> Option<Sample> {
        self.samples.iter().copied().min_by_key(|s| s.rtt_ns)
    }

    /// Translate a timestamp on the peer's clock into ours.
    pub fn peer_to_local(&self, peer_ns: u64) -> Option<u64> {
        let off = self.offset_ns()?;
        Some((peer_ns as i64 - off).max(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peer clock runs 5 s ahead; a symmetric 40 ms link should recover
    /// exactly that, because the midpoint lands on the peer's reply instant.
    #[test]
    fn recovers_offset_on_a_symmetric_link() {
        let mut c = ClockSync::new();
        let (offset, one_way) = (5_000_000_000i64, 20_000_000u64);
        let t1 = 1_000_000_000u64;
        let t2 = (t1 + one_way) as i64 + offset;
        let t3 = t1 + 2 * one_way;
        c.record(t1, t2 as u64, t3);
        assert_eq!(c.offset_ns(), Some(offset));
    }

    /// The whole reason for the min-RTT filter: one clean probe among noisy
    /// ones must win. A mean would sit somewhere in the middle of the skew.
    #[test]
    fn picks_the_lowest_rtt_sample_not_the_average() {
        let mut c = ClockSync::new();
        let offset = 3_000_000_000i64;
        // Three delayed probes, asymmetric so each reports a wrong offset...
        for extra in [400_000_000u64, 250_000_000, 900_000_000] {
            let t1 = 0;
            let t2 = (10_000_000i64) + offset; // peer replied early in the trip
            let t3 = t1 + 20_000_000 + extra; // return path dragged
            c.record(t1, t2 as u64, t3);
        }
        // ...then one clean symmetric probe.
        let t1 = 0;
        let one_way = 10_000_000u64;
        let t3 = t1 + 2 * one_way;
        c.record(t1, (one_way as i64 + offset) as u64, t3);

        assert_eq!(c.offset_ns(), Some(offset));
        assert_eq!(c.rtt_ns(), Some(2 * one_way));
    }

    #[test]
    fn window_is_bounded_and_drops_oldest() {
        let mut c = ClockSync::new();
        // Oldest sample is the fastest; once it ages out the estimate must
        // move, otherwise the window isn't really sliding.
        c.record(0, 5_000_000, 10_000_000);
        for i in 1..=WINDOW as u64 {
            let t1 = i * 1_000_000_000;
            c.record(t1, t1 + 50_000_000, t1 + 100_000_000);
        }
        assert_eq!(c.samples.len(), WINDOW);
        assert_eq!(c.rtt_ns(), Some(100_000_000));
    }

    #[test]
    fn ignores_a_reply_that_predates_its_request() {
        let mut c = ClockSync::new();
        c.record(10_000_000, 1, 9_000_000);
        assert_eq!(c.offset_ns(), None);
    }
}
