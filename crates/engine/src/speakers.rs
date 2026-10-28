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

/// The meeting's speakers as voices, matched range by range. A range's
/// slots are its own (it was diarized alone): slot 0 of one range and of
/// the next are unrelated, and only voices link them.
pub struct Linker {
    threshold: f32,
    share: f32,
    /// Per speaker, the sum of its matched embeddings weighted by seconds:
    /// its direction is the speaker's voice.
    centroids: Vec<Vec<f32>>,
    /// Per speaker, the seconds of audio behind its centroid.
    seconds: Vec<f64>,
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
            centroids: Vec::new(),
            seconds: Vec::new(),
        }
    }

    /// Starts from a meeting's speakers so far (`voices`: speaker, voice,
    /// seconds, as `voices` gives them), so that a pass taken up again
    /// keeps their numbers. A speaker missing from them stays one with no
    /// voice: nobody joins it.
    pub fn seeded(mut self, voices: &[(u32, Vec<f32>, f64)]) -> Self {
        let count = voices.iter().map(|v| v.0 as usize + 1).max().unwrap_or(0);
        let width = voices.first().map_or(0, |v| v.1.len());
        self.centroids = vec![vec![0.0; width]; count];
        self.seconds = vec![0.0; count];
        for (speaker, embedding, seconds) in voices {
            let i = *speaker as usize;
            self.centroids[i] = embedding.iter().map(|x| x * *seconds as f32).collect();
            self.seconds[i] = *seconds;
        }
        self
    }

    /// The similarity at which two slots of a range are one speaker.
    pub fn with_share(mut self, share: f32) -> Self {
        self.share = share;
        self
    }

    /// The meeting's speakers found so far.
    pub fn speakers(&self) -> usize {
        self.centroids.len()
    }

    /// Each speaker's voice so far, L2-normalized, and the seconds of audio
    /// behind it: what the client matches against the voices it knows by
    /// name (cli voices.rs).
    pub fn voices(&self) -> Vec<(u32, Vec<f32>, f64)> {
        self.centroids
            .iter()
            .zip(&self.seconds)
            .enumerate()
            .map(|(i, (c, &s))| {
                let n = c
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    .sqrt()
                    .max(f32::MIN_POSITIVE);
                (i as u32, c.iter().map(|x| x / n).collect(), s)
            })
            .collect()
    }

    /// The meeting speaker of the range's slots that can be told: a slot
    /// with no voice (too little audio alone), or a short voice like no
    /// one, has none, and its words go to the nearest turn.
    pub fn link(&mut self, voices: &[Voice]) -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        let mut taken = vec![false; self.centroids.len()];
        // Longest first: the surest voices choose first.
        let mut ordered: Vec<&Voice> = voices.iter().collect();
        ordered.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
        for v in ordered {
            let best = self
                .centroids
                .iter()
                .enumerate()
                .filter(|(i, _)| !taken[*i])
                .map(|(i, c)| (i, cosine(&v.embedding, c)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            // The same voice as a slot of this range already took.
            let shared = self
                .centroids
                .iter()
                .enumerate()
                .filter(|(i, _)| taken[*i])
                .map(|(i, c)| (i, cosine(&v.embedding, c)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|&(_, sim)| sim >= self.share && best.is_none_or(|(_, b)| sim > b));
            if let Some((i, _)) = shared {
                map.insert(v.slot, i as u32);
                continue;
            }
            let join = match best {
                Some((i, sim)) if sim >= self.threshold => Some(i),
                _ if v.seconds >= MIN_NEW || self.centroids.is_empty() => None,
                Some((i, sim)) if sim >= LOOSE => Some(i),
                _ => continue,
            };
            let speaker = match join {
                Some(i) => {
                    for (a, b) in self.centroids[i].iter_mut().zip(&v.embedding) {
                        *a += b * v.seconds as f32;
                    }
                    self.seconds[i] += v.seconds;
                    i
                }
                None => {
                    self.centroids
                        .push(v.embedding.iter().map(|x| x * v.seconds as f32).collect());
                    self.seconds.push(v.seconds);
                    taken.push(false);
                    self.centroids.len() - 1
                }
            };
            taken[speaker] = true;
            map.insert(v.slot, speaker as u32);
        }
        map
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
        let m = l.link(&[
            voice(0, &[1.0, 0.0, 0.0], 20.0),
            voice(1, &[0.0, 1.0, 0.0], 10.0),
        ]);
        assert_eq!((m[&0], m[&1]), (0, 1));
        // Range 2: its slot 0 is a third voice.
        let m = l.link(&[voice(0, &[0.0, 0.0, 1.0], 15.0)]);
        assert_eq!(m[&0], 2);
        // Range 3: the first voice is back, as slot 1.
        let m = l.link(&[voice(1, &[0.9, 0.1, 0.0], 12.0)]);
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
        first.link(&[
            voice(0, &[1.0, 0.0, 0.0], 20.0),
            voice(1, &[0.0, 1.0, 0.0], 10.0),
        ]);
        let mut again = Linker::new(0.5).seeded(&first.voices());
        // The second voice comes back as slot 0, a new one as slot 1.
        let m = again.link(&[
            voice(0, &[0.1, 0.9, 0.0], 12.0),
            voice(1, &[0.0, 0.0, 1.0], 8.0),
        ]);
        assert_eq!((m[&0], m[&1]), (1, 2));
        assert_eq!(again.voices()[1].2, 22.0);
    }

    #[test]
    fn slots_of_a_range_merge_only_as_one_voice_and_short_voices_found_no_one() {
        let mut l = Linker::new(0.5);
        let m = l.link(&[voice(0, &[1.0, 0.0], 20.0), voice(1, &[0.0, 1.0], 20.0)]);
        assert_eq!((m[&0], m[&1]), (0, 1));
        // Both the first voice: one voice the diarization split in two.
        let m = l.link(&[voice(0, &[1.0, 0.1], 3.0), voice(1, &[1.0, 0.05], 9.0)]);
        assert_eq!((m[&0], m[&1]), (0, 0));
        // Close to the first, but not that close: the next best instead.
        let m = l.link(&[voice(0, &[1.0, 2.0], 3.0), voice(1, &[1.0, 0.05], 9.0)]);
        assert_eq!(m[&1], 0);
        assert_eq!(m[&0], 1);
        // A short voice like no one: no speaker, not a new one.
        let m = l.link(&[voice(2, &[-1.0, -1.0], 2.0)]);
        assert!(!m.contains_key(&2));
        assert_eq!(l.speakers(), 2);
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
