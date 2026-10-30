//! The final transcript as Markdown: who spoke, when, and what.
//!
//! The microphone without diarization is the person recording (`--me`);
//! the call's voices are "Speaker 1", "Speaker 2"... in order of first
//! appearance; a diarized microphone (a room) gives "Room 1", "Room 2"...
//! A voice known by name (voices.rs) takes the name instead, or keeps its
//! number with the name as a question ("Speaker 2 (Luca?)") when it is
//! not sure. A speaker's label is chosen at its first segment and kept: the
//! page is written once. A speaker's consecutive segments go under one
//! heading line.
//!
//! Times are printed on the meeting's displayed clock: 00:00 is its first
//! word, not its first sample (a meeting of a series starts at the cut,
//! up to 15 minutes before anyone speaks). The `Writer` carries that one
//! offset, fixed when the page is created and kept with it; segments stay
//! on the meeting's clock everywhere else
//! (docs/decisions/2026-10-29-the-meeting-starts-at-its-first-word.md).

use logbook_engine::protocol::{SpeakerVoice, Utterance};
use serde::{Deserialize, Serialize};

use crate::voices::{Library, Named};

pub struct Meeting<'a> {
    pub utterances: &'a [Utterance],
    pub warnings: &'a [String],
    /// The person behind the microphone.
    pub me: &'a str,
    /// "2026-10-24 17:40": the displayed start, at `offset`.
    pub started: &'a str,
    /// From the first word (`offset`) to the last.
    pub duration_s: f64,
    /// Where the displayed clock starts, in seconds of the meeting.
    pub offset: f64,
    /// The speakers' voices, and the voices known by name.
    pub voices: &'a [SpeakerVoice],
    pub known: &'a Library,
}

/// The transcript, and the writer that named its speakers.
pub fn markdown(m: &Meeting) -> (String, Writer) {
    let mut writer = Writer::new(m.me, m.offset);
    let body = writer.body(m.utterances, m.voices, m.known);
    let mut out = writer.header(m.started, m.duration_s, m.warnings);
    out.push_str("---\n\n");
    out.push_str(&body);
    (out.trim_end().to_string() + "\n", writer)
}

/// The transcript written a part at a time, as the final segments come
/// during the meeting: the names and the last speaker carry over from one
/// part to the next, and are kept with a pending meeting.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Writer {
    me: String,
    /// (channel, speaker) → name, in order of first appearance.
    names: Vec<((String, Option<u32>), String)>,
    /// The names taken from voices, each given to one speaker only.
    #[serde(default)]
    known: Vec<String>,
    previous: Option<String>,
    /// The seconds of the meeting printed as 00:00 (`first_word`): fixed
    /// when the writer is made, with the page, never changed after, since
    /// the page is never rewritten. 0 in a writer saved before it was kept.
    #[serde(default)]
    offset: f64,
}

/// The displayed clock's start for a meeting whose first final segments
/// are `utterances`: the earliest start, down to a whole second; 0 with
/// none (a meeting of notes only starts at its first sample).
pub fn first_word(utterances: &[Utterance]) -> f64 {
    utterances
        .iter()
        .map(|u| u.start)
        .reduce(f64::min)
        .map_or(0.0, |t| t.max(0.0).floor())
}

impl Writer {
    /// The meeting's page is written again from its start, on a new page
    /// (the old one was deleted): its first turn shows its speaker. The
    /// names and the clock stay the meeting's.
    pub fn new_page(&mut self) {
        self.previous = None;
    }

    pub fn new(me: &str, offset: f64) -> Self {
        Self {
            me: me.to_string(),
            offset,
            ..Default::default()
        }
    }

    /// The seconds of the meeting printed as 00:00.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// A time of the meeting on the displayed clock.
    pub fn shown(&self, t: f64) -> f64 {
        (t - self.offset).max(0.0)
    }

    /// The Markdown of the next segments; `voices`, the speakers' voices
    /// so far, are named from `known`.
    pub fn body(
        &mut self,
        utterances: &[Utterance],
        voices: &[SpeakerVoice],
        known: &Library,
    ) -> String {
        let mut body = String::new();
        for u in utterances {
            let name = self.name_of(u, voices, known);
            if self.previous.as_ref() != Some(&name) {
                body.push_str(&format!(
                    "**{}** · {}\n\n",
                    escape(&name),
                    clock(self.shown(u.start))
                ));
                self.previous = Some(name);
            }
            body.push_str(&escape(&u.text));
            body.push_str("\n\n");
        }
        body
    }

    /// "Recorded … · 1 h 2 min · Marco, Speaker 1", and the warnings.
    pub fn header(&self, started: &str, duration_s: f64, warnings: &[String]) -> String {
        let speakers: Vec<&str> = self.names.iter().map(|(_, n)| n.as_str()).collect();
        let mut out = format!(
            "Recorded {} · {} · {}\n\n",
            started,
            duration(duration_s),
            if speakers.is_empty() {
                "no speech".to_string()
            } else {
                speakers.join(", ")
            }
        );
        for warning in warnings {
            out.push_str(&format!("> {}\n\n", escape(warning)));
        }
        out
    }

    /// The diarized speakers and their labels: (channel, speaker, label).
    pub fn labels(&self) -> Vec<(String, u32, String)> {
        self.names
            .iter()
            .filter_map(|((c, s), n)| s.map(|s| (c.clone(), s, n.clone())))
            .collect()
    }

    /// Every speaker and its label, the microphone's included, in order
    /// of first appearance: (channel, speaker, label).
    pub fn speakers(&self) -> Vec<(String, Option<u32>, String)> {
        self.names
            .iter()
            .map(|((c, s), n)| (c.clone(), *s, n.clone()))
            .collect()
    }

    /// The label of a channel's speaker, once it has spoken.
    pub fn label(&self, channel: &str, speaker: Option<u32>) -> Option<&str> {
        self.names
            .iter()
            .find(|((c, s), _)| c == channel && *s == speaker)
            .map(|(_, n)| n.as_str())
    }

    /// Gives the speaker labelled `from` the label `to`: the person's name,
    /// which no other speaker of the meeting takes from then on.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let mut found = false;
        for (_, name) in &mut self.names {
            if name == from {
                *name = to.to_string();
                found = true;
            }
        }
        if found && !self.known.iter().any(|k| k == to) {
            self.known.push(to.to_string());
        }
        if self.previous.as_deref() == Some(from) {
            self.previous = Some(to.to_string());
        }
        found
    }

    /// The whole transcript's body with the labels already given, as the
    /// page has it: what the report is written from.
    pub fn text(
        &self,
        utterances: &[Utterance],
        voices: &[SpeakerVoice],
        known: &Library,
    ) -> String {
        let mut writer = self.clone();
        writer.previous = None;
        writer.body(utterances, voices, known)
    }

    fn name_of(&mut self, u: &Utterance, voices: &[SpeakerVoice], known: &Library) -> String {
        let key = (u.channel.clone(), u.speaker);
        if let Some((_, name)) = self.names.iter().find(|(k, _)| *k == key) {
            return name.clone();
        }
        let named = u.speaker.and_then(|s| {
            let voice = voices
                .iter()
                .find(|v| v.channel == u.channel && v.speaker == s)?;
            known.name(voice, &self.known)
        });
        // Numbers go to the speakers without a name: Luca does not take
        // "Speaker 1".
        let count = |mic: bool| {
            self.names
                .iter()
                .filter(|((c, s), n)| (c == "mic") == mic && s.is_some() && !self.known.contains(n))
                .count()
        };
        let numbered = match (u.channel.as_str(), u.speaker) {
            ("mic", None) => self.me.clone(),
            ("mic", Some(_)) => format!("Room {}", count(true) + 1),
            (_, None) => "Unknown speaker".to_string(),
            (_, Some(_)) => format!("Speaker {}", count(false) + 1),
        };
        let name = match named {
            Some(Named::Sure(name)) => {
                self.known.push(name.clone());
                name
            }
            Some(Named::Maybe(name)) => format!("{numbered} ({name}?)"),
            None => numbered,
        };
        self.names.push((key, name.clone()));
        name
    }
}

/// Spoken text is text: characters Markdown reads as marks are escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '|' | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn clock(t: f64) -> String {
    let s = t as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn duration(t: f64) -> String {
    let minutes = (t / 60.0).round() as u64;
    match minutes {
        0 => format!("{} s", t.round() as u64),
        m if m < 60 => format!("{m} min"),
        m => format!("{} h {} min", m / 60, m % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(channel: &str, speaker: Option<u32>, start: f64, text: &str) -> Utterance {
        Utterance {
            channel: channel.into(),
            speaker,
            start,
            end: start + 1.0,
            text: text.into(),
            words: Vec::new(),
        }
    }

    #[test]
    fn speakers_are_named_and_grouped() {
        let utterances = vec![
            u("mic", None, 1.0, "Buongiorno a tutti."),
            u("system", Some(3), 4.0, "Ciao Marco."),
            u("system", Some(3), 6.0, "Partiamo dal bilancio."),
            u("system", Some(0), 9.0, "Le vendite *crescono*."),
            u("mic", None, 75.0, "Perfetto."),
        ];
        let (md, _) = markdown(&Meeting {
            utterances: &utterances,
            warnings: &[],
            me: "Marco",
            started: "2026-10-24 17:40",
            duration_s: 80.0,
            offset: 0.0,
            voices: &[],
            known: &Library::default(),
        });
        assert!(
            md.starts_with("Recorded 2026-10-24 17:40 · 1 min · Marco, Speaker 1, Speaker 2\n")
        );
        assert!(md.contains(
            "**Speaker 1** · 00:04\n\nCiao Marco.\n\nPartiamo dal bilancio.\n\n**Speaker 2**"
        ));
        assert!(md.contains("Le vendite \\*crescono\\*."));
        assert!(md.contains("**Marco** · 01:15"));

        // In parts, the same text: names and the last speaker carry over.
        let mut w = Writer::new("Marco", 0.0);
        let none = Library::default();
        let parts = w.body(&utterances[..2], &[], &none) + &w.body(&utterances[2..], &[], &none);
        assert!(md.trim_end().ends_with(parts.trim_end()));
    }

    #[test]
    fn times_read_from_the_first_word() {
        // A meeting cut at 11:00 in which the first word is at 07:12.3.
        let utterances = vec![
            u("system", Some(0), 432.3, "Ciao."),
            u("mic", None, 440.0, "Buongiorno."),
            u("mic", None, 4000.0, "A dopo."),
        ];
        let offset = first_word(&utterances);
        assert_eq!(offset, 432.0);
        let (md, writer) = markdown(&Meeting {
            utterances: &utterances,
            warnings: &[],
            me: "Marco",
            started: "2026-10-29 11:07",
            duration_s: 4001.0 - offset,
            offset,
            voices: &[],
            known: &Library::default(),
        });
        assert!(md.starts_with("Recorded 2026-10-29 11:07 · 59 min · Speaker 1, Marco\n"));
        assert!(md.contains("**Speaker 1** · 00:00\n"));
        assert!(md.contains("**Marco** · 00:08\n"));
        // Kept with the meeting (meeting.json, the history): the same
        // clock for the text the report is read from.
        let saved: Writer = serde_json::from_str(&serde_json::to_string(&writer).unwrap()).unwrap();
        assert_eq!(saved.offset(), 432.0);
        assert!(
            saved
                .text(&utterances, &[], &Library::default())
                .starts_with("**Speaker 1** · 00:00")
        );
        // A writer saved before the offset was kept: 0, as its page has it.
        let old: Writer =
            serde_json::from_str(r#"{"me":"Marco","names":[],"known":[],"previous":null}"#)
                .unwrap();
        assert_eq!(old.offset(), 0.0);
        assert_eq!(first_word(&[]), 0.0);
    }

    #[test]
    fn known_voices_take_their_names_and_numbers_go_to_the_others() {
        let voice = |speaker: u32, e: Vec<f32>| SpeakerVoice {
            channel: "system".into(),
            speaker,
            embedding: e,
            seconds: 30.0,
        };
        let voices = vec![
            voice(3, vec![1.0, 0.0, 0.0]),
            voice(0, vec![0.0, 1.0, 0.0]),
            voice(1, vec![0.0, 0.8, 0.6]),
        ];
        let mut known = Library::default();
        let print = |e: Vec<f32>| crate::voices::Print {
            embedding: e,
            seconds: 30.0,
            meeting: "m".into(),
        };
        known.add("Luca", print(vec![1.0, 0.0, 0.0]));
        known.add("Anna", print(vec![0.0, 0.0, 1.0]));
        let mut w = Writer::new("Marco", 0.0);
        let md = w.body(
            &[
                u("system", Some(3), 4.0, "Ciao Marco."),
                u("system", Some(0), 9.0, "Le vendite crescono."),
                u("system", Some(1), 12.0, "Bene."),
            ],
            &voices,
            &known,
        );
        assert!(md.contains("**Luca** · 00:04"));
        assert!(md.contains("**Speaker 1** · 00:09"));
        // 0.6 from Anna: offered, not given.
        assert!(md.contains("**Speaker 2 (Anna?)** · 00:12"));
        assert_eq!(w.labels()[0], ("system".to_string(), 3, "Luca".to_string()));
    }
}
