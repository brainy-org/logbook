//! What the final pass's diarization and voice prints hear of a channel: its
//! audio band-passed (100 Hz–7.5 kHz) and brought to -23 LUFS, one gain per
//! range (docs/pipeline.md, "Final pass";
//! docs/decisions/2026-11-06-a-meeting-through-the-satellite.md, "One
//! microphone on the table"). A table microphone is 15–30 dB quieter than
//! a headset: raised, the diarization model misses less speech, but only
//! with the top band cut, whose hiss raised with it makes the model worse.
//! The words are transcribed from the audio as it came.
//!
//! A channel's ranges are conditioned one after another, each starting
//! from the state the previous one left (`Conditioner` travels with the
//! range and comes back with its result): the filters have no click at a
//! range's edge and the gain moves from one range's to the next's over
//! RAMP. Cheap: seven biquads, a gain and a limiter per sample, about
//! 20 ms for a minute of audio.

use std::collections::VecDeque;

const RATE: f64 = 16_000.0;
/// Loudness is measured every 100 ms (BS.1770's step).
const HOP: usize = 1_600;
/// BS.1770's gating block, in hops (400 ms).
const BLOCK: usize = 4;
/// At a range's start the gain moves from where the last one ended to its
/// own in this long.
const RAMP: usize = 8_000;
/// The limiter's look-ahead: 5 ms.
const LOOKAHEAD: usize = 80;
/// The limiter's release time constant: 50 ms.
const RELEASE: f64 = 0.05;

/// How a channel is conditioned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The high-pass's corner, in Hz (2nd-order Butterworth).
    pub highpass: f64,
    /// The low-pass's corner, in Hz (4th-order Butterworth); 0 is none.
    pub lowpass: f64,
    /// The loudness speech is brought to, in LUFS.
    pub target: f64,
    /// The most gain, either way, in dB.
    pub max_gain: f64,
    /// The share of the change in a range's gain taken at each range
    /// (1: none kept from the last one).
    pub follow: f64,
    /// The highest sample after the gain, in dBFS.
    pub ceiling: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            highpass: 100.0,
            lowpass: 7_500.0,
            target: -23.0,
            max_gain: 30.0,
            follow: 0.5,
            ceiling: -1.0,
        }
    }
}

/// A biquad, transposed direct form II, its state kept between calls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a0: f64, a: [f64; 2]) -> Self {
        Self {
            b: b.map(|x| x / a0),
            a: a.map(|x| x / a0),
            z: [0.0; 2],
        }
    }

    /// RBJ's high-pass; Q 1/√2 is a 2nd-order Butterworth.
    pub fn highpass(fs: f64, f0: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f0 / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        Self::new(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            1.0 + alpha,
            [-2.0 * cos, 1.0 - alpha],
        )
    }

    /// RBJ's low-pass.
    pub fn lowpass(fs: f64, f0: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * f0 / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        Self::new(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            1.0 + alpha,
            [-2.0 * cos, 1.0 - alpha],
        )
    }

    /// RBJ's high shelf, `gain` in dB.
    pub fn high_shelf(fs: f64, f0: f64, q: f64, gain: f64) -> Self {
        let a = 10f64.powf(gain / 40.0);
        let w = 2.0 * std::f64::consts::PI * f0 / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let s = 2.0 * a.sqrt() * alpha;
        Self::new(
            [
                a * ((a + 1.0) + (a - 1.0) * cos + s),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - s),
            ],
            (a + 1.0) - (a - 1.0) * cos + s,
            [
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - s,
            ],
        )
    }

    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// A 4th-order Butterworth low-pass, two biquads; none for 0 Hz.
fn lowpass(f0: f64) -> Vec<Biquad> {
    if f0 <= 0.0 {
        return Vec::new();
    }
    [0.541_196_100_146_197, 1.306_562_964_876_376_5]
        .iter()
        .map(|&q| Biquad::lowpass(RATE, f0, q))
        .collect()
}

/// BS.1770's K-weighting at 16 kHz (pyloudnorm's parameters).
fn k_weighting() -> [Biquad; 2] {
    [
        Biquad::high_shelf(RATE, 1_500.0, std::f64::consts::FRAC_1_SQRT_2, 4.0),
        Biquad::highpass(RATE, 38.0, 0.5),
    ]
}

fn loudness(power: f64) -> f64 {
    -0.691 + 10.0 * power.max(1e-20).log10()
}

fn power(loudness: f64) -> f64 {
    10f64.powf((loudness + 0.691) / 10.0)
}

/// BS.1770's gated loudness of hops' mean squares: 400 ms blocks every
/// 100 ms, gated at -70 LUFS and 10 LU below their mean. None for silence.
fn gated(hops: &[f64]) -> Option<f64> {
    let blocks: Vec<f64> = if hops.len() < BLOCK {
        vec![hops.iter().sum::<f64>() / hops.len().max(1) as f64]
    } else {
        hops.windows(BLOCK)
            .map(|w| w.iter().sum::<f64>() / BLOCK as f64)
            .collect()
    };
    let loud: Vec<f64> = blocks.into_iter().filter(|&p| p > power(-70.0)).collect();
    if loud.is_empty() {
        return None;
    }
    let relative = loudness(loud.iter().sum::<f64>() / loud.len() as f64) - 10.0;
    let kept: Vec<f64> = loud.into_iter().filter(|&p| p > power(relative)).collect();
    Some(loudness(kept.iter().sum::<f64>() / kept.len() as f64))
}

/// One channel's conditioning and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Conditioner {
    settings: Settings,
    highpass: Biquad,
    lowpass: Vec<Biquad>,
    k: [Biquad; 2],
    /// The last range's gain, in dB.
    gain: Option<f64>,
    /// The limiter's gain on the last sample.
    held: f64,
}

impl Default for Conditioner {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

impl Conditioner {
    pub fn new(settings: Settings) -> Self {
        Self {
            highpass: Biquad::highpass(RATE, settings.highpass, std::f64::consts::FRAC_1_SQRT_2),
            lowpass: lowpass(settings.lowpass),
            settings,
            k: k_weighting(),
            gain: None,
            held: 1.0,
        }
    }

    /// The next range of the channel (16 kHz), conditioned.
    pub fn run(&mut self, pcm: &[f32]) -> Vec<f32> {
        let mut out: Vec<f64> = pcm
            .iter()
            .map(|&x| {
                let mut y = self.highpass.run(x as f64);
                for stage in &mut self.lowpass {
                    y = stage.run(y);
                }
                y
            })
            .collect();
        if out.is_empty() {
            return Vec::new();
        }
        let gain = self.gain(&out);
        // From the last range's gain to this one's over RAMP, then steady.
        let from = self.gain.unwrap_or(gain);
        let n = RAMP.min(out.len());
        let linear = |db: f64| 10f64.powf(db / 20.0);
        for (i, x) in out.iter_mut().enumerate() {
            *x *= if i < n {
                linear(from + (gain - from) * i as f64 / n as f64)
            } else {
                linear(gain)
            };
        }
        self.gain = Some(gain);
        self.limit(&mut out);
        out.into_iter().map(|x| x as f32).collect()
    }

    /// This range's gain in dB: its speech brought to the target, within
    /// `max_gain`, `follow` of the way from the last range's; the last
    /// one's kept through silence.
    fn gain(&mut self, x: &[f64]) -> f64 {
        let s = self.settings;
        // Mean square of the K-weighted signal, per hop.
        let mut hops = Vec::with_capacity(x.len() / HOP + 1);
        for chunk in x.chunks(HOP) {
            let mut sum = 0.0;
            for &v in chunk {
                let shelved = self.k[0].run(v);
                let y = self.k[1].run(shelved);
                sum += y * y;
            }
            hops.push(sum / chunk.len() as f64);
        }
        let wanted = gated(&hops).map(|l| (s.target - l).clamp(-s.max_gain, s.max_gain));
        match (wanted, self.gain) {
            (Some(g), Some(last)) => last + s.follow * (g - last),
            (Some(g), None) => g,
            (None, last) => last.unwrap_or(0.0),
        }
    }

    /// Peaks held under the ceiling: the gain each sample needs, its least
    /// over the next LOOKAHEAD samples, released over RELEASE and averaged
    /// over LOOKAHEAD so that it reaches each peak smoothly.
    fn limit(&mut self, x: &mut [f64]) {
        let ceiling = 10f64.powf(self.settings.ceiling / 20.0);
        let need: Vec<f64> = x
            .iter()
            .map(|v| (ceiling / v.abs().max(1e-12)).min(1.0))
            .collect();
        if need.iter().all(|&g| g >= 1.0) && self.held >= 1.0 {
            return;
        }
        // The least of need[i..i + LOOKAHEAD], a monotonic queue.
        let mut ahead = vec![1.0; x.len()];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for j in (0..x.len()).rev() {
            while queue.back().is_some_and(|&q| need[q] >= need[j]) {
                queue.pop_back();
            }
            queue.push_back(j);
            while queue.front().is_some_and(|&q| q >= j + LOOKAHEAD) {
                queue.pop_front();
            }
            ahead[j] = need[*queue.front().unwrap()];
        }
        let up = 1.0 - (-1.0 / (RELEASE * RATE)).exp();
        let mut held = self.held;
        let mut window: VecDeque<f64> = std::iter::repeat_n(held, LOOKAHEAD).collect();
        let mut sum = held * LOOKAHEAD as f64;
        for (v, &a) in x.iter_mut().zip(&ahead) {
            held = a.min(held + (1.0 - held) * up);
            sum += held - window.pop_front().unwrap_or(held);
            window.push_back(held);
            let g = (sum / LOOKAHEAD as f64).min(1.0);
            *v = (*v * g).clamp(-ceiling, ceiling);
        }
        self.held = held;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, amp: f64, seconds: f64) -> Vec<f32> {
        (0..(seconds * RATE) as usize)
            .map(|i| (amp * (2.0 * std::f64::consts::PI * f * i as f64 / RATE).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn db(a: f64, b: f64) -> f64 {
        20.0 * (a / b).log10()
    }

    /// The high-pass alone, through the steady part of a tone.
    fn through_highpass(f: f64) -> f64 {
        let mut hp = Biquad::highpass(RATE, 100.0, std::f64::consts::FRAC_1_SQRT_2);
        let x = sine(f, 0.5, 2.0);
        let y: Vec<f32> = x.iter().map(|&v| hp.run(v as f64) as f32).collect();
        db(rms(&y[16_000..]), rms(&x[16_000..]))
    }

    #[test]
    fn the_high_pass_cuts_mains_hum_and_leaves_speech() {
        // A 2nd-order Butterworth: -3 dB at 100 Hz, 12 dB an octave below.
        let at50 = through_highpass(50.0);
        assert!((-13.0..-11.0).contains(&at50), "50 Hz: {at50:.2} dB");
        assert!((through_highpass(100.0) + 3.01).abs() < 0.1);
        for f in [300.0, 1_000.0, 4_000.0] {
            let g = through_highpass(f);
            assert!(g.abs() < 0.5, "{f} Hz: {g:.2} dB");
        }
    }

    #[test]
    fn the_band_is_flat_through_speech_and_cut_at_the_top() {
        // The whole chain with no gain: tones 3 s long, the first second
        // left out (the filters settling).
        let through = |f: f64| {
            let mut c = Conditioner::new(Settings {
                max_gain: 0.0,
                ..Settings::default()
            });
            let x = sine(f, 0.1, 3.0);
            let y = c.run(&x);
            db(rms(&y[16_000..]), rms(&x[16_000..]))
        };
        assert!(through(50.0) < -11.0);
        for f in [300.0, 1_000.0, 3_000.0, 5_000.0] {
            let g = through(f);
            assert!(g.abs() < 0.5, "{f} Hz: {g:.2} dB");
        }
        assert!((through(7_500.0) + 3.0).abs() < 0.3);
        assert!(through(7_900.0) < -15.0);
    }

    #[test]
    fn ranges_join_without_a_click() {
        // A tone conditioned whole, and in three ranges: the same samples.
        let x = sine(220.0, 0.05, 6.0);
        let settings = Settings {
            follow: 1.0,
            ..Settings::default()
        };
        let mut whole = Conditioner::new(settings);
        let a = whole.run(&x);
        let mut parts = Conditioner::new(settings);
        let mut b = Vec::new();
        for part in x.chunks(32_000) {
            b.extend(parts.run(part));
        }
        let worst = a
            .iter()
            .zip(&b)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
        // And no jump at an edge larger than the tone's own steps.
        let step = |y: &[f32], i: usize| (y[i] - y[i - 1]).abs();
        let biggest = (1..a.len()).map(|i| step(&a, i)).fold(0.0f32, f32::max);
        for edge in [32_000, 64_000] {
            assert!(step(&b, edge) <= biggest * 1.01);
        }
    }

    #[test]
    fn speech_is_brought_to_the_target() {
        // A 1 kHz tone at -40 LUFS or so, alone in its range.
        let x = sine(1_000.0, 0.01, 10.0);
        let mut c = Conditioner::default();
        let y = c.run(&x);
        let mut k = k_weighting();
        let ms = y[16_000..]
            .iter()
            .map(|&v| {
                let shelved = k[0].run(v as f64);
                k[1].run(shelved).powi(2)
            })
            .sum::<f64>()
            / (y.len() - 16_000) as f64;
        assert!((loudness(ms) + 23.0).abs() < 0.5, "{:.2}", loudness(ms));
    }

    #[test]
    fn the_gain_is_limited_and_peaks_held_under_the_ceiling() {
        // Very quiet (about -60 LUFS, 37 dB short): at most max_gain up.
        let quiet = sine(1_000.0, 1e-3, 4.0);
        let mut c = Conditioner::default();
        let y = c.run(&quiet);
        let gain = db(rms(&y[16_000..]), rms(&quiet[16_000..]));
        assert!((gain - 30.0).abs() < 0.3, "{gain:.2}");
        // Under BS.1770's -70 LUFS gate it is silence: left as it is.
        let silence = sine(1_000.0, 1e-4, 4.0);
        let y = Conditioner::default().run(&silence);
        let gain = db(rms(&y[16_000..]), rms(&silence[16_000..]));
        assert!(gain.abs() < 0.1, "{gain:.2}");
        // Loud already, with a click: brought down, nothing over -1 dBFS.
        let mut loud = sine(1_000.0, 0.9, 4.0);
        loud[30_000] = 1.0;
        let mut c = Conditioner::new(Settings {
            max_gain: 20.0,
            target: 0.0,
            ..Settings::default()
        });
        let y = c.run(&loud);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        assert!(y.iter().all(|v| v.abs() <= ceiling + 1e-6));
        assert!(rms(&y) > 0.5, "not silenced: {}", rms(&y));
    }

    #[test]
    fn the_gain_follows_a_change_of_level_halfway_a_range() {
        let mut c = Conditioner::default();
        let at = |c: &mut Conditioner, amp: f64| {
            let x = sine(1_000.0, amp, 60.0);
            let y = c.run(&x);
            db(rms(&y[16_000..]), rms(&x[16_000..]))
        };
        let first = at(&mut c, 0.01);
        // 20 dB louder: the gain moves half the way, then the rest.
        let second = at(&mut c, 0.1);
        let third = at(&mut c, 0.1);
        assert!(
            (first - second - 10.0).abs() < 0.5,
            "{first:.1} {second:.1}"
        );
        assert!((second - third - 5.0).abs() < 0.5, "{second:.1} {third:.1}");
        // The first samples of a range start from the last one's gain.
        let x = sine(1_000.0, 0.1, 2.0);
        let last = c.gain.unwrap();
        let y = c.run(&x);
        let early = db(rms(&y[..800]), rms(&x[..800]));
        assert!((early - last).abs() < 1.0, "{early:.1} vs {last:.1}");
    }
}
