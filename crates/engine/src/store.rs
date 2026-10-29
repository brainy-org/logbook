//! Files replaced whole, for what a server keeps in its data folder: the
//! people's tokens (server auth.rs) and the final passes' results
//! (finals.rs). The folder may be a volume every replica of the server
//! mounts (docs/decisions/2026-10-29-replicas-share-the-data-folder.md).
//!
//! No locks: on a network filesystem they are not to be trusted. A file is
//! written to a temporary file of its own, then renamed over the old one,
//! which is atomic: a reader sees the old file or the new, never part of
//! one. What two servers could write at once is laid out so that either
//! order is right: one file per record, never one for all.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

/// Reads of a file that failed for another reason than its absence: a
/// file replaced on another computer of an NFS volume while being read
/// fails once (ESTALE), and is there whole when read again.
const READS: usize = 3;

/// `path` with `bytes`, whole: the old file until the new one is complete.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = temporary(path)?;
    let written = write_private(&tmp, bytes).and_then(|()| {
        std::fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `path` with `bytes`, whole, only if there is no `path`: false if there
/// was one, which is left as it was. Two servers that create the same
/// file at once: one of them makes it.
pub fn create(path: &Path, bytes: &[u8]) -> Result<bool> {
    let tmp = temporary(path)?;
    write_private(&tmp, bytes)?;
    // A hard link, unlike a rename, fails when its name is taken.
    let linked = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error).with_context(|| format!("could not write {}", path.display())),
    }
}

/// The file at `path`; None if there is none.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut attempt = 1;
    loop {
        match std::fs::read(path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) if attempt < READS => {
                tracing::debug!("reading {} again: {error}", path.display());
                std::thread::sleep(Duration::from_millis(20));
                attempt += 1;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        }
    }
}

/// Deletes `path`: false if there was none.
pub fn remove(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("could not delete {}", path.display())),
    }
}

/// The names `dir` holds written to disk: a rename or a deletion that
/// must survive the computer stopping.
pub fn sync_dir(dir: &Path) -> Result<()> {
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("could not write {}", dir.display()))
}

/// Whether `name` is a temporary file `replace` or `create` left behind
/// (its server stopped while writing it).
pub fn is_temporary(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".tmp")
}

pub fn random_hex(len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A name of its own beside `path`: two servers writing the same file at
/// once never write into one temporary file.
fn temporary(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("no file name: {}", path.display()))?
        .to_string_lossy();
    Ok(path.with_file_name(format!(".{name}.{}.tmp", random_hex(8)?)))
}

/// Written readable by the server's user only, and on disk when it returns.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_makes_a_file_once_and_replace_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("logbook-store-{}", random_hex(8).unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.json");
        assert_eq!(read(&path).unwrap(), None);
        assert!(create(&path, b"one").unwrap());
        assert!(!create(&path, b"two").unwrap());
        assert_eq!(read(&path).unwrap().as_deref(), Some(&b"one"[..]));
        replace(&path, b"three").unwrap();
        assert_eq!(read(&path).unwrap().as_deref(), Some(&b"three"[..]));
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "a temporary file is left"
        );
        assert!(remove(&path).unwrap());
        assert!(!remove(&path).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
