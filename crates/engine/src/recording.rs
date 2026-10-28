//! A session's audio on disk, for the final pass, and only for it.
//!
//! Raw 16 kHz mono s16le, written as the audio arrives (on the session's
//! clock, as the client sends it), in files of `SEGMENT` per channel: the
//! progressive final pass reads a range a minute or so behind the meeting,
//! and the files it has passed are deleted (`forget_before`), so the
//! server keeps minutes of a meeting, not hours. What is left is deleted
//! when dropped: when the session ends (with or without a final pass, or
//! with the connection lost), and once a final pass has read an upload,
//! whether it succeeded or not. A server that dies does not drop them:
//! `claim` deletes them when it starts again. Nothing else on the server
//! keeps a meeting's audio.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const RATE: u64 = 16_000;
/// Samples per file: 5 minutes, 9.6 MB. The client's pending folder
/// keeps a meeting's audio in the same files (client pending.rs).
pub const SEGMENT: u64 = 300 * RATE;

/// Takes `dir` for this server, and deletes the recordings a server that
/// died left there: no session of this one owns them, and their clients
/// upload their own copy. The folder is held while the returned file is
/// open; a second server on it is refused, before it deletes the first
/// one's live recordings.
pub fn claim(dir: &Path) -> Result<(File, usize)> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join(".lock");
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    if lock.try_lock().is_err() {
        bail!(
            "another logbook-server records in {}: give each its own --recordings",
            dir.display()
        );
    }
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("could not read {}", dir.display()))?;
    let mut cleared = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pcm") {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not delete {}", path.display()))?;
            cleared += 1;
        }
    }
    Ok((lock, cleared))
}

pub struct Recording {
    tracks: Vec<Track>,
}

/// One channel: its files, the last one being written.
struct Track {
    /// `<dir>/<session>-<channel>`; a file adds `-<n>.pcm`.
    stem: PathBuf,
    writer: Option<BufWriter<File>>,
    written: u64,
    /// The files still on disk: (n, path).
    files: VecDeque<(u64, PathBuf)>,
}

impl Track {
    fn path(&self, n: u64) -> PathBuf {
        segment_path(&self.stem, n)
    }
}

/// File `n` of a channel's audio: samples [n·SEGMENT, (n+1)·SEGMENT).
pub fn segment_path(stem: &Path, n: u64) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(format!("-{n:05}.pcm"));
    PathBuf::from(name)
}

impl Drop for Track {
    fn drop(&mut self) {
        self.writer = None;
        for (_, path) in self.files.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Recording {
    pub fn create(dir: &Path, session: &str, channels: usize) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        let tracks = (0..channels)
            .map(|i| Track {
                stem: dir.join(format!("{session}-{i}")),
                writer: None,
                written: 0,
                files: VecDeque::new(),
            })
            .collect();
        Ok(Self { tracks })
    }

    pub fn write(&mut self, channel: usize, pcm: &[f32]) -> Result<()> {
        let Some(track) = self.tracks.get_mut(channel) else {
            return Ok(());
        };
        let mut rest = pcm;
        while !rest.is_empty() {
            let n = track.written / SEGMENT;
            if track.written.is_multiple_of(SEGMENT) || track.writer.is_none() {
                if let Some(mut w) = track.writer.take() {
                    w.flush()?;
                }
                let path = track.path(n);
                let file = File::options()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("could not create {}", path.display()))?;
                if track.files.back().is_none_or(|&(last, _)| last != n) {
                    track.files.push_back((n, path));
                }
                track.writer = Some(BufWriter::new(file));
            }
            let room = (SEGMENT - track.written % SEGMENT) as usize;
            let (now, later) = rest.split_at(room.min(rest.len()));
            let bytes: Vec<u8> = now
                .iter()
                .flat_map(|s| ((s * 32768.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes())
                .collect();
            track
                .writer
                .as_mut()
                .expect("opened above")
                .write_all(&bytes)
                .with_context(|| format!("could not write {}", track.path(n).display()))?;
            track.written += now.len() as u64;
            rest = later;
        }
        Ok(())
    }

    /// Writes what is buffered, so that a `reader` sees it.
    pub fn flush(&mut self) -> Result<()> {
        for track in &mut self.tracks {
            if let Some(w) = &mut track.writer {
                w.flush()
                    .with_context(|| format!("could not write {}", track.stem.display()))?;
            }
        }
        Ok(())
    }

    /// Reads a channel's audio by range while it is written (flush first).
    pub fn reader(&self, channel: usize) -> Option<RangeReader> {
        self.tracks.get(channel).map(|t| RangeReader {
            stem: t.stem.clone(),
        })
    }

    /// Deletes the channel's files that end before `time`: the final pass
    /// is past them. The file being written stays.
    pub fn forget_before(&mut self, channel: usize, time: f64) {
        let Some(track) = self.tracks.get_mut(channel) else {
            return;
        };
        let before = (time.max(0.0) * RATE as f64) as u64;
        let current = track.written / SEGMENT;
        while let Some(&(n, _)) = track.files.front() {
            if n >= current || (n + 1) * SEGMENT > before {
                break;
            }
            let (_, path) = track.files.pop_front().unwrap();
            let _ = std::fs::remove_file(path);
        }
    }

    /// The channels, complete, in channel order.
    pub fn finish(mut self) -> Result<Vec<RecordedFile>> {
        let mut out = Vec::with_capacity(self.tracks.len());
        for track in &mut self.tracks {
            if let Some(mut w) = track.writer.take() {
                w.flush()
                    .with_context(|| format!("could not write {}", track.stem.display()))?;
            }
            out.push(RecordedFile {
                stem: track.stem.clone(),
                paths: track.files.drain(..).map(|(_, p)| p).collect(),
                samples: track.written,
            });
        }
        Ok(out)
    }
}

/// A range of a channel being recorded. It does not keep the files: a
/// session that ends deletes them, and a read after that fails.
#[derive(Clone)]
pub struct RangeReader {
    stem: PathBuf,
}

impl RangeReader {
    /// Reads the files of `stem` (`<stem>-<n>.pcm`), whoever wrote them.
    pub fn new(stem: &Path) -> Self {
        Self {
            stem: stem.to_path_buf(),
        }
    }

    /// Samples [from, to) in seconds, as floats; shorter where the files are.
    pub fn read(&self, from: f64, to: f64) -> Result<Vec<f32>> {
        let mut at = (from.max(0.0) * RATE as f64) as u64;
        let end = (to.max(0.0) * RATE as f64) as u64;
        let mut out = Vec::with_capacity(end.saturating_sub(at) as usize);
        while at < end {
            let n = at / SEGMENT;
            let path = segment_path(&self.stem, n);
            let mut file =
                File::open(&path).with_context(|| format!("could not read {}", path.display()))?;
            file.seek(SeekFrom::Start((at % SEGMENT) * 2))?;
            let want = end.min((n + 1) * SEGMENT) - at;
            let mut bytes = Vec::with_capacity(want as usize * 2);
            file.take(want * 2).read_to_end(&mut bytes)?;
            out.extend(
                bytes
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
            );
            if (bytes.len() as u64) < want * 2 {
                break;
            }
            at += want;
        }
        Ok(out)
    }
}

/// One channel's files, complete; deleted when dropped.
pub struct RecordedFile {
    stem: PathBuf,
    paths: Vec<PathBuf>,
    samples: u64,
}

impl RecordedFile {
    /// Reads it by range, while it exists.
    pub fn reader(&self) -> RangeReader {
        RangeReader {
            stem: self.stem.clone(),
        }
    }

    /// How long it is, in samples.
    pub fn samples(&self) -> u64 {
        self.samples
    }
}

impl Drop for RecordedFile {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_round_trips_and_the_files_go_away() {
        let dir = std::env::temp_dir().join(format!("logbook-test-{}", std::process::id()));
        let mut recording = Recording::create(&dir, "s", 2).unwrap();
        recording.write(1, &[0.5, -0.25]).unwrap();
        recording.write(1, &[0.0]).unwrap();
        recording.flush().unwrap();
        let reader = recording.reader(1).unwrap();
        assert_eq!(reader.read(0.0, 1.0).unwrap(), vec![0.5, -0.25, 0.0]);
        assert_eq!(reader.read(1.0 / 16_000.0, 1.0).unwrap(), vec![-0.25, 0.0]);
        let files = recording.finish().unwrap();
        assert_eq!(files[0].samples(), 0);
        assert_eq!(files[1].samples(), 3);
        assert_eq!(
            files[1].reader().read(0.0, 1.0).unwrap(),
            vec![0.5, -0.25, 0.0]
        );
        let path = files[1].paths[0].clone();
        drop(files);
        assert!(!path.exists());

        // Dropped before it is finished (the connection was lost): gone too.
        let mut recording = Recording::create(&dir, "t", 1).unwrap();
        recording.write(0, &[0.1]).unwrap();
        let path = recording.tracks[0].files[0].1.clone();
        drop(recording);
        assert!(!path.exists());
        // Left by a server that died; and a second server on the folder.
        std::fs::write(dir.join("old-0.pcm"), [0u8; 4]).unwrap();
        let (lock, cleared) = claim(&dir).unwrap();
        assert_eq!(cleared, 1);
        std::fs::write(dir.join("live-0.pcm"), [0u8; 4]).unwrap();
        assert!(claim(&dir).is_err());
        assert!(dir.join("live-0.pcm").exists());
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_recording_is_read_across_files_and_forgotten_as_it_is_passed() {
        let dir = std::env::temp_dir().join(format!("logbook-seg-{}", std::process::id()));
        let mut recording = Recording::create(&dir, "s", 1).unwrap();
        // 11 minutes, each sample its own second (as a small value).
        let minute: Vec<f32> = vec![0.0; 60 * RATE as usize];
        for m in 0..11 {
            let mut chunk = minute.clone();
            chunk[0] = m as f32 / 100.0;
            recording.write(0, &chunk).unwrap();
        }
        recording.flush().unwrap();
        assert_eq!(recording.tracks[0].files.len(), 3);
        let reader = recording.reader(0).unwrap();
        // Across the first file's end: minute 5 starts the second file.
        let pcm = reader.read(299.0, 301.0).unwrap();
        assert_eq!(pcm.len(), 2 * RATE as usize);
        assert!((pcm[RATE as usize] - 0.05).abs() < 1e-3);
        // Past the first file: it goes; the one being written never does.
        recording.forget_before(0, 400.0);
        assert_eq!(recording.tracks[0].files.len(), 2);
        assert!(reader.read(10.0, 11.0).is_err());
        recording.forget_before(0, 10_000.0);
        assert_eq!(recording.tracks[0].files.len(), 1);
        drop(recording);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
