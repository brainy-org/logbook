//! A session's audio on disk, for the final pass, and only for it.
//!
//! One file per channel, raw 16 kHz mono s16le, written as the audio
//! arrives (on the session's clock, as the client sends it). The files are
//! deleted when they are dropped: when the session ends without a final
//! pass, when the connection is lost, and once the final pass has read them,
//! whether it succeeded or not. A server that dies does not drop them:
//! `clear_leftovers` deletes them when it starts again. Nothing else on the
//! server keeps a meeting's audio.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Deletes the recordings a server that died left in `dir`: no session of
/// this one owns them, and their clients upload their own copy.
pub fn clear_leftovers(dir: &Path) -> Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", dir.display()));
        }
    };
    let mut cleared = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pcm") {
            std::fs::remove_file(&path)
                .with_context(|| format!("could not delete {}", path.display()))?;
            cleared += 1;
        }
    }
    Ok(cleared)
}

pub struct Recording {
    files: Vec<(BufWriter<File>, RecordedFile)>,
}

impl Recording {
    pub fn create(dir: &Path, session: &str, channels: usize) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        let mut files = Vec::with_capacity(channels);
        for i in 0..channels {
            let path = dir.join(format!("{session}-{i}.pcm"));
            let file = File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| format!("could not create {}", path.display()))?;
            files.push((BufWriter::new(file), RecordedFile { path }));
        }
        Ok(Self { files })
    }

    pub fn write(&mut self, channel: usize, pcm: &[f32]) -> Result<()> {
        let Some((writer, file)) = self.files.get_mut(channel) else {
            return Ok(());
        };
        let bytes: Vec<u8> = pcm
            .iter()
            .flat_map(|s| ((s * 32768.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes())
            .collect();
        writer
            .write_all(&bytes)
            .with_context(|| format!("could not write {}", file.path.display()))
    }

    /// The files, complete, in channel order.
    pub fn finish(self) -> Result<Vec<RecordedFile>> {
        let mut out = Vec::with_capacity(self.files.len());
        for (mut writer, file) in self.files {
            writer
                .flush()
                .with_context(|| format!("could not write {}", file.path.display()))?;
            out.push(file);
        }
        Ok(out)
    }
}

/// One channel's file; deleted when dropped.
pub struct RecordedFile {
    path: PathBuf,
}

impl RecordedFile {
    pub fn read(&self) -> Result<Vec<f32>> {
        let mut bytes = Vec::new();
        File::open(&self.path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .with_context(|| format!("could not read {}", self.path.display()))?;
        Ok(bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect())
    }
}

impl Drop for RecordedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        let files = recording.finish().unwrap();
        assert_eq!(files[0].read().unwrap(), Vec::<f32>::new());
        assert_eq!(files[1].read().unwrap(), vec![0.5, -0.25, 0.0]);
        let path = files[1].path.clone();
        drop(files);
        assert!(!path.exists());

        // Dropped before it is finished (the connection was lost): gone too.
        let recording = Recording::create(&dir, "t", 1).unwrap();
        let path = recording.files[0].1.path.clone();
        drop(recording);
        assert!(!path.exists());
        // Left by a server that died.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old-0.pcm"), [0u8; 4]).unwrap();
        assert_eq!(clear_leftovers(&dir).unwrap(), 1);
        let _ = std::fs::remove_dir(&dir);
    }
}
