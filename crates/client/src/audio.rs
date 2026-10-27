//! Where the audio comes from: a file played at the pace of speech, the
//! microphone, or what the computer plays (`Recording`: the channels of a
//! live meeting, the call's echo cancelled from the microphone).

use std::path::Path;
use std::process::Command;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use logbook_engine::protocol::ChannelConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::aec::{self, Gain};
use crate::remote::Audio;

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

/// What the microphone hears of the system channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Echo {
    /// Nothing: headphones, or one of the two channels only.
    None,
    /// The call: the server drops its words from the mic's.
    Filter,
    /// The call: cancelled from the mic's audio first (aec.rs), then filtered.
    Cancel,
}

/// A live meeting's audio being recorded: its channels ("mic", "system"),
/// on one clock from `start`. Dropping `stop` ends it: `audio` then ends
/// once the devices have stopped.
pub struct Recording {
    pub channels: Vec<ChannelConfig>,
    pub audio: Audio,
    pub start: Instant,
    pub stop: Stop,
    /// (channel, device) as recorded.
    pub devices: Vec<(String, String)>,
}

/// Ends a recording when dropped.
pub struct Stop(#[allow(dead_code)] Vec<std::sync::mpsc::Sender<()>>);

impl Recording {
    /// Each source is a channel: (what, which device, diarized). The
    /// microphone is channel 0 and the system channel 1 when there are
    /// both; with `Echo::Cancel` or a gain the microphone goes through
    /// aec.rs.
    pub fn start(
        sources: Vec<(Source, Option<String>, bool)>,
        echo: Echo,
        gain: Gain,
    ) -> Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut stops = Vec::new();
        let mut channels = Vec::new();
        let mut devices = Vec::new();
        let mic = sources
            .iter()
            .any(|(source, _, _)| matches!(source, Source::Microphone));
        // Every channel's sample 0, and what latencies are measured from.
        let start = Instant::now();
        for (index, (source, device, diarize)) in sources.into_iter().enumerate() {
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            let name = capture(source, device, index as u8, start, tx.clone(), stop_rx)?;
            let channel = match source {
                Source::Microphone => "mic",
                Source::System => "system",
            };
            devices.push((channel.to_string(), name));
            stops.push(stop_tx);
            let echo_of = match source {
                Source::Microphone if echo != Echo::None => Some("system".into()),
                _ => None,
            };
            channels.push(ChannelConfig {
                name: channel.into(),
                diarize,
                echo_of,
            });
        }
        // The session stops when the last sender is gone: the capture threads'.
        drop(tx);
        let gain = if mic { gain } else { Gain::Off };
        let audio = match (echo == Echo::Cancel, gain) {
            (false, Gain::Off) => rx,
            (cancel, gain) => aec::process(rx, 0, cancel.then_some(1), gain),
        };
        Ok(Self {
            channels,
            audio,
            start,
            stop: Stop(stops),
            devices,
        })
    }
}

/// Where live audio is recorded from.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// An input device: your voice.
    Microphone,
    /// What the computer plays: the other people in the call. An output
    /// device recorded as input: a Core Audio process tap on macOS (14.6+),
    /// WASAPI loopback on Windows; on Linux, PipeWire's "Monitor of" input
    /// devices, picked by name.
    System,
}

/// Records `source` on `channel` until `stop` is dropped; returns the
/// device's name. `device` picks one by part of its name (`logbook
/// devices`), else the default input or output. The channel's sample 0 is
/// `start`, the session's start (`Timeline`). cpal's stream lives on a
/// thread of its own (it is not Send on macOS).
pub fn capture(
    source: Source,
    device: Option<String>,
    channel: u8,
    start: Instant,
    tx: UnboundedSender<(u8, Vec<i16>)>,
    stop: std::sync::mpsc::Receiver<()>,
) -> Result<String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String>>();
    std::thread::spawn(move || {
        let timeline = Arc::new(Mutex::new(Timeline { start, sent: 0 }));
        let started = (|| -> Result<(cpal::Stream, String)> {
            let device = pick(source, device.as_deref())?;
            let name = device_name(&device);
            // An output device has no input config: cpal records it through
            // a loopback (macOS, Windows) with the output's format.
            let supported = if device.supports_input() {
                device.default_input_config()?
            } else {
                device.default_output_config()?
            };
            let format = supported.sample_format();
            #[allow(unused_mut)]
            let mut config: cpal::StreamConfig = supported.into();
            // WASAPI's default buffer is one 10 ms period: a device whose
            // audio arrives in bursts (Remote Desktop's microphone, from the
            // network) overflows it, and Windows drops over half the frames
            // as a discontinuity. 100 ms of room; the callbacks keep coming
            // every period, so it adds no latency
            // (docs/decisions/2026-10-27-windows-audio-buffer.md).
            #[cfg(windows)]
            {
                config.buffer_size = cpal::BufferSize::Fixed(config.sample_rate / 10);
            }
            let stream = match format {
                cpal::SampleFormat::F32 => {
                    record::<f32>(&device, config, channel, &timeline, tx.clone(), source)?
                }
                cpal::SampleFormat::I16 => {
                    record::<i16>(&device, config, channel, &timeline, tx.clone(), source)?
                }
                cpal::SampleFormat::I32 => {
                    record::<i32>(&device, config, channel, &timeline, tx.clone(), source)?
                }
                other => bail!("{name} records {other:?}, which is not supported"),
            };
            stream.play()?;
            Ok((stream, name))
        })();
        match started {
            Ok((stream, name)) => {
                let _ = ready_tx.send(Ok(name));
                // A device that delivers nothing (an idle output) still
                // moves along the clock: silence, every IDLE_TICK.
                while let Err(RecvTimeoutError::Timeout) = stop.recv_timeout(IDLE_TICK) {
                    let silence = timeline.lock().unwrap().idle(Instant::now());
                    if silence > 0 {
                        let _ = tx.send((channel, vec![0; silence]));
                    }
                }
                drop(stream);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        }
    });
    ready_rx
        .recv()
        .context("the recording thread ended")?
        .with_context(|| format!("could not record the {source:?}"))
}

fn record<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channel: u8,
    timeline: &Arc<Mutex<Timeline>>,
    tx: UnboundedSender<(u8, Vec<i16>)>,
    source: Source,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate as f64, SAMPLE_RATE as f64);
    let timeline = timeline.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono = data.chunks(channels).map(|frame| {
                frame
                    .iter()
                    .map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s))
                    .sum::<f32>()
                    / channels as f32
            });
            let out = resampler.push(mono);
            if out.is_empty() {
                return;
            }
            let silence = timeline.lock().unwrap().gap(Instant::now(), out.len());
            if silence > 0 {
                let _ = tx.send((channel, vec![0; silence]));
            }
            let _ = tx.send((channel, out));
        },
        move |error| tracing::warn!("{source:?}: {error}"),
        None,
    )?)
}

fn pick(source: Source, name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => match source {
            Source::Microphone => host
                .default_input_device()
                .context("no default input device"),
            Source::System => host
                .default_output_device()
                .context("no default output device"),
        },
        Some(name) => {
            let wanted = name.to_lowercase();
            host.devices()?
                .find(|d| device_name(d).to_lowercase().contains(&wanted))
                .with_context(|| format!("no device named like {name:?}: see `logbook devices`"))
        }
    }
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unnamed device".into())
}

/// The devices by name, the default first: what can be recorded as the
/// microphone, and what as the system (output devices).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Devices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub fn device_names() -> Result<Devices> {
    let host = cpal::default_host();
    let input = host.default_input_device().map(|d| device_name(&d));
    let output = host.default_output_device().map(|d| device_name(&d));
    let mut out = Devices::default();
    for d in host.devices()? {
        let name = device_name(&d);
        if d.supports_input() && !out.inputs.contains(&name) {
            out.inputs.push(name.clone());
        }
        if d.supports_output() && !out.outputs.contains(&name) {
            out.outputs.push(name);
        }
    }
    let first = |list: &mut Vec<String>, default: Option<String>| {
        if let Some(i) = default.and_then(|d| list.iter().position(|n| *n == d)) {
            let name = list.remove(i);
            list.insert(0, name);
        }
    };
    first(&mut out.inputs, input);
    first(&mut out.outputs, output);
    Ok(out)
}

/// Every device, with what it can be used for.
pub fn devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let input = host.default_input_device().map(|d| device_name(&d));
    let output = host.default_output_device().map(|d| device_name(&d));
    Ok(host
        .devices()?
        .map(|d| {
            let name = device_name(&d);
            let mut uses = Vec::new();
            if d.supports_input() {
                uses.push(if input.as_deref() == Some(&name) {
                    "microphone (default)"
                } else {
                    "microphone"
                });
            }
            if d.supports_output() {
                uses.push(if output.as_deref() == Some(&name) {
                    "system (default)"
                } else {
                    "system"
                });
            }
            format!("{name:<40} {}", uses.join(", "))
        })
        .collect())
}

/// A gap longer than this between the samples a channel has sent and the
/// time elapsed is filled with silence; shorter ones are callback jitter.
const MAX_GAP: f64 = 0.5;
/// How often a channel whose device delivers nothing is checked.
const IDLE_TICK: Duration = Duration::from_millis(100);
/// A device silent for longer than this is idle, not late: its channel is
/// filled. Short, because the echo canceller holds the mic until the
/// system channel has reached it (aec.rs): this is added to the mic's
/// latency while nothing plays.
const IDLE_AFTER: f64 = 0.25;
/// How far behind the clock idle silence stops: room for the samples of a
/// device that wakes up, which arrive late.
const IDLE_MARGIN: f64 = 0.1;

/// Keeps a channel on the session's clock. macOS stops an output device
/// when nothing plays, and its process tap then delivers nothing, not
/// silence: the system channel would start at the first sound and lose
/// every quiet stretch, its times drifting from the microphone's, and the
/// VAD would never see the pause that closes a phrase. Nor would the
/// channel move while nothing plays: the echo filter, which holds the mic's
/// words until the system channel has passed them, would hold them all.
/// (docs/decisions/2026-10-24-system-audio-through-cpal.md)
struct Timeline {
    start: Instant,
    sent: u64,
}

impl Timeline {
    /// Samples of silence to send before `n` samples that arrive at `now`.
    fn gap(&mut self, now: Instant, n: usize) -> usize {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f64();
        let due = (elapsed * SAMPLE_RATE as f64) as i64 - n as i64;
        let missing = due - self.sent as i64;
        let silence = if missing as f64 > MAX_GAP * SAMPLE_RATE as f64 {
            missing as usize
        } else {
            0
        };
        self.sent += (silence + n) as u64;
        silence
    }

    /// Samples of silence to send at `now` when no audio has come for
    /// longer than `IDLE_AFTER`: up to `IDLE_MARGIN` before `now`.
    fn idle(&mut self, now: Instant) -> usize {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f64();
        let due = (elapsed * SAMPLE_RATE as f64) as i64;
        if ((due - self.sent as i64) as f64) <= IDLE_AFTER * SAMPLE_RATE as f64 {
            return 0;
        }
        let to = ((elapsed - IDLE_MARGIN) * SAMPLE_RATE as f64) as i64;
        let silence = (to - self.sent as i64).max(0) as usize;
        self.sent += silence as u64;
        silence
    }
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
    fn a_channel_that_starts_late_or_pauses_is_padded_to_the_clock() {
        let start = Instant::now();
        let mut t = Timeline { start, sent: 0 };
        // First samples 2 s after the start: 2 s of silence first.
        assert_eq!(t.gap(start + Duration::from_secs(2), 1600), 32_000 - 1600);
        // On time, or a little late: nothing.
        assert_eq!(t.gap(start + Duration::from_millis(2100), 1600), 0);
        assert_eq!(t.gap(start + Duration::from_millis(2400), 1600), 0);
        // Three seconds without a callback.
        let silence = t.gap(start + Duration::from_millis(5400), 1600);
        assert_eq!(t.sent, 5400 * 16);
        assert!(silence > 40_000, "{silence}");
    }

    #[test]
    fn an_idle_device_still_moves_along_the_clock() {
        let start = Instant::now();
        let mut t = Timeline { start, sent: 0 };
        assert_eq!(t.idle(start + Duration::from_millis(200)), 0);
        // Nothing for a second: silence up to 0.9 s.
        assert_eq!(t.idle(start + Duration::from_secs(1)), 14_400);
        assert_eq!(t.idle(start + Duration::from_millis(1100)), 0);
        // The device wakes up: its samples follow, no more silence.
        assert_eq!(t.gap(start + Duration::from_millis(1300), 1600), 0);
    }

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
