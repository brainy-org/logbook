//! Final passes by id, the ones live sessions started and the ones clients
//! uploaded (`POST /finals`), so that a client that lost its connection
//! fetches the result later (`GET /finals/{id}`).
//!
//! The CLI keeps them in memory (`Finals::default`). The server keeps them
//! in its data folder (`Finals::on_disk`), a file per pass: when replicas
//! share the folder, any of them answers for a pass another one runs. The
//! pass itself runs where its audio is, on the server that started it (its
//! owner), which notes every `BEAT` that it is alive. A running pass whose
//! owner has not done so for `LEASE` is unknown: the owner stopped, and
//! the client, which keeps its own copy of the meeting, uploads it again,
//! as it does after a restart.
//!
//! A result is kept for `KEEP` after it is ready, then dropped. The id is
//! the only key to a meeting's transcript: 128 random bits, never logged
//! next to the text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::protocol::{FinalState, PassProgress};
use crate::store;

const KEEP: Duration = Duration::from_secs(3600);
/// How often a server notes that it is alive.
const BEAT: Duration = Duration::from_secs(10);
/// How long a server not heard from is taken for alive: several beats, so
/// that a busy disk or a clock a little off does not drop a running pass.
const LEASE: Duration = Duration::from_secs(60);
/// How often a server deletes the results past `KEEP`.
const SWEEP: Duration = Duration::from_secs(300);
/// Writes of a result before it is given up (the pass is then unknown).
const WRITES: usize = 3;

pub struct Finals {
    store: Store,
}

enum Store {
    Memory(Mutex<HashMap<String, Entry>>),
    Disk(Disk),
}

struct Entry {
    state: FinalState,
    /// When the result was ready; None while the pass runs.
    ready: Option<Instant>,
}

struct Disk {
    /// `<id>.json`, a `Kept`.
    dir: PathBuf,
    /// `<instance>`: when the server last noted it is alive.
    servers: PathBuf,
    /// This server, until it stops: a new one at every start.
    instance: String,
    /// Dropped to stop the beat.
    stop: Option<Sender<()>>,
    beat: Option<JoinHandle<()>>,
}

/// A pass as kept on disk.
#[derive(Serialize, Deserialize)]
struct Kept {
    /// The server that runs it.
    owner: String,
    /// Seconds since the Unix epoch at which it was ready.
    ready: Option<u64>,
    state: FinalState,
}

impl Default for Finals {
    fn default() -> Self {
        Self {
            store: Store::Memory(Mutex::default()),
        }
    }
}

impl Finals {
    /// The passes in `dir`, which other servers may share: `finals/` and
    /// `servers/` in it. A thread notes that this server is alive until
    /// it is dropped.
    pub fn on_disk(dir: &Path) -> Result<Self> {
        let finals = dir.join("finals");
        let servers = dir.join("servers");
        for d in [&finals, &servers] {
            std::fs::create_dir_all(d).with_context(|| format!("{}", d.display()))?;
        }
        let instance = store::random_hex(8)?;
        let alive = servers.join(&instance);
        store::replace(&alive, now().to_string().as_bytes())?;
        let (stop, stopped) = mpsc::channel::<()>();
        let (d, s) = (finals.clone(), servers.clone());
        let beat = std::thread::Builder::new()
            .name("finals".into())
            .spawn(move || {
                let mut swept: Option<Instant> = None;
                loop {
                    if swept.is_none_or(|t| t.elapsed() >= SWEEP) {
                        sweep(&d, &s);
                        swept = Some(Instant::now());
                    }
                    match stopped.recv_timeout(BEAT) {
                        Err(RecvTimeoutError::Timeout) => {
                            if let Err(e) = store::replace(&alive, now().to_string().as_bytes()) {
                                warn!("could not note that this server is alive: {e:#}");
                            }
                        }
                        _ => break,
                    }
                }
            })?;
        Ok(Self {
            store: Store::Disk(Disk {
                dir: finals,
                servers,
                instance,
                stop: Some(stop),
                beat: Some(beat),
            }),
        })
    }

    /// A new pass, running; returns its id. `progress`, for the pass of a
    /// whole meeting: queued, until the finalizer takes it.
    pub fn start(&self, audio_s: f64, progress: Option<PassProgress>) -> Result<String> {
        let id = store::random_hex(16)?;
        let state = FinalState::Running { audio_s, progress };
        match &self.store {
            Store::Memory(entries) => {
                let mut entries = entries.lock().unwrap();
                entries.retain(|_, e| e.ready.is_none_or(|t| t.elapsed() < KEEP));
                entries.insert(id.clone(), Entry { state, ready: None });
            }
            Store::Disk(disk) => disk.write(&id, None, state)?,
        }
        Ok(id)
    }

    /// How far the running pass `id` has got. Only the finalizer, which
    /// writes its result after, calls it: no result is overwritten. A
    /// write that fails is only a stale progress.
    pub fn progress(&self, id: &str, audio_s: f64, progress: PassProgress) {
        let state = FinalState::Running {
            audio_s,
            progress: Some(progress),
        };
        match &self.store {
            Store::Memory(entries) => {
                if let Some(entry) = entries.lock().unwrap().get_mut(id)
                    && entry.ready.is_none()
                {
                    entry.state = state;
                }
            }
            Store::Disk(disk) => {
                if let Err(e) = disk.write(id, None, state) {
                    warn!("could not note a final pass's progress: {e:#}");
                }
            }
        }
    }

    pub fn finish(&self, id: &str, state: FinalState) {
        match &self.store {
            Store::Memory(entries) => {
                if let Some(entry) = entries.lock().unwrap().get_mut(id) {
                    entry.state = state;
                    entry.ready = Some(Instant::now());
                }
            }
            Store::Disk(disk) => disk.finish(id, state),
        }
    }

    /// The pass `id`; None if there is none (or no longer).
    pub fn get(&self, id: &str) -> Result<Option<FinalState>> {
        match &self.store {
            Store::Memory(entries) => {
                let entries = entries.lock().unwrap();
                Ok(entries
                    .get(id)
                    .filter(|e| e.ready.is_none_or(|t| t.elapsed() < KEEP))
                    .map(|e| e.state.clone()))
            }
            Store::Disk(disk) => disk.get(id),
        }
    }
}

impl Disk {
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn write(&self, id: &str, ready: Option<u64>, state: FinalState) -> Result<()> {
        let kept = Kept {
            owner: self.instance.clone(),
            ready,
            state,
        };
        store::replace(&self.path(id), &serde_json::to_vec(&kept)?)
    }

    /// The result, or, if it cannot be written, no pass at all: a pass
    /// left running while its owner is alive would be waited for forever.
    fn finish(&self, id: &str, state: FinalState) {
        let ready = Some(now());
        for attempt in 1..=WRITES {
            match self.write(id, ready, state.clone()) {
                Ok(()) => return,
                Err(e) => warn!(attempt, "could not keep a final pass's result: {e:#}"),
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if let Err(e) = store::remove(&self.path(id)) {
            error!("a final pass is left running, its client waits for it: {e:#}");
        }
    }

    fn get(&self, id: &str) -> Result<Option<FinalState>> {
        // The id names a file: nothing but what `start` makes.
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let Some(bytes) = store::read(&self.path(id))? else {
            return Ok(None);
        };
        let kept: Kept = serde_json::from_slice(&bytes).context("a final pass's file")?;
        let known = match kept.ready {
            Some(ready) => now() < ready + KEEP.as_secs(),
            None => kept.owner == self.instance || alive(&self.servers, &kept.owner)?,
        };
        Ok(known.then_some(kept.state))
    }
}

impl Drop for Disk {
    /// The beat stopped, then the server's note deleted: its running
    /// passes are unknown at once, not after `LEASE`.
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(beat) = self.beat.take() {
            let _ = beat.join();
        }
        let _ = store::remove(&self.servers.join(&self.instance));
    }
}

/// Whether the server `instance` noted it is alive within `LEASE`.
fn alive(servers: &Path, instance: &str) -> Result<bool> {
    let Some(bytes) = store::read(&servers.join(instance))? else {
        return Ok(false);
    };
    let beat: u64 = String::from_utf8_lossy(&bytes).trim().parse().unwrap_or(0);
    Ok(now() < beat + LEASE.as_secs())
}

/// Deletes the results past `KEEP`, the passes of servers that stopped,
/// those servers' notes and the temporary files of writes cut short. Any
/// server may: deleting twice is deleting once.
fn sweep(finals: &Path, servers: &Path) {
    let swept = (|| -> Result<()> {
        let now = now();
        let old = |path: &Path| {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            modified.is_some_and(|m| m.elapsed().unwrap_or_default() > KEEP)
        };
        for entry in std::fs::read_dir(finals)?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let gone = if store::is_temporary(&name) {
                old(&path)
            } else {
                match store::read(&path)? {
                    None => false,
                    Some(bytes) => match serde_json::from_slice::<Kept>(&bytes) {
                        Ok(Kept {
                            ready: Some(ready), ..
                        }) => now >= ready + KEEP.as_secs(),
                        Ok(Kept { owner, .. }) => !alive(servers, &owner)?,
                        Err(_) => old(&path),
                    },
                }
            };
            if gone {
                store::remove(&path)?;
            }
        }
        for entry in std::fs::read_dir(servers)?.flatten() {
            let path = entry.path();
            if old(&path) {
                store::remove(&path)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = swept {
        warn!("could not delete old final passes: {e:#}");
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> FinalState {
        FinalState::Failed {
            message: "no".into(),
        }
    }

    #[test]
    fn a_pass_is_found_by_its_id() {
        let finals = Finals::default();
        let id = finals.start(12.0, None).unwrap();
        assert_eq!(id.len(), 32);
        assert!(matches!(
            finals.get(&id).unwrap(),
            Some(FinalState::Running { .. })
        ));
        finals.finish(&id, failed());
        assert!(matches!(
            finals.get(&id).unwrap(),
            Some(FinalState::Failed { .. })
        ));
        assert!(finals.get("other").unwrap().is_none());
        assert_ne!(finals.start(1.0, None).unwrap(), id);
    }

    #[test]
    fn a_pass_says_how_far_it_has_got_until_its_result() {
        use crate::protocol::PassPhase;
        let at = |done| PassProgress {
            phase: PassPhase::Transcribing,
            done,
        };
        let progress = |finals: &Finals, id: &str| match finals.get(id).unwrap() {
            Some(FinalState::Running { progress, .. }) => progress,
            other => panic!("{other:?}"),
        };
        let dir =
            std::env::temp_dir().join(format!("logbook-finals-{}", store::random_hex(8).unwrap()));
        let (memory, disk) = (Finals::default(), Finals::on_disk(&dir).unwrap());
        for finals in [&memory, &disk] {
            let id = finals.start(60.0, Some(PassProgress::QUEUED)).unwrap();
            assert_eq!(progress(finals, &id), Some(PassProgress::QUEUED));
            finals.progress(&id, 60.0, at(0.5));
            assert_eq!(progress(finals, &id), Some(at(0.5)));
            finals.finish(&id, failed());
            assert!(matches!(
                finals.get(&id).unwrap(),
                Some(FinalState::Failed { .. })
            ));
        }
        // A progress after the result leaves it (on disk only the
        // finalizer writes either, in that order).
        let id = memory.start(60.0, Some(PassProgress::QUEUED)).unwrap();
        memory.finish(&id, failed());
        memory.progress(&id, 60.0, at(0.9));
        assert!(matches!(
            memory.get(&id).unwrap(),
            Some(FinalState::Failed { .. })
        ));
        drop(disk);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn any_server_on_the_folder_answers_and_a_stopped_owner_s_pass_is_unknown() {
        let dir =
            std::env::temp_dir().join(format!("logbook-finals-{}", store::random_hex(8).unwrap()));
        let a = Finals::on_disk(&dir).unwrap();
        let b = Finals::on_disk(&dir).unwrap();

        let done = a.start(5.0, None).unwrap();
        assert!(matches!(
            b.get(&done).unwrap(),
            Some(FinalState::Running { .. })
        ));
        a.finish(&done, failed());
        assert!(matches!(
            b.get(&done).unwrap(),
            Some(FinalState::Failed { .. })
        ));

        let running = a.start(5.0, None).unwrap();
        assert!(b.get("../../etc/passwd").unwrap().is_none());
        drop(a);
        assert!(b.get(&running).unwrap().is_none(), "its owner stopped");
        assert!(
            b.get(&done).unwrap().is_some(),
            "a result outlives its owner"
        );

        drop(b);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_silent_owner_is_taken_for_stopped_after_the_lease() {
        let dir =
            std::env::temp_dir().join(format!("logbook-finals-{}", store::random_hex(8).unwrap()));
        let servers = dir.join("servers");
        std::fs::create_dir_all(&servers).unwrap();
        store::replace(
            &servers.join("quiet"),
            (now() - LEASE.as_secs()).to_string().as_bytes(),
        )
        .unwrap();
        store::replace(&servers.join("busy"), now().to_string().as_bytes()).unwrap();
        assert!(!alive(&servers, "quiet").unwrap());
        assert!(alive(&servers, "busy").unwrap());
        assert!(!alive(&servers, "never").unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
