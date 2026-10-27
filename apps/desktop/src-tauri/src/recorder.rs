//! The meeting being recorded: one at a time, run by crates/client
//! (`meeting::meeting`) as the CLI's `live` is, and told to the pages as
//! events: `meeting-state` (its phase), `utterance` (the live
//! transcript), `levels` (how loud each channel is, for the meter) and,
//! at its end, `meeting-done` or `meeting-failed`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use logbook_client::audio::{Recording, Stop};
use logbook_client::meeting::{self, Backend, Notes, Publish};
use logbook_client::remote::{self, Audio};
use logbook_engine::protocol::{ChannelConfig, Utterance};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// How often the meter is updated.
const LEVELS_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// The microphone and the call being opened.
    Preparing,
    Recording,
    /// The audio has stopped: the final pass, the page and the report.
    Finishing,
}

/// What the pages are told of the meeting being recorded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub phase: Phase,
    /// The models are loaded (or a server is used): the live transcript
    /// follows. Before, the audio is recorded and waits for them.
    pub ready: bool,
    pub title: String,
    /// Milliseconds since the Unix epoch; set once recording starts.
    pub started: Option<u64>,
    /// The channels, as the utterances name them.
    pub channels: Vec<String>,
    /// The person behind the microphone.
    pub me: String,
    pub notes: String,
}

struct Running {
    state: State,
    notes: Notes,
    stop: Option<Stop>,
}

#[derive(Default)]
pub struct Recorder {
    running: Arc<Mutex<Option<Running>>>,
}

impl Recorder {
    pub fn state(&self) -> Option<State> {
        self.running.lock().unwrap().as_ref().map(|r| {
            let mut state = r.state.clone();
            state.notes = r.notes.lock().unwrap().clone();
            state
        })
    }

    pub fn set_notes(&self, text: String) {
        if let Some(r) = self.running.lock().unwrap().as_ref() {
            *r.notes.lock().unwrap() = text;
        }
    }

    /// Ends the recording; the meeting goes on to its end.
    pub fn stop(&self, app: &AppHandle) {
        let mut running = self.running.lock().unwrap();
        if let Some(r) = running.as_mut() {
            r.stop.take();
            r.state.phase = Phase::Finishing;
            let _ = app.emit("meeting-state", Some(&r.state));
        }
    }

    /// Starts a meeting; an error if one is running.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        app: AppHandle,
        backend: Backend,
        data: PathBuf,
        publish: Publish,
        sources: Vec<(logbook_client::audio::Source, Option<String>, bool)>,
        echo: logbook_client::audio::Echo,
        gain: logbook_client::aec::Gain,
    ) -> Result<State> {
        let notes = Notes::default();
        let state = {
            let mut running = self.running.lock().unwrap();
            if running.is_some() {
                bail!("a meeting is already being recorded");
            }
            let state = State {
                phase: Phase::Preparing,
                ready: false,
                title: publish.title.clone().unwrap_or_default(),
                started: None,
                channels: Vec::new(),
                me: publish.me.clone(),
                notes: String::new(),
            };
            *running = Some(Running {
                state: state.clone(),
                notes: notes.clone(),
                stop: None,
            });
            state
        };
        let running = self.running.clone();
        tauri::async_runtime::spawn(async move {
            let result = run(
                &app, &running, backend, &data, publish, sources, echo, gain, notes,
            )
            .await;
            running.lock().unwrap().take();
            let _ = app.emit("meeting-state", None::<State>);
            match result {
                Ok(outcome) => {
                    let _ = app.emit("meeting-done", outcome);
                }
                Err(error) => {
                    tracing::warn!("{error:#}");
                    let _ = app.emit("meeting-failed", format!("{error:#}"));
                }
            }
        });
        Ok(state)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    app: &AppHandle,
    running: &Arc<Mutex<Option<Running>>>,
    backend: Backend,
    data: &std::path::Path,
    publish: Publish,
    sources: Vec<(logbook_client::audio::Source, Option<String>, bool)>,
    echo: logbook_client::audio::Echo,
    gain: logbook_client::aec::Gain,
    notes: Notes,
) -> Result<Option<meeting::Outcome>> {
    let _ = app.emit(
        "meeting-state",
        running.lock().unwrap().as_ref().map(|r| &r.state),
    );
    backend.check(publish.polish || publish.report)?;
    // Recording starts at once, the models loading or not: what is said
    // meanwhile waits in the audio's channel (about 64 KB a second for
    // two channels) and is transcribed once they are ready.
    let recording = Recording::start(sources, echo, gain)?;
    for (channel, device) in &recording.devices {
        tracing::info!("{channel}: {device}");
    }
    let channels: Vec<ChannelConfig> = recording.channels.clone();
    {
        let mut guard = running.lock().unwrap();
        let Some(r) = guard.as_mut() else {
            return Ok(None);
        };
        r.stop = Some(recording.stop);
        // Stopped before the microphone opened: ends as soon as it starts.
        if r.state.phase == Phase::Finishing {
            r.stop.take();
        } else {
            r.state.phase = Phase::Recording;
        }
        r.state.started = Some(now_ms());
        r.state.channels = channels.iter().map(|c| c.name.clone()).collect();
        let _ = app.emit("meeting-state", Some(&r.state));
    }
    let audio = meter(app.clone(), recording.audio, channels.len());
    // Usually done at the app's start (`lib.rs`, `preload`).
    backend.prepare(publish.polish || publish.report, 1).await?;
    {
        let mut guard = running.lock().unwrap();
        if let Some(r) = guard.as_mut() {
            r.state.ready = true;
            let _ = app.emit("meeting-state", Some(&r.state));
        }
    }
    let mut events = Events { app: app.clone() };
    meeting::meeting(
        &backend,
        data,
        channels,
        &publish,
        audio,
        notes,
        &mut events,
    )
    .await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The live session's events, to the pages.
struct Events {
    app: AppHandle,
}

impl remote::Events for Events {
    fn utterance(&mut self, u: Utterance) {
        #[derive(Clone, Serialize)]
        struct Live<'a> {
            channel: &'a str,
            speaker: Option<u32>,
            start: f64,
            end: f64,
            text: &'a str,
        }
        let _ = self.app.emit(
            "utterance",
            Live {
                channel: &u.channel,
                speaker: u.speaker,
                start: u.start,
                end: u.end,
                text: &u.text,
            },
        );
    }

    fn finalizing(&mut self, audio_s: f64, _id: &str) {
        tracing::info!("finishing the final pass ({:.0} s of audio)…", audio_s);
    }
}

/// The audio passed on unchanged, each channel's loudest sample in the
/// last `LEVELS_EVERY` sent to the pages as `levels`, 0 to 1.
fn meter(app: AppHandle, mut audio: Audio, channels: usize) -> Audio {
    let (tx, out) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut peaks = vec![0u16; channels];
        let mut last = Instant::now();
        while let Some((channel, samples)) = audio.recv().await {
            if let Some(peak) = peaks.get_mut(channel as usize) {
                let frame = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                *peak = (*peak).max(frame);
            }
            if last.elapsed() >= LEVELS_EVERY {
                let levels: Vec<f32> = peaks.iter().map(|&p| p as f32 / 32768.0).collect();
                let _ = app.emit("levels", levels);
                peaks.iter_mut().for_each(|p| *p = 0);
                last = Instant::now();
            }
            if tx.send((channel, samples)).is_err() {
                break;
            }
        }
        let _ = app.emit("levels", vec![0.0f32; channels]);
    });
    out
}
