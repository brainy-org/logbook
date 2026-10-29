//! A meeting's speakers beyond the diarization model's eight
//! (docs/decisions/2026-10-24-speakers-across-a-long-meeting.md).
//!
//! The final diarization stream numbers voices consistently, but has eight
//! slots: a ninth voice is given one of them. Each range of the meeting
//! (a minute or two) is therefore checked by voice: every slot that spoke
//! in it gets an embedding (WeSpeaker) of its audio where it speaks alone,
//! and is matched to the meeting's speakers by cosine similarity, a new
//! speaker below `threshold`. Two slots of one range become the same
//! speaker only when both are that voice (SHARE).

use std::collections::HashMap;

use anyhow::Result;

use crate::parakeet::{Model, SAMPLE_RATE, Turn};

/// The least audio of a slot worth an embedding; with less, the slot keeps
/// its speaker of the last range.
const MIN_EMBED: f64 = 1.5;
/// The most audio embedded per slot and range: 10 s told the same voices
/// apart as 30 s (AMI, VoxConverse: the same DER), in a third of the time,
/// which is most of a range's diarization.
const MAX_EMBED: f64 = 10.0;

/// A slot's voice in one range.
#[derive(Clone, Debug)]
pub struct Voice {
    pub slot: u32,
    /// L2-normalized.
    pub embedding: Vec<f32>,
    /// Audio it was made from.
    pub seconds: f64,
}

/// The voices of the slots in `turns`, from `pcm`, whose first sample is
/// at `offset` seconds. Each from up to MAX_EMBED of audio where that slot
/// speaks alone; a slot with less than MIN_EMBED has none.
pub fn voices(model: &Model, pcm: &[f32], offset: f64, turns: &[Turn]) -> Result<Vec<Voice>> {
    voices_up_to(model, pcm, offset, turns, MAX_EMBED)
}

/// `voices`, with at most `max` seconds of audio per voice.
pub fn voices_up_to(
    model: &Model,
    pcm: &[f32],
    offset: f64,
    turns: &[Turn],
    max: f64,
) -> Result<Vec<Voice>> {
    let rate = SAMPLE_RATE as f64;
    let mut slots: Vec<u32> = turns.iter().map(|t| t.speaker).collect();
    slots.sort_unstable();
    slots.dedup();
    let mut out = Vec::new();
    for slot in slots {
        let mut audio = Vec::new();
        let mut seconds = 0.0;
        'turns: for t in turns.iter().filter(|t| t.speaker == slot) {
            for (a, b) in alone(t, turns) {
                if seconds >= max {
                    break 'turns;
                }
                let b = b.min(a + max - seconds);
                let i = (((a - offset) * rate).max(0.0) as usize).min(pcm.len());
                let j = (((b - offset) * rate).max(0.0) as usize).clamp(i, pcm.len());
                audio.extend_from_slice(&pcm[i..j]);
                seconds += (j - i) as f64 / rate;
            }
        }
        if seconds >= MIN_EMBED {
            out.push(Voice {
                slot,
                embedding: model.embed(&audio)?,
                seconds,
            });
        }
    }
    Ok(out)
}

/// The parts of `t` where no other slot speaks.
fn alone(t: &Turn, all: &[Turn]) -> Vec<(f64, f64)> {
    let mut parts = vec![(t.start, t.end)];
    for o in all.iter().filter(|o| o.speaker != t.speaker) {
        parts = parts
            .into_iter()
            .flat_map(|(a, b)| {
                if o.end <= a || o.start >= b {
                    vec![(a, b)]
                } else {
                    let mut v = Vec::new();
                    if o.start > a {
                        v.push((a, o.start));
                    }
                    if o.end < b {
                        v.push((o.end, b));
                    }
                    v
                }
            })
            .collect();
    }
    parts
}

/// The most speakers a channel's linker keeps: past it, the one heard
/// least recently is forgotten (docs/decisions/2026-10-29-bounded-meetings.md).
/// Each is compared with every voice of every range, sent with every
/// `final_segments` and saved with the meeting: a meeting of days on a
/// channel that founds a speaker now and then would otherwise grow all
/// three without end.
pub const MAX_SPEAKERS: usize = 64;

/// The meeting's speakers as voices, matched range by range. A range's
/// slots are its own (it was diarized alone): slot 0 of one range and of
/// the next are unrelated, and only voices link them.
pub struct Linker {
    threshold: f32,
    share: f32,
    /// The speakers kept, by id (increasing): at most `cap`.
    speakers: Vec<Speaker>,
    /// The id the next new speaker takes: ids are never given twice in a
    /// meeting, so a forgotten speaker's number is not someone else's.
    next: u32,
    cap: usize,
}

struct Speaker {
    id: u32,
    /// The sum of its matched embeddings weighted by seconds: its
    /// direction is the speaker's voice.
    centroid: Vec<f32>,
    /// Seconds of audio behind the centroid.
    seconds: f64,
    /// When it was last heard, on the clock `link` is given.
    heard: f64,
}

/// The least audio of a voice that may found a new speaker: shorter
/// embeddings are noisy, and each made a speaker of its own (measured on
/// AMI: 7 to 14 speakers found for 4).
const MIN_NEW: f64 = 4.0;
/// How similar a voice too short to found a speaker must be to join one.
const LOOSE: f32 = 0.2;
/// How similar a slot must be to a speaker another slot of the same range
/// took, to be that speaker too: the diarization split one voice into two
/// slots (seen on AMI: one person became two speakers for the rest of the
/// meeting).
const SHARE: f32 = 0.6;

impl Linker {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            share: SHARE,
            speakers: Vec::new(),
            next: 0,
            cap: MAX_SPEAKERS,
        }
    }

    /// Starts from a meeting's speakers so far (`voices`: speaker, voice,
    /// seconds, as `voices` gives them), so that a pass taken up again
    /// keeps their numbers. A speaker missing from them (forgotten, or
    /// never given a voice) is not found again: its number is not given
    /// to anyone. Past the cap, the highest numbers (the latest found)
    /// are kept.
    pub fn seeded(mut self, voices: &[(u32, Vec<f32>, f64)]) -> Self {
        let mut voices: Vec<&(u32, Vec<f32>, f64)> = voices.iter().collect();
        voices.sort_by_key(|v| v.0);
        voices.dedup_by_key(|v| v.0);
        self.next = voices.last().map_or(0, |v| v.0 + 1);
        let skip = voices.len().saturating_sub(self.cap);
        // Seeded speakers were heard before this pass: before its 0, in
        // the order they were found.
        let count = voices.len() - skip;
        self.speakers = voices[skip..]
            .iter()
            .enumerate()
            .map(|(k, (id, embedding, seconds))| Speaker {
                id: *id,
                centroid: embedding.iter().map(|x| x * *seconds as f32).collect(),
                seconds: *seconds,
                heard: (k as f64) - (count as f64),
            })
            .collect();
        self
    }

    /// The similarity at which two slots of a range are one speaker.
    pub fn with_share(mut self, share: f32) -> Self {
        self.share = share;
        self
    }

    /// The most speakers kept (`MAX_SPEAKERS` by default).
    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap.max(1);
        self
    }

    /// The meeting's speakers numbered so far, forgotten ones included.
    pub fn speakers(&self) -> usize {
        self.next as usize
    }

    /// Each speaker kept, its voice so far, L2-normalized, and the seconds
    /// of audio behind it: what the client matches against the voices it
    /// knows by name (cli voices.rs). By number; numbers may skip the
    /// forgotten.
    pub fn voices(&self) -> Vec<(u32, Vec<f32>, f64)> {
        self.speakers
            .iter()
            .map(|s| {
                let n = s
                    .centroid
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    .sqrt()
                    .max(f32::MIN_POSITIVE);
                (s.id, s.centroid.iter().map(|x| x / n).collect(), s.seconds)
            })
            .collect()
    }

    /// The meeting speaker of the range's slots that can be told: a slot
    /// with no voice (too little audio alone), or a short voice like no
    /// one, has none, and its words go to the nearest turn. `at`: when the
    /// range was heard (its end on the meeting's clock), which keeps the
    /// speakers heard most recently when there are too many.
    pub fn link(&mut self, voices: &[Voice], at: f64) -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        // Ids taken by a slot of this range.
        let mut taken: Vec<u32> = Vec::new();
        // Longest first: the surest voices choose first.
        let mut ordered: Vec<&Voice> = voices.iter().collect();
        ordered.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
        for v in ordered {
            let best = self
                .speakers
                .iter()
                .enumerate()
                .filter(|(_, s)| !taken.contains(&s.id))
                .map(|(i, s)| (i, cosine(&v.embedding, &s.centroid)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            // The same voice as a slot of this range already took.
            let shared = self
                .speakers
                .iter()
                .enumerate()
                .filter(|(_, s)| taken.contains(&s.id))
                .map(|(i, s)| (i, cosine(&v.embedding, &s.centroid)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|&(_, sim)| sim >= self.share && best.is_none_or(|(_, b)| sim > b));
            if let Some((i, _)) = shared {
                self.speakers[i].heard = self.speakers[i].heard.max(at);
                map.insert(v.slot, self.speakers[i].id);
                continue;
            }
            let join = match best {
                Some((i, sim)) if sim >= self.threshold => Some(i),
                _ if v.seconds >= MIN_NEW || self.speakers.is_empty() => None,
                Some((i, sim)) if sim >= LOOSE => Some(i),
                _ => continue,
            };
            let id = match join {
                Some(i) => {
                    let s = &mut self.speakers[i];
                    for (a, b) in s.centroid.iter_mut().zip(&v.embedding) {
                        *a += b * v.seconds as f32;
                    }
                    s.seconds += v.seconds;
                    s.heard = s.heard.max(at);
                    s.id
                }
                None => {
                    if self.speakers.len() >= self.cap {
                        self.forget_one(&taken);
                    }
                    let id = self.next;
                    self.next += 1;
                    self.speakers.push(Speaker {
                        id,
                        centroid: v.embedding.iter().map(|x| x * v.seconds as f32).collect(),
                        seconds: v.seconds,
                        heard: at,
                    });
                    id
                }
            };
            taken.push(id);
            map.insert(v.slot, id);
        }
        map
    }

    /// Forgets the speaker heard least recently, but none of `taken` (this
    /// range's: their ids are in its map already).
    fn forget_one(&mut self, taken: &[u32]) {
        let oldest = self
            .speakers
            .iter()
            .enumerate()
            .filter(|(_, s)| !taken.contains(&s.id))
            .min_by(|a, b| a.1.heard.total_cmp(&b.1.heard).then(a.1.id.cmp(&b.1.id)))
            .map(|(i, _)| i);
        if let Some(i) = oldest {
            self.speakers.remove(i);
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(slot: u32, dir: &[f32], seconds: f64) -> Voice {
        let n = dir.iter().map(|x| x * x).sum::<f32>().sqrt();
        Voice {
            slot,
            embedding: dir.iter().map(|x| x / n).collect(),
            seconds,
        }
    }

    #[test]
    fn voices_link_ranges_and_new_ones_found_speakers() {
        let mut l = Linker::new(0.5);
        // Range 1: two voices.
        let m = l.link(
            &[
                voice(0, &[1.0, 0.0, 0.0], 20.0),
                voice(1, &[0.0, 1.0, 0.0], 10.0),
            ],
            60.0,
        );
        assert_eq!((m[&0], m[&1]), (0, 1));
        // Range 2: its slot 0 is a third voice.
        let m = l.link(&[voice(0, &[0.0, 0.0, 1.0], 15.0)], 120.0);
        assert_eq!(m[&0], 2);
        // Range 3: the first voice is back, as slot 1.
        let m = l.link(&[voice(1, &[0.9, 0.1, 0.0], 12.0)], 180.0);
        assert_eq!(m[&1], 0);
        assert_eq!(l.speakers(), 3);
        // Each speaker's voice: unit length, with all the audio behind it.
        let v = l.voices();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].2, 32.0);
        let n: f32 = v[0].1.iter().map(|x| x * x).sum();
        assert!((n - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_pass_taken_up_again_keeps_the_speakers_numbers() {
        let mut first = Linker::new(0.5);
        first.link(
            &[
                voice(0, &[1.0, 0.0, 0.0], 20.0),
                voice(1, &[0.0, 1.0, 0.0], 10.0),
            ],
            240.0,
        );
        let mut again = Linker::new(0.5).seeded(&first.voices());
        // The second voice comes back as slot 0, a new one as slot 1.
        let m = again.link(
            &[
                voice(0, &[0.1, 0.9, 0.0], 12.0),
                voice(1, &[0.0, 0.0, 1.0], 8.0),
            ],
            300.0,
        );
        assert_eq!((m[&0], m[&1]), (1, 2));
        assert_eq!(again.voices()[1].2, 22.0);
    }

    #[test]
    fn slots_of_a_range_merge_only_as_one_voice_and_short_voices_found_no_one() {
        let mut l = Linker::new(0.5);
        let m = l.link(
            &[voice(0, &[1.0, 0.0], 20.0), voice(1, &[0.0, 1.0], 20.0)],
            360.0,
        );
        assert_eq!((m[&0], m[&1]), (0, 1));
        // Both the first voice: one voice the diarization split in two.
        let m = l.link(
            &[voice(0, &[1.0, 0.1], 3.0), voice(1, &[1.0, 0.05], 9.0)],
            420.0,
        );
        assert_eq!((m[&0], m[&1]), (0, 0));
        // Close to the first, but not that close: the next best instead.
        let m = l.link(
            &[voice(0, &[1.0, 2.0], 3.0), voice(1, &[1.0, 0.05], 9.0)],
            480.0,
        );
        assert_eq!(m[&1], 0);
        assert_eq!(m[&0], 1);
        // A short voice like no one: no speaker, not a new one.
        let m = l.link(&[voice(2, &[-1.0, -1.0], 2.0)], 540.0);
        assert!(!m.contains_key(&2));
        assert_eq!(l.speakers(), 2);
    }

    #[test]
    fn past_the_cap_the_speaker_heard_least_recently_is_forgotten() {
        let dir = |i: usize| {
            let mut d = vec![0.0f32; 8];
            d[i] = 1.0;
            d
        };
        let mut l = Linker::new(0.5).with_cap(3);
        // Speakers 0, 1, 2, heard at 10, 20, 30; 0 again at 40.
        for (i, at) in [(0, 10.0), (1, 20.0), (2, 30.0), (0, 40.0)] {
            l.link(&[voice(0, &dir(i), 10.0)], at);
        }
        // A fourth voice: speaker 1, heard last at 20, is forgotten; the
        // new one is 3, not 1.
        let m = l.link(&[voice(0, &dir(3), 10.0)], 50.0);
        assert_eq!(m[&0], 3);
        let ids: Vec<u32> = l.voices().iter().map(|v| v.0).collect();
        assert_eq!(ids, [0, 2, 3]);
        assert_eq!(l.speakers(), 4);
        // The others keep their numbers.
        let m = l.link(&[voice(0, &dir(2), 10.0), voice(1, &dir(0), 5.0)], 60.0);
        assert_eq!((m[&0], m[&1]), (2, 0));
        // Speaker 1's voice back: a new speaker, 4 (3 is forgotten now).
        let m = l.link(&[voice(0, &dir(1), 10.0)], 70.0);
        assert_eq!(m[&0], 4);
        let ids: Vec<u32> = l.voices().iter().map(|v| v.0).collect();
        assert_eq!(ids, [0, 2, 4]);
        // Taken up again from sparse numbers: they hold, and the next new
        // one is 5.
        let mut again = Linker::new(0.5).with_cap(3).seeded(&l.voices());
        let m = again.link(&[voice(0, &dir(2), 10.0), voice(1, &dir(5), 10.0)], 5.0);
        assert_eq!((m[&0], m[&1]), (2, 5));
        let ids: Vec<u32> = again.voices().iter().map(|v| v.0).collect();
        assert_eq!(ids, [2, 4, 5]);
    }

    #[test]
    fn a_range_s_own_speakers_are_never_forgotten_for_its_new_one() {
        let dir = |i: usize| {
            let mut d = vec![0.0f32; 8];
            d[i] = 1.0;
            d
        };
        let mut l = Linker::new(0.5).with_cap(2);
        l.link(&[voice(0, &dir(0), 10.0)], 10.0);
        l.link(&[voice(0, &dir(1), 10.0)], 20.0);
        // Speaker 0 (heard at 10) speaks in this range, which also founds
        // a speaker: 1 goes, though 0 was heard before it.
        let m = l.link(&[voice(0, &dir(0), 10.0), voice(1, &dir(2), 8.0)], 30.0);
        assert_eq!((m[&0], m[&1]), (0, 2));
        let ids: Vec<u32> = l.voices().iter().map(|v| v.0).collect();
        assert_eq!(ids, [0, 2]);
    }

    #[test]
    fn alone_is_what_no_other_slot_overlaps() {
        let t = Turn {
            speaker: 0,
            start: 0.0,
            end: 10.0,
        };
        let o = Turn {
            speaker: 1,
            start: 4.0,
            end: 6.0,
        };
        assert_eq!(alone(&t, &[t, o]), vec![(0.0, 4.0), (6.0, 10.0)]);
    }
}
