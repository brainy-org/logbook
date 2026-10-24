//! The final pass over a whole recording (docs/pipeline.md, "Final pass").
//!
//! The live transcript is made of phrases cut as they were spoken, and of
//! speakers told apart a second at a time. Once the meeting is over, each
//! channel is read again the way brainy-worker reads a recording: cut at
//! pauses into pieces of at most 30 s, transcribed in batches, diarized in
//! one call over the whole recording (the model sees all the voices before
//! it names them), and grouped into segments with brainy-worker's rules.
//! Then the mic's echo of the call is dropped, as live.

use anyhow::Result;

use crate::channel::speaker_of;
use crate::echo::EchoFilter;
use crate::parakeet::{Model, SAMPLE_RATE, Word};
use crate::protocol::{ChannelConfig, Utterance};

/// Pieces of speech per call to the model, as brainy-worker.
const BATCH_PIECES: usize = 8;
/// The longest piece the model reads, which it was trained on.
const MAX_PIECE: f64 = 30.0;
// How words become segments: brainy-worker's MAX_SEGMENT_MS,
// SENTENCE_AFTER_MS and PAUSE_MS (extract/audio.py).
const MAX_SEGMENT: f64 = 30.0;
const SENTENCE_AFTER: f64 = 8.0;
const PAUSE: f64 = 1.2;

/// One recorded channel; its audio is read only when its turn comes, so
/// that one channel at a time is in memory.
pub struct Recorded {
    pub config: ChannelConfig,
    pub audio: Box<dyn FnOnce() -> Result<Vec<f32>> + Send>,
}

pub struct Final {
    /// Every channel's segments, by start time.
    pub utterances: Vec<Utterance>,
    /// What was left out, said as observed ("speakers were not told apart
    /// on system: ...").
    pub warnings: Vec<String>,
}

pub struct Models<'a> {
    pub asr: &'a Model,
    pub vad: &'a Model,
    /// A diarization context no live stream uses: one call holds it for
    /// the whole recording.
    pub diarization: &'a Model,
    /// Longer recordings are not diarized (one call reads the whole of it).
    pub diarize_max_seconds: f64,
}

pub fn run(models: &Models, channels: Vec<Recorded>) -> Result<Final> {
    let mut warnings = Vec::new();
    let mut per_channel: Vec<(ChannelConfig, Vec<Utterance>)> = Vec::new();
    for recorded in channels {
        let config = recorded.config;
        let pcm = (recorded.audio)()?;
        let words = transcribe(models, &pcm)?;
        let duration = pcm.len() as f64 / SAMPLE_RATE as f64;
        let speakers = if !config.diarize || words.is_empty() {
            vec![None; words.len()]
        } else if duration > models.diarize_max_seconds {
            warnings.push(format!(
                "speakers were not told apart on {}: the recording is longer than {} s",
                config.name, models.diarize_max_seconds
            ));
            vec![None; words.len()]
        } else {
            match models.diarization.diarize(&pcm) {
                Ok(turns) => words
                    .iter()
                    .map(|w| speaker_of(w.start, w.end, &turns))
                    .collect(),
                Err(error) => {
                    warnings.push(format!(
                        "speakers were not told apart on {}: {error:#}",
                        config.name
                    ));
                    vec![None; words.len()]
                }
            }
        };
        drop(pcm);
        let segments = segments(&config.name, words, &speakers);
        per_channel.push((config, segments));
    }

    let mut utterances = Vec::new();
    for (config, segments) in &per_channel {
        let source = config
            .echo_of
            .as_ref()
            .and_then(|name| per_channel.iter().find(|(c, _)| &c.name == name));
        match source {
            None => utterances.extend(segments.iter().cloned()),
            Some((_, heard)) => {
                let mut filter = EchoFilter::default();
                for u in heard {
                    filter.heard(u);
                }
                for u in segments {
                    filter.hold(u.clone());
                }
                utterances.extend(filter.release(f64::INFINITY));
            }
        }
    }
    utterances.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(Final {
        utterances,
        warnings,
    })
}

/// The words of a whole channel, with times from its start.
fn transcribe(models: &Models, pcm: &[f32]) -> Result<Vec<Word>> {
    let rate = SAMPLE_RATE as f64;
    let pieces = models.vad.vad_segments(pcm, MAX_PIECE)?;
    let mut words = Vec::new();
    for batch in pieces.chunks(BATCH_PIECES) {
        let clips: Vec<&[f32]> = batch
            .iter()
            .map(|&(start, end)| {
                let from = ((start * rate) as usize).min(pcm.len());
                let to = ((end * rate) as usize).clamp(from, pcm.len());
                &pcm[from..to]
            })
            .collect();
        for (&(start, _), transcript) in batch.iter().zip(models.asr.transcribe(&clips)?) {
            words.extend(transcript.words.into_iter().map(|w| Word {
                start: w.start + start,
                end: w.end + start,
                ..w
            }));
        }
    }
    Ok(words)
}

/// Words grouped into segments: a new one at a change of speaker, after a
/// pause, at a sentence end once the segment is long enough, or at the cap.
/// brainy-worker's `to_segments`.
fn segments(channel: &str, words: Vec<Word>, speakers: &[Option<u32>]) -> Vec<Utterance> {
    let mut out: Vec<Utterance> = Vec::new();
    for (word, &speaker) in words.into_iter().zip(speakers) {
        let breaks = match out.last() {
            None => true,
            Some(u) => {
                let last = &u.words[u.words.len() - 1];
                u.speaker != speaker
                    || word.start - last.end >= PAUSE
                    || word.end - u.start > MAX_SEGMENT
                    || (last.end - u.start >= SENTENCE_AFTER && ends_sentence(&last.w))
            }
        };
        if breaks {
            out.push(Utterance {
                channel: channel.to_string(),
                speaker,
                start: word.start,
                end: word.end,
                text: word.w.clone(),
                words: vec![word],
            });
        } else {
            let u = out.last_mut().unwrap();
            u.end = word.end;
            u.text.push(' ');
            u.text.push_str(&word.w);
            u.words.push(word);
        }
    }
    out
}

/// brainy-worker's SENTENCE_END: `[.?!…]` followed by closing quotes or
/// brackets at most.
fn ends_sentence(word: &str) -> bool {
    let trimmed = word.trim_end_matches(['\'', '"', '»', '”', ')']);
    trimmed.ends_with(['.', '?', '!', '…'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str, start: f64, end: f64) -> Word {
        Word {
            w: w.into(),
            start,
            end,
            conf: 1.0,
        }
    }

    #[test]
    fn segments_break_at_a_pause_and_at_a_change_of_speaker() {
        let words = vec![
            word("Buongiorno.", 0.0, 0.6),
            word("Oggi", 2.0, 2.3),
            word("parliamo", 2.4, 2.9),
            word("Grazie.", 3.1, 3.5),
        ];
        let out = segments("system", words, &[Some(0), Some(0), Some(0), Some(1)]);
        let texts: Vec<&str> = out.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, vec!["Buongiorno.", "Oggi parliamo", "Grazie."]);
    }

    #[test]
    fn a_long_segment_breaks_at_a_sentence_end() {
        let mut words: Vec<Word> = (0..20)
            .map(|i| word("parola", i as f64 * 0.5, i as f64 * 0.5 + 0.4))
            .collect();
        words[17].w = "fine.".into();
        let out = segments("mic", words, &[None; 20]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].words.len(), 18);
    }

    #[test]
    fn sentence_ends_allow_closing_quotes() {
        assert!(ends_sentence("detto.»"));
        assert!(ends_sentence("vero?\""));
        assert!(!ends_sentence("parola"));
    }
}
