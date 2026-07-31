use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use librespot::playback::audio_backend::{Sink, SinkResult};
use librespot::playback::convert::Converter;
use librespot::playback::decoder::AudioPacket;
use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, Sample, SampleRate};

const FREQUENCIES: [f32; 3] = [100.0, 1_000.0, 10_000.0];
const Q: f32 = 0.8;

struct Shared {
    enabled: AtomicBool,
    bands: [AtomicU32; 3],
    revision: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct EqualizerControl(Arc<Shared>);

impl EqualizerControl {
    pub(crate) fn new(enabled: bool, bands: [f32; 3]) -> Self {
        Self(Arc::new(Shared {
            enabled: AtomicBool::new(enabled),
            bands: bands.map(|gain| AtomicU32::new(gain.clamp(-6.0, 6.0).to_bits())),
            revision: AtomicU64::new(0),
        }))
    }

    pub(crate) fn set(&self, enabled: bool, bands: [f32; 3]) {
        for (slot, gain) in self.0.bands.iter().zip(bands) {
            slot.store(gain.clamp(-6.0, 6.0).to_bits(), Ordering::Relaxed);
        }
        self.0.enabled.store(enabled, Ordering::Relaxed);
        self.0.revision.fetch_add(1, Ordering::Release);
    }

    fn revision(&self) -> u64 {
        self.0.revision.load(Ordering::Acquire)
    }

    fn snapshot(&self) -> (u64, bool, [f32; 3]) {
        loop {
            let before = self.revision();
            let enabled = self.0.enabled.load(Ordering::Relaxed);
            let bands = self
                .0
                .bands
                .each_ref()
                .map(|gain| f32::from_bits(gain.load(Ordering::Relaxed)));
            if before == self.revision() {
                return (before, enabled, bands);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(sample_rate: u32, frequency: f32, gain_db: f32) -> Self {
        if gain_db.abs() < f32::EPSILON {
            return Self::identity();
        }
        let frequency = frequency.min(sample_rate as f32 * 0.45);
        let w0 = std::f32::consts::TAU * frequency / sample_rate as f32;
        let alpha = w0.sin() / (2.0 * Q);
        let a = 10f32.powf(gain_db / 40.0);
        let a0 = 1.0 + alpha / a;

        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * w0.cos()) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * w0.cos()) / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        let output = self.b0 * sample + self.z1;
        self.z1 = self.b1 * sample - self.a1 * output + self.z2;
        self.z2 = self.b2 * sample - self.a2 * output;
        output
    }
}

pub(crate) struct EqualizerProcessor {
    control: EqualizerControl,
    revision: u64,
    enabled: bool,
    sample_rate: u32,
    channels: usize,
    channel: usize,
    headroom: f32,
    filters: Vec<[Biquad; 3]>,
}

impl EqualizerProcessor {
    pub(crate) fn new(control: EqualizerControl) -> Self {
        Self {
            control,
            revision: u64::MAX,
            enabled: false,
            sample_rate: 0,
            channels: 0,
            channel: 0,
            headroom: 1.0,
            filters: Vec::new(),
        }
    }

    pub(crate) fn process(&mut self, sample: f32, sample_rate: u32, channels: usize) -> f32 {
        self.refresh(sample_rate, channels.max(1));
        let channel = self.channel;
        self.channel = (self.channel + 1) % self.channels;
        if !self.enabled {
            return sample;
        }
        self.filters[channel]
            .iter_mut()
            .fold(sample * self.headroom, |sample, filter| {
                filter.process(sample)
            })
    }

    fn refresh(&mut self, sample_rate: u32, channels: usize) {
        let revision = self.control.revision();
        if revision == self.revision && sample_rate == self.sample_rate && channels == self.channels
        {
            return;
        }

        let channel_layout_changed = sample_rate != self.sample_rate || channels != self.channels;
        let (revision, enabled, bands) = self.control.snapshot();
        self.filters = vec![[Biquad::identity(); 3]; channels];
        for filters in &mut self.filters {
            for (filter, (frequency, gain)) in
                filters.iter_mut().zip(FREQUENCIES.into_iter().zip(bands))
            {
                *filter = Biquad::peaking(sample_rate, frequency, gain);
            }
        }
        self.revision = revision;
        self.enabled = enabled;
        self.sample_rate = sample_rate;
        self.channels = channels;
        if channel_layout_changed {
            self.channel = 0;
        }
        let max_boost = bands.into_iter().fold(0.0_f32, f32::max);
        self.headroom = 10f32.powf(-max_boost / 20.0);
    }

    fn reset(&mut self) {
        self.revision = u64::MAX;
        self.sample_rate = 0;
        self.channels = 0;
        self.channel = 0;
        self.filters.clear();
    }
}

pub(crate) struct EqualizedSource<S> {
    inner: S,
    processor: EqualizerProcessor,
}

impl<S> EqualizedSource<S> {
    pub(crate) fn new(inner: S, control: EqualizerControl) -> Self {
        Self {
            inner,
            processor: EqualizerProcessor::new(control),
        }
    }
}

impl<S: Source> Iterator for EqualizedSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let sample_rate = self.inner.sample_rate().get();
        let channels = self.inner.channels().get() as usize;
        self.inner
            .next()
            .map(|sample| self.processor.process(sample, sample_rate, channels))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for EqualizedSource<S> {
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
        self.inner.try_seek(pos)?;
        self.processor.reset();
        Ok(())
    }
}

pub(crate) struct EqualizedSink {
    inner: Box<dyn Sink>,
    processor: EqualizerProcessor,
}

impl EqualizedSink {
    pub(crate) fn new(inner: Box<dyn Sink>, control: EqualizerControl) -> Self {
        Self {
            inner,
            processor: EqualizerProcessor::new(control),
        }
    }
}

impl Sink for EqualizedSink {
    fn start(&mut self) -> SinkResult<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.processor.reset();
        self.inner.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        let packet = match packet {
            AudioPacket::Samples(mut samples) => {
                for sample in &mut samples {
                    *sample = self.processor.process(
                        *sample as f32,
                        librespot::playback::SAMPLE_RATE,
                        librespot::playback::NUM_CHANNELS as usize,
                    ) as f64;
                }
                AudioPacket::Samples(samples)
            }
            raw => raw,
        };
        self.inner.write(packet, converter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_rms(processor: &mut EqualizerProcessor, frequency: f32) -> f32 {
        let rate = 44_100;
        let samples = (0..rate / 4)
            .map(|i| (std::f32::consts::TAU * frequency * i as f32 / rate as f32).sin() * 0.25);
        let output: Vec<_> = samples
            .map(|sample| processor.process(sample, rate, 1))
            .skip(2_000)
            .collect();
        (output.iter().map(|sample| sample * sample).sum::<f32>() / output.len() as f32).sqrt()
    }

    #[test]
    fn disabled_equalizer_is_sample_exact() {
        let control = EqualizerControl::new(false, [6.0, -6.0, 3.0]);
        let mut processor = EqualizerProcessor::new(control);
        let input = [0.25, -0.5, 0.75, -1.0];
        let output = input.map(|sample| processor.process(sample, 44_100, 2));
        assert_eq!(output, input);
    }

    #[test]
    fn low_band_boost_favors_bass() {
        let control = EqualizerControl::new(true, [6.0, 0.0, 0.0]);
        let low = sine_rms(&mut EqualizerProcessor::new(control.clone()), 100.0);
        let high = sine_rms(&mut EqualizerProcessor::new(control), 5_000.0);
        assert!(low > high * 1.5, "low={low}, high={high}");
    }
}
