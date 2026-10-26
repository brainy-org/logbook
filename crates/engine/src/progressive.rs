//! The final pass as the meeting goes (docs/pipeline.md, "Final pass";
//! docs/decisions/2026-10-24-speakers-across-a-long-meeting.md).
//!
//! The live transcript is cut phrase by phrase for latency; the final one
//! is read again in ranges of about a minute, as the meeting goes: each
//! channel's audio from where it was last cut to a quiet point is
//! diarized alone, transcribed in pieces of up to 30 s, and its voices
//! matched to the meeting's speakers (speakers.rs), off the session's
//! thread (`transcribe_range`). The segments are sent up to a time every
//! channel has reached. What is left at the end is the last range.
//!
//! The owner (a live session, or a recording read whole) feeds the audio,
//! marks the quiet points (`quiet`: where the live pipeline heard speech
//! end), asks for the ranges to transcribe (`next_ranges`), reads them
//! itself, hands back what was made of them (`transcribed`) and takes the
//! segments that are ready (`ready`).

use std::collections::VecDeque;

use anyhow::Result;

use crate::channel::speaker_of;
use crate::echo::EchoFilter;
use crate::parakeet::{Model, SAMPLE_RATE, Turn, Word};
use crate::protocol::{ChannelConfig, Utterance};
use crate::speakers::{self, Linker, Voice};

/// Pieces of speech per call to the model, as brainy-worker.
const BATCH_PIECES: usize = 8;
/// The longest piece the model reads, which it was trained on.
const MAX_PIECE: f64 = 30.0;
// How words become segments: brainy-worker's MAX_SEGMENT_MS,
// SENTENCE_AFTER_MS and PAUSE_MS (extract/audio.py).
const MAX_SEGMENT: f64 = 30.0;
const SENTENCE_AFTER: f64 = 8.0;
const PAUSE: f64 = 1.2;
/// The least new audio worth a range, during the meeting.
const STEP: f64 = 45.0;
/// The most audio in a range, when a quiet point allows. A range is
/// diarized alone and its voices matched to the meeting's: measured on
/// VoxConverse, windows of 60 s told 17 speakers apart (DER 2.2%), of 120 s
/// fewer (10.8%).
const MAX_RANGE: f64 = 60.0;
/// How far behind the audio received a range must end: a quiet point may
/// be the start of speech the VAD has not reported yet.
const MARGIN: f64 = 1.0;
/// How far behind the channel it echoes a channel's segments stop: the echo
/// filter must have heard every word of the other channel near them.
const ECHO_LEAD: f64 = 2.0;
/// Turns ending this long before the last segments sent are dropped.
const TURN_MEMORY: f64 = 10.0;

/// Audio of one channel to transcribe, in seconds of the meeting.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub channel: usize,
    pub from: f64,
    pub to: f64,
    /// Tell its speakers apart.
    pub diarize: bool,
}

/// What the finalizer makes of a range.
#[derive(Default)]
pub struct Transcribed {
    /// On the meeting's clock.
    pub words: Vec<Word>,
    /// The range's speakers, by its own diarization's slots, on the
    /// meeting's clock.
    pub turns: Vec<Turn>,
    /// The voices of those slots.
    pub voices: Vec<Voice>,
}

pub struct Progressive {
    tracks: Vec<Track>,
    /// For each channel that may hear another: that one's index and the filter.
    echo: Vec<Option<(usize, EchoFilter)>>,
    /// Segments starting before this have been sent.
    done: f64,
    /// Ranges handed out and not yet transcribed.
    out: usize,
    finishing: bool,
}

struct Track {
    config: ChannelConfig,
    /// The turns of the ranges transcribed, by meeting speaker.
    speakers: Vec<Turn>,
    linker: Linker,
    fed: u64,
    /// Points where the channel is quiet, increasing: where ranges may end.
    quiet: VecDeque<f64>,
    /// Transcribed up to here.
    cut: f64,
    /// Transcribed words not sent yet.
    words: Vec<Word>,
}

impl Track {
    fn time(&self) -> f64 {
        self.fed as f64 / SAMPLE_RATE as f64
    }
}

impl Progressive {
    /// `threshold`: the voice similarity of one speaker (speakers.rs).
    pub fn new(configs: Vec<ChannelConfig>, threshold: f32) -> Result<Self> {
        let mut echo = Vec::new();
        for config in &configs {
            echo.push(config.echo_of.as_ref().and_then(|name| {
                configs
                    .iter()
                    .position(|c| &c.name == name)
                    .map(|j| (j, EchoFilter::default()))
            }));
        }
        let tracks = configs
            .into_iter()
            .map(|config| Track {
                config,
                speakers: Vec::new(),
                linker: Linker::new(threshold),
                fed: 0,
                quiet: VecDeque::new(),
                cut: 0.0,
                words: Vec::new(),
            })
            .collect();
        Ok(Self {
            tracks,
            echo,
            done: 0.0,
            out: 0,
            finishing: false,
        })
    }

    /// The channel's next audio, on the meeting's clock.
    pub fn feed(&mut self, channel: usize, pcm: &[f32]) {
        if let Some(track) = self.tracks.get_mut(channel) {
            track.fed += pcm.len() as u64;
        }
    }

    /// A point where the channel is quiet: a range may end there.
    pub fn quiet(&mut self, channel: usize, time: f64) {
        if let Some(track) = self.tracks.get_mut(channel)
            && time > track.cut
            && track.quiet.back().is_none_or(|&q| time > q)
        {
            track.quiet.push_back(time);
        }
    }

    /// No more audio: the rest of every channel is to be transcribed.
    pub fn finish(&mut self) {
        if self.finishing {
            return;
        }
        self.finishing = true;
        for track in &mut self.tracks {
            let end = track.time();
            track.quiet.retain(|&q| q < end);
            track.quiet.push_back(end);
        }
    }

    /// The ranges to transcribe now, if any (none while some are out).
    pub fn next_ranges(&mut self) -> Vec<Range> {
        if self.out > 0 {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for (i, track) in self.tracks.iter_mut().enumerate() {
            let limit = if self.finishing {
                f64::INFINITY
            } else {
                track.time() - MARGIN
            };
            // The latest quiet point at least STEP on and within MAX_RANGE,
            // or, in speech that long, the first one after it. A point
            // short of STEP is no range: taken as the latest within
            // MAX_RANGE, it would wait for one that never comes there.
            let cap = limit.min(track.cut + MAX_RANGE);
            let enough = |q: f64| self.finishing || q - track.cut >= STEP;
            let to = track
                .quiet
                .iter()
                .rev()
                .find(|&&q| q <= cap && enough(q))
                .or_else(|| track.quiet.iter().find(|&&q| q <= limit && enough(q)))
                .copied();
            let Some(to) = to else {
                continue;
            };
            if to <= track.cut {
                // Stale: the channel was cut there or later already.
                while track.quiet.front().is_some_and(|&q| q <= to) {
                    track.quiet.pop_front();
                }
                continue;
            }
            while track.quiet.front().is_some_and(|&q| q <= to) {
                track.quiet.pop_front();
            }
            ranges.push(Range {
                channel: i,
                from: track.cut,
                to,
                diarize: track.config.diarize,
            });
        }
        self.out = ranges.len();
        ranges
    }

    /// What the finalizer made of a range handed out by `next_ranges`.
    pub fn transcribed(&mut self, range: &Range, result: Transcribed) {
        self.out = self.out.saturating_sub(1);
        let Some(track) = self.tracks.get_mut(range.channel) else {
            return;
        };
        let words = result.words;
        track.cut = track.cut.max(range.to);
        // The range's slots are its own: only voices make them the
        // meeting's speakers.
        let map = track.linker.link(&result.voices);
        track.speakers.extend(
            result
                .turns
                .iter()
                .filter_map(|t| map.get(&t.speaker).map(|&speaker| Turn { speaker, ..*t })),
        );
        for (filter_owner, echo) in self.echo.iter_mut().enumerate() {
            if let Some((source, filter)) = echo
                && *source == range.channel
                && filter_owner != range.channel
            {
                filter.heard(&Utterance {
                    channel: track.config.name.clone(),
                    speaker: None,
                    start: range.from,
                    end: range.to,
                    text: String::new(),
                    words: words.clone(),
                });
            }
        }
        track.words.extend(words);
    }

    /// The segments every channel has reached, by start: (sent up to, them).
    /// None when nothing new is ready. Ranges still out do not hold back
    /// the others: a channel's cut moves only when its words are in.
    pub fn ready(&mut self) -> Option<(f64, Vec<Utterance>)> {
        let complete =
            self.finishing && self.out == 0 && self.tracks.iter().all(|t| t.quiet.is_empty());
        // f64::MAX, not infinity, for "everything": `until` goes out as JSON.
        let mut until = if complete {
            f64::MAX
        } else {
            self.tracks
                .iter()
                .map(|t| t.cut)
                .fold(f64::INFINITY, f64::min)
        };
        for (i, echo) in self.echo.iter().enumerate() {
            if let Some((source, _)) = echo
                && *source != i
                && !complete
            {
                until = until.min(self.tracks[*source].cut - ECHO_LEAD);
            }
        }
        if until <= self.done {
            return None;
        }
        let mut out = Vec::new();
        for i in 0..self.tracks.len() {
            let track = &mut self.tracks[i];
            let split = track.words.partition_point(|w| w.start < until);
            let words: Vec<Word> = track.words.drain(..split).collect();
            let speakers: Vec<Option<u32>> = if track.config.diarize {
                words
                    .iter()
                    .map(|w| speaker_of(w.start, w.end, &track.speakers))
                    .collect()
            } else {
                vec![None; words.len()]
            };
            let segs = segments(&track.config.name, words, &speakers);
            let keep_from = until.min(track.cut) - TURN_MEMORY;
            track.speakers.retain(|t| t.end >= keep_from);
            match &mut self.echo[i] {
                Some((source, filter)) if *source != i => {
                    for s in segs {
                        filter.hold(s);
                    }
                    let settled = if complete {
                        f64::INFINITY
                    } else {
                        self.tracks[*source].cut
                    };
                    out.extend(filter.release(settled));
                }
                _ => out.extend(segs),
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        self.done = until;
        Some((until, out))
    }

    /// Where the channel has been transcribed up to: its audio before
    /// this is no longer needed.
    pub fn cut(&self, channel: usize) -> f64 {
        self.tracks.get(channel).map_or(0.0, |t| t.cut)
    }

    /// Everything has been transcribed and sent.
    pub fn is_done(&self) -> bool {
        self.finishing
            && self.out == 0
            && self
                .tracks
                .iter()
                .all(|t| t.quiet.is_empty() && t.words.is_empty())
            && self.echo.iter().flatten().all(|(_, f)| f.is_empty())
    }
}

/// The models a range is read with.
pub struct RangeModels<'a> {
    pub asr: &'a Model,
    pub vad: &'a Model,
    /// A diarization context for whole ranges (one call holds it).
    pub diarization: &'a Model,
    /// The speaker encoder: a range's slots become the meeting's speakers
    /// by voice.
    pub voice: &'a Model,
}

/// A range read again: its words, its speakers (diarized alone) and their
/// voices. `pcm` is the range's audio. Long: run it off the session's thread.
pub fn transcribe_range(models: &RangeModels, pcm: &[f32], range: &Range) -> Result<Transcribed> {
    let words = transcribe(models.asr, models.vad, pcm, range.from)?;
    if !range.diarize || words.is_empty() {
        return Ok(Transcribed {
            words,
            ..Default::default()
        });
    }
    let turns: Vec<Turn> = models
        .diarization
        .diarize(pcm)?
        .into_iter()
        .map(|t| Turn {
            start: t.start + range.from,
            end: t.end + range.from,
            ..t
        })
        .collect();
    let voices = speakers::voices(models.voice, pcm, range.from, &turns)?;
    Ok(Transcribed {
        words,
        turns,
        voices,
    })
}

/// The words of `pcm`, whose first sample is at `offset` seconds of the
/// meeting: cut by Silero VAD into pieces of at most 30 s, transcribed
/// `BATCH_PIECES` at a time.
pub fn transcribe(asr: &Model, vad: &Model, pcm: &[f32], offset: f64) -> Result<Vec<Word>> {
    let rate = SAMPLE_RATE as f64;
    let pieces = vad.vad_segments(pcm, MAX_PIECE)?;
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
        for (&(start, _), transcript) in batch.iter().zip(asr.transcribe(&clips)?) {
            words.extend(transcript.words.into_iter().map(|w| Word {
                start: w.start + start + offset,
                end: w.end + start + offset,
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

    fn said(words: Vec<Word>) -> Transcribed {
        Transcribed {
            words,
            ..Default::default()
        }
    }

    fn config(name: &str, echo_of: Option<&str>) -> ChannelConfig {
        ChannelConfig {
            name: name.into(),
            diarize: false,
            echo_of: echo_of.map(Into::into),
        }
    }

    #[test]
    fn segments_follow_the_meeting_in_order_and_complete_at_the_end() {
        let mut p = Progressive::new(vec![config("a", None), config("b", None)], 0.5).unwrap();
        let second = vec![0.0f32; SAMPLE_RATE as usize];
        for _ in 0..100 {
            p.feed(0, &second);
            p.feed(1, &second);
        }
        // Too little since the start: nothing yet.
        p.quiet(0, 20.0);
        p.quiet(1, 20.0);
        assert!(p.next_ranges().is_empty());
        p.quiet(0, 50.0);
        p.quiet(1, 60.0);
        let ranges = p.next_ranges();
        assert_eq!(ranges.len(), 2);
        assert_eq!((ranges[0].from, ranges[0].to), (0.0, 50.0));
        assert_eq!((ranges[1].from, ranges[1].to), (0.0, 60.0));
        assert!(p.next_ranges().is_empty(), "none while ranges are out");
        p.transcribed(
            &ranges[1],
            said(vec![word("b1", 10.0, 10.5), word("b2", 55.0, 55.5)]),
        );
        // Channel a is not in yet: nothing is ready.
        assert!(p.ready().is_none());
        p.transcribed(&ranges[0], said(vec![word("a1", 5.0, 5.5)]));
        // Up to the earlier cut (50): b2 waits.
        let (until, ready) = p.ready().unwrap();
        assert_eq!(until, 50.0);
        let texts: Vec<&str> = ready.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, ["a1", "b1"]);
        assert!(!p.is_done());
        p.finish();
        let ranges = p.next_ranges();
        assert_eq!(ranges.len(), 2);
        for r in &ranges {
            assert_eq!(r.to, 100.0);
            p.transcribed(r, Transcribed::default());
        }
        let (until, ready) = p.ready().unwrap();
        assert_eq!(until, f64::MAX);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].text, "b2");
        assert!(p.is_done());
    }

    #[test]
    fn a_pause_too_early_does_not_hold_the_channel_after_long_speech() {
        let mut p = Progressive::new(vec![config("a", None)], 0.5).unwrap();
        let second = vec![0.0f32; SAMPLE_RATE as usize];
        for _ in 0..200 {
            p.feed(0, &second);
        }
        // A pause at 20 s, then speech past MAX_RANGE, then quiet again.
        p.quiet(0, 20.0);
        for tenth in 900..1500 {
            p.quiet(0, tenth as f64 / 10.0);
        }
        let ranges = p.next_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].from, ranges[0].to), (0.0, 90.0));
    }

    #[test]
    fn the_echo_is_dropped_and_waits_for_the_channel_it_echoes() {
        let mut p = Progressive::new(
            vec![config("mic", Some("system")), config("system", None)],
            0.5,
        )
        .unwrap();
        let second = vec![0.0f32; SAMPLE_RATE as usize];
        for _ in 0..100 {
            p.feed(0, &second);
            p.feed(1, &second);
        }
        p.quiet(0, 60.0);
        p.quiet(1, 60.0);
        let ranges = p.next_ranges();
        p.transcribed(
            &ranges[0],
            said(vec![
                word("ciao", 10.0, 10.4),
                word("io", 30.0, 30.2),
                word("parlo", 30.3, 30.6),
                word("sopra", 30.7, 31.0),
                word("tardi", 59.0, 59.5),
            ]),
        );
        p.transcribed(&ranges[1], said(vec![word("ciao", 10.1, 10.5)]));
        // The mic stops ECHO_LEAD before the system channel's cut.
        let (until, ready) = p.ready().unwrap();
        assert_eq!(until, 58.0);
        let texts: Vec<&str> = ready.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, ["ciao", "io parlo sopra"]);
        assert_eq!(ready[0].channel, "system");
    }

    #[test]
    fn sentence_ends_allow_closing_quotes() {
        assert!(ends_sentence("detto.»"));
        assert!(ends_sentence("vero?\""));
        assert!(!ends_sentence("parola"));
    }
}
