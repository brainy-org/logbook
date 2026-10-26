//! The meeting transcribed in this process, without logbook-server: the
//! same engine session, batcher, finalizer and polisher the server runs
//! (crates/engine), driven through channels instead of a WebSocket. The
//! models are fetched from the mirror on first use, as the server does,
//! and loaded once per run.
//!
//! The audio stays in the meeting's pending folder: the session records
//! its final pass's audio there, and a final pass over a pending meeting
//! copies it there, both deleted as they are read (engine recording.rs).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use logbook_engine::finalizer::{self, FinalJob};
use logbook_engine::finals::Finals;
use logbook_engine::mirror::Mirror;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::polish::{self, Polisher};
use logbook_engine::protocol::{ChannelConfig, FinalState};
use logbook_engine::recording::Recording;
use logbook_engine::session::{self, Input, Shared};
use logbook_engine::{Engine, Options, batcher, polisher};
use tokio::sync::OnceCell;

use crate::client::{self, Audio, Events, Finalized, Live};

/// How often a final pass running here is looked at.
const POLL: Duration = Duration::from_millis(200);

/// Where the models and the clean-up model come from.
#[derive(clap::Args, Clone)]
#[command(next_help_heading = "Without --server")]
pub struct Config {
    /// Folder of the GGUF files, used as it is; by default the pinned
    /// ones in ~/.cache/brainy-org, fetched from the mirror when missing.
    #[arg(long, global = true, env = "LOGBOOK_MODELS")]
    models: Option<std::path::PathBuf>,
    /// An OpenAI-compatible API, up to `/v1`, whose model cleans the final
    /// transcript up with --polish. The meeting's text goes there.
    #[arg(long, global = true, env = "LOGBOOK_POLISH_URL")]
    polish_url: Option<String>,
    #[arg(long, global = true, env = "LOGBOOK_POLISH_MODEL")]
    polish_model: Option<String>,
    #[arg(
        long,
        global = true,
        env = "LOGBOOK_POLISH_KEY",
        hide_env_values = true
    )]
    polish_key: Option<String>,
}

/// The models, loaded on first use.
pub struct Local {
    config: Config,
    loaded: OnceCell<Arc<Loaded>>,
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
        }
    }

    /// --polish needs a model here: an error now, not at the meeting's end.
    pub fn check_polish(&self, polish: bool) -> Result<()> {
        if polish && (self.config.polish_url.is_none() || self.config.polish_model.is_none()) {
            bail!("--polish without --server needs LOGBOOK_POLISH_URL and LOGBOOK_POLISH_MODEL");
        }
        Ok(())
    }

    /// The models loaded, fetched first if missing; `sessions` at once
    /// (one diarization context each).
    pub async fn load(&self, sessions: usize) -> Result<()> {
        self.loaded(sessions).await.map(|_| ())
    }

    async fn loaded(&self, sessions: usize) -> Result<Arc<Loaded>> {
        let config = self.config.clone();
        self.loaded
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || start(config, sessions))
                    .await?
                    .map(Arc::new)
            })
            .await
            .cloned()
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
        let mut out = Live {
            finalized: None,
            final_id: None,
            intact: true,
        };
        let loaded = match self.loaded(1).await {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!("live: {error:#}");
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
                        Some(m) => client::handle(m, &mut out, final_pass.is_some(), |u| u, events),
                        None => Err(anyhow::anyhow!("the session stopped")),
                    };
                    match done {
                        Ok(false) => {}
                        Ok(true) => return out,
                        Err(error) => {
                            eprintln!("live: {error:#}");
                            out.intact = false;
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// The final pass over a meeting kept here, its audio copied to `dir`
    /// while it runs.
    pub async fn final_pass(
        &self,
        channels: Vec<ChannelConfig>,
        polish: bool,
        audio: Vec<Vec<i16>>,
        dir: &Path,
    ) -> Result<Finalized> {
        let loaded = self.loaded(1).await?;
        let dir = dir.to_path_buf();
        let count = audio.len();
        let (files, audio_s) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut recording = Recording::create(&dir, "final", count)?;
            let mut longest = 0;
            for (i, pcm) in audio.iter().enumerate() {
                let pcm: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
                longest = longest.max(pcm.len());
                recording.write(i, &pcm)?;
            }
            Ok((recording.finish()?, longest as f64 / SAMPLE_RATE as f64))
        })
        .await??;
        let id = loaded.finals.start(audio_s)?;
        loaded
            .finalizer
            .send(FinalJob::Whole {
                id: id.clone(),
                channels: channels.into_iter().zip(files).collect(),
                polish,
            })
            .context("the finalizer has stopped")?;
        loop {
            match loaded.finals.get(&id) {
                Some(FinalState::Running { .. }) => tokio::time::sleep(POLL).await,
                Some(FinalState::Done {
                    utterances,
                    warnings,
                }) => {
                    return Ok(Finalized {
                        utterances,
                        warnings,
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

fn start(config: Config, sessions: usize) -> Result<Loaded> {
    let (models, library) =
        Mirror::new().locate(config.models, |message| eprintln!("{message}"))?;
    eprintln!("loading the models…");
    let options = Options::defaults(&models, library, sessions.max(1));
    let engine = Arc::new(Engine::load(&options)?);
    let finals = Arc::new(Finals::default());
    let polisher = match (config.polish_url, config.polish_model) {
        (Some(url), Some(model)) => Some(polisher::spawn(
            Polisher::new(polish::Options {
                url,
                model,
                key: config.polish_key,
                timeout: Duration::from_secs(polish::TIMEOUT),
            }),
            finals.clone(),
            1,
        )),
        _ => None,
    };
    Ok(Loaded {
        batcher: batcher::spawn(engine.asr.clone(), logbook_engine::MAX_BATCH),
        finalizer: finalizer::spawn(engine.clone(), finals.clone(), polisher.clone()),
        finals,
        polisher,
        engine,
    })
}
