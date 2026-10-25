//! The microphone's audio through WebRTC's audio processing before anything
//! else hears it: the call's echo cancelled
//! (docs/decisions/2026-10-24-echo-cancellation-on-the-client.md), and its
//! level raised (`Gain`, AGC2: a voice far from the mic is quiet, and the
//! VAD and the model miss it). Either, or both.
//!
//! Without headphones the microphone hears the speakers, louder than the
//! person: the transcription of the mic is mostly the call, and what the
//! person says over it is lost (the server's echo filter compares words,
//! and can drop the call's words, not recover the person's). WebRTC's AEC3
//! (sonora, a Rust port) subtracts from the mic what the system channel
//! played, the way a video call does.
//!
//! Both channels are on the session's clock, so sample i of the mic is
//! processed after sample i of the system channel has been given to AEC3
//! as the far end: the mic waits for the system channel to reach it (at
//! most `IDLE_AFTER` in audio.rs while nothing plays, a callback's worth
//! otherwise). AEC3 estimates the remaining delay, the speakers' and the
//! room's, by itself. The system channel goes on unchanged.
//!
//! The gain comes after the echo is cancelled, so the call's echo is not
//! raised with the voice; AGC2's limiter keeps it from clipping.

use std::collections::VecDeque;

use sonora::config::{AdaptiveDigital, EchoCanceller, FixedDigital, GainController2};
use sonora::{AudioProcessing, Config, StreamConfig};
use tokio::sync::mpsc::unbounded_channel;

use crate::audio::SAMPLE_RATE;
use crate::client::Audio;

/// AEC3 works on 10 ms blocks.
const BLOCK: usize = SAMPLE_RATE as usize / 100;

/// How the mic's level is raised.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gain {
    /// As the device gives it.
    Off,
    /// To a speaking level, as the voice comes and goes (AGC2's adaptive
    /// digital controller: up to 50 dB, 6 dB a second at most).
    Auto,
    /// By this many dB, 0 to 50, with a limiter.
    Fixed(f32),
}

impl std::str::FromStr for Gain {
    type Err = String;

    /// "auto", "off", or a number of dB.
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "off" | "0" => Ok(Self::Off),
            _ => match s.trim_end_matches("dB").parse::<f32>() {
                Ok(db) if (0.0..50.0).contains(&db) => Ok(Self::Fixed(db)),
                _ => Err(format!("{s:?}: \"auto\", \"off\" or dB from 0 to under 50")),
            },
        }
    }
}

/// `audio` with the mic processed: its echo of the `system` channel
/// cancelled, if there is one, and its level raised by `gain`.
pub fn process(mut audio: Audio, mic: u8, system: Option<u8>, gain: Gain) -> Audio {
    let (tx, out) = unbounded_channel();
    tokio::spawn(async move {
        let mut canceller = Canceller::new(system.is_some(), gain);
        while let Some((channel, samples)) = audio.recv().await {
            if Some(channel) == system {
                canceller.far.extend(&samples);
                if tx.send((channel, samples)).is_err() {
                    return;
                }
            } else if channel == mic {
                canceller.near.extend(&samples);
            } else if tx.send((channel, samples)).is_err() {
                return;
            }
            let cleaned = canceller.process();
            if !cleaned.is_empty() && tx.send((mic, cleaned)).is_err() {
                return;
            }
        }
        // The end: what the system channel never reached goes as it is.
        let rest: Vec<i16> = canceller.near.drain(..).collect();
        if !rest.is_empty() {
            let _ = tx.send((mic, rest));
        }
    });
    out
}

struct Canceller {
    apm: AudioProcessing,
    /// Cancel the system channel's echo: each mic block waits for its
    /// system block.
    echo: bool,
    /// The system channel's samples not yet given to AEC3.
    far: VecDeque<i16>,
    /// The mic's samples not yet processed. `far` and `near` start at the
    /// same sample of the session, and are consumed a block of each at once.
    near: VecDeque<i16>,
    far_block: Vec<i16>,
    near_block: Vec<i16>,
    scratch: Vec<i16>,
}

impl Canceller {
    fn new(echo: bool, gain: Gain) -> Self {
        let gain_controller2 = match gain {
            Gain::Off => None,
            Gain::Auto => Some(GainController2 {
                adaptive_digital: Some(AdaptiveDigital::default()),
                ..Default::default()
            }),
            Gain::Fixed(gain_db) => Some(GainController2 {
                fixed_digital: FixedDigital { gain_db },
                ..Default::default()
            }),
        };
        let config = Config {
            echo_canceller: echo.then(EchoCanceller::default),
            gain_controller2,
            ..Default::default()
        };
        let stream = StreamConfig::new(SAMPLE_RATE, 1);
        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(stream)
            .render_config(stream)
            .build();
        Self {
            apm,
            echo,
            far: VecDeque::new(),
            near: VecDeque::new(),
            far_block: vec![0; BLOCK],
            near_block: vec![0; BLOCK],
            scratch: vec![0; BLOCK],
        }
    }

    /// The mic's samples ready, processed: the ones both channels have
    /// reached, when the echo is cancelled.
    fn process(&mut self) -> Vec<i16> {
        let ready = if self.echo {
            self.far.len().min(self.near.len())
        } else {
            self.near.len()
        };
        let blocks = ready / BLOCK;
        let mut out = Vec::with_capacity(blocks * BLOCK);
        for _ in 0..blocks {
            for (dst, src) in self.near_block.iter_mut().zip(self.near.drain(..BLOCK)) {
                *dst = src;
            }
            if self.echo {
                for (dst, src) in self.far_block.iter_mut().zip(self.far.drain(..BLOCK)) {
                    *dst = src;
                }
                // A failed block goes through as it was: a word with echo
                // is better than a hole in the mic's audio.
                if self
                    .apm
                    .process_render_i16(&self.far_block, &mut self.scratch)
                    .is_err()
                {
                    out.extend_from_slice(&self.near_block);
                    continue;
                }
                // The delay between the two is the session's clock: none.
                let _ = self.apm.set_stream_delay_ms(0);
            }
            match self
                .apm
                .process_capture_i16(&self.near_block, &mut self.scratch)
            {
                Ok(()) => out.extend_from_slice(&self.scratch),
                Err(_) => out.extend_from_slice(&self.near_block),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noise-like signal, the same for a given seed.
    fn noise(seed: u32, n: usize) -> Vec<i16> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((x as i32 >> 20) as i16).wrapping_mul(2)
            })
            .collect()
    }

    fn energy(s: &[i16]) -> f64 {
        s.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / s.len() as f64
    }

    /// A quiet tone, with pauses.
    fn quiet_voice(n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                // 1 s on, 0.3 s off.
                if t % 1.3 < 1.0 {
                    ((t * 2.0 * std::f64::consts::PI * 220.0).sin() * 300.0) as i16
                } else {
                    0
                }
            })
            .collect()
    }

    fn run(gain: Gain, near: &[i16]) -> Vec<i16> {
        let mut c = Canceller::new(false, gain);
        let mut out = Vec::new();
        for piece in near.chunks(700) {
            c.near.extend(piece);
            out.extend(c.process());
        }
        out
    }

    #[test]
    fn the_gain_raises_a_quiet_mic_without_the_system_channel() {
        let n = SAMPLE_RATE as usize * 8;
        let near = quiet_voice(n);
        let tail = n - 2 * SAMPLE_RATE as usize..n - SAMPLE_RATE as usize;
        let before = energy(&near[tail.clone()]);
        // Every sample out, in step: no far end to wait for.
        let off = run(Gain::Off, &near);
        assert_eq!(off.len(), n / BLOCK * BLOCK);
        let fixed = run(Gain::Fixed(12.0), &near);
        // 12 dB is about 16 times the energy.
        let ratio = energy(&fixed[tail.clone()]) / before;
        assert!((10.0..25.0).contains(&ratio), "fixed: {ratio}");
        // `Auto` raises only what AGC2's own VAD takes for speech, which a
        // tone is not: measured on a recording instead (the decision record).
        let auto = run(Gain::Auto, &near);
        assert_eq!(auto.len(), off.len());
    }

    #[test]
    fn gains_are_read() {
        assert_eq!("auto".parse(), Ok(Gain::Auto));
        assert_eq!("off".parse(), Ok(Gain::Off));
        assert_eq!("12".parse(), Ok(Gain::Fixed(12.0)));
        assert_eq!("6dB".parse(), Ok(Gain::Fixed(6.0)));
        assert!("60".parse::<Gain>().is_err());
        assert!("-3".parse::<Gain>().is_err());
    }

    #[test]
    fn the_far_end_is_taken_out_of_the_mic() {
        // The mic hears the system channel 30 ms later, at half its level.
        let n = SAMPLE_RATE as usize * 6;
        let far = noise(7, n);
        let delay = 480;
        let near: Vec<i16> = (0..n)
            .map(|i| if i >= delay { far[i - delay] / 2 } else { 0 })
            .collect();
        let mut c = Canceller::new(true, Gain::Off);
        let mut out = Vec::new();
        // Fed in uneven pieces, as the devices do.
        let (mut f, mut m) = (0, 0);
        while f < n || m < n {
            let f_end = (f + 1000).min(n);
            c.far.extend(&far[f..f_end]);
            f = f_end;
            let m_end = (m + 700).min(n);
            c.near.extend(&near[m..m_end]);
            m = m_end;
            out.extend(c.process());
        }
        assert_eq!(out.len(), n);
        // After AEC3 has converged (the last 2 s): far quieter than before.
        let tail = out.len() - 2 * SAMPLE_RATE as usize..out.len();
        let before = energy(&near[tail.clone()]);
        let after = energy(&out[tail]);
        assert!(after < before / 10.0, "{before} -> {after}");
    }
}
