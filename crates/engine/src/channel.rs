//! One audio channel of a live session (docs/pipeline.md).
//!
//! The VAD stream reports speech and pauses; a pause long enough closes a
//! phrase (`Phrasing`), and the phrase becomes a clip for
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
/// Past this length a phrase closes at any pause the VAD reports, however
/// short: continuous speech (a talk, a video) has few pauses of min_pause,
/// and every word of a phrase waits for its end.
const SOFT_PHRASE: f64 = 6.0;
/// Audio added to both ends of a clip, beyond the VAD's own padding, so that
/// a soft first or last sound is not cut.
const CLIP_PAD: f64 = 0.1;
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
    /// The phrase, padded.
    start: f64,
    end: f64,
    /// Where the clip starts: up to `context` before the phrase. The
    /// transcript's times count from here.
    audio_start: f64,
    /// Where the VAD heard the previous phrase end: words centred before it
    /// are that phrase's, transcribed again in the context only to give the
    /// model the language and the sentence, and dropped. Not the phrase's
    /// own start: word times run early after a silence (a first word at
    /// 0.64-0.96 s for speech from 1.0 s), and would be cut.
    boundary: f64,
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
    phrasing: Phrasing,
    /// Seconds of the audio before a phrase sent with it (`Options::context`).
    context: f64,
    /// Where the last phrase cut ended, by the VAD.
    last_end: f64,
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
        min_pause: f64,
        context: f64,
    ) -> Self {
        Self {
            index,
            config,
            vad,
            diar,
            buf: Vec::new(),
            buf_start: 0,
            fed: 0,
            phrasing: Phrasing::new(min_pause),
            context: context.max(0.0),
            last_end: f64::NEG_INFINITY,
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
        let mut cuts = Vec::new();
        for event in self.vad.feed(pcm, false)? {
            self.phrasing.event(event.edge, event.time, &mut cuts);
        }
        self.phrasing.tick(self.time(), &mut cuts);
        if let Some(diar) = &mut self.diar {
            self.turns.extend(diar.feed(pcm, false)?);
        }
        let clips = cuts.into_iter().map(|(s, e)| self.cut(s, e)).collect();
        self.trim();
        Ok(clips)
    }

    /// The end of the audio: closes the phrase being spoken and flushes
    /// diarization. Phrases no longer wait for it.
    pub fn finish(&mut self) -> Result<Vec<Clip>> {
        let mut cuts = Vec::new();
        for event in self.vad.feed(&[], true)? {
            self.phrasing.event(event.edge, event.time, &mut cuts);
        }
        self.phrasing.finish(self.time(), &mut cuts);
        if let Some(diar) = &mut self.diar {
            self.turns.extend(diar.feed(&[], true)?);
        }
        self.finished = true;
        Ok(cuts.into_iter().map(|(s, e)| self.cut(s, e)).collect())
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

    /// Every utterance of this channel that ends before this time has been
    /// sent: what is still to come starts at it or later.
    pub fn settled(&self) -> f64 {
        if self.finished && self.phrases.is_empty() {
            return f64::INFINITY;
        }
        self.phrases
            .front()
            .map(|p| p.start)
            .or(self.phrasing.open)
            .unwrap_or(self.time())
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
                    start: w.start + phrase.audio_start,
                    end: w.end + phrase.audio_start,
                    ..w
                })
                .filter(|w| (w.start + w.end) / 2.0 >= phrase.boundary)
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
            .or(self.phrasing.open)
            .unwrap_or(self.time());
        self.turns.retain(|t| t.end >= oldest - TURN_MEMORY);
        Ok(out)
    }

    fn cut(&mut self, start: f64, end: f64) -> Clip {
        let from = sample_at(start - CLIP_PAD).max(self.buf_start);
        let to = sample_at(end + CLIP_PAD).min(self.fed).max(from);
        let audio_from = sample_at(start - CLIP_PAD - self.context)
            .max(self.buf_start)
            .min(from);
        let samples = self.buf
            [(audio_from - self.buf_start) as usize..(to - self.buf_start) as usize]
            .to_vec();
        let id = self.next_id;
        self.next_id += 1;
        self.phrases.push_back(Phrase {
            id,
            start: seconds(from),
            end: seconds(to),
            audio_start: seconds(audio_from),
            boundary: self.last_end,
            transcript: None,
        });
        self.last_end = end;
        Clip {
            channel: self.index,
            id,
            samples,
        }
    }

    fn trim(&mut self) {
        let keep_from = sample_at(self.phrasing.open.unwrap_or(self.time()) - KEEP - self.context);
        if keep_from > self.buf_start {
            let drop = ((keep_from - self.buf_start) as usize).min(self.buf.len());
            self.buf.drain(..drop);
            self.buf_start += drop as u64;
        }
    }
}

/// Where phrases start and end, from the VAD's speech regions. The VAD
/// reports every pause of its own min_pause (short); a phrase closes at a
/// pause of `min_pause`, or at any pause once it is SOFT_PHRASE long, or is
/// cut at MAX_PHRASE.
struct Phrasing {
    min_pause: f64,
    /// Start of the phrase being spoken.
    open: Option<f64>,
    /// End of its last speech, while it is in a pause.
    last_end: Option<f64>,
    in_speech: bool,
}

impl Phrasing {
    fn new(min_pause: f64) -> Self {
        Self {
            min_pause,
            open: None,
            last_end: None,
            in_speech: false,
        }
    }

    /// The pause that closes a phrase from `start` whose speech ended at `end`.
    fn pause_needed(&self, start: f64, end: f64) -> f64 {
        if end - start >= SOFT_PHRASE {
            0.0
        } else {
            self.min_pause
        }
    }

    fn event(&mut self, edge: VadEdge, time: f64, cuts: &mut Vec<(f64, f64)>) {
        match edge {
            VadEdge::Start => {
                if let (Some(start), Some(end)) = (self.open, self.last_end)
                    && time - end >= self.pause_needed(start, end)
                {
                    cuts.push((start, end));
                    self.open = None;
                }
                self.open.get_or_insert(time);
                self.last_end = None;
                self.in_speech = true;
            }
            VadEdge::End => {
                if self.open.is_some() {
                    self.last_end = Some(time);
                }
                self.in_speech = false;
            }
        }
    }

    fn tick(&mut self, now: f64, cuts: &mut Vec<(f64, f64)>) {
        match (self.open, self.last_end) {
            (Some(start), Some(end)) if !self.in_speech => {
                if now - end >= self.pause_needed(start, end) {
                    cuts.push((start, end));
                    self.open = None;
                    self.last_end = None;
                }
            }
            (Some(start), _) if self.in_speech && now - start >= MAX_PHRASE => {
                cuts.push((start, now));
                self.open = Some(now);
            }
            _ => {}
        }
    }

    fn finish(&mut self, now: f64, cuts: &mut Vec<(f64, f64)>) {
        if let Some(start) = self.open.take() {
            let end = if self.in_speech {
                now
            } else {
                self.last_end.unwrap_or(now)
            };
            cuts.push((start, end));
        }
        self.last_end = None;
        self.in_speech = false;
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

    fn phrases(events: &[(VadEdge, f64)], ticks: &[f64]) -> Vec<(f64, f64)> {
        let mut p = Phrasing::new(0.5);
        let mut cuts = Vec::new();
        let mut ticks = ticks.iter().peekable();
        for &(edge, time) in events {
            while let Some(&&t) = ticks.peek()
                && t < time
            {
                p.tick(t, &mut cuts);
                ticks.next();
            }
            p.event(edge, time, &mut cuts);
        }
        for &t in ticks {
            p.tick(t, &mut cuts);
        }
        cuts
    }

    #[test]
    fn a_short_pause_does_not_close_a_short_phrase() {
        use VadEdge::*;
        let cuts = phrases(
            &[(Start, 1.0), (End, 2.0), (Start, 2.3), (End, 3.0)],
            &[2.2, 3.4, 3.6],
        );
        assert_eq!(cuts, vec![(1.0, 3.0)]);
    }

    #[test]
    fn a_long_phrase_closes_at_any_pause() {
        use VadEdge::*;
        let cuts = phrases(
            &[(Start, 1.0), (End, 7.5), (Start, 7.7), (End, 9.0)],
            &[7.6, 9.6],
        );
        assert_eq!(cuts, vec![(1.0, 7.5), (7.7, 9.0)]);
    }

    #[test]
    fn speech_without_pauses_is_cut_at_the_cap() {
        let mut p = Phrasing::new(0.5);
        let mut cuts = Vec::new();
        p.event(VadEdge::Start, 1.0, &mut cuts);
        p.tick(15.0, &mut cuts);
        assert!(cuts.is_empty());
        p.tick(21.0, &mut cuts);
        p.finish(25.0, &mut cuts);
        assert_eq!(cuts, vec![(1.0, 21.0), (21.0, 25.0)]);
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
