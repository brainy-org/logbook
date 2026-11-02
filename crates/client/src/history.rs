//! The meetings the app keeps once they are published: the final
//! transcript, the notes, the report, so that they can be read again,
//! their speakers renamed and their report written again
//! (`<data>/meetings/<id>.json`, the id of the pending folder they came
//! from; the notes' screenshots in `<data>/meetings/<id>/`). No audio: it
//! is gone by then (pending.rs), and the voices stay in voices.rs, which
//! keeps them for the last meetings only.
//! The CLI keeps nothing here (`Saved::keep`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use logbook_engine::protocol::Utterance;
use serde::{Deserialize, Serialize};

use crate::brainy::Page;
use crate::transcript::Writer;
use crate::voices::{self, Library};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Kept {
    pub id: String,
    pub title: String,
    /// RFC 3339, local time: the displayed start, at the first word
    /// (`writer`'s offset), not the meeting's first sample.
    pub started: String,
    /// From the first word to the last.
    pub duration_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<Page>,
    /// The speakers' labels, as the report names them, and the displayed clock's
    /// offset (`Writer::shown`).
    pub writer: Writer,
    /// The final transcript, without the words' times, on the meeting's
    /// clock: shown through `writer`.
    pub utterances: Vec<Utterance>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// The report's blocks on the page, replaced when it is written again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub report_blocks: Vec<String>,
    /// Why the last report could not be written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_error: Option<String>,
}

/// A kept meeting as the list shows it.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub started: String,
    pub duration_s: f64,
    pub speakers: Vec<String>,
    pub page: Option<String>,
}

impl Kept {
    /// The transcript with its speakers' labels, as the report names them.
    pub fn transcript(&self) -> String {
        self.writer.text(&self.utterances, &[], &Library::default())
    }

    fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            title: self.title.clone(),
            started: self.started.clone(),
            duration_s: self.duration_s,
            speakers: self.writer.speakers().into_iter().map(|s| s.2).collect(),
            page: self.page.as_ref().map(|p| p.url.clone()),
        }
    }
}

fn dir(data: &Path) -> PathBuf {
    data.join("meetings")
}

/// The screenshots of meeting `id`'s notes (shots.rs).
pub fn images(data: &Path, id: &str) -> Result<PathBuf> {
    Ok(path(data, id)?.with_extension(""))
}

fn path(data: &Path, id: &str) -> Result<PathBuf> {
    // The id names a file: nothing that walks out of the folder.
    anyhow::ensure!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "{id:?} is not a meeting"
    );
    Ok(dir(data).join(format!("{id}.json")))
}

pub fn save(data: &Path, kept: &Kept) -> Result<()> {
    let mut kept = kept.clone();
    for u in &mut kept.utterances {
        u.words.clear();
    }
    std::fs::create_dir_all(dir(data))?;
    let path = path(data, &kept.id)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&kept)?)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .with_context(|| format!("could not write {}", path.display()))
}

pub fn load(data: &Path, id: &str) -> Result<Kept> {
    let path = path(data, id)?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("no meeting {id} in {}", dir(data).display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("could not read {}", path.display()))
}

/// Every kept meeting, the latest first.
pub fn list(data: &Path) -> Result<Vec<Summary>> {
    let entries = match std::fs::read_dir(dir(data)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(kept) = serde_json::from_slice::<Kept>(&std::fs::read(&path)?)
        {
            out.push(kept.summary());
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

/// A data folder of its own for trashed meetings: `trash/meetings/…`.
fn trash_dir(data: &Path) -> PathBuf {
    data.join("trash")
}

/// Moves `from`'s meeting `id` (its file and its screenshots) to `to`.
fn move_meeting(from: &Path, to: &Path, id: &str) -> Result<()> {
    let file = path(from, id)?;
    let shots = images(from, id)?;
    std::fs::create_dir_all(dir(to))?;
    std::fs::rename(&file, path(to, id)?)
        .with_context(|| format!("could not move {}", file.display()))?;
    match std::fs::rename(&shots, images(to, id)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("could not move {}", shots.display()))
        }
        _ => Ok(()),
    }
}

/// Forgets a meeting here, with its screenshots, as the app does: into
/// `<data>/trash` first, from where `restore` puts it back until `purge`
/// (or `empty_trash`, at the app's next start) removes it. Its page in
/// brainy stays.
pub fn trash(data: &Path, id: &str) -> Result<()> {
    move_meeting(data, &trash_dir(data), id)
}

/// A meeting `trash` took, back in the list.
pub fn restore(data: &Path, id: &str) -> Result<()> {
    move_meeting(&trash_dir(data), data, id)
}

/// Removes a trashed meeting for good, with the voices kept from it to
/// be named (voices.rs): a meeting deleted leaves no voice of the people
/// in it but those named, which stay known by name.
pub fn purge(data: &Path, id: &str) -> Result<()> {
    voices::forget_meeting(data, id)?;
    let trash = trash_dir(data);
    let _ = std::fs::remove_dir_all(images(&trash, id)?);
    match std::fs::remove_file(path(&trash, id)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Removes every trashed meeting: those whose undo was not waited for.
pub fn empty_trash(data: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir(&trash_dir(data))) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) {
                let _ = voices::forget_meeting(data, id);
            }
        }
    }
    let _ = std::fs::remove_dir_all(trash_dir(data));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meeting_is_kept_listed_and_forgotten() {
        let data = std::env::temp_dir().join(format!("logbook-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut writer = Writer::new("Marco", 0.0);
        let utterances = vec![Utterance {
            channel: "system".into(),
            speaker: Some(0),
            start: 4.0,
            end: 5.0,
            text: "Ciao.".into(),
            words: Vec::new(),
        }];
        writer.body(&utterances, &[], &Library::default());
        let kept = Kept {
            id: "20261027-091500".into(),
            title: "Weekly".into(),
            started: "2026-10-27T09:15:00+01:00".into(),
            duration_s: 5.0,
            page: None,
            writer,
            utterances,
            warnings: Vec::new(),
            notes: "- budget".into(),
            report: None,
            report_blocks: Vec::new(),
            report_error: None,
        };
        save(&data, &kept).unwrap();
        let listed = list(&data).unwrap();
        assert_eq!(listed[0].speakers, ["Speaker 1"]);
        let mut loaded = load(&data, &kept.id).unwrap();
        assert!(loaded.transcript().starts_with("**Speaker 1** · 00:04"));
        assert!(loaded.writer.rename("Speaker 1", "Luca"));
        assert!(loaded.transcript().starts_with("**Luca** · 00:04"));
        assert!(load(&data, "../x").is_err());
        trash(&data, &kept.id).unwrap();
        assert!(list(&data).unwrap().is_empty());
        restore(&data, &kept.id).unwrap();
        assert_eq!(list(&data).unwrap().len(), 1);
        // Its speakers' voices, kept to be named, go with it for good.
        let heard = data.join("voices").join(format!("{}.json", kept.id));
        std::fs::create_dir_all(heard.parent().unwrap()).unwrap();
        std::fs::write(&heard, "{}").unwrap();
        trash(&data, &kept.id).unwrap();
        assert!(heard.exists());
        purge(&data, &kept.id).unwrap();
        assert!(restore(&data, &kept.id).is_err());
        assert!(!heard.exists());
        // Or at the next start, for one whose undo was not waited for.
        std::fs::write(&heard, "{}").unwrap();
        save(&data, &kept).unwrap();
        trash(&data, &kept.id).unwrap();
        empty_trash(&data);
        assert!(!heard.exists());
        let _ = std::fs::remove_dir_all(&data);
    }
}
