//! One audio channel of a live session (docs/pipeline.md).
//!
//! The VAD stream closes a phrase at a pause; the phrase becomes a clip for
//! the transcriber, which runs elsewhere, in batches across sessions. When
//! its transcript comes back and diarization has caught up with the phrase's
//! end, each word goes to the speaker whose turns overlap it most, and the
//! phrase leaves as one utterance per run of words from one speaker. Phrases
//! leave in the order they were spoken.

use std::collections::VecDeque;

use anyhow::Result;

use crate::parakeet::{DiarStream, SAMPLE_RATE, Transcript, Turn, VadEdge, VadStream, Word};
use crate::protocol::{ChannelConfig, Utterance};

/// A phrase longer than this is cut where it stands: the model was trained on
/// pieces of at most 30 s, and a listener waits for the whole phrase.
const MAX_PHRASE: f64 = 20.0;
/// Audio kept before the oldest point a clip may still start at: a VAD start
/// is known min_speech late, and is padded by speech_pad.
const KEEP: f64 = 2.0;
/// A word in no turn goes to the nearest turn up to this far (turns start a
/// little after their first word); further, it has no speaker.
const NEAREST_TURN: f64 = 1.0;
/// Turns ending this long before the oldest phrase still to place are dropped.
const TURN_MEMORY: f64 = 10.0;

/// Audio for the transcriber. `id` comes back with the transcript.
pub struct Clip {
    pub channel: usize,
    pub id: u64,
    pub samples: Vec<f32>,
}

struct Phrase {
    id: u64,
    start: f64,
    end: f64,
    transcript: Option<Transcript>,
}

pub struct Channel {
    index: usize,
    config: ChannelConfig,
    vad: VadStream,
    diar: Option<DiarStream>,
    /// Audio from sample `buf_start` of the channel on.
    buf: Vec<f32>,
    buf_start: u64,
    fed: u64,
    /// Start of the phrase being spoken.
    open: Option<f64>,
    turns: Vec<Turn>,
    phrases: VecDeque<Phrase>,
    next_id: u64,
    finished: bool,
}

fn seconds(samples: u64) -> f64 {
    samples as f64 / SAMPLE_RATE as f64
}

fn sample_at(time: f64) -> u64 {
    (time.max(0.0) * SAMPLE_RATE as f64).round() as u64
}

impl Channel {
    pub fn new(
        index: usize,
        config: ChannelConfig,
        vad: VadStream,
        diar: Option<DiarStream>,
    ) -> Self {
        Self {
            index,
            config,
            vad,
            diar,
            buf: Vec::new(),
            buf_start: 0,
            fed: 0,
            open: None,
            turns: Vec::new(),
            phrases: VecDeque::new(),
            next_id: 0,
            finished: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Seconds of audio received.
    pub fn time(&self) -> f64 {
        seconds(self.fed)
    }

    /// Takes 16 kHz audio; returns the phrases it closed.
    pub fn feed(&mut self, pcm: &[f32]) -> Result<Vec<Clip>> {
        self.buf.extend_from_slice(pcm);
        self.fed += pcm.len() as u64;
        let mut clips = Vec::new();
        for event in self.vad.feed(pcm, false)? {
            match event.edge {
                VadEdge::Start => {
                    self.open.get_or_insert(event.time);
                }
                VadEdge::End => {
                    if let Some(start) = self.open.take() {
                        clips.push(self.cut(start, event.time));
                    }
                }
            }
        }
        let now = self.time();
        if let Some(start) = self.open
            && now - start >= MAX_PHRASE
        {
            clips.push(self.cut(start, now));
            self.open = Some(now);
        }
        if let Some(diar) = &mut self.diar {
            self.turns.extend(diar.feed(pcm, false)?);
        }
        self.trim();
        Ok(clips)
    }

    /// The end of the audio: closes the phrase being spoken and flushes
    /// diarization. Phrases no longer wait for it.
    pub fn finish(&mut self) -> Result<Vec<Clip>> {
        let mut clips = Vec::new();
        for event in self.vad.feed(&[], true)? {
            match event.edge {
                VadEdge::Start => {
                    self.open.get_or_insert(event.time);
                }
                VadEdge::End => {
                    if let Some(start) = self.open.take() {
                        clips.push(self.cut(start, event.time));
                    }
                }
            }
        }
        if let Some(start) = self.open.take() {
            clips.push(self.cut(start, self.time()));
        }
        if let Some(diar) = &mut self.diar {
            self.turns.extend(diar.feed(&[], true)?);
        }
        self.finished = true;
        Ok(clips)
    }

    /// A transcript for one of this channel's clips; a failed one is kept as
    /// empty, so the phrases after it are not held.
    pub fn transcribed(&mut self, id: u64, transcript: Transcript) {
        if let Some(phrase) = self.phrases.iter_mut().find(|p| p.id == id) {
            phrase.transcript = Some(transcript);
        }
    }

    /// Nothing is waiting for a transcript or for diarization.
    pub fn idle(&self) -> bool {
        self.phrases.is_empty()
    }

    /// The utterances of the phrases that can leave now, in order.
    pub fn ready(&mut self) -> Result<Vec<Utterance>> {
        let diarized = match (&self.diar, self.finished) {
            (Some(diar), false) => diar.time(),
            _ => f64::INFINITY,
        };
        let mut out = Vec::new();
        while let Some(phrase) = self.phrases.front() {
            if phrase.transcript.is_none() || phrase.end > diarized {
                break;
            }
            let phrase = self.phrases.pop_front().unwrap();
            let transcript = phrase.transcript.unwrap();
            let words: Vec<Word> = transcript
                .words
                .into_iter()
                .map(|w| Word {
                    start: w.start + phrase.start,
                    end: w.end + phrase.start,
                    ..w
                })
                .collect();
            if words.is_empty() {
                continue;
            }
            let speakers: Vec<Option<u32>> = match &mut self.diar {
                None => vec![None; words.len()],
                Some(diar) => {
                    let mut turns = self.turns.clone();
                    if !self.finished {
                        turns.extend(diar.active()?);
                    }
                    words
                        .iter()
                        .map(|w| speaker_of(w.start, w.end, &turns))
                        .collect()
                }
            };
            out.extend(runs(&self.config.name, words, &speakers));
        }
        let oldest = self
            .phrases
            .front()
            .map(|p| p.start)
            .or(self.open)
            .unwrap_or(self.time());
        self.turns.retain(|t| t.end >= oldest - TURN_MEMORY);
        Ok(out)
    }

    fn cut(&mut self, start: f64, end: f64) -> Clip {
        let from = sample_at(start).max(self.buf_start);
        let to = sample_at(end).min(self.fed).max(from);
        let samples =
            self.buf[(from - self.buf_start) as usize..(to - self.buf_start) as usize].to_vec();
        let id = self.next_id;
        self.next_id += 1;
        self.phrases.push_back(Phrase {
            id,
            start: seconds(from),
            end: seconds(to),
            transcript: None,
        });
        Clip {
            channel: self.index,
            id,
            samples,
        }
    }

    fn trim(&mut self) {
        let keep_from = sample_at(self.open.unwrap_or(self.time()) - KEEP);
        if keep_from > self.buf_start {
            let drop = ((keep_from - self.buf_start) as usize).min(self.buf.len());
            self.buf.drain(..drop);
            self.buf_start += drop as u64;
        }
    }
}

/// The speaker whose turns overlap [start, end] the most; for a word in no
/// turn, the speaker of the nearest turn up to NEAREST_TURN away. The same
/// rule as brainy-worker's `_speaker_of` (extract/audio.py).
pub fn speaker_of(start: f64, end: f64, turns: &[Turn]) -> Option<u32> {
    let mut overlap: Vec<(u32, f64)> = Vec::new();
    let mut nearest: Option<(f64, u32)> = None;
    for turn in turns {
        let shared = end.min(turn.end) - start.max(turn.start);
        if shared > 0.0 {
            match overlap.iter_mut().find(|(s, _)| *s == turn.speaker) {
                Some((_, total)) => *total += shared,
                None => overlap.push((turn.speaker, shared)),
            }
        } else {
            let distance = (turn.start - end).max(start - turn.end);
            if nearest.is_none_or(|(d, _)| distance < d) {
                nearest = Some((distance, turn.speaker));
            }
        }
    }
    if let Some((speaker, _)) = overlap.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        return Some(*speaker);
    }
    nearest.filter(|(d, _)| *d <= NEAREST_TURN).map(|(_, s)| s)
}

/// Words split into utterances at each change of speaker.
fn runs(channel: &str, words: Vec<Word>, speakers: &[Option<u32>]) -> Vec<Utterance> {
    let mut out: Vec<Utterance> = Vec::new();
    for (word, &speaker) in words.into_iter().zip(speakers) {
        match out.last_mut() {
            Some(u) if u.speaker == speaker => {
                u.end = word.end;
                u.text.push(' ');
                u.text.push_str(&word.w);
                u.words.push(word);
            }
            _ => out.push(Utterance {
                channel: channel.to_string(),
                speaker,
                start: word.start,
                end: word.end,
                text: word.w.clone(),
                words: vec![word],
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(speaker: u32, start: f64, end: f64) -> Turn {
        Turn {
            speaker,
            start,
            end,
        }
    }

    fn word(w: &str, start: f64, end: f64) -> Word {
        Word {
            w: w.into(),
            start,
            end,
            conf: 1.0,
        }
    }

    #[test]
    fn a_word_goes_to_the_turn_that_overlaps_it_most() {
        let turns = [turn(0, 0.0, 1.0), turn(1, 0.9, 3.0)];
        assert_eq!(speaker_of(0.8, 1.5, &turns), Some(1));
        assert_eq!(speaker_of(0.1, 0.5, &turns), Some(0));
    }

    #[test]
    fn a_word_before_its_turn_goes_to_the_nearest_one_close_enough() {
        let turns = [turn(2, 5.0, 8.0)];
        assert_eq!(speaker_of(4.5, 4.8, &turns), Some(2));
        assert_eq!(speaker_of(1.0, 2.0, &turns), None);
    }

    #[test]
    fn a_change_of_speaker_starts_a_new_utterance() {
        let words = vec![
            word("Ciao", 0.0, 0.4),
            word("Marco.", 0.5, 0.9),
            word("Ciao!", 1.2, 1.5),
        ];
        let out = runs("system", words, &[Some(0), Some(0), Some(1)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Ciao Marco.");
        assert_eq!((out[0].start, out[0].end), (0.0, 0.9));
        assert_eq!(out[1].speaker, Some(1));
    }
}
