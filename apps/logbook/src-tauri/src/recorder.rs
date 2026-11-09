//! The meeting being recorded: one at a time, run by crates/client
//! (`series::meetings`) as the CLI's `live` is, and told to the pages as
//! events: `meeting-state` (its phase), `utterance` (the live
//! transcript), `levels` (how loud each channel is, for the meter) and,
//! at its end, `meeting-done` or `meeting-failed`. A recording left
//! running is cut into meetings (a long pause, eight hours): each one
//! that ends while the next records is `meetings-changed` (or
//! `meeting-failed`), and the state's `part` counts them. The title given
//! while it records goes to the meeting it was given in and the later
//! ones (`series::Notebook`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use logbook_client::audio::{Recording, Stop};
use logbook_client::meeting::{self, Backend, Publish};
use logbook_client::remote::{self, Audio};
use logbook_client::series::{self, Limits, Notebook};
use logbook_engine::protocol::{ChannelConfig, Utterance};
use serde::Serialize;
#[cfg(not(target_os = "android"))]
use tauri::Manager;
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
    /// The notes of meeting `part`, as `meeting_state` tells them; the
    /// `meeting-state` events leave them empty (they are in `Notebook`,
    /// and change with every key): a page drawn again asks for them.
    pub notes: String,
    /// Which meeting of the recording this is, from 0: the notes start
    /// empty in each.
    pub part: usize,
    /// Once the recording has stopped: where finishing its meeting is.
    pub step: Option<meeting::Step>,
}

struct Running {
    state: State,
    notes: Notebook,
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
            state.notes = r.notes.get(state.part);
            state
        })
    }

    /// The notes typed in meeting `part` of the recording: an earlier
    /// one's, sent just after it closed, still go to it.
    pub fn set_notes(&self, part: usize, text: String) {
        if let Some(r) = self.running.lock().unwrap().as_ref() {
            r.notes.set(part, text);
        }
    }

    /// The recording's title from meeting `part` on (series.rs): the
    /// pages are told.
    pub fn set_title(&self, app: &AppHandle, part: usize, title: String) {
        if let Some(r) = self.running.lock().unwrap().as_mut() {
            r.notes.set_title(part, &title);
            r.state.title = title.trim().to_string();
            let _ = app.emit("meeting-state", Some(&r.state));
        }
    }

    /// Ends the recording; the meeting goes on to its end.
    pub fn stop(&self, app: &AppHandle) {
        stop(&self.running, app);
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
        let notes = Notebook::default();
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
                part: 0,
                step: None,
            };
            *running = Some(Running {
                state: state.clone(),
                notes: notes.clone(),
                stop: None,
            });
            state
        };
        // The screenshots of the last recording no meeting took.
        logbook_client::shots::clear(&data);
        #[cfg(not(target_os = "android"))]
        crate::screenshot::listen(&app);
        let running = self.running.clone();
        tauri::async_runtime::spawn(async move {
            let result = run(
                &app, &running, backend, &data, publish, sources, echo, gain, notes,
            )
            .await;
            // The service kept the recording and its meetings' ends going.
            #[cfg(target_os = "android")]
            crate::android::end(&app);
            // A meeting it left pending is tried again (finisher.rs).
            #[cfg(not(target_os = "android"))]
            app.state::<crate::App>().finisher.start(&app);
            running.lock().unwrap().take();
            #[cfg(not(target_os = "android"))]
            crate::screenshot::unlisten(&app);
            let _ = app.emit("meeting-state", None::<State>);
            match result {
                Ok(outcome) => {
                    // Nothing said: nothing kept to show.
                    let outcome = outcome.filter(|o| !o.silent);
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

/// Ends the recording `running` holds, if any.
fn stop(running: &Mutex<Option<Running>>, app: &AppHandle) {
    let mut running = running.lock().unwrap();
    if let Some(r) = running.as_mut() {
        r.stop.take();
        r.state.phase = Phase::Finishing;
        let _ = app.emit("meeting-state", Some(&r.state));
        #[cfg(target_os = "android")]
        crate::android::finishing(app);
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
    notes: Notebook,
) -> Result<Option<meeting::Outcome>> {
    let _ = app.emit(
        "meeting-state",
        running.lock().unwrap().as_ref().map(|r| &r.state),
    );
    backend.check(publish.model())?;
    // Android: the permissions, and the service that keeps the process
    // recording with the screen off, started before the microphone opens.
    #[cfg(target_os = "android")]
    {
        let (handle, running) = (app.clone(), running.clone());
        let title = publish.title.clone().unwrap_or_default();
        crate::android::begin(app, &title, move || stop(&running, &handle)).await?;
    }
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
    backend.prepare(publish.model(), 1).await?;
    {
        let mut guard = running.lock().unwrap();
        if let Some(r) = guard.as_mut() {
            r.state.ready = true;
            let _ = app.emit("meeting-state", Some(&r.state));
        }
    }
    let mut events = Events {
        app: app.clone(),
        running: running.clone(),
    };
    series::meetings(
        &backend,
        data,
        channels,
        &publish,
        audio,
        &notes,
        Limits::default(),
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
    running: Arc<Mutex<Option<Running>>>,
}

impl series::Series for Events {
    fn meeting_started(&mut self, n: usize, at: f64) {
        tracing::info!("a new meeting starts at {:.0} s of the recording", at);
        let mut guard = self.running.lock().unwrap();
        if let Some(r) = guard.as_mut() {
            // The meeting that closed keeps its notes; this one's start empty.
            r.state.part = n;
            r.state.notes = String::new();
            let _ = self.app.emit("meeting-state", Some(&r.state));
        }
    }

    fn meeting_step(&mut self, _n: usize, step: &meeting::Step) {
        let mut guard = self.running.lock().unwrap();
        if let Some(r) = guard.as_mut()
            && r.state.phase == Phase::Finishing
        {
            r.state.step = Some(step.clone());
            let _ = self.app.emit("meeting-state", Some(&r.state));
        }
    }

    fn meeting_ended(&mut self, _n: usize, outcome: &Result<Option<meeting::Outcome>>) {
        match outcome {
            Ok(_) => {
                let _ = self.app.emit("meetings-changed", ());
            }
            Err(error) => {
                tracing::warn!("{error:#}");
                let _ = self.app.emit("meeting-failed", format!("{error:#}"));
            }
        }
    }
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
