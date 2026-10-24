//! Logbook's engine: libparakeet, the per-channel pipeline and the wire
//! protocol. The server and the client drive it; neither holds pipeline
//! logic of its own.

pub mod channel;
pub mod echo;
pub mod final_pass;
pub mod parakeet;
pub mod protocol;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use channel::Channel;
use final_pass::{Final, Models, Recorded};
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
    /// Recordings longer than this are not diarized in the final pass.
    pub diarize_max_seconds: f64,
}

/// The shortest pause the VAD reports.
const VAD_MIN_PAUSE: f64 = 0.15;

/// The models, loaded once and shared by every session.
pub struct Engine {
    pub asr: Arc<Model>,
    vad: Arc<Model>,
    diarization: Vec<Arc<Model>>,
    /// The final pass's own copy: one call holds it for a whole recording.
    final_diarization: Arc<Model>,
    diarize_max_seconds: f64,
    next: AtomicUsize,
    latency: Latency,
    vad_options: String,
    min_pause: f64,
}

impl Engine {
    pub fn load(options: &Options) -> Result<Self> {
        let lib = Parakeet::open(options.library.as_deref())?;
        let asr = Model::load(&lib, &options.asr, Kind::Asr)?;
        let vad = Model::load(&lib, &options.vad, Kind::Vad)?;
        let diarization = (0..options.diarization_contexts.max(1))
            .map(|_| Model::load(&lib, &options.diarization, Kind::Diarization))
            .collect::<Result<_>>()?;
        let final_diarization = Model::load(&lib, &options.diarization, Kind::Diarization)?;
        // The VAD reports every pause from VAD_MIN_PAUSE on; which of them
        // close a phrase is the channel's choice (`Phrasing`). speech_pad
        // must stay at most min_pause / 2 in a stream.
        let vad_options = serde_json::json!({
            "threshold": 0.5,
            "min_pause": VAD_MIN_PAUSE,
            "min_speech": 0.25,
            "speech_pad": 0.05,
        })
        .to_string();
        Ok(Self {
            asr,
            vad,
            diarization,
            final_diarization,
            diarize_max_seconds: options.diarize_max_seconds,
            next: AtomicUsize::new(0),
            latency: options.latency,
            vad_options,
            min_pause: options.min_pause,
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
        Ok(Channel::new(index, config, vad, diar, self.min_pause))
    }

    /// The final pass over recorded channels (final_pass.rs). Long: run it
    /// off the sessions' threads.
    pub fn final_pass(&self, channels: Vec<Recorded>) -> Result<Final> {
        let models = Models {
            asr: &self.asr,
            vad: &self.vad,
            diarization: &self.final_diarization,
            diarize_max_seconds: self.diarize_max_seconds,
        };
        final_pass::run(&models, channels)
    }
}
