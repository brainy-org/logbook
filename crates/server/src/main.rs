//! logbook-server: live transcription and diarization over a WebSocket
//! (`/live`, docs/protocol.md). The models are loaded once and stay loaded;
//! every session shares them.

mod batcher;
mod session;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::parakeet::Latency;
use logbook_engine::protocol::{ClientMessage, ServerMessage, decode_audio};
use logbook_engine::{Engine, Options};
use tracing::{info, warn};

use crate::batcher::Job;
use crate::session::Input;

/// The revision of mudler/parakeet-cpp-gguf brainy-worker pins
/// (`audio_revision`): the same files, already in the Hugging Face cache.
const MODELS_REVISION: &str = "741158ae71e64ef5c89385862c18f777d07a97a1";

#[derive(Parser)]
#[command(version, about = "Live transcription and diarization server")]
struct Args {
    #[arg(long, env = "LOGBOOK_LISTEN", default_value = "127.0.0.1:3300")]
    listen: String,
    /// libparakeet; by default the loader's search path.
    #[arg(long, env = "PARAKEET_LIB")]
    library: Option<PathBuf>,
    /// Folder of the GGUF files; by default brainy-worker's pinned revision in
    /// the Hugging Face cache.
    #[arg(long, env = "LOGBOOK_MODELS")]
    models: Option<PathBuf>,
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
    /// Phrases transcribed in one call at most.
    #[arg(long, default_value_t = 16)]
    max_batch: usize,
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
    engine: Engine,
    batcher: Sender<Job>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();
    let models = match args.models {
        Some(dir) => dir,
        None => {
            let home = std::env::var("HOME").context("HOME is not set: pass --models")?;
            PathBuf::from(home)
                .join(".cache/huggingface/hub/models--mudler--parakeet-cpp-gguf/snapshots")
                .join(MODELS_REVISION)
        }
    };
    let options = Options {
        library: args.library,
        asr: models.join(&args.asr),
        vad: models.join(&args.vad),
        diarization: models.join(&args.diarization),
        diarization_contexts: args.diarization_contexts,
        latency: args.latency.into(),
        min_pause: args.min_pause,
    };
    let engine = Engine::load(&options)?;
    let batcher = batcher::spawn(engine.asr.clone(), args.max_batch);
    let state = Arc::new(AppState { engine, batcher });
    let app = Router::new().route("/live", get(live)).with_state(state);
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("listening on ws://{}/live", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn live(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let configs = loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
                Ok(ClientMessage::Start { channels }) => break channels,
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
    std::thread::spawn(move || {
        session::run(
            &thread_state.engine,
            &thread_state.batcher,
            configs,
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
