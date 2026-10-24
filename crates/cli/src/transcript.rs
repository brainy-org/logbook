//! The final transcript as Markdown: who spoke, when, and what.
//!
//! The microphone without diarization is the person recording (`--me`);
//! the call's voices are "Speaker 1", "Speaker 2"... in order of first
//! appearance; a diarized microphone (a room) gives "Room 1", "Room 2"...
//! A speaker's consecutive segments go under one heading line.

use logbook_engine::protocol::Utterance;

pub struct Meeting<'a> {
    pub utterances: &'a [Utterance],
    pub warnings: &'a [String],
    /// The person behind the microphone.
    pub me: &'a str,
    /// "2026-10-24 17:40"
    pub started: &'a str,
    pub duration_s: f64,
}

pub fn markdown(m: &Meeting) -> String {
    let mut names: Vec<((String, Option<u32>), String)> = Vec::new();
    let mut name_of = |u: &Utterance| -> String {
        let key = (u.channel.clone(), u.speaker);
        if let Some((_, name)) = names.iter().find(|(k, _)| *k == key) {
            return name.clone();
        }
        let name = match (u.channel.as_str(), u.speaker) {
            ("mic", None) => m.me.to_string(),
            ("mic", Some(_)) => {
                let n = names
                    .iter()
                    .filter(|((c, s), _)| c == "mic" && s.is_some())
                    .count();
                format!("Room {}", n + 1)
            }
            (_, None) => "Unknown speaker".to_string(),
            (_, Some(_)) => {
                let n = names
                    .iter()
                    .filter(|((c, s), _)| c != "mic" && s.is_some())
                    .count();
                format!("Speaker {}", n + 1)
            }
        };
        names.push((key, name.clone()));
        name
    };

    let mut body = String::new();
    let mut previous: Option<String> = None;
    for u in m.utterances {
        let name = name_of(u);
        if previous.as_ref() != Some(&name) {
            body.push_str(&format!("**{}** · {}\n\n", escape(&name), clock(u.start)));
            previous = Some(name);
        }
        body.push_str(&escape(&u.text));
        body.push_str("\n\n");
    }

    let speakers: Vec<&str> = names.iter().map(|(_, n)| n.as_str()).collect();
    let mut out = format!(
        "Recorded {} · {} · {}\n\n",
        m.started,
        duration(m.duration_s),
        if speakers.is_empty() {
            "no speech".to_string()
        } else {
            speakers.join(", ")
        }
    );
    for warning in m.warnings {
        out.push_str(&format!("> {}\n\n", escape(warning)));
    }
    out.push_str("---\n\n");
    out.push_str(&body);
    out.trim_end().to_string() + "\n"
}

/// Spoken text is text: characters Markdown reads as marks are escaped.
fn escape(text: &str) -> String {
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
        let md = markdown(&Meeting {
            utterances: &utterances,
            warnings: &[],
            me: "Marco",
            started: "2026-10-24 17:40",
            duration_s: 80.0,
        });
        assert!(
            md.starts_with("Recorded 2026-10-24 17:40 · 1 min · Marco, Speaker 1, Speaker 2\n")
        );
        assert!(md.contains(
            "**Speaker 1** · 00:04\n\nCiao Marco.\n\nPartiamo dal bilancio.\n\n**Speaker 2**"
        ));
        assert!(md.contains("Le vendite \\*crescono\\*."));
        assert!(md.contains("**Marco** · 01:15"));
    }
}
