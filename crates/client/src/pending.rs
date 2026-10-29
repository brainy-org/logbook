//! A meeting kept on this computer until its final transcript is published
//! (docs/decisions/2026-10-24-the-client-keeps-the-meeting.md).
//!
//! One folder per meeting under `<data>/pending`: `meeting.json` (the
//! channels, when it started, how to publish it, the server's pass id once
//! known, how far the final segments have come, the brainy page and how
//! far it is written), each channel's audio as raw 16 kHz s16le on the
//! meeting's clock, written as it is captured, in files of 5 minutes
//! (`<channel>-<n>.pcm`, engine recording.rs's layout), `segments.jsonl`
//! (the final segments as they come during the meeting), `notes.md` (what
//! the person wrote during it, for the report) with the screenshots it
//! names (shots.rs), `final.json` once the
//! final pass is complete, and `report.md` once the report is written,
//! until it is on the page. `meeting.json` also records the tasks found
//! and how many are rows in brainy (tasks.rs), so that none is written
//! twice. The audio is kept only from where the final
//! segments have come (`trim`): a final pass taken up again starts there,
//! and a meeting of days keeps minutes of it; all of it goes with
//! `final.json`. The folder goes away once the transcript is published;
//! until then `logbook finish` picks it up again. No brainy token is ever
//! written.
//!
//! `.lock` is held (an OS lock on the file, gone with the process) by the
//! process that records the meeting or finishes it: `claim` leaves a
//! meeting another holds alone, so `finish` (or the app) never finishes a
//! meeting still being recorded, nor one being finished elsewhere.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use logbook_engine::protocol::ChannelConfig;
use logbook_engine::protocol::SAMPLE_RATE;
use logbook_engine::recording::{RangeReader, SEGMENT, segment_path};
use serde::{Deserialize, Serialize};

use crate::brainy::Page;
use crate::remote::Finalized;
use crate::tasks::TaskLog;
use crate::transcript::Writer;
use logbook_engine::protocol::{SpeakerVoice, Utterance};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Meeting {
    /// RFC 3339, local time.
    pub started: String,
    pub channels: Vec<ChannelConfig>,
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
    /// Final segments starting before this are in `segments.jsonl`: a
    /// final pass taken up again starts here, and the audio before it is
    /// gone. `f64::MAX` once they all are.
    #[serde(default)]
    pub final_until: f64,
    /// The speakers' voices as last sent: their names come from them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<SpeakerVoice>,
    /// The tasks the model found, once it has, and how many of them are
    /// rows of the collection: what a later `finish` takes up, so that no
    /// row is written twice (tasks.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TaskLog>,
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
    /// The brainy collection (its id) the meeting's tasks go to as rows
    /// (tasks.rs); none: no tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<String>,
}

pub struct Pending {
    pub dir: PathBuf,
    pub meeting: Meeting,
    /// `.lock`, held while this process records or finishes the meeting;
    /// None when only read (`open`).
    lock: Option<File>,
}

/// The audio being written: each channel's file being written and how
/// many samples it has had. Unbuffered: what a live session reads back to
/// take the final pass up again (remote.rs) is on disk once written here.
pub struct Spool {
    stems: Vec<PathBuf>,
    files: Vec<Option<File>>,
    written: Vec<u64>,
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
        let lock = lock(&dir)?.context("a new folder is locked by another process")?;
        let pending = Self {
            dir,
            meeting,
            lock: Some(lock),
        };
        pending.save()?;
        let count = pending.meeting.channels.len();
        let spool = Spool {
            stems: (0..count).map(|i| stem(&pending.dir, i)).collect(),
            files: (0..count).map(|_| None).collect(),
            written: vec![0; count],
        };
        Ok((pending, spool))
    }

    /// The meeting, to read: whoever holds it may change it meanwhile.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("meeting.json");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            meeting: serde_json::from_str(&text)
                .with_context(|| format!("could not read {}", path.display()))?,
            lock: None,
        })
    }

    /// The meeting, to finish: held by this process until dropped. None
    /// if another holds it (it is being recorded, or finished elsewhere).
    pub fn claim(dir: &Path) -> Result<Option<Self>> {
        let Some(lock) = lock(dir)? else {
            return Ok(None);
        };
        let mut pending = Self::open(dir)?;
        pending.lock = Some(lock);
        for i in 0..pending.meeting.channels.len() {
            split_whole_file(&pending.dir, i)?;
        }
        Ok(Some(pending))
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

    /// Whether this process holds the meeting's `.lock`: only then may it
    /// change it.
    pub fn held(&self) -> bool {
        self.lock.is_some()
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
        self.trim(f64::MAX);
        Ok(())
    }

    /// Final segments have come up to `until`: noted, and the audio before
    /// it deleted (a file whose last sample is before it).
    pub fn set_final_until(&mut self, until: f64) -> Result<()> {
        if until <= self.meeting.final_until {
            return Ok(());
        }
        self.meeting.final_until = until;
        self.save()?;
        self.trim(until);
        Ok(())
    }

    /// Deletes the files of audio wholly before `until` (all of them at
    /// `f64::MAX`).
    fn trim(&self, until: f64) {
        let before = (until.max(0.0) * SAMPLE_RATE as f64).min(u64::MAX as f64) as u64;
        for i in 0..self.meeting.channels.len() {
            let stem = stem(&self.dir, i);
            for n in segments(&self.dir, i) {
                if until == f64::MAX || (n + 1).saturating_mul(SEGMENT) <= before {
                    let _ = std::fs::remove_file(segment_path(&stem, n));
                }
            }
        }
    }

    /// A channel's audio, by range on the meeting's clock, from
    /// `final_until` on.
    pub fn reader(&self, channel: usize) -> RangeReader {
        reader(&self.dir, channel)
    }

    /// Samples of a channel kept so far, counted from the meeting's start.
    pub fn samples(&self, channel: usize) -> u64 {
        let stem = stem(&self.dir, channel);
        match segments(&self.dir, channel).last() {
            Some(&n) => {
                let bytes = std::fs::metadata(segment_path(&stem, n)).map_or(0, |m| m.len());
                n * SEGMENT + bytes / 2
            }
            None => 0,
        }
    }

    /// Final segments that came during the meeting; returns where in
    /// `segments.jsonl` they start, for `segments_from`.
    pub fn add_segments(&self, utterances: &[Utterance]) -> Result<u64> {
        let path = self.dir.join("segments.jsonl");
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("could not write {}", path.display()))?;
        let at = file
            .metadata()
            .with_context(|| format!("could not read {}", path.display()))?
            .len();
        let mut text = String::new();
        for u in utterances {
            text.push_str(&serde_json::to_string(u)?);
            text.push('\n');
        }
        file.write_all(text.as_bytes())
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(at)
    }

    /// The final segments from byte `at` of `segments.jsonl` (as
    /// `add_segments` returned it) to its end: the ones not on the page
    /// yet, read again instead of held in memory while brainy fails.
    pub fn segments_from(&self, at: u64) -> Result<Vec<Utterance>> {
        let path = self.dir.join("segments.jsonl");
        let mut file =
            File::open(&path).with_context(|| format!("could not read {}", path.display()))?;
        file.seek(SeekFrom::Start(at))
            .with_context(|| format!("could not read {}", path.display()))?;
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.with_context(|| format!("could not read {}", path.display()))?;
            // A line cut by a crash is dropped, as in `segments`.
            if let Ok(u) = serde_json::from_str(&line) {
                out.push(u);
            }
        }
        Ok(out)
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

    /// What the person wrote during the meeting.
    pub fn save_notes(&self, notes: &str) -> Result<()> {
        let path = self.dir.join("notes.md");
        std::fs::write(&path, notes).with_context(|| format!("could not write {}", path.display()))
    }

    /// The notes, empty if none were written.
    pub fn notes(&self) -> String {
        std::fs::read_to_string(self.dir.join("notes.md")).unwrap_or_default()
    }

    /// The report written, kept until it is on the page: a meeting
    /// finished again does not ask the model for it again.
    pub fn save_report(&self, markdown: &str) -> Result<()> {
        let path = self.dir.join("report.md");
        std::fs::write(&path, markdown)
            .with_context(|| format!("could not write {}", path.display()))
    }

    /// The report kept by `save_report`, if any.
    pub fn report(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("report.md"))
            .ok()
            .filter(|r| !r.trim().is_empty())
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

    pub fn remove(&mut self) -> Result<()> {
        // No longer listed, then let go (Windows removes no open file),
        // then gone.
        let path = self.dir.join("meeting.json");
        std::fs::remove_file(&path)
            .with_context(|| format!("could not remove {}", path.display()))?;
        self.lock = None;
        std::fs::remove_dir_all(&self.dir)
            .with_context(|| format!("could not remove {}", self.dir.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = self.dir.join("meeting.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.meeting)?)
            .with_context(|| format!("could not write {}", path.display()))
    }
}

/// `dir`'s `.lock`, held; None if another holds it.
fn lock(dir: &Path) -> Result<Option<File>> {
    let path = dir.join(".lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    match try_lock(&file) {
        Ok(true) => Ok(Some(file)),
        Ok(false) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not lock {}", path.display())),
    }
}

/// An exclusive lock on `file` if no one holds one: false if someone does.
#[cfg(not(target_os = "android"))]
fn try_lock(file: &File) -> std::io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// On Android `File::try_lock` answers "not supported" (std does not
/// implement it there), while the `flock` it calls elsewhere is there.
#[cfg(target_os = "android")]
fn try_lock(file: &File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: a valid descriptor, owned by `file` for the whole call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(error),
    }
}

/// A channel's audio in the pending folder `dir`, while it is written.
pub fn reader(dir: &Path, channel: usize) -> RangeReader {
    RangeReader::new(&stem(dir, channel))
}

/// `<dir>/<channel>`: its files are `<channel>-<n>.pcm`.
fn stem(dir: &Path, channel: usize) -> PathBuf {
    dir.join(channel.to_string())
}

/// The numbers of a channel's files on disk, increasing.
fn segments(dir: &Path, channel: usize) -> Vec<u64> {
    let prefix = format!("{channel}-");
    let mut found: Vec<u64> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_prefix(&prefix)?
                .strip_suffix(".pcm")?
                .parse()
                .ok()
        })
        .collect();
    found.sort_unstable();
    found
}

/// A meeting kept before the audio was in files of 5 minutes had one file
/// per channel (`<channel>.pcm`): cut into them, a part at a time.
fn split_whole_file(dir: &Path, channel: usize) -> Result<()> {
    let whole = dir.join(format!("{channel}.pcm"));
    let Ok(mut file) = File::open(&whole) else {
        return Ok(());
    };
    let stem = stem(dir, channel);
    let mut buf = vec![0u8; SEGMENT as usize * 2];
    let mut n = 0;
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            let read = file.read(&mut buf[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        let path = segment_path(&stem, n);
        std::fs::write(&path, &buf[..filled & !1])
            .with_context(|| format!("could not write {}", path.display()))?;
        n += 1;
        if filled < buf.len() {
            break;
        }
    }
    std::fs::remove_file(&whole).with_context(|| format!("could not delete {}", whole.display()))
}

impl Spool {
    pub fn write(&mut self, channel: usize, samples: &[i16]) -> Result<()> {
        let Some(stem) = self.stems.get(channel) else {
            return Ok(());
        };
        let mut rest = samples;
        while !rest.is_empty() {
            let written = self.written[channel];
            let n = written / SEGMENT;
            let file = match &mut self.files[channel] {
                Some(file) if !written.is_multiple_of(SEGMENT) => file,
                slot => {
                    let path = segment_path(stem, n);
                    let file = File::options()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .with_context(|| format!("could not create {}", path.display()))?;
                    slot.insert(file)
                }
            };
            let room = (SEGMENT - written % SEGMENT) as usize;
            let (now, later) = rest.split_at(room.min(rest.len()));
            let bytes: Vec<u8> = now.iter().flat_map(|s| s.to_le_bytes()).collect();
            file.write_all(&bytes)?;
            self.written[channel] += now.len() as u64;
            rest = later;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
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
            publish: Saved {
                brainy: false,
                brainy_url: None,
                parent: None,
                title: None,
                me: "Me".into(),
                report: false,
                keep: false,
                tasks: None,
            },
            final_id: None,
            page: None,
            writer: None,
            published_until: 0.0,
            final_until: 0.0,
            voices: Vec::new(),
            tasks: None,
        };
        let (mut pending, mut spool) = Pending::create(&root, "m", meeting.clone()).unwrap();
        let (mut second, _) = Pending::create(&root, "m", meeting).unwrap();
        assert!(second.dir.ends_with("m-2"));
        second.remove().unwrap();
        spool.write(0, &[1, -2, 3]).unwrap();
        spool.finish().unwrap();
        assert_eq!(pending.samples(0), 3);
        assert_eq!(pending.notes(), "");
        pending.save_notes("- budget").unwrap();
        assert_eq!(pending.notes(), "- budget");
        pending.set_final_id(Some("abc".into())).unwrap();

        let dirs = Pending::list(&root).unwrap();
        assert_eq!(dirs.len(), 1);
        // Being recorded: read, not claimed, until its recorder lets go.
        assert!(Pending::claim(&dirs[0]).unwrap().is_none());
        assert!(Pending::open(&dirs[0]).is_ok());
        drop(pending);
        let mut opened = Pending::claim(&dirs[0]).unwrap().unwrap();
        assert!(Pending::claim(&dirs[0]).unwrap().is_none(), "claimed once");
        assert_eq!(opened.meeting.final_id.as_deref(), Some("abc"));
        let read = opened.reader(0).read(0.0, 3.0 / 16_000.0).unwrap();
        assert_eq!(read.len(), 3);
        assert!((read[1] * 32768.0 + 2.0).abs() < 1e-3);
        assert!(opened.final_result().unwrap().is_none());
        let finalized = Finalized {
            warnings: vec!["w".into()],
            ..Finalized::default()
        };
        opened.save_final(&finalized).unwrap();
        assert_eq!(opened.final_result().unwrap().unwrap().warnings, ["w"]);
        assert_eq!(opened.samples(0), 0);
        opened.remove().unwrap();
        assert!(Pending::list(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_audio_goes_as_the_final_segments_come() {
        let root = std::env::temp_dir().join(format!("logbook-spool-{}", std::process::id()));
        let meeting: Meeting = serde_json::from_value(serde_json::json!({
            "started": "2026-10-28T09:00:00+01:00",
            "channels": [{"name": "mic", "diarize": false}],
            "publish": {"brainy": false, "brainy_url": null, "parent": null, "title": null, "me": "Me"},
        }))
        .unwrap();
        let (mut pending, mut spool) = Pending::create(&root, "m", meeting).unwrap();
        // 11 minutes, written as a live meeting does: 100 ms at a time.
        let frame = vec![7i16; 1600];
        for _ in 0..11 * 600 {
            spool.write(0, &frame).unwrap();
        }
        assert_eq!(pending.samples(0), 11 * 60 * 16_000);
        assert_eq!(segments(&pending.dir, 0), [0, 1, 2]);
        // Read across the first file's end, while it is written.
        let pcm = pending.reader(0).read(299.5, 300.5).unwrap();
        assert_eq!(pcm.len(), 16_000);
        // Final segments up to 7 minutes: the first file (0–5 min) goes.
        pending.set_final_until(420.0).unwrap();
        assert_eq!(segments(&pending.dir, 0), [1, 2]);
        assert_eq!(pending.samples(0), 11 * 60 * 16_000);
        assert_eq!(pending.reader(0).read(420.0, 421.0).unwrap().len(), 16_000);
        assert_eq!(
            Pending::open(&pending.dir).unwrap().meeting.final_until,
            420.0
        );
        pending.save_final(&Finalized::default()).unwrap();
        assert!(segments(&pending.dir, 0).is_empty());

        // A meeting kept with one file per channel is cut into files
        // when it is claimed to be finished.
        let whole: Vec<u8> = (0..SEGMENT + 10).flat_map(|_| 3i16.to_le_bytes()).collect();
        std::fs::write(pending.dir.join("0.pcm"), whole).unwrap();
        let dir = pending.dir.clone();
        drop(pending);
        let mut opened = Pending::claim(&dir).unwrap().unwrap();
        assert_eq!(segments(&opened.dir, 0), [0, 1]);
        assert_eq!(opened.samples(0), SEGMENT + 10);
        assert!(!opened.dir.join("0.pcm").exists());
        opened.remove().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
