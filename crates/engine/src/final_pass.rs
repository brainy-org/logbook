//! The final pass over a whole recording at once: a meeting a client
//! recorded itself and uploaded, or kept and reads again here
//! (docs/pipeline.md, "Final pass"). The same steps as the progressive
//! pass of a live session (progressive.rs): each channel is read once, a
//! part at a time, to find its pauses (Silero VAD), then range by range,
//! as a live session would have. Nothing holds a whole channel: a meeting
//! of days takes the memory of one of minutes.

use anyhow::Result;

use crate::parakeet::SAMPLE_RATE;
use crate::progressive::{self, Progressive, RangeModels};
use crate::protocol::{ChannelConfig, SpeakerVoice, Utterance};
use crate::recording::RangeReader;

/// Pieces the VAD cuts a channel into, to find where ranges may end.
const PIECE: f64 = 30.0;
/// Audio read at once to find the pauses: 10 minutes, 38 MB as floats.
const PART: f64 = 600.0;
/// A pause the VAD reports this close to a part's end may be the part's
/// end, not a pause: it is not a place to cut.
const PART_EDGE: f64 = 0.5;

/// One recorded channel, read by range.
pub struct Recorded {
    pub config: ChannelConfig,
    pub reader: RangeReader,
    /// Where in the reader's audio the pass starts, in seconds.
    pub start: f64,
    /// Seconds of audio from `start`.
    pub seconds: f64,
}

pub struct Final {
    /// Every channel's segments, by start time.
    pub utterances: Vec<Utterance>,
    /// What was left out, said as observed.
    pub warnings: Vec<String>,
    /// The diarized channels' speakers' voices.
    pub voices: Vec<SpeakerVoice>,
    /// Where it starts on the meeting's clock: every segment from there
    /// on is in it.
    pub from: f64,
}

/// `offset`: where on the meeting's clock the channels' `start` is (the
/// times given back are on it); `voices`: the meeting's speakers before
/// it, whose numbers are kept.
pub fn run(
    models: &RangeModels,
    threshold: f32,
    channels: Vec<Recorded>,
    offset: f64,
    voices: &[SpeakerVoice],
) -> Result<Final> {
    let configs: Vec<ChannelConfig> = channels.iter().map(|r| r.config.clone()).collect();
    let mut pass = Progressive::new(configs, threshold, voices)?;
    let part = (PART * SAMPLE_RATE as f64) as usize;
    for (i, recorded) in channels.iter().enumerate() {
        let mut at = 0.0;
        while at < recorded.seconds {
            let to = (at + PART).min(recorded.seconds);
            let pcm = recorded
                .reader
                .read(recorded.start + at, recorded.start + to)?;
            if pcm.is_empty() {
                break;
            }
            pass.feed(i, &pcm);
            let last = pcm.len() < part;
            for (_, end) in models.vad.vad_segments(&pcm, PIECE)? {
                if last || end < to - at - PART_EDGE {
                    pass.quiet(i, at + end);
                }
            }
            at += pcm.len() as f64 / SAMPLE_RATE as f64;
        }
    }
    pass.finish();
    let mut utterances = Vec::new();
    loop {
        let ranges = pass.next_ranges();
        let idle = ranges.is_empty();
        for range in ranges {
            let recorded = &channels[range.channel];
            let pcm = recorded
                .reader
                .read(recorded.start + range.from, recorded.start + range.to)?;
            let result = progressive::transcribe_range(models, &pcm, &range)?;
            pass.transcribed(&range, result);
        }
        while let Some((_, ready)) = pass.ready() {
            utterances.extend(ready.into_iter().map(|u| u.shifted(offset)));
        }
        if pass.is_done() || idle {
            break;
        }
    }
    Ok(Final {
        utterances,
        warnings: Vec::new(),
        voices: pass.voices(),
        from: offset,
    })
}
