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
//! A meeting started with its expected people (`--expect`, the app's
//! picker) is named from them only: with a library of hundreds, strangers
//! close to someone's voice are many, with five people they are few.
//!
//! Voiceprints are biometric data: they never leave this computer, the
//! meetings' ones are kept for the last KEEP_MEETINGS meetings only (and
//! go when the meeting is deleted: history.rs `purge`), and
//! `logbook speakers forget` (or the app's voices page) removes a person.

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
/// The prints kept per person, the latest: a person is matched by their
/// best print, so one named in every meeting would otherwise grow without
/// end and draw strangers' voices to their name.
const MAX_PRINTS: usize = 5;
/// The names offered for a speaker being named (`candidates`).
pub const CANDIDATES: usize = 3;

/// The encoder the voices come from: another model's embeddings are in
/// another space, and the library is not used with them.
fn encoder() -> String {
    let (file, sha, _, _) = logbook_engine::pins::MODELS
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
    /// The latest meeting (its id) a speaker took their name in by voice,
    /// without being named: when they were last heard. Only an id, never
    /// a voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<String>,
}

impl Person {
    /// The latest meeting they were heard in: recognized there, or named.
    pub fn last_heard(&self) -> &str {
        let named = self.prints.iter().map(|p| p.meeting.as_str()).max();
        named
            .into_iter()
            .chain(self.heard.as_deref())
            .max()
            .unwrap_or_default()
    }
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
            Some(p) => {
                p.prints.push(print);
                let over = p.prints.len().saturating_sub(MAX_PRINTS);
                p.prints.drain(..over);
            }
            None => self.people.push(Person {
                name: name.to_string(),
                prints: vec![print],
                heard: None,
            }),
        }
    }

    /// Whether there was someone by that name.
    pub fn forget(&mut self, name: &str) -> bool {
        let before = self.people.len();
        self.people.retain(|p| p.name != name);
        self.people.len() != before
    }

    /// Gives person `from` the name `to`, their prints kept. Refused when
    /// someone else already has that name (any case, as `expected` reads
    /// it): two people under one name would be one voice to the pages.
    /// Changing only the case of one's own name is allowed.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let to = to.trim();
        if to.is_empty() {
            bail!("the name is empty");
        }
        let at = self
            .people
            .iter()
            .position(|p| p.name == from)
            .with_context(|| format!("no voice is known as {from}"))?;
        if let Some(other) = self
            .people
            .iter()
            .enumerate()
            .find(|(i, p)| *i != at && p.name.eq_ignore_ascii_case(to))
        {
            bail!("a voice is already known as {}", other.1.name);
        }
        self.people[at].name = to.to_string();
        Ok(())
    }

    /// The people named `names` only (any case), the others left out;
    /// everyone with no names.
    pub fn only(mut self, names: &[String]) -> Self {
        if !names.is_empty() {
            self.people
                .retain(|p| names.iter().any(|n| n.eq_ignore_ascii_case(&p.name)));
        }
        self
    }

    /// `names` as the library spells them; an error for one it does not
    /// know, before a meeting is named from them.
    pub fn expected(&self, names: &[String]) -> Result<Vec<String>> {
        names
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
            .map(|n| {
                self.people
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(n))
                    .map(|p| p.name.clone())
                    .with_context(|| format!("no voice is known as {n} (`logbook speakers`)"))
            })
            .collect()
    }

    /// The name of a speaker whose voice is `voice`, if any; `taken` are
    /// the names already given in this meeting (one person, one speaker).
    /// When the likeliest is taken the next one may be offered, never
    /// given: the taken person is still the closer voice, and one person
    /// split in two by diarization is more likely than a lookalike.
    pub fn name(&self, voice: &SpeakerVoice, taken: &[String]) -> Option<Named> {
        if voice.seconds < MIN_SECONDS {
            return None;
        }
        let scores = self.scores(voice, &[]);
        let at = scores
            .iter()
            .position(|(_, n)| !taken.iter().any(|t| t == n))?;
        let (best, name) = scores[at];
        let second = scores
            .iter()
            .enumerate()
            .find(|(i, _)| *i != at)
            .map_or(f32::MIN, |(_, s)| s.0);
        if best < MAYBE {
            return None;
        }
        if best >= SURE && best - second >= MARGIN {
            Some(Named::Sure(name.to_string()))
        } else {
            Some(Named::Maybe(name.to_string()))
        }
    }

    /// The people a speaker could be, likeliest first, at most `k`: those
    /// whose voice is at least as close as a name offered as a question.
    pub fn candidates(&self, voice: &SpeakerVoice, taken: &[String], k: usize) -> Vec<String> {
        if voice.seconds < MIN_SECONDS {
            return Vec::new();
        }
        self.scores(voice, taken)
            .into_iter()
            .take_while(|(score, _)| *score >= MAYBE)
            .take(k)
            .map(|(_, name)| name.to_string())
            .collect()
    }

    /// Why `voice` may not be `name`'s, if it may not: it sounds like
    /// someone else beyond doubt, or like none of the prints `name` has
    /// (no closer than a stranger to a name offered as a question). A
    /// person is matched by their best print, so one print of another
    /// voice names that voice as them in every meeting after.
    pub fn doubt(&self, name: &str, voice: &SpeakerVoice) -> Option<Doubted> {
        let scores = self.scores(voice, &[]);
        let own = scores.iter().find(|(_, n)| *n == name).map(|s| s.0);
        let (best, other) = *scores.first()?;
        if other != name && best >= SURE && best - own.unwrap_or(f32::MIN) >= MARGIN {
            return Some(Doubted {
                name: name.to_string(),
                own,
                like: Some((other.to_string(), best)),
            });
        }
        match own {
            Some(own) if own < MAYBE => Some(Doubted {
                name: name.to_string(),
                own: Some(own),
                like: None,
            }),
            _ => None,
        }
    }

    /// Every person but `taken`, by their best print's similarity to
    /// `voice`, highest first.
    fn scores(&self, voice: &SpeakerVoice, taken: &[String]) -> Vec<(f32, &str)> {
        let mut scores: Vec<(f32, &str)> = self
            .people
            .iter()
            .filter(|p| !taken.contains(&p.name))
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
        scores
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

/// Drops the voices kept from meeting `id` (its speakers, named or not),
/// once the meeting itself is gone; the voices known by name stay.
pub fn forget_meeting(data: &Path, id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "{id:?} is not a meeting"
    );
    match std::fs::remove_file(heard_dir(data).join(format!("{id}.json"))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// The people known by name among `labels`, the speakers of meeting
/// `meeting` as written on its page, were heard there.
pub fn heard_in(data: &Path, meeting: &str, labels: &[String]) -> Result<()> {
    let mut lib = Library::load(data)?;
    let mut changed = false;
    for p in &mut lib.people {
        if labels.contains(&p.name) && p.heard.as_deref().is_none_or(|h| h < meeting) {
            p.heard = Some(meeting.to_string());
            changed = true;
        }
    }
    if changed { lib.save(data) } else { Ok(()) }
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

/// A voice not saved under a name because it may not be theirs
/// (`Library::doubt`): the error of `name` without `anyway`, which the
/// caller tells from others by downcasting.
#[derive(Clone, Debug, PartialEq)]
pub struct Doubted {
    pub name: String,
    /// Its similarity to their closest print; None for a new person.
    pub own: Option<f32>,
    /// The person it sounds like instead, and how much.
    pub like: Option<(String, f32)>,
}

impl std::fmt::Display for Doubted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.like, self.own) {
            (Some((other, score)), Some(own)) => write!(
                f,
                "this voice sounds like {other}'s ({score:.2}), not {}'s ({own:.2})",
                self.name
            ),
            (Some((other, score)), None) => {
                write!(f, "this voice sounds like {other}'s ({score:.2})")
            }
            (None, own) => write!(
                f,
                "this voice is unlike {}'s ({:.2} to their closest), as a stranger's would be",
                self.name,
                own.unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for Doubted {}

/// The voice of SPEAKER (its label, or its number for "Speaker N") in
/// MEETING ("last" for the latest) saved under NAME (`logbook speakers
/// name`, the app's rename). A voice that may not be NAME's is refused
/// with `Doubted`, unless `anyway`: the person said it is.
pub fn name(
    data: &Path,
    meeting: &str,
    speaker: &str,
    name: &str,
    anyway: bool,
) -> Result<Enrolled> {
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
    if !anyway && let Some(doubt) = lib.doubt(name, &s.voice) {
        return Err(doubt.into());
    }
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

/// The names offered for SPEAKER (its label) of kept meeting MEETING,
/// likeliest first; `taken`, the meeting's other speakers' names, are not.
pub fn candidates(
    data: &Path,
    meeting: &str,
    speaker: &str,
    taken: &[String],
) -> Result<Vec<String>> {
    let path = heard_dir(data).join(format!("{meeting}.json"));
    let heard: Heard = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    if heard.encoder != encoder() {
        return Ok(Vec::new());
    }
    let Ok(s) = heard_speaker(&heard, speaker) else {
        return Ok(Vec::new());
    };
    Ok(Library::load(data)?.candidates(&s.voice, taken, CANDIDATES))
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

/// Gives person FROM the name TO (`logbook speakers rename`, the app's
/// voices page). Meetings already written keep the name they had.
pub fn rename(data: &Path, from: &str, to: &str) -> Result<()> {
    let mut lib = Library::load(data)?;
    lib.rename(from.trim(), to)?;
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
    fn the_next_likeliest_is_offered_when_the_likeliest_is_taken() {
        let mut lib = Library::new();
        lib.add("Luca", print(&[1.0, 0.0, 0.0]));
        lib.add("Anna", print(&[0.8, 0.6, 0.0]));
        lib.add("Bo", print(&[0.0, 0.0, 1.0]));
        let v = voice(&[0.8, 0.6, 0.0], 20.0);
        // Anna (1.0) beats Luca (0.8) by more than the margin.
        assert_eq!(lib.name(&v, &[]), Some(Named::Sure("Anna".into())));
        // Anna is someone else here: Luca is offered, not given.
        assert_eq!(
            lib.name(&v, &["Anna".into()]),
            Some(Named::Maybe("Luca".into()))
        );
        assert_eq!(lib.candidates(&v, &[], 3), vec!["Anna", "Luca"]);
        assert_eq!(lib.candidates(&v, &["Anna".into()], 3), vec!["Luca"]);
    }

    #[test]
    fn a_voice_unlike_the_name_is_doubted() {
        let mut lib = Library::new();
        lib.add("Luca", print(&[1.0, 0.0, 0.0]));
        lib.add("Anna", print(&[0.0, 1.0, 0.0]));
        // Luca again, through another microphone: no doubt.
        assert_eq!(lib.doubt("Luca", &voice(&[1.0, 0.3, 0.0], 20.0)), None);
        // A new person whose voice is no one's: no doubt either.
        assert_eq!(lib.doubt("Bo", &voice(&[0.0, 0.0, 1.0], 20.0)), None);
        // Anna's voice, named Luca or a new person.
        let anna = voice(&[0.1, 1.0, 0.0], 20.0);
        let d = lib.doubt("Luca", &anna).unwrap();
        assert_eq!(d.like.as_ref().unwrap().0, "Anna");
        assert!(d.own.unwrap() < MAYBE);
        assert_eq!(lib.doubt("Bo", &anna).unwrap().own, None);
        // A voice like no one, named Luca: unlike his.
        let d = lib.doubt("Luca", &voice(&[0.0, 0.0, 1.0], 20.0)).unwrap();
        assert_eq!(d.like, None);
    }

    #[test]
    fn a_meeting_names_from_its_expected_people_only() {
        let mut lib = Library::new();
        lib.add("Luca", print(&[1.0, 0.0, 0.0]));
        lib.add("Anna", print(&[0.0, 1.0, 0.0]));
        assert_eq!(lib.expected(&["anna ".into()]).unwrap(), vec!["Anna"]);
        assert!(lib.expected(&["Bo".into()]).is_err());
        let only = lib.clone().only(&["Anna".into()]);
        assert_eq!(only.name(&voice(&[1.0, 0.1, 0.0], 20.0), &[]), None);
        assert_eq!(lib.only(&[]).people.len(), 2);
    }

    #[test]
    fn a_person_is_renamed_but_never_onto_someone_else() {
        let mut lib = Library::new();
        lib.add("Luca", print(&[1.0, 0.0, 0.0]));
        lib.add("Anna", print(&[0.0, 1.0, 0.0]));
        // Someone else's name, in any case: refused, nothing changed.
        assert!(lib.rename("Luca", "anna").is_err());
        assert!(lib.rename("Luca", "  ").is_err());
        assert!(lib.rename("Bo", "Carla").is_err());
        assert_eq!(lib.people[0].name, "Luca");
        // A new name, and one's own in another case.
        lib.rename("Luca", " Luca Bianchi ").unwrap();
        lib.rename("Anna", "ANNA").unwrap();
        assert_eq!(lib.people[0].name, "Luca Bianchi");
        assert_eq!(lib.people[1].name, "ANNA");
        // The prints follow the person: their voice is still named.
        let v = voice(&[1.0, 0.1, 0.0], 20.0);
        assert_eq!(lib.name(&v, &[]), Some(Named::Sure("Luca Bianchi".into())));
    }

    #[test]
    fn a_person_was_last_heard_where_recognized_or_named() {
        let data = std::env::temp_dir().join(format!("logbook-heard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut lib = Library::new();
        let mut p = print(&[1.0, 0.0]);
        p.meeting = "20261027-091500".into();
        lib.add("Luca", p);
        lib.save(&data).unwrap();
        assert_eq!(lib.people[0].last_heard(), "20261027-091500");
        heard_in(
            &data,
            "20261030-100000",
            &["Luca".into(), "Speaker 2".into()],
        )
        .unwrap();
        // An older meeting finished late does not move it back.
        heard_in(&data, "20261028-100000", &["Luca".into()]).unwrap();
        let lib = Library::load(&data).unwrap();
        assert_eq!(lib.people[0].last_heard(), "20261030-100000");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_person_keeps_their_latest_prints() {
        let mut lib = Library::new();
        for i in 0..MAX_PRINTS + 2 {
            let mut p = print(&[1.0, 0.0]);
            p.meeting = i.to_string();
            lib.add("Luca", p);
        }
        let prints = &lib.people[0].prints;
        assert_eq!(prints.len(), MAX_PRINTS);
        assert_eq!(prints[0].meeting, "2");
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
        name(&data, "last", "2", "Luca", false).unwrap();
        name(&data, "20261027-091500", "speaker 1", "Anna", false).unwrap();
        let lib = Library::load(&data).unwrap();
        assert_eq!(lib.people.len(), 2);
        assert_eq!(lib.people[0].name, "Luca");
        assert!(name(&data, "last", "3", "Bo", false).is_err());
        // Anna's voice under Luca's name: refused, unless the person says so.
        let error = name(&data, "last", "1", "Luca", false).unwrap_err();
        assert!(error.downcast_ref::<Doubted>().is_some());
        assert_eq!(Library::load(&data).unwrap().people[0].prints.len(), 1);
        name(&data, "last", "1", "Luca", true).unwrap();
        assert_eq!(Library::load(&data).unwrap().people[0].prints.len(), 2);
        forget(&data, "Luca").unwrap();
        assert_eq!(Library::load(&data).unwrap().people.len(), 1);
        let _ = std::fs::remove_dir_all(&data);
    }
}
