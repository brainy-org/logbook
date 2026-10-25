//! logbook-server: live transcription and diarization over a WebSocket
//! (`/live`), final passes over HTTP (`/finals`): docs/protocol.md. The
//! models are loaded once and stay loaded; every session shares them.

mod batcher;
mod finalizer;
mod finals;
mod polisher;
mod recording;
mod session;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::codec;
use logbook_engine::mirror::{self, Mirror};
use logbook_engine::parakeet::Latency;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::polish::{self, Polisher};
use logbook_engine::protocol::{
    ClientMessage, FinalState, ServerMessage, Started, Upload, decode_audio,
};
use logbook_engine::{Engine, Options};
use tracing::{info, warn};

use crate::finalizer::FinalJob;
use crate::finals::Finals;
use crate::recording::Recording;
use crate::session::{Input, Shared};

/// The speaker encoder (WeSpeaker ResNet34-LM, the mirror's
/// `parakeet-voice` bundle) that tells voices apart beyond the
/// diarization's eight.
const VOICE: &str = "wespeaker-resnet34-lm.gguf";

/// The largest upload: 14 MB an hour a channel (`codec`), so about 8 hours
/// of a meeting with two channels.
const MAX_UPLOAD: usize = 256 << 20;

#[derive(Parser)]
#[command(version, about = "Live transcription and diarization server")]
struct Args {
    #[arg(long, env = "LOGBOOK_LISTEN", default_value = "127.0.0.1:3300")]
    listen: String,
    /// libparakeet; by default the pinned release for this machine, fetched
    /// from the mirror into ~/.cache/brainy-org when missing.
    #[arg(long, env = "PARAKEET_LIB")]
    library: Option<PathBuf>,
    /// Folder of the GGUF files, used as it is; by default
    /// ~/.cache/brainy-org/parakeet-741158ae, where the pinned files
    /// missing are fetched from the mirror (the ones brainy-worker loads).
    #[arg(long, env = "LOGBOOK_MODELS")]
    models: Option<PathBuf>,
    /// A GitHub token with read:packages for the mirror on ghcr.io, needed
    /// only while something is fetched.
    #[arg(long, env = "LOGBOOK_REGISTRY_TOKEN", hide_env_values = true)]
    registry_token: Option<String>,
    /// The token's user.
    #[arg(long, env = "LOGBOOK_REGISTRY_USERNAME")]
    registry_username: Option<String>,
    #[arg(long, default_value = "tdt-0.6b-v3-q8_0.gguf")]
    asr: String,
    #[arg(long, default_value = "silero-vad-f16.gguf")]
    vad: String,
    #[arg(long, default_value = "nemotron-3-diarization-q8_0.gguf")]
    diarization: String,
    /// Copies of the diarization model (about 110 MB each); one runs one call
    /// at a time.
    #[arg(long, default_value_t = 2)]
    diarization_contexts: usize,
    #[arg(long, value_enum, default_value_t = LatencyArg::Low)]
    latency: LatencyArg,
    /// Seconds of silence that close a phrase.
    #[arg(long, default_value_t = 0.5)]
    min_pause: f64,
    /// Seconds of the audio before a phrase transcribed with it (then
    /// dropped), so that a short phrase is read in its language and
    /// sentence; 0 for none. Each clip grows by up to this much.
    #[arg(long, default_value_t = 3.0)]
    context: f64,
    /// The speaker encoder of the final pass: its GGUF, or a parakeet
    /// bundle with a "voice" component; by default
    /// `wespeaker-resnet34-lm.gguf` in the models folder.
    #[arg(long, env = "LOGBOOK_VOICE")]
    voice: Option<PathBuf>,
    /// Voice similarity above which two ranges' voices are one speaker.
    #[arg(long, default_value_t = 0.45)]
    voice_threshold: f32,
    /// Phrases transcribed in one call at most.
    #[arg(long, default_value_t = 16)]
    max_batch: usize,
    /// Where final passes keep their audio until they have run (32 KB a
    /// second per channel); by default a folder in the system's
    /// temporary directory.
    #[arg(long, env = "LOGBOOK_RECORDINGS")]
    recordings: Option<PathBuf>,
    /// An OpenAI-compatible API, up to `/v1`, whose model cleans the final
    /// transcript up (fillers, false starts, punctuation):
    /// `http://localhost:11434/v1` for Ollama. The text goes there: a
    /// hosted one reads every meeting. None: the transcript as heard.
    #[arg(long, env = "LOGBOOK_POLISH_URL", requires = "polish_model")]
    polish_url: Option<String>,
    #[arg(long, env = "LOGBOOK_POLISH_MODEL")]
    polish_model: Option<String>,
    /// Its API key, if it asks for one.
    #[arg(long, env = "LOGBOOK_POLISH_KEY", hide_env_values = true)]
    polish_key: Option<String>,
    /// Parts the model is asked to clean at once (one per session at most).
    #[arg(long, default_value_t = 4)]
    polish_parallel: usize,
    /// Seconds a request may take; a part that takes longer stays as heard.
    #[arg(long, default_value_t = 120)]
    polish_timeout: u64,
}

#[derive(Clone, Copy, ValueEnum)]
enum LatencyArg {
    /// 1.04 s
    Low,
    /// 0.64 s
    VeryLow,
    /// 0.32 s
    UltraLow,
}

impl From<LatencyArg> for Latency {
    fn from(l: LatencyArg) -> Self {
        match l {
            LatencyArg::Low => Latency::Low,
            LatencyArg::VeryLow => Latency::VeryLow,
            LatencyArg::UltraLow => Latency::UltraLow,
        }
    }
}

struct AppState {
    shared: Shared,
    sessions: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    // What is not given comes from the mirror, fetched once into the cache.
    let mirror = Mirror::new(mirror::credentials(
        args.registry_username,
        args.registry_token,
    ));
    let say = |message: &str| info!("{message}");
    let models = match args.models {
        Some(dir) => dir,
        None => {
            let dir = mirror::cache_dir()?.join(mirror::MODELS_DIR);
            mirror.models(&dir, say)?;
            dir
        }
    };
    let library = match args.library {
        Some(path) => path,
        None => mirror.library(&mirror::cache_dir()?, say)?,
    };
    #[cfg(target_os = "linux")]
    beside_its_libraries(&library)?;
    let options = Options {
        library: Some(library),
        asr: models.join(&args.asr),
        vad: models.join(&args.vad),
        diarization: models.join(&args.diarization),
        diarization_contexts: args.diarization_contexts,
        latency: args.latency.into(),
        min_pause: args.min_pause,
        context: args.context,
        voice: args.voice.unwrap_or_else(|| models.join(VOICE)),
        voice_threshold: args.voice_threshold,
    };
    let recordings = args
        .recordings
        .unwrap_or_else(|| std::env::temp_dir().join("logbook-recordings"));
    // Held until the server exits.
    let (_lock, cleared) = recording::claim(&recordings)?;
    if cleared > 0 {
        info!(cleared, "deleted the recordings of a server that died");
    }
    let engine = Arc::new(Engine::load(&options)?);
    let finals = Arc::new(Finals::default());
    let polisher = match (args.polish_url, args.polish_model) {
        (Some(url), Some(model)) => {
            info!(url, model, "the final transcript is cleaned up");
            let polisher = Polisher::new(polish::Options {
                url,
                model,
                key: args.polish_key,
                timeout: Duration::from_secs(args.polish_timeout),
            });
            Some(polisher::spawn(
                polisher,
                finals.clone(),
                args.polish_parallel,
            ))
        }
        _ => None,
    };
    let shared = Shared {
        batcher: batcher::spawn(engine.asr.clone(), args.max_batch),
        finalizer: finalizer::spawn(engine.clone(), finals.clone(), polisher.clone()),
        finals,
        polisher,
        engine,
        recordings,
    };
    let state = Arc::new(AppState {
        shared,
        sessions: AtomicU64::new(0),
    });
    let app = Router::new()
        .route("/live", get(live))
        .route(
            "/finals",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
        )
        .route("/finals/{id}", get(final_state))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("listening on http://{}", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}

/// The CUDA build ships cudart and cuBLAS next to libparakeet, which the
/// loader finds only through LD_LIBRARY_PATH, read when the process
/// starts: the server starts again with that folder in it.
#[cfg(target_os = "linux")]
fn beside_its_libraries(library: &std::path::Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let Some(dir) = library.parent() else {
        return Ok(());
    };
    let others = std::fs::read_dir(dir)?.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.contains(".so") && !name.starts_with("libparakeet")
    });
    let path = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    if !others || std::env::split_paths(&path).any(|p| p == dir) {
        return Ok(());
    }
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&path));
    let error = std::process::Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .env("LD_LIBRARY_PATH", std::env::join_paths(paths)?)
        .exec();
    Err(error).context("could not start again with libparakeet's libraries")
}

/// A final pass over a meeting the client recorded itself: a `meeting`
/// part (the channels), then one `audio` part per channel (`codec`).
async fn upload(State(state): State<Arc<AppState>>, multipart: Multipart) -> Response {
    match start_upload(&state.shared, multipart).await {
        Ok(id) => (StatusCode::ACCEPTED, Json(Started { id })).into_response(),
        Err(error) => {
            warn!("upload refused: {error:#}");
            (StatusCode::BAD_REQUEST, format!("{error:#}")).into_response()
        }
    }
}

async fn start_upload(shared: &Shared, mut multipart: Multipart) -> Result<String> {
    let mut upload: Option<Upload> = None;
    let mut audio = Vec::new();
    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("meeting") => upload = Some(serde_json::from_slice(&field.bytes().await?)?),
            Some("audio") => audio.push(field.bytes().await?),
            _ => {}
        }
    }
    let upload = upload.context("no meeting part")?;
    if upload.channels.is_empty() || upload.channels.len() != audio.len() {
        anyhow::bail!(
            "{} channels and {} audio parts",
            upload.channels.len(),
            audio.len()
        );
    }
    // Decoding an hour takes about a second of CPU: off the async threads.
    let dir = shared.recordings.clone();
    let (files, audio_s) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut name = [0u8; 8];
        getrandom::fill(&mut name).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
        let name: String = name.iter().map(|b| format!("{b:02x}")).collect();
        let mut recording = Recording::create(&dir, &format!("upload-{name}"), audio.len())?;
        let mut longest = 0;
        for (i, data) in audio.iter().enumerate() {
            let pcm = codec::decode(data).with_context(|| format!("audio part {i}"))?;
            longest = longest.max(pcm.len());
            recording.write(i, &pcm)?;
        }
        Ok((recording.finish()?, longest as f64 / SAMPLE_RATE as f64))
    })
    .await??;
    let id = shared.finals.start(audio_s)?;
    info!(audio_s = format!("{audio_s:.1}"), "upload received");
    let _ = shared.finalizer.send(FinalJob::Whole {
        id: id.clone(),
        channels: upload.channels.into_iter().zip(files).collect(),
    });
    Ok(id)
}

async fn final_state(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<FinalState>, StatusCode> {
    state
        .shared
        .finals
        .get(&id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn live(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let (configs, final_pass) = loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
                Ok(ClientMessage::Start {
                    channels,
                    final_pass,
                }) => break (channels, final_pass),
                Ok(_) | Err(_) => {
                    warn!("the first message was not a start");
                    return;
                }
            },
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
            Some(Ok(_)) => {}
        }
    };
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Input>();
    let reply = in_tx.clone();
    let thread_state = state.clone();
    // Names the session in the logs and its recording's files.
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let session = format!(
        "{millis}-{}",
        state.sessions.fetch_add(1, Ordering::Relaxed)
    );
    std::thread::spawn(move || {
        session::run(
            &thread_state.shared,
            session,
            configs,
            final_pass,
            in_rx,
            reply,
            out_tx,
        );
    });

    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let last = matches!(message, ServerMessage::Done | ServerMessage::Error { .. });
            let json = serde_json::to_string(&message).expect("server messages serialize");
            if sink.send(Message::Text(json.into())).await.is_err() || last {
                break;
            }
        }
        let _ = sink.close().await;
    });

    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Binary(frame)) => {
                if let Some((channel, pcm)) = decode_audio(&frame) {
                    let _ = in_tx.send(Input::Audio(channel, pcm));
                }
            }
            Ok(Message::Text(text)) => {
                if let Ok(ClientMessage::Stop) = serde_json::from_str(text.as_str()) {
                    let _ = in_tx.send(Input::Stop);
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = in_tx.send(Input::Closed);
    let _ = writer.await;
}
