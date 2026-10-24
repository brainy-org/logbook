//! Where the audio comes from: a file played at the pace of speech, or the
//! microphone.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc::UnboundedSender;

pub const SAMPLE_RATE: u32 = 16_000;
/// Audio is sent in frames of this length.
const FRAME: Duration = Duration::from_millis(100);

/// Any audio or video file as 16 kHz mono samples, through ffmpeg.
pub fn decode(path: &Path) -> Result<Vec<i16>> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map", "0:a:0", "-ac", "1", "-ar", "16000", "-f", "s16le", "-",
        ])
        .output()
        .context("could not run ffmpeg")?;
    if !output.status.success() {
        bail!(
            "ffmpeg could not read {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect())
}

/// Sends `samples` on channel 0 as if spoken: the frame holding second `t`
/// leaves at `start + t / speed`. The sender is dropped at the end.
pub async fn play(
    samples: Arc<Vec<i16>>,
    speed: f64,
    start: Instant,
    tx: UnboundedSender<(u8, Vec<i16>)>,
) {
    let frame = (SAMPLE_RATE as f64 * FRAME.as_secs_f64()) as usize;
    for (i, chunk) in samples.chunks(frame).enumerate() {
        let due = start + FRAME.mul_f64((i + 1) as f64 / speed);
        tokio::time::sleep_until(due.into()).await;
        if tx.send((0, chunk.to_vec())).is_err() {
            return;
        }
    }
}

/// Records the default input device on channel 0 until `stop` is dropped.
/// cpal's stream lives on a thread of its own (it is not Send on macOS).
pub fn microphone(
    tx: UnboundedSender<(u8, Vec<i16>)>,
    stop: std::sync::mpsc::Receiver<()>,
) -> Result<String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String>>();
    std::thread::spawn(move || {
        let started = (|| -> Result<(cpal::Stream, String)> {
            let device = cpal::default_host()
                .default_input_device()
                .context("no input device")?;
            let name = device
                .description()
                .map(|d| d.to_string())
                .unwrap_or_else(|_| "input".into());
            let supported = device.default_input_config()?;
            if supported.sample_format() != cpal::SampleFormat::F32 {
                bail!(
                    "{name} records {:?}; only f32 is supported",
                    supported.sample_format()
                );
            }
            let config: cpal::StreamConfig = supported.into();
            let channels = config.channels as usize;
            let mut resampler = Resampler::new(config.sample_rate as f64, SAMPLE_RATE as f64);
            let stream = device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = data
                        .chunks(channels)
                        .map(|f| f.iter().sum::<f32>() / channels as f32);
                    let out = resampler.push(mono);
                    if !out.is_empty() {
                        let _ = tx.send((0, out));
                    }
                },
                |error| eprintln!("microphone: {error}"),
                None,
            )?;
            stream.play()?;
            Ok((stream, name))
        })();
        match started {
            Ok((stream, name)) => {
                let _ = ready_tx.send(Ok(name));
                let _ = stop.recv();
                drop(stream);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        }
    });
    ready_rx.recv().context("the microphone thread ended")?
}

/// Linear resampling to 16 kHz, carried across callbacks. Enough for speech;
/// the model was trained on 16 kHz.
struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<f32>,
}

impl Resampler {
    fn new(from: f64, to: f64) -> Self {
        Self {
            step: from / to,
            pos: 0.0,
            buf: Vec::new(),
        }
    }

    fn push(&mut self, input: impl Iterator<Item = f32>) -> Vec<i16> {
        self.buf.extend(input);
        let mut out = Vec::new();
        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            let s = self.buf[i] * (1.0 - f) + self.buf[i + 1] * f;
            out.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.pos -= used as f64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_48k_gives_a_third_of_the_samples() {
        let mut r = Resampler::new(48_000.0, 16_000.0);
        let mut n = 0;
        for _ in 0..10 {
            n += r.push(std::iter::repeat_n(0.25, 4800)).len();
        }
        assert!((15_999..=16_000).contains(&n), "{n}");
    }
}
