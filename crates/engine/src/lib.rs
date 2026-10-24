//! Logbook's engine: libparakeet, the per-channel pipeline and the wire
//! protocol. The server and the client drive it; neither holds pipeline
//! logic of its own.

pub mod channel;
pub mod parakeet;
pub mod protocol;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use channel::Channel;
use parakeet::{Kind, Latency, Model, Parakeet};
use protocol::ChannelConfig;

pub struct Options {
    /// libparakeet; None for the loader's search path.
    pub library: Option<PathBuf>,
    pub asr: PathBuf,
    pub vad: PathBuf,
    pub diarization: PathBuf,
    /// Copies of the diarization model. A context runs one stream call at a
    /// time; sessions are spread over the copies.
    pub diarization_contexts: usize,
    pub latency: Latency,
    /// Seconds of silence that close a phrase: shorter gives text sooner and
    /// shorter phrases, which the model reads with less context.
    pub min_pause: f64,
}

/// The models, loaded once and shared by every session.
pub struct Engine {
    pub asr: Arc<Model>,
    vad: Arc<Model>,
    diarization: Vec<Arc<Model>>,
    next: AtomicUsize,
    latency: Latency,
    vad_options: String,
}

impl Engine {
    pub fn load(options: &Options) -> Result<Self> {
        let lib = Parakeet::open(options.library.as_deref())?;
        let asr = Model::load(&lib, &options.asr, Kind::Asr)?;
        let vad = Model::load(&lib, &options.vad, Kind::Vad)?;
        let diarization = (0..options.diarization_contexts.max(1))
            .map(|_| Model::load(&lib, &options.diarization, Kind::Diarization))
            .collect::<Result<_>>()?;
        let vad_options = serde_json::json!({
            "threshold": 0.5,
            "min_pause": options.min_pause,
            "min_speech": 0.25,
            "speech_pad": 0.1,
        })
        .to_string();
        Ok(Self {
            asr,
            vad,
            diarization,
            next: AtomicUsize::new(0),
            latency: options.latency,
            vad_options,
        })
    }

    /// A channel with its own VAD stream and, if diarized, its own
    /// diarization stream on the next context in turn.
    pub fn channel(&self, index: usize, config: ChannelConfig) -> Result<Channel> {
        let vad = self.vad.vad_stream(&self.vad_options)?;
        let diar = if config.diarize {
            let n = self.next.fetch_add(1, Ordering::Relaxed) % self.diarization.len();
            Some(self.diarization[n].diar_stream(self.latency)?)
        } else {
            None
        };
        Ok(Channel::new(index, config, vad, diar))
    }
}
