//! Voices known by name: the call's speakers named on the page, not
//! "Speaker 2" (docs/decisions/2026-10-27-speakers-named-by-voice.md).
//!
//! The engine sends every speaker's voice (`SpeakerVoice`: the mean of its
//! WeSpeaker embeddings) with the final segments. This file keeps, on this
//! computer only, the voices the person put a name on (`<data>/voices.json`,
//! `logbook speakers name`), and the last meetings' voices to name them
//! from (`<data>/voices/<meeting>.json`). A speaker is named when its voice
//! is close to one person's and to no one else's; otherwise it stays
//! "Speaker N", with the likeliest name as a question when there is one.
//!
//! Voiceprints are biometric data: they never leave this computer, the
//! meetings' ones are kept for the last KEEP_MEETINGS meetings only, and
//! `logbook speakers forget` (or the app's settings) removes a person.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use logbook_engine::protocol::SpeakerVoice;
use serde::{Deserialize, Serialize};

/// Similarity to a person's voice at which a speaker takes the name.
/// Measured (WeSpeaker ResNet34-LM, 30 s of voice to name a person from,
/// another meeting's speaker): with a library of ~95 people and half the
/// speakers unknown, 0.66 to 0.69 kept wrong names under 0.5% on AMI (95%
/// named right with headsets, 84% from a headset to a table microphone);
/// Italian parliament speeches needed 0.78. A personal library is smaller,
/// so fewer voices compete: 0.70, with the margin below.
const SURE: f32 = 0.70;
/// The best person must beat the second by this much: two people close to
/// the voice are a doubt, not a name.
const MARGIN: f32 = 0.10;
/// Below SURE but above this, the name is offered as a question: about the
/// similarity at which one stranger in a thousand matches (0.52 to 0.63
/// across the measured conditions).
const MAYBE: f32 = 0.55;
/// A voice from less audio is too noisy to be named (3 s of voice tripled
/// the error of 30 s).
const MIN_SECONDS: f64 = 8.0;
/// The least audio a voice needs to be saved under a name, and the amount
/// below which it is saved with a warning.
pub const MIN_ENROLL: f64 = 10.0;
pub const GOOD_ENROLL: f64 = 30.0;
/// The meetings whose voices are kept to be named afterwards.
const KEEP_MEETINGS: usize = 20;

/// The encoder the voices come from: another model's embeddings are in
/// another space, and the library is not used with them.
fn encoder() -> String {
    let (file, sha, _, _) = logbook_engine::mirror::MODELS
        .iter()
        .find(|(.., bundle)| *bundle == "parakeet-voice")
        .expect("the voice model is in the mirror's list");
    format!("{file} sha256:{sha}")
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Library {
    pub encoder: String,
    pub people: Vec<Person>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Person {
    pub name: String,
    /// One per time the person was named: a voice heard through another
    /// microphone or codec is a print of its own.
    pub prints: Vec<Print>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Print {
    pub embedding: Vec<f32>,
    pub seconds: f64,
    /// The meeting it comes from.
    pub meeting: String,
}

/// A name for a meeting's speaker.
#[derive(Clone, Debug, PartialEq)]
pub enum Named {
    Sure(String),
    Maybe(String),
}

impl Library {
    fn path(data: &Path) -> PathBuf {
        data.join("voices.json")
    }

    /// The library, empty if there is none or it is another encoder's.
    pub fn load(data: &Path) -> Result<Self> {
        let path = Self::path(data);
        let lib: Library = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("could not read {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
        };
        if lib.encoder != encoder() {
            tracing::warn!(
                "{} holds voices of another model ({}): not used",
                path.display(),
                lib.encoder
            );
            return Ok(Self::new());
        }
        Ok(lib)
    }

    fn new() -> Self {
        Self {
            encoder: encoder(),
            people: Vec::new(),
        }
    }

    pub fn save(&self, data: &Path) -> Result<()> {
        std::fs::create_dir_all(data)?;
        let path = Self::path(data);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
    }

    pub fn add(&mut self, name: &str, print: Print) {
        match self.people.iter_mut().find(|p| p.name == name) {
            Some(p) => p.prints.push(print),
            None => self.people.push(Person {
                name: name.to_string(),
                prints: vec![print],
            }),
        }
    }

    /// Whether there was someone by that name.
    pub fn forget(&mut self, name: &str) -> bool {
        let before = self.people.len();
        self.people.retain(|p| p.name != name);
        self.people.len() != before
    }

    /// The name of a speaker whose voice is `voice`, if any; `taken` are
    /// the names already given in this meeting (one person, one speaker).
    pub fn name(&self, voice: &SpeakerVoice, taken: &[String]) -> Option<Named> {
        if voice.seconds < MIN_SECONDS {
            return None;
        }
        let mut scores: Vec<(f32, &str)> = self
            .people
            .iter()
            .map(|p| {
                let best = p
                    .prints
                    .iter()
                    .map(|q| cosine(&q.embedding, &voice.embedding))
                    .fold(f32::MIN, f32::max);
                (best, p.name.as_str())
            })
            .collect();
        scores.sort_by(|a, b| b.0.total_cmp(&a.0));
        let (best, name) = *scores.first()?;
        let second = scores.get(1).map_or(f32::MIN, |s| s.0);
        if taken.iter().any(|t| t == name) || best < MAYBE {
            return None;
        }
        if best >= SURE && best - second >= MARGIN {
            Some(Named::Sure(name.to_string()))
        } else {
            Some(Named::Maybe(name.to_string()))
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MIN;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// A past meeting's speakers: their labels on the page and their voices.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Heard {
    pub encoder: String,
    /// RFC 3339.
    pub started: String,
    /// The brainy page, if it was written to one.
    pub page: Option<String>,
    pub speakers: Vec<HeardSpeaker>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HeardSpeaker {
    /// As on the page: "Speaker 2", "Luca", "Speaker 3 (Luca?)".
    pub label: String,
    pub voice: SpeakerVoice,
}

fn heard_dir(data: &Path) -> PathBuf {
    data.join("voices")
}

/// Keeps a meeting's voices to be named later, and drops the oldest past
/// KEEP_MEETINGS.
pub fn remember(data: &Path, meeting: &str, heard: &Heard) -> Result<()> {
    if heard.speakers.is_empty() {
        return Ok(());
    }
    let dir = heard_dir(data);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{meeting}.json")),
        serde_json::to_vec(heard)?,
    )?;
    let mut all = meetings(data)?;
    while all.len() > KEEP_MEETINGS {
        let (oldest, _) = all.remove(0);
        let _ = std::fs::remove_file(dir.join(format!("{oldest}.json")));
    }
    Ok(())
}

/// The meetings whose voices are kept, oldest first.
pub fn meetings(data: &Path) -> Result<Vec<(String, Heard)>> {
    let dir = heard_dir(data);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if let Ok(heard) = serde_json::from_slice::<Heard>(&std::fs::read(&path)?) {
            out.push((name.to_string(), heard));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// A speaker saved under a name.
#[derive(Clone, Debug, Serialize)]
pub struct Enrolled {
    /// The meeting it was heard in, and its label there.
    pub meeting: String,
    pub label: String,
    pub name: String,
    /// Seconds of its voice, and the person's prints now.
    pub seconds: f64,
    pub prints: usize,
}

impl Enrolled {
    /// Below `GOOD_ENROLL`: the person may not be recognized every time.
    pub fn weak(&self) -> bool {
        self.seconds < GOOD_ENROLL
    }
}

/// The voice of SPEAKER (its label, or its number for "Speaker N") in
/// MEETING ("last" for the latest) saved under NAME (`logbook speakers
/// name`, the app's rename).
pub fn name(data: &Path, meeting: &str, speaker: &str, name: &str) -> Result<Enrolled> {
    let name = name.trim();
    if name.is_empty() {
        bail!("the name is empty");
    }
    let all = meetings(data)?;
    let (id, heard) = if meeting == "last" {
        all.last().context("no meeting's voices are kept")?
    } else {
        all.iter()
            .find(|(id, _)| id == meeting)
            .with_context(|| format!("no voices kept for meeting {meeting} (`logbook speakers`)"))?
    };
    if heard.encoder != encoder() {
        bail!(
            "meeting {id}'s voices come from another model ({})",
            heard.encoder
        );
    }
    let s = heard_speaker(heard, speaker).with_context(|| format!("meeting {id}"))?;
    if s.voice.seconds < MIN_ENROLL {
        bail!(
            "{} spoke alone for {:.0} s in meeting {id}: too little to know the voice again",
            s.label,
            s.voice.seconds
        );
    }
    let mut lib = Library::load(data)?;
    lib.add(
        name,
        Print {
            embedding: s.voice.embedding.clone(),
            seconds: s.voice.seconds,
            meeting: id.clone(),
        },
    );
    lib.save(data)?;
    let prints = lib
        .people
        .iter()
        .find(|p| p.name == name)
        .map_or(0, |p| p.prints.len());
    Ok(Enrolled {
        meeting: id.clone(),
        label: s.label.clone(),
        name: name.to_string(),
        seconds: s.voice.seconds,
        prints,
    })
}

/// The one speaker of `heard` that `wanted` names: its label, any case,
/// or the number of "Speaker N".
fn heard_speaker<'a>(heard: &'a Heard, wanted: &str) -> Result<&'a HeardSpeaker> {
    let wanted = wanted.trim();
    let matches: Vec<&HeardSpeaker> = heard
        .speakers
        .iter()
        .filter(|s| {
            s.label.eq_ignore_ascii_case(wanted)
                || s.label
                    .strip_prefix("Speaker ")
                    .and_then(|rest| rest.split(' ').next())
                    .is_some_and(|n| n == wanted)
        })
        .collect();
    match matches.as_slice() {
        [s] => Ok(s),
        [] => bail!(
            "no speaker {wanted}; there are: {}",
            heard
                .speakers
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => bail!("more than one speaker is {wanted}"),
    }
}

/// Changes a speaker's label in a kept meeting's voices, after it was
/// renamed there: naming it again finds it by its new label.
pub fn relabel(data: &Path, meeting: &str, from: &str, to: &str) -> Result<()> {
    let path = heard_dir(data).join(format!("{meeting}.json"));
    let mut heard: Heard = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for s in &mut heard.speakers {
        if s.label == from {
            s.label = to.to_string();
        }
    }
    std::fs::write(&path, serde_json::to_vec(&heard)?)
        .with_context(|| format!("could not write {}", path.display()))
}

/// Forgets the voice of a person; an error if no one has that name.
pub fn forget(data: &Path, name: &str) -> Result<()> {
    let mut lib = Library::load(data)?;
    if !lib.forget(name.trim()) {
        bail!("no voice is known as {name}");
    }
    lib.save(data)
}

pub fn heard(started: &str, page: Option<String>, speakers: Vec<HeardSpeaker>) -> Heard {
    Heard {
        encoder: encoder(),
        started: started.to_string(),
        page,
        speakers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    fn voice(v: &[f32], seconds: f64) -> SpeakerVoice {
        SpeakerVoice {
            channel: "system".into(),
            speaker: 0,
            embedding: unit(v),
            seconds,
        }
    }

    fn print(v: &[f32]) -> Print {
        Print {
            embedding: unit(v),
            seconds: 30.0,
            meeting: "m".into(),
        }
    }

    #[test]
    fn a_voice_is_named_only_when_sure_and_once() {
        let mut lib = Library::new();
        lib.add("Luca", print(&[1.0, 0.0, 0.0]));
        lib.add("Anna", print(&[0.0, 1.0, 0.0]));
        // Close to Luca, far from Anna.
        let v = voice(&[1.0, 0.1, 0.0], 20.0);
        assert_eq!(lib.name(&v, &[]), Some(Named::Sure("Luca".into())));
        // Luca is already someone else in this meeting.
        assert_eq!(lib.name(&v, &["Luca".into()]), None);
        // Too little audio.
        assert_eq!(lib.name(&voice(&[1.0, 0.1, 0.0], 5.0), &[]), None);
        // Between Luca and Anna: a doubt, not a name.
        let between = voice(&[1.0, 0.9, 0.0], 20.0);
        assert_eq!(lib.name(&between, &[]), Some(Named::Maybe("Luca".into())));
        // Like no one.
        assert_eq!(lib.name(&voice(&[0.0, 0.0, 1.0], 20.0), &[]), None);
    }

    #[test]
    fn a_meeting_speaker_is_saved_under_a_name_and_forgotten() {
        let data = std::env::temp_dir().join(format!("logbook-voices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let speakers = vec![
            HeardSpeaker {
                label: "Speaker 1".into(),
                voice: voice(&[1.0, 0.0], 40.0),
            },
            HeardSpeaker {
                label: "Speaker 2 (Luca?)".into(),
                voice: voice(&[0.0, 1.0], 25.0),
            },
        ];
        remember(
            &data,
            "20261027-091500",
            &heard("2026-10-27T09:15:00+01:00", None, speakers),
        )
        .unwrap();
        name(&data, "last", "2", "Luca").unwrap();
        name(&data, "20261027-091500", "speaker 1", "Anna").unwrap();
        let lib = Library::load(&data).unwrap();
        assert_eq!(lib.people.len(), 2);
        assert_eq!(lib.people[0].name, "Luca");
        assert!(name(&data, "last", "3", "Bo").is_err());
        forget(&data, "Luca").unwrap();
        assert_eq!(Library::load(&data).unwrap().people.len(), 1);
        let _ = std::fs::remove_dir_all(&data);
    }
}
