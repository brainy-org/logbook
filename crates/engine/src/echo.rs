//! The call heard again by the microphone (docs/pipeline.md, "Echo").
//!
//! Without headphones the microphone picks up the speakers, and the call's
//! phrases come twice: once from the system channel, once from the mic.
//! Both channels are on the session's clock, so an echoed word is a mic word
//! that the system channel said at about the same time. The filter holds the
//! mic's utterances until the system channel has sent everything up to their
//! end, drops the echoed words, and keeps a run of the mic's own words only
//! when it is long enough to be speech of its own (talking over the call).

use std::collections::VecDeque;

use crate::parakeet::Word;
use crate::protocol::Utterance;

/// How far apart in time a mic word and a system word can be and still be
/// one sound: the room's delay is milliseconds, the word times of two
/// transcriptions of different audio differ by a few hundred.
const TOLERANCE: f64 = 0.8;
/// In an utterance with echo, a run of non-echoed words shorter than this is
/// a misheard echo ("tu" for "tutto"), not someone speaking.
const MIN_OWN_RUN: usize = 3;
/// System words older than this before the oldest utterance held are dropped.
const MEMORY: f64 = 30.0;

#[derive(Default)]
pub struct EchoFilter {
    heard: VecDeque<Word>,
    held: VecDeque<Utterance>,
}

impl EchoFilter {
    /// An utterance the system channel sent.
    pub fn heard(&mut self, utterance: &Utterance) {
        self.heard.extend(utterance.words.iter().cloned());
    }

    /// An utterance of the mic, held until the system channel catches up.
    pub fn hold(&mut self, utterance: Utterance) {
        self.held.push_back(utterance);
    }

    /// Nothing is held.
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The mic's utterances that can leave now that the system channel has
    /// sent everything ending before `settled`, without their echo.
    pub fn release(&mut self, settled: f64) -> Vec<Utterance> {
        let mut out = Vec::new();
        while let Some(u) = self.held.front() {
            if u.end + TOLERANCE > settled {
                break;
            }
            let u = self.held.pop_front().unwrap();
            out.extend(self.without_echo(u));
        }
        let oldest = self.held.front().map(|u| u.start).unwrap_or(settled);
        while self.heard.front().is_some_and(|w| w.end < oldest - MEMORY) {
            self.heard.pop_front();
        }
        out
    }

    fn without_echo(&self, u: Utterance) -> Vec<Utterance> {
        let echoed: Vec<bool> = u.words.iter().map(|w| self.is_echo(w)).collect();
        if !echoed.contains(&true) {
            return vec![u];
        }
        let mut out = Vec::new();
        let mut run: Vec<Word> = Vec::new();
        let close = |run: &mut Vec<Word>, out: &mut Vec<Utterance>| {
            if run.len() >= MIN_OWN_RUN {
                out.push(Utterance {
                    channel: u.channel.clone(),
                    speaker: u.speaker,
                    start: run[0].start,
                    end: run[run.len() - 1].end,
                    text: run
                        .iter()
                        .map(|w| w.w.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    words: std::mem::take(run),
                });
            }
            run.clear();
        };
        for (word, echo) in u.words.iter().zip(&echoed) {
            if *echo {
                close(&mut run, &mut out);
            } else {
                run.push(word.clone());
            }
        }
        close(&mut run, &mut out);
        out
    }

    fn is_echo(&self, word: &Word) -> bool {
        let text = normalize(&word.w);
        if text.is_empty() {
            return false;
        }
        self.heard
            .iter()
            .filter(|h| (h.start - word.start).abs() <= TOLERANCE)
            .any(|h| similar(&text, &normalize(&h.w)))
    }
}

/// Lowercase letters and digits only: "Tutto." and "tutto" are one word.
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The same word, or one a transcription of the other's echo could give:
/// an edit or two apart in a long enough word ("chiudendoli", "chiudendomi").
fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let shorter = a.chars().count().min(b.chars().count());
    let allowed = match shorter {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    };
    allowed > 0 && edit_distance(a, b) <= allowed
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utterance(channel: &str, words: &[(&str, f64)]) -> Utterance {
        let words: Vec<Word> = words
            .iter()
            .map(|&(w, t)| Word {
                w: w.into(),
                start: t,
                end: t + 0.3,
                conf: 1.0,
            })
            .collect();
        Utterance {
            channel: channel.into(),
            speaker: None,
            start: words[0].start,
            end: words[words.len() - 1].end,
            text: words
                .iter()
                .map(|w| w.w.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            words,
        }
    }

    #[test]
    fn the_call_heard_by_the_mic_is_dropped() {
        let mut f = EchoFilter::default();
        f.heard(&utterance(
            "system",
            &[
                ("Cambia", 10.0),
                ("quel", 10.4),
                ("modo", 10.7),
                ("e", 11.0),
                ("cambi", 11.2),
                ("tutto.", 11.5),
            ],
        ));
        f.hold(utterance(
            "mic",
            &[
                ("Cambia", 10.1),
                ("quel", 10.5),
                ("modo", 10.8),
                ("e", 11.1),
                ("cambi", 11.3),
                ("tu.", 11.6),
            ],
        ));
        assert!(f.release(20.0).is_empty());
    }

    #[test]
    fn words_of_your_own_over_the_call_are_kept() {
        let mut f = EchoFilter::default();
        f.heard(&utterance(
            "system",
            &[("la", 5.0), ("maggior", 5.2), ("parte", 5.5)],
        ));
        f.hold(utterance(
            "mic",
            &[
                ("la", 5.1),
                ("maggior", 5.3),
                ("parte", 5.6),
                ("ma", 6.5),
                ("questo", 6.8),
                ("chi", 7.1),
                ("lo", 7.3),
                ("dice", 7.5),
            ],
        ));
        let out = f.release(20.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "ma questo chi lo dice");
        assert_eq!(out[0].start, 6.5);
    }

    #[test]
    fn the_mic_alone_is_untouched() {
        let mut f = EchoFilter::default();
        f.hold(utterance("mic", &[("Um", 6.6)]));
        assert_eq!(f.release(20.0)[0].text, "Um");
    }

    #[test]
    fn the_mic_waits_for_the_system_channel() {
        let mut f = EchoFilter::default();
        f.hold(utterance("mic", &[("ciao", 3.0)]));
        assert!(f.release(3.5).is_empty());
        assert_eq!(f.release(5.0).len(), 1);
    }

    #[test]
    fn misheard_long_words_still_match() {
        assert!(similar("chiudendoli", "chiudendomi"));
        assert!(similar("dieci", "dieci"));
        assert!(!similar("tu", "tutto"));
        assert!(!similar("di", "da"));
    }
}
