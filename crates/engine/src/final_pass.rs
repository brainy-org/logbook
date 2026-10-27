//! The final pass over a whole recording at once: a meeting a client
//! recorded itself and uploaded (docs/pipeline.md, "Final pass"). The same
//! steps as the progressive pass of a live session (progressive.rs): each
//! channel is read once to find its pauses (Silero VAD), then range by
//! range, as a live session would have.

use anyhow::Result;

use crate::progressive::{self, Progressive, RangeModels};
use crate::protocol::{ChannelConfig, SpeakerVoice, Utterance};

/// Pieces the VAD cuts a channel into, to find where ranges may end.
const PIECE: f64 = 30.0;

/// One recorded channel. Its audio is read whole once (one channel at a
/// time in memory), then by range.
pub struct Recorded {
    pub config: ChannelConfig,
    pub audio: Box<dyn Fn() -> Result<Vec<f32>> + Send>,
    /// Samples [from, to) in seconds.
    pub range: Box<dyn Fn(f64, f64) -> Result<Vec<f32>> + Send>,
}

pub struct Final {
    /// Every channel's segments, by start time.
    pub utterances: Vec<Utterance>,
    /// What was left out, said as observed.
    pub warnings: Vec<String>,
    /// The diarized channels' speakers' voices.
    pub voices: Vec<SpeakerVoice>,
}

pub fn run(models: &RangeModels, threshold: f32, channels: Vec<Recorded>) -> Result<Final> {
    let configs: Vec<ChannelConfig> = channels.iter().map(|r| r.config.clone()).collect();
    let mut pass = Progressive::new(configs, threshold)?;
    for (i, recorded) in channels.iter().enumerate() {
        let pcm = (recorded.audio)()?;
        pass.feed(i, &pcm);
        for (_, end) in models.vad.vad_segments(&pcm, PIECE)? {
            pass.quiet(i, end);
        }
    }
    pass.finish();
    let mut utterances = Vec::new();
    loop {
        let ranges = pass.next_ranges();
        let idle = ranges.is_empty();
        for range in ranges {
            let pcm = (channels[range.channel].range)(range.from, range.to)?;
            let result = progressive::transcribe_range(models, &pcm, &range)?;
            pass.transcribed(&range, result);
        }
        while let Some((_, ready)) = pass.ready() {
            utterances.extend(ready);
        }
        if pass.is_done() || idle {
            break;
        }
    }
    Ok(Final {
        utterances,
        warnings: Vec::new(),
        voices: pass.voices(),
    })
}
