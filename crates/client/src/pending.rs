//! A meeting kept on this computer until its final transcript is published
//! (docs/decisions/2026-10-24-the-client-keeps-the-meeting.md).
//!
//! One folder per meeting under `<data>/pending`: `meeting.json` (the
//! channels, when it started, how to publish it, the server's pass id once
//! known, the brainy page and how far it is written), one raw 16 kHz s16le
//! file per channel on the session's clock, written as the audio is
//! captured, `segments.jsonl` (the final segments as they come during the
//! meeting), `notes.md` (what the person wrote during it, for the report)
//! and `final.json` once the final pass is complete (the audio is
//! deleted then: it is no longer needed). The folder goes away once the
//! transcript is published; until then `logbook finish` picks it up again.
//! No brainy token is ever written.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use logbook_engine::protocol::ChannelConfig;
use serde::{Deserialize, Serialize};

use crate::brainy::Page;
use crate::remote::Finalized;
use crate::transcript::Writer;
use logbook_engine::protocol::{SpeakerVoice, Utterance};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Meeting {
    /// RFC 3339, local time.
    pub started: String,
    pub channels: Vec<ChannelConfig>,
    /// The server's model may clean the transcript up (--polish).
    #[serde(default)]
    pub polish: bool,
    pub publish: Saved,
    /// The server's final pass for it, once one is known to be running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_id: Option<String>,
    /// The brainy page, once created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<Page>,
    /// The page's names and last speaker, to write its next part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<Writer>,
    /// Segments starting before this are on the page.
    #[serde(default)]
    pub published_until: f64,
    /// The speakers' voices as last sent: their names come from them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<SpeakerVoice>,
}

/// How to publish the transcript, as asked when the meeting started.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Saved {
    pub brainy: bool,
    pub brainy_url: Option<String>,
    pub parent: Option<String>,
    pub title: Option<String>,
    pub me: String,
    /// Have a language model write the meeting's report (engine report.rs).
    #[serde(default)]
    pub report: bool,
    /// Keep the transcript, the notes and the report in the history
    /// (history.rs) once published: the app does, the CLI does not.
    #[serde(default)]
    pub keep: bool,
}

pub struct Pending {
    pub dir: PathBuf,
    pub meeting: Meeting,
}

/// The audio being written, one file per channel.
pub struct Spool {
    files: Vec<BufWriter<File>>,
}

impl Pending {
    /// A new folder named after the meeting's start (`20261024-174012`).
    pub fn create(root: &Path, name: &str, meeting: Meeting) -> Result<(Self, Spool)> {
        let root = root.join("pending");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("could not create {}", root.display()))?;
        let mut dir = root.join(name);
        let mut n = 1;
        while let Err(error) = std::fs::create_dir(&dir) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error).with_context(|| format!("could not create {}", dir.display()));
            }
            n += 1;
            dir = root.join(format!("{name}-{n}"));
        }
        let pending = Self { dir, meeting };
        pending.save()?;
        let files = (0..pending.meeting.channels.len())
            .map(|i| {
                let path = pending.audio_path(i);
                File::create(&path)
                    .map(BufWriter::new)
                    .with_context(|| format!("could not create {}", path.display()))
            })
            .collect::<Result<_>>()?;
        Ok((pending, Spool { files }))
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("meeting.json");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            meeting: serde_json::from_str(&text)
                .with_context(|| format!("could not read {}", path.display()))?,
        })
    }

    /// Every pending meeting, oldest first.
    pub fn list(root: &Path) -> Result<Vec<PathBuf>> {
        let root = root.join("pending");
        let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&root) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("meeting.json").is_file())
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", root.display()));
            }
        };
        dirs.sort();
        Ok(dirs)
    }

    pub fn set_final_id(&mut self, id: Option<String>) -> Result<()> {
        self.meeting.final_id = id;
        self.save()
    }

    /// Keeps the final pass's result; the audio is no longer needed.
    pub fn save_final(&self, finalized: &Finalized) -> Result<()> {
        let path = self.dir.join("final.json");
        let tmp = self.dir.join("final.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(finalized)?)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .with_context(|| format!("could not write {}", path.display()))?;
        for i in 0..self.meeting.channels.len() {
            let _ = std::fs::remove_file(self.audio_path(i));
        }
        Ok(())
    }

    /// Final segments that came during the meeting.
    pub fn add_segments(&self, utterances: &[Utterance]) -> Result<()> {
        let path = self.dir.join("segments.jsonl");
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("could not write {}", path.display()))?;
        let mut text = String::new();
        for u in utterances {
            text.push_str(&serde_json::to_string(u)?);
            text.push('\n');
        }
        file.write_all(text.as_bytes())
            .with_context(|| format!("could not write {}", path.display()))
    }

    /// Every final segment that came during the meeting, by start.
    pub fn segments(&self) -> Result<Vec<Utterance>> {
        let path = self.dir.join("segments.jsonl");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        text.lines()
            // A line cut by a crash is dropped: its segment comes again
            // with the final pass.
            .filter_map(|line| serde_json::from_str(line).ok())
            .map(Ok)
            .collect()
    }

    pub fn final_result(&self) -> Result<Option<Finalized>> {
        let path = self.dir.join("final.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                    format!("could not read {}", path.display())
                })?))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
        }
    }

    /// Every channel's samples, in channel order.
    pub fn audio(&self) -> Result<Vec<Vec<i16>>> {
        (0..self.meeting.channels.len())
            .map(|i| {
                let path = self.audio_path(i);
                let mut bytes = Vec::new();
                File::open(&path)
                    .and_then(|mut f| f.read_to_end(&mut bytes))
                    .with_context(|| format!("could not read {}", path.display()))?;
                Ok(bytes
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect())
            })
            .collect()
    }

    /// What the person wrote during the meeting.
    pub fn save_notes(&self, notes: &str) -> Result<()> {
        let path = self.dir.join("notes.md");
        std::fs::write(&path, notes).with_context(|| format!("could not write {}", path.display()))
    }

    /// The notes, empty if none were written.
    pub fn notes(&self) -> String {
        std::fs::read_to_string(self.dir.join("notes.md")).unwrap_or_default()
    }

    /// The transcript as a file to read while it waits to be published.
    pub fn save_transcript(&self, markdown: &str) -> Result<PathBuf> {
        let path = self.dir.join("transcript.md");
        std::fs::write(&path, markdown)
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(path)
    }

    /// The folder's name: the meeting's id (`20261024-174012`).
    pub fn id(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The data folder the meeting is pending in.
    pub fn data(&self) -> &Path {
        self.dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("."))
    }

    pub fn remove(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.dir)
            .with_context(|| format!("could not remove {}", self.dir.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = self.dir.join("meeting.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.meeting)?)
            .with_context(|| format!("could not write {}", path.display()))
    }

    fn audio_path(&self, channel: usize) -> PathBuf {
        self.dir.join(format!("{channel}.pcm"))
    }
}

impl Spool {
    pub fn write(&mut self, channel: usize, samples: &[i16]) -> Result<()> {
        let Some(file) = self.files.get_mut(channel) else {
            return Ok(());
        };
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        Ok(file.write_all(&bytes)?)
    }

    pub fn finish(self) -> Result<()> {
        for mut file in self.files {
            file.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meeting_survives_until_it_is_removed() {
        let root = std::env::temp_dir().join(format!("logbook-pending-{}", std::process::id()));
        let meeting = Meeting {
            started: "2026-10-24T17:40:00+02:00".into(),
            channels: vec![ChannelConfig {
                name: "mic".into(),
                diarize: false,
                echo_of: None,
            }],
            polish: false,
            publish: Saved {
                brainy: false,
                brainy_url: None,
                parent: None,
                title: None,
                me: "Me".into(),
                report: false,
                keep: false,
            },
            final_id: None,
            page: None,
            writer: None,
            published_until: 0.0,
            voices: Vec::new(),
        };
        let (mut pending, mut spool) = Pending::create(&root, "m", meeting.clone()).unwrap();
        let (second, _) = Pending::create(&root, "m", meeting).unwrap();
        assert!(second.dir.ends_with("m-2"));
        second.remove().unwrap();
        spool.write(0, &[1, -2, 3]).unwrap();
        spool.finish().unwrap();
        assert_eq!(pending.notes(), "");
        pending.save_notes("- budget").unwrap();
        assert_eq!(pending.notes(), "- budget");
        pending.set_final_id(Some("abc".into())).unwrap();

        let dirs = Pending::list(&root).unwrap();
        assert_eq!(dirs.len(), 1);
        let opened = Pending::open(&dirs[0]).unwrap();
        assert_eq!(opened.meeting.final_id.as_deref(), Some("abc"));
        assert_eq!(opened.audio().unwrap(), vec![vec![1, -2, 3]]);
        assert!(opened.final_result().unwrap().is_none());
        let finalized = Finalized {
            utterances: Vec::new(),
            warnings: vec!["w".into()],
            voices: Vec::new(),
        };
        opened.save_final(&finalized).unwrap();
        assert_eq!(opened.final_result().unwrap().unwrap().warnings, ["w"]);
        assert!(opened.audio().is_err());
        opened.remove().unwrap();
        assert!(Pending::list(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
