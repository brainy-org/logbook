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

use logbook_engine::protocol::{SpeakerVoice, Utterance};
use serde::{Deserialize, Serialize};

use crate::voices::{Library, Named};

pub struct Meeting<'a> {
    pub utterances: &'a [Utterance],
    pub warnings: &'a [String],
    /// The person behind the microphone.
    pub me: &'a str,
    /// "2026-10-24 17:40"
    pub started: &'a str,
    pub duration_s: f64,
    /// The speakers' voices, and the voices known by name.
    pub voices: &'a [SpeakerVoice],
    pub known: &'a Library,
}

/// The transcript, and the writer that named its speakers.
pub fn markdown(m: &Meeting) -> (String, Writer) {
    let mut writer = Writer::new(m.me);
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
}

impl Writer {
    pub fn new(me: &str) -> Self {
        Self {
            me: me.to_string(),
            ..Default::default()
        }
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
                body.push_str(&format!("**{}** · {}\n\n", escape(&name), clock(u.start)));
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
        let mut w = Writer::new("Marco");
        let none = Library::default();
        let parts = w.body(&utterances[..2], &[], &none) + &w.body(&utterances[2..], &[], &none);
        assert!(md.trim_end().ends_with(parts.trim_end()));
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
        let mut w = Writer::new("Marco");
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
