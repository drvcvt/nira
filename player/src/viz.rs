//! Visualizer analysis — all DSP lives here in Rust; the UI only renders.
//!
//! A [`Tap`] wraps every rodio source and mirrors a mono downmix of the
//! samples into a small ring buffer. [`VizBus::frame`] windows the latest
//! samples, FFTs them, folds the magnitudes into log-spaced bands
//! (cava-style) and flags beats via bass-energy flux against a short
//! rolling average. librespot output is a separate engine with no tap —
//! the visualizer follows the rodio side only.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, Sample, SampleRate};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;

/// Mono samples kept for analysis ≈ 85 ms @ 48 kHz.
const RING_CAP: usize = 4096;
const FFT_SIZE: usize = 2048;
pub const BANDS: usize = 48;
const F_LO: f32 = 40.0;
const F_HI: f32 = 16_000.0;
/// Bass = the lowest bands; drives beats and particle energy.
const BASS_BANDS: usize = 6;
/// Rolling window the beat detector compares against (~1.3 s at 30 fps).
const BASS_HISTORY: usize = 40;
/// Refractory gap between beats — caps at ~270 BPM.
const BEAT_GAP: Duration = Duration::from_millis(220);

/// One analysis frame for the renderer. Values are 0..1.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VizFrame {
    pub bands: Vec<f32>,
    pub bass: f32,
    pub beat: bool,
}

struct BeatState {
    bass_history: VecDeque<f32>,
    last_beat: Option<Instant>,
}

/// Shared analysis state, one per [`crate::Player`].
pub struct VizBus {
    ring: Mutex<VecDeque<f32>>,
    sample_rate: AtomicU32,
    beat: Mutex<BeatState>,
    fft: Mutex<FftPlanner<f32>>,
}

impl VizBus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ring: Mutex::new(VecDeque::with_capacity(RING_CAP)),
            sample_rate: AtomicU32::new(0),
            beat: Mutex::new(BeatState {
                bass_history: VecDeque::with_capacity(BASS_HISTORY),
                last_beat: None,
            }),
            fft: Mutex::new(FftPlanner::new()),
        })
    }

    fn push(&self, chunk: &[f32]) {
        let Ok(mut ring) = self.ring.lock() else {
            return;
        };
        ring.extend(chunk.iter().copied());
        while ring.len() > RING_CAP {
            ring.pop_front();
        }
    }

    /// Analyse the newest window. `None` until enough audio flowed through.
    pub fn frame(&self) -> Option<VizFrame> {
        let rate = self.sample_rate.load(Ordering::Relaxed);
        if rate == 0 {
            return None;
        }
        let window: Vec<f32> = {
            let ring = self.ring.lock().ok()?;
            if ring.len() < FFT_SIZE {
                return None;
            }
            ring.iter().skip(ring.len() - FFT_SIZE).copied().collect()
        };

        // Hann window → FFT → normalised magnitudes.
        let n = FFT_SIZE as f32;
        let mut buf: Vec<Complex32> = window
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1.0)).cos();
                Complex32::new(s * w, 0.0)
            })
            .collect();
        self.fft
            .lock()
            .ok()?
            .plan_fft_forward(FFT_SIZE)
            .process(&mut buf);
        let mags: Vec<f32> = buf[..FFT_SIZE / 2]
            .iter()
            .map(|c| c.norm() / (n / 4.0))
            .collect();

        // Log-spaced bands, peak per band, mapped -60..0 dB → 0..1.
        let mut bands = vec![0f32; BANDS];
        for (b, out) in bands.iter_mut().enumerate() {
            let f0 = F_LO * (F_HI / F_LO).powf(b as f32 / BANDS as f32);
            let f1 = F_LO * (F_HI / F_LO).powf((b + 1) as f32 / BANDS as f32);
            let i0 = (f0 * n / rate as f32) as usize;
            let i1 = ((f1 * n / rate as f32) as usize).max(i0 + 1).min(mags.len());
            if i0 >= mags.len() {
                break;
            }
            let peak = mags[i0..i1.max(i0 + 1)].iter().copied().fold(0f32, f32::max);
            let db = 20.0 * (peak + 1e-7).log10();
            *out = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        }

        let bass = bands[..BASS_BANDS].iter().sum::<f32>() / BASS_BANDS as f32;

        // Beat: bass energy clearly above its own recent average, with a
        // refractory gap (Parallelcube-style energy-ratio detection).
        let mut beat = false;
        if let Ok(mut st) = self.beat.lock() {
            let avg = if st.bass_history.is_empty() {
                bass
            } else {
                st.bass_history.iter().sum::<f32>() / st.bass_history.len() as f32
            };
            st.bass_history.push_back(bass);
            while st.bass_history.len() > BASS_HISTORY {
                st.bass_history.pop_front();
            }
            let gap_ok = st.last_beat.is_none_or(|t| t.elapsed() > BEAT_GAP);
            if bass > 0.25 && bass > avg * 1.35 && gap_ok {
                st.last_beat = Some(Instant::now());
                beat = true;
            }
        }

        Some(VizFrame { bands, bass, beat })
    }
}

/// Wraps a rodio source and mirrors a mono downmix of everything passing
/// through into the [`VizBus`]. Forwarding is sample-exact; pushes are
/// batched so the ring Mutex is touched every ~256 frames, not per sample.
pub struct Tap<S> {
    inner: S,
    bus: Arc<VizBus>,
    frame_sum: f32,
    frame_left: u16,
    batch: Vec<f32>,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, bus: Arc<VizBus>) -> Self {
        Self {
            inner,
            bus,
            frame_sum: 0.0,
            frame_left: 0,
            batch: Vec::with_capacity(256),
        }
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let Some(s) = self.inner.next() else {
            if !self.batch.is_empty() {
                self.bus.push(&self.batch);
                self.batch.clear();
            }
            return None;
        };
        if self.frame_left == 0 {
            self.frame_left = self.inner.channels().get();
            self.frame_sum = 0.0;
        }
        self.frame_sum += s;
        self.frame_left -= 1;
        if self.frame_left == 0 {
            let ch = self.inner.channels().get() as f32;
            self.batch.push(self.frame_sum / ch);
            if self.batch.len() >= 256 {
                self.bus
                    .sample_rate
                    .store(self.inner.sample_rate().get(), Ordering::Relaxed);
                self.bus.push(&self.batch);
                self.batch.clear();
            }
        }
        Some(s)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 440 Hz sine through the bus must peak in the band containing
    /// 440 Hz — the end-to-end sanity check for window/FFT/binning.
    #[test]
    fn sine_lands_in_the_right_band() {
        let bus = VizBus::new();
        bus.sample_rate.store(48_000, Ordering::Relaxed);
        let samples: Vec<f32> = (0..RING_CAP)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.8)
            .collect();
        bus.push(&samples);
        let frame = bus.frame().expect("enough samples");
        let loudest = frame
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        // 440 Hz → band index: log2(440/40) / log2(16000/40) * 48 ≈ 19.2
        assert!(
            (17..=21).contains(&loudest),
            "440 Hz peaked in band {loudest} ({:?})",
            &frame.bands[15..24]
        );
    }
}
