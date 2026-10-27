//! logbook-server: live transcription and diarization over a WebSocket
//! (`/live`), final passes over HTTP (`/finals`), and with a language
//! model the meeting's report (`/reports`): docs/protocol.md. The
//! models are loaded once and stay loaded; every session shares them.
//! Each of those routes needs a person's token, made on the admin page
//! (`/`, auth.rs).

mod auth;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Extension, Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::codec;
use logbook_engine::finalizer::{self, FinalJob};
use logbook_engine::finals::Finals;
use logbook_engine::mirror::Mirror;
use logbook_engine::parakeet::Latency;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::polish::{self, Polisher};
use logbook_engine::protocol::{
    ClientMessage, FinalState, Report, ReportRequest, ServerMessage, Started, Upload, decode_audio,
};
use logbook_engine::recording::{self, Recording};
use logbook_engine::report;
use logbook_engine::session::{self, Input, Shared};
use logbook_engine::{Engine, Options, batcher, polisher};
use tracing::{info, warn};

/// The largest upload: 14 MB an hour a channel (`codec`), so about 8 hours
/// of a meeting with two channels.
const MAX_UPLOAD: usize = 256 << 20;
/// The largest report request: the transcript and the notes at their
/// limits (engine report.rs), in UTF-8.
const MAX_REPORT: usize = 4 << 20;

#[derive(Parser)]
#[command(version, about = "Live transcription and diarization server")]
struct Args {
    #[arg(long, env = "LOGBOOK_LISTEN", default_value = "127.0.0.1:3300")]
    listen: String,
    /// Folder of the GGUF files, used as it is; by default
    /// ~/.cache/brainy-org/parakeet-741158ae, where the pinned files
    /// missing are fetched from the mirror (the ones brainy-worker loads).
    #[arg(long, env = "LOGBOOK_MODELS")]
    models: Option<PathBuf>,
    #[arg(long, default_value = logbook_engine::ASR)]
    asr: String,
    #[arg(long, default_value = logbook_engine::VAD)]
    vad: String,
    #[arg(long, default_value = logbook_engine::DIARIZATION)]
    diarization: String,
    /// Copies of the diarization model (about 110 MB each); one runs one call
    /// at a time.
    #[arg(long, default_value_t = 2)]
    diarization_contexts: usize,
    #[arg(long, value_enum, default_value_t = LatencyArg::Low)]
    latency: LatencyArg,
    /// Seconds of silence that close a phrase.
    #[arg(long, default_value_t = logbook_engine::MIN_PAUSE)]
    min_pause: f64,
    /// Seconds of the audio before a phrase transcribed with it (then
    /// dropped), so that a short phrase is read in its language and
    /// sentence; 0 for none. Each clip grows by up to this much.
    #[arg(long, default_value_t = logbook_engine::CONTEXT)]
    context: f64,
    /// The speaker encoder of the final pass: its GGUF, or a parakeet
    /// bundle with a "voice" component; by default
    /// `wespeaker-resnet34-lm.gguf` in the models folder.
    #[arg(long, env = "LOGBOOK_VOICE")]
    voice: Option<PathBuf>,
    /// Voice similarity above which two ranges' voices are one speaker.
    #[arg(long, default_value_t = logbook_engine::VOICE_THRESHOLD)]
    voice_threshold: f32,
    /// Phrases transcribed in one call at most.
    #[arg(long, default_value_t = logbook_engine::MAX_BATCH)]
    max_batch: usize,
    /// Where final passes keep their audio until they have run (32 KB a
    /// second per channel); by default a folder in the system's
    /// temporary directory.
    #[arg(long, env = "LOGBOOK_RECORDINGS")]
    recordings: Option<PathBuf>,
    /// Where the server keeps the people's tokens (their hashes) and its
    /// admin token; by default `logbook-server` in the user's data folder.
    #[arg(long, env = "LOGBOOK_DATA")]
    data: Option<PathBuf>,
    /// The token of the admin page (`/`), where the people's tokens
    /// are made and revoked. None: the one in the data folder's
    /// `admin.token`, made at first start.
    #[arg(long, env = "LOGBOOK_ADMIN_TOKEN", hide_env_values = true)]
    admin_token: Option<String>,
    /// Let anyone who reaches the server use it, without a token: for a
    /// server only this computer reaches (tests, `logbook bench`).
    #[arg(long, env = "LOGBOOK_NO_AUTH")]
    no_auth: bool,
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
    #[arg(long, default_value_t = polish::TIMEOUT)]
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
    /// The language model's reports, with the clean-up's model.
    reporter: Option<Reporter>,
}

struct Reporter {
    agent: polish::Agent,
    options: polish::Options,
    /// Reports asked at once: each holds a blocking thread for a minute
    /// or so, and the model's provider limits requests.
    slots: tokio::sync::Semaphore,
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
    let (models, library) = Mirror::new().locate(args.models, |message| info!("{message}"))?;
    let options = Options {
        library: Some(library),
        asr: models.join(&args.asr),
        vad: models.join(&args.vad),
        diarization: models.join(&args.diarization),
        diarization_contexts: args.diarization_contexts,
        latency: args.latency.into(),
        min_pause: args.min_pause,
        context: args.context,
        voice: args
            .voice
            .unwrap_or_else(|| models.join(logbook_engine::VOICE)),
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
    let data = match args.data {
        Some(data) => data,
        None => dirs::data_dir()
            .context("no data folder for this user: --data")?
            .join("logbook-server"),
    };
    let auth = Arc::new(auth::Auth::open(&data, args.admin_token, !args.no_auth)?);
    if args.no_auth {
        warn!("--no-auth: anyone who reaches the server can use it");
    } else {
        info!(tokens = auth.count(), "a token is needed for every meeting");
    }
    let engine = Arc::new(Engine::load(&options)?);
    let finals = Arc::new(Finals::default());
    let model = match (args.polish_url, args.polish_model) {
        (Some(url), Some(model)) => {
            info!(
                url,
                model, "the final transcript is cleaned up and reported on"
            );
            Some(polish::Options {
                url,
                model,
                key: args.polish_key,
                timeout: Duration::from_secs(args.polish_timeout),
            })
        }
        _ => None,
    };
    let polisher = model.clone().map(|options| {
        polisher::spawn(Polisher::new(options), finals.clone(), args.polish_parallel)
    });
    let reporter = model.map(|options| Reporter {
        agent: polish::agent(&options),
        options,
        slots: tokio::sync::Semaphore::new(args.polish_parallel.max(1)),
    });
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
        reporter,
    });
    // Every route of a meeting is in `meetings`, behind a person's token:
    // one added outside it is open to anyone who reaches the server.
    let meetings = Router::new()
        .route("/live", get(live))
        .route(
            "/finals",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
        )
        .route("/finals/{id}", get(final_state))
        .route(
            "/reports",
            post(write_report).layer(DefaultBodyLimit::max(MAX_REPORT)),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            auth::require_token,
        ))
        .with_state(state);
    let admin = Router::new()
        .route("/admin/api/tokens", get(auth::list).post(auth::create))
        .route("/admin/api/tokens/{id}", delete(auth::revoke))
        .route_layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            auth::require_admin,
        ))
        .route("/", get(auth::page))
        .route("/assets/geist.woff2", get(auth::font))
        .with_state(auth.clone());
    let app = meetings.merge(admin);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("listening on http://{}", args.listen);
    match &auth.admin_file {
        Some(file) => info!(
            "tokens are made at http://{}/ with the admin token in {}",
            args.listen,
            file.display()
        ),
        None => info!(
            "tokens are made at http://{}/ with the admin token (--admin-token)",
            args.listen
        ),
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// A final pass over a meeting the client recorded itself: a `meeting`
/// part (the channels), then one `audio` part per channel (`codec`).
async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<auth::Caller>,
    multipart: Multipart,
) -> Response {
    info!(caller = caller.0, "upload");
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
        polish: upload.polish,
    });
    Ok(id)
}

/// The meeting's report by the server's model; 501 without one.
async fn write_report(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<auth::Caller>,
    Json(request): Json<ReportRequest>,
) -> Response {
    info!(caller = caller.0, "report");
    let Some(reporter) = &state.reporter else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "this server has no language model (--polish-url)",
        )
            .into_response();
    };
    let Ok(_slot) = reporter.slots.acquire().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let state = state.clone();
    let written = tokio::task::spawn_blocking(move || {
        let reporter = state.reporter.as_ref().expect("checked above");
        report::write(&reporter.agent, &reporter.options, &request)
    })
    .await;
    match written {
        Ok(Ok(markdown)) => Json(Report { markdown }).into_response(),
        Ok(Err(error)) => {
            warn!("report failed: {error:#}");
            (StatusCode::BAD_GATEWAY, format!("{error:#}")).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
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

async fn live(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<auth::Caller>,
) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state, caller))
}

async fn handle(socket: WebSocket, state: Arc<AppState>, caller: auth::Caller) {
    let (mut sink, mut stream) = socket.split();
    let (configs, final_pass, polish) = loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
                Ok(ClientMessage::Start {
                    channels,
                    final_pass,
                    polish,
                }) => break (channels, final_pass, polish),
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
    info!(session, caller = caller.0, "live session");
    std::thread::spawn(move || {
        session::run(
            &thread_state.shared,
            session,
            configs,
            final_pass.then_some(polish),
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
