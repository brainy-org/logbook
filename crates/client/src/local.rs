//! The meeting transcribed in this process, without logbook-server: the
//! same engine session, batcher, finalizer and polisher the server runs
//! (crates/engine), driven through channels instead of a WebSocket. The
//! models are fetched from the mirror on first use, as the server does,
//! and loaded once per run.
//!
//! The audio stays in the meeting's pending folder: the session records
//! its final pass's audio there, deleted as it is read (engine
//! recording.rs), and a final pass over a pending meeting reads the audio
//! kept there, from where its final segments stopped.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use logbook_engine::final_pass::Recorded;
use logbook_engine::finalizer::{self, FinalJob};
use logbook_engine::finals::Finals;
use logbook_engine::mirror::{Fetching, Mirror};
use logbook_engine::polish::{self, Polisher};
use logbook_engine::protocol::{ChannelConfig, FinalState, ReportRequest, SpeakerVoice};
use logbook_engine::session::{self, Input, Resume, Shared};
use logbook_engine::{Engine, Options, batcher, polisher, report};
use serde::Serialize;
use tokio::sync::OnceCell;

use crate::remote::{self, Audio, Events, Finalized, Live};

/// How often a final pass running here is looked at.
const POLL: Duration = Duration::from_millis(200);

/// Where the models and the language model come from.
#[derive(Clone, Default)]
pub struct Config {
    /// Folder of the GGUF files, used as it is; by default the pinned
    /// ones in ~/.cache/brainy-org, fetched from the mirror when missing.
    pub models: Option<std::path::PathBuf>,
    /// An OpenAI-compatible API, up to `/v1`, whose model cleans the final
    /// transcript up (polish) and writes the report. The meeting's text
    /// goes there.
    pub polish_url: Option<String>,
    pub polish_model: Option<String>,
    pub polish_key: Option<String>,
}

impl Config {
    fn model(&self) -> Option<polish::Options> {
        Some(polish::Options {
            url: self.polish_url.clone()?,
            model: self.polish_model.clone()?,
            key: self.polish_key.clone(),
            timeout: Duration::from_secs(polish::TIMEOUT),
        })
    }
}

/// Where the models are, as they get ready: for a page that shows it.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Fetched from the mirror: `done` bytes of `total`, all files together.
    Fetching {
        done: u64,
        total: u64,
    },
    /// On disk, being loaded.
    Loading,
    Ready,
    Failed {
        error: String,
    },
}

/// Hears each `Status`, from the thread that loads the models.
pub type OnStatus = Arc<dyn Fn(Status) + Send + Sync>;

/// The models, loaded on first use.
pub struct Local {
    config: Config,
    loaded: OnceCell<Arc<Loaded>>,
    on_status: Option<OnStatus>,
}

struct Loaded {
    engine: Arc<Engine>,
    batcher: std::sync::mpsc::Sender<logbook_engine::batcher::Job>,
    finalizer: std::sync::mpsc::Sender<FinalJob>,
    finals: Arc<Finals>,
    polisher: Option<std::sync::mpsc::Sender<polisher::PolishJob>>,
}

impl Local {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            loaded: OnceCell::new(),
            on_status: None,
        }
    }

    /// `f` hears how the models get ready: fetched, loaded, or not.
    pub fn with_status(mut self, f: OnStatus) -> Self {
        self.on_status = Some(f);
        self
    }

    /// A clean-up or a report needs a model here: an error now, not at
    /// the meeting's end.
    pub fn check_model(&self, needed: bool) -> Result<()> {
        if needed && self.config.model().is_none() {
            bail!(
                "the clean-up and the report without a server need a language model \
                 (LOGBOOK_POLISH_URL and LOGBOOK_POLISH_MODEL)"
            );
        }
        Ok(())
    }

    /// The meeting's report by the language model of `Config`.
    pub async fn report(&self, request: ReportRequest) -> Result<String> {
        let options = self
            .config
            .model()
            .context("no language model is set for the report")?;
        tokio::task::spawn_blocking(move || {
            report::write(&polish::agent(&options), &options, &request)
        })
        .await?
    }

    /// The models loaded, fetched first if missing; `sessions` at once
    /// (one diarization context each).
    pub async fn load(&self, sessions: usize) -> Result<()> {
        self.loaded(sessions).await.map(|_| ())
    }

    async fn loaded(&self, sessions: usize) -> Result<Arc<Loaded>> {
        let config = self.config.clone();
        let on_status = self.on_status.clone();
        let loaded = self
            .loaded
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || start(config, sessions, on_status))
                    .await?
                    .map(Arc::new)
            })
            .await
            .cloned();
        if let Some(f) = &self.on_status {
            f(match &loaded {
                Ok(_) => Status::Ready,
                Err(error) => Status::Failed {
                    error: format!("{error:#}"),
                },
            });
        }
        loaded
    }

    /// A live session in this process; `dir` is where its final pass
    /// records the audio (the meeting's pending folder).
    pub async fn live(
        &self,
        channels: Vec<ChannelConfig>,
        final_pass: Option<bool>,
        mut audio: Audio,
        dir: &Path,
        events: &mut impl Events,
    ) -> Live {
        let mut out = Live::new();
        let loaded = match self.loaded(1).await {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::warn!("live: {error:#}");
                out.intact = false;
                return out;
            }
        };
        let shared = loaded.shared(dir);
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (in_tx, in_rx) = std::sync::mpsc::channel();
        let reply = in_tx.clone();
        std::thread::spawn(move || {
            session::run(
                &shared,
                "local".into(),
                channels,
                final_pass,
                Resume::default(),
                in_rx,
                reply,
                out_tx,
            );
        });
        let mut ended = false;
        loop {
            tokio::select! {
                frame = audio.recv(), if !ended => match frame {
                    Some((channel, samples)) => {
                        let pcm = samples.iter().map(|&s| s as f32 / 32768.0).collect();
                        let _ = in_tx.send(Input::Audio(channel as usize, pcm));
                    }
                    None => {
                        ended = true;
                        let _ = in_tx.send(Input::Stop);
                    }
                },
                message = out_rx.recv() => {
                    let done = match message {
                        Some(m) => remote::handle(m, &mut out, final_pass.is_some(), Some, events),
                        None => Err(anyhow::anyhow!("the session stopped")),
                    };
                    match done {
                        Ok(false) => {}
                        Ok(true) => return out,
                        Err(error) => {
                            tracing::warn!("live: {error:#}");
                            out.intact = false;
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// The final pass over a meeting kept here, read where it is
    /// (`channels` from `offset` on the meeting's clock), with the
    /// meeting's speakers before it.
    pub async fn final_pass(
        &self,
        channels: Vec<Recorded>,
        offset: f64,
        voices: Vec<SpeakerVoice>,
        polish: bool,
    ) -> Result<Finalized> {
        let loaded = self.loaded(1).await?;
        let audio_s = channels.iter().map(|c| c.seconds).fold(0.0, f64::max);
        let id = loaded.finals.start(audio_s)?;
        loaded
            .finalizer
            .send(FinalJob::Whole {
                id: id.clone(),
                channels,
                files: Vec::new(),
                offset,
                voices,
                polish,
            })
            .context("the finalizer has stopped")?;
        loop {
            match loaded.finals.get(&id) {
                Some(FinalState::Running { .. }) => tokio::time::sleep(POLL).await,
                Some(FinalState::Done {
                    utterances,
                    warnings,
                    voices,
                }) => {
                    return Ok(Finalized {
                        utterances,
                        warnings,
                        voices,
                    });
                }
                Some(FinalState::Failed { message }) => bail!("{message}"),
                None => bail!("the final pass was lost"),
            }
        }
    }
}

impl Loaded {
    fn shared(&self, recordings: &Path) -> Shared {
        Shared {
            engine: self.engine.clone(),
            batcher: self.batcher.clone(),
            finalizer: self.finalizer.clone(),
            finals: self.finals.clone(),
            polisher: self.polisher.clone(),
            recordings: recordings.to_path_buf(),
        }
    }
}

fn start(config: Config, sessions: usize, on_status: Option<OnStatus>) -> Result<Loaded> {
    let mut mirror = Mirror::new();
    if let Some(f) = on_status.clone() {
        mirror = mirror.with_progress(move |p: &Fetching| {
            f(Status::Fetching {
                done: p.done,
                total: p.total,
            })
        });
    }
    let (models, library) =
        mirror.locate(config.models.clone(), |message| tracing::info!("{message}"))?;
    tracing::info!("loading the models…");
    if let Some(f) = &on_status {
        f(Status::Loading);
    }
    let options = Options::defaults(&models, library, sessions.max(1));
    let engine = Arc::new(Engine::load(&options)?);
    let finals = Arc::new(Finals::default());
    let polisher = config
        .model()
        .map(|options| polisher::spawn(Polisher::new(options), finals.clone(), 1));
    Ok(Loaded {
        batcher: batcher::spawn(engine.asr.clone(), logbook_engine::MAX_BATCH),
        finalizer: finalizer::spawn(engine.clone(), finals.clone(), polisher.clone()),
        finals,
        polisher,
        engine,
    })
}
