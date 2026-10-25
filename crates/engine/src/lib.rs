//! Logbook's engine: libparakeet, the per-channel pipeline and the wire
//! protocol. The server and the client drive it; neither holds pipeline
//! logic of its own.

pub mod channel;
pub mod codec;
pub mod echo;
pub mod final_pass;
pub mod parakeet;
pub mod polish;
pub mod progressive;
pub mod protocol;
pub mod speakers;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use channel::Channel;
use final_pass::{Final, Recorded};
use parakeet::{Kind, Latency, Model, Parakeet};
use progressive::{Progressive, Transcribed};
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
    /// Seconds of the audio before a phrase transcribed with it and then
    /// dropped: the model picks the language per clip, and a short phrase
    /// alone ("Ma va là.") comes out in another one ("Mais voilà.").
    pub context: f64,
    /// The speaker encoder (WeSpeaker; its own file or a bundle's "voice"
    /// component): the final pass's ranges are diarized alone, and only
    /// voices tell which of a range's speakers is which of the meeting's.
    pub voice: PathBuf,
    /// The voice similarity above which two ranges' voices are one speaker.
    pub voice_threshold: f32,
}

/// The shortest pause the VAD reports.
const VAD_MIN_PAUSE: f64 = 0.15;

/// The models, loaded once and shared by every session.
pub struct Engine {
    pub asr: Arc<Model>,
    vad: Arc<Model>,
    diarization: Vec<Arc<Model>>,
    /// The final passes' diarization context, for whole ranges
    /// (progressive.rs): the live streams' contexts are kept for low latency.
    final_diarization: Arc<Model>,
    next: AtomicUsize,
    latency: Latency,
    vad_options: String,
    min_pause: f64,
    context: f64,
    voice: Arc<Model>,
    voice_threshold: f32,
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
        // A file of its own, or a bundle's component.
        let voice = Model::load(&lib, &options.voice, Kind::Speaker).or_else(|_| {
            Model::load_component(&lib, &options.voice, Some("voice"), Kind::Speaker)
        })?;
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
            next: AtomicUsize::new(0),
            latency: options.latency,
            vad_options,
            min_pause: options.min_pause,
            context: options.context,
            voice,
            voice_threshold: options.voice_threshold,
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
        Ok(Channel::new(
            index,
            config,
            vad,
            diar,
            self.min_pause,
            self.context,
        ))
    }

    /// A progressive final pass for a live session's channels.
    pub fn progressive(&self, configs: Vec<ChannelConfig>) -> Result<Progressive> {
        Progressive::new(configs, self.voice_threshold)
    }

    /// A range of a channel read again (progressive.rs): `pcm` is its
    /// audio. Long: run it off the sessions' threads, one at a time (it
    /// holds the final diarization context).
    pub fn final_range(&self, pcm: &[f32], range: &progressive::Range) -> Result<Transcribed> {
        progressive::transcribe_range(&self.range_models(), pcm, range)
    }

    /// The final pass over recorded channels (final_pass.rs). Long: run it
    /// off the sessions' threads.
    pub fn final_pass(&self, channels: Vec<Recorded>) -> Result<Final> {
        final_pass::run(&self.range_models(), self.voice_threshold, channels)
    }

    fn range_models(&self) -> progressive::RangeModels<'_> {
        progressive::RangeModels {
            asr: &self.asr,
            vad: &self.vad,
            diarization: &self.final_diarization,
            voice: &self.voice,
        }
    }
}
