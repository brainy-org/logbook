//! Every pending meeting brought to its end (`logbook finish`), oldest
//! first: once, or with `wait` until the server (and brainy, for a meeting
//! that publishes there) can be reached, trying again with backoff, for as
//! long as it takes.
//!
//! What is tried again is only what waiting can fix: the server or brainy
//! not answering, or answering that they cannot now (a 5xx, 408, 429). A
//! failure that is not about reaching them (brainy refusing the page with
//! a 4xx, a token refused, a final pass that failed, a folder that cannot
//! be read) is reported and its meeting left alone for the rest of the
//! run: trying it again every few seconds would fail the same way. A
//! meeting another process holds (one being recorded, or finished by
//! another `finish` or the app: `Pending::claim`) is not touched; with
//! `wait` it is looked at again, since it is left pending if its holder
//! fails.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use crate::meeting::{self, Backend, Credentials, Outcome};
use crate::pending::Pending;

/// The first wait after a round that could not reach something, doubled
/// after each such round up to `MAX_WAIT`.
pub const FIRST_WAIT: Duration = Duration::from_secs(5);
pub const MAX_WAIT: Duration = Duration::from_secs(300);

/// An HTTP answer that is not a success, from the server or brainy: its
/// status says whether trying again can help.
#[derive(Debug)]
pub struct HttpStatus {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpStatus {}

/// A failure known to pass once the server is back (it lost a pass it
/// had just started: it restarted).
#[derive(Debug)]
pub struct Transient(pub String);

impl std::fmt::Display for Transient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Transient {}

/// Statuses that say "not now": the server busy, restarting, or behind a
/// proxy that cannot reach it.
fn transient_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (500..=599).contains(&status) && status != 501
}

/// Whether waiting can fix `error`: something on the way to the server
/// or brainy did not answer, or answered "not now".
pub fn is_transient(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return match e.status() {
                Some(status) => transient_status(status.as_u16()),
                None => e.is_connect() || e.is_timeout() || e.is_request() || e.is_body(),
            };
        }
        if let Some(e) = cause.downcast_ref::<HttpStatus>() {
            return transient_status(e.status);
        }
        if let Some(e) = cause.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            return matches!(
                e.kind(),
                ConnectionRefused
                    | ConnectionReset
                    | ConnectionAborted
                    | NotConnected
                    | BrokenPipe
                    | TimedOut
                    | HostUnreachable
                    | NetworkUnreachable
                    | NetworkDown
            );
        }
        cause.is::<Transient>()
    })
}

/// One try at one meeting.
pub enum Attempt {
    Finished(Outcome),
    /// Another process holds it: recorded or finished there.
    Busy,
    Failed(anyhow::Error),
}

/// What a run tells the person, as it goes.
pub trait Progress {
    fn started(&mut self, _dir: &Path) {}
    fn finished(&mut self, dir: &Path, outcome: &Outcome);
    fn busy(&mut self, _dir: &Path) {}
    /// `retry`: it is tried again after the wait; otherwise it is left
    /// pending for this run.
    fn failed(&mut self, dir: &Path, error: &anyhow::Error, retry: bool);
    fn waiting(&mut self, _wait: Duration) {}
}

/// How the run waits between rounds: the clock, or a test's.
pub trait Sleep {
    fn sleep(&mut self, wait: Duration) -> impl Future<Output = ()> + Send;
}

pub struct Clock;

impl Sleep for Clock {
    async fn sleep(&mut self, wait: Duration) {
        tokio::time::sleep(wait).await
    }
}

/// The meetings a run finished, and those it leaves pending.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub finished: usize,
    pub left: usize,
}

/// The waits between rounds: `FIRST_WAIT`, doubled up to `MAX_WAIT`,
/// back to the first once a meeting finishes (what failed may be back).
pub struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: FIRST_WAIT }
    }
}

impl Backoff {
    pub fn wait(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(MAX_WAIT);
        wait
    }

    pub fn reset(&mut self) {
        self.next = FIRST_WAIT;
    }
}

/// Every pending meeting under `data` (or only `dir`), oldest first,
/// finished on `backend`; with `wait`, until none is left that waiting
/// could finish.
pub async fn finish_all(
    backend: &Backend,
    data: &Path,
    dir: Option<&Path>,
    brainy: &Credentials,
    wait: bool,
    progress: &mut impl Progress,
) -> Result<Summary> {
    let list = || match dir {
        Some(dir) => Ok(vec![dir.to_path_buf()]),
        None => Pending::list(data),
    };
    let attempt = |dir: PathBuf| async move {
        match Pending::claim(&dir) {
            Ok(None) => Attempt::Busy,
            Err(error) => Attempt::Failed(error),
            Ok(Some(mut pending)) => match meeting::finish(backend, &mut pending, brainy).await {
                Ok(outcome) => Attempt::Finished(outcome),
                Err(error) => Attempt::Failed(error),
            },
        }
    };
    run(list, attempt, wait, &mut Clock, progress).await
}

/// The rounds: each tries every meeting listed but those given up on,
/// oldest first; after a round in which something could not be reached
/// (or was busy), with `wait`, waits and goes again.
pub async fn run<F, Fut>(
    mut list: impl FnMut() -> Result<Vec<PathBuf>>,
    mut attempt: F,
    wait: bool,
    sleep: &mut impl Sleep,
    progress: &mut impl Progress,
) -> Result<Summary>
where
    F: FnMut(PathBuf) -> Fut,
    Fut: Future<Output = Attempt>,
{
    let mut summary = Summary::default();
    // Failed for a reason waiting does not fix: not tried again this run.
    let mut given_up: HashSet<PathBuf> = HashSet::new();
    let mut backoff = Backoff::default();
    loop {
        let mut again = 0;
        let mut finished_any = false;
        for dir in list()? {
            if given_up.contains(&dir) {
                continue;
            }
            progress.started(&dir);
            match attempt(dir.clone()).await {
                Attempt::Finished(outcome) => {
                    progress.finished(&dir, &outcome);
                    summary.finished += 1;
                    finished_any = true;
                }
                Attempt::Busy => {
                    progress.busy(&dir);
                    again += 1;
                }
                Attempt::Failed(error) if is_transient(&error) => {
                    progress.failed(&dir, &error, wait);
                    again += 1;
                }
                Attempt::Failed(error) => {
                    progress.failed(&dir, &error, false);
                    given_up.insert(dir);
                }
            }
        }
        if !wait || again == 0 {
            summary.left = given_up.len() + again;
            return Ok(summary);
        }
        if finished_any {
            backoff.reset();
        }
        let pause = backoff.wait();
        progress.waiting(pause);
        sleep.sleep(pause).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    /// Waits recorded, none slept.
    #[derive(Default)]
    struct Recorded(Vec<Duration>);

    impl Sleep for Recorded {
        async fn sleep(&mut self, wait: Duration) {
            self.0.push(wait);
        }
    }

    #[derive(Default)]
    struct Log(Vec<String>);

    impl Progress for Log {
        fn finished(&mut self, dir: &Path, _: &Outcome) {
            self.0.push(format!("finished {}", dir.display()));
        }
        fn busy(&mut self, dir: &Path) {
            self.0.push(format!("busy {}", dir.display()));
        }
        fn failed(&mut self, dir: &Path, _: &anyhow::Error, retry: bool) {
            let what = if retry { "retry" } else { "skip" };
            self.0.push(format!("{what} {}", dir.display()));
        }
    }

    #[derive(Clone, Copy)]
    enum Then {
        Done,
        Busy,
        Unreachable,
        Refused,
    }

    fn unreachable() -> anyhow::Error {
        let io = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        anyhow::Error::new(io).context("could not reach http://h:3300/finals")
    }

    fn refused() -> anyhow::Error {
        anyhow::Error::new(HttpStatus {
            status: 400,
            message: "brainy answered 400 Bad Request".into(),
        })
        .context("not written to brainy")
    }

    /// Each meeting's answers in turn (the last one repeats); pending
    /// until it is done. Returns the log, the waits and the order tried.
    async fn script(
        meetings: &[(&str, &[Then])],
        wait: bool,
    ) -> (Summary, Vec<String>, Vec<Duration>, Vec<String>) {
        let answers: HashMap<PathBuf, VecDeque<Then>> = meetings
            .iter()
            .map(|(name, then)| (PathBuf::from(name), then.iter().copied().collect()))
            .collect();
        let state = Arc::new(Mutex::new(answers));
        let tried = Arc::new(Mutex::new(Vec::new()));
        let list = {
            let state = state.clone();
            move || {
                // Listed in the order of their names: oldest first.
                let mut dirs: Vec<PathBuf> = state.lock().unwrap().keys().cloned().collect();
                dirs.sort();
                Ok(dirs)
            }
        };
        let attempt = |dir: PathBuf| {
            let (state, tried) = (state.clone(), tried.clone());
            async move {
                tried.lock().unwrap().push(dir.display().to_string());
                let mut state = state.lock().unwrap();
                let queue = state.get_mut(&dir).unwrap();
                let then = if queue.len() > 1 {
                    queue.pop_front().unwrap()
                } else {
                    queue[0]
                };
                match then {
                    Then::Done => {
                        state.remove(&dir);
                        Attempt::Finished(Outcome::default())
                    }
                    Then::Busy => Attempt::Busy,
                    Then::Unreachable => Attempt::Failed(unreachable()),
                    Then::Refused => Attempt::Failed(refused()),
                }
            }
        };
        let (mut log, mut sleep) = (Log::default(), Recorded::default());
        let summary = run(list, attempt, wait, &mut sleep, &mut log)
            .await
            .unwrap();
        let tried = tried.lock().unwrap().clone();
        (summary, log.0, sleep.0, tried)
    }

    #[test]
    fn what_waiting_can_fix() {
        assert!(is_transient(&unreachable()));
        assert!(!is_transient(&refused()));
        let status = |status| {
            anyhow::Error::new(HttpStatus {
                status,
                message: String::new(),
            })
        };
        assert!(is_transient(&status(503)));
        assert!(is_transient(&status(429)));
        assert!(!is_transient(&status(401)));
        assert!(!is_transient(&status(404)));
        assert!(!is_transient(&status(501)));
        assert!(is_transient(&anyhow::Error::new(Transient("lost".into()))));
        assert!(!is_transient(&anyhow::anyhow!("the final pass failed")));
    }

    #[test]
    fn the_wait_doubles_up_to_five_minutes() {
        let mut b = Backoff::default();
        let waits: Vec<u64> = (0..9).map(|_| b.wait().as_secs()).collect();
        assert_eq!(waits, [5, 10, 20, 40, 80, 160, 300, 300, 300]);
        b.reset();
        assert_eq!(b.wait().as_secs(), 5);
    }

    #[tokio::test]
    async fn without_wait_each_meeting_is_tried_once() {
        use Then::*;
        let (summary, log, waits, _) = script(
            &[("a", &[Unreachable]), ("b", &[Done]), ("c", &[Busy])],
            false,
        )
        .await;
        assert_eq!(
            summary,
            Summary {
                finished: 1,
                left: 2
            }
        );
        assert_eq!(log, ["skip a", "finished b", "busy c"]);
        assert!(waits.is_empty());
    }

    #[tokio::test]
    async fn waiting_tries_again_until_the_server_is_back() {
        use Then::*;
        // "a" is reached on the fourth round; "b" is refused once and not
        // tried again; "c" is finished at once; "d" is busy (being
        // recorded) for two rounds, then left pending by its holder.
        let (summary, log, waits, tried) = script(
            &[
                ("a", &[Unreachable, Unreachable, Unreachable, Done]),
                ("b", &[Refused]),
                ("c", &[Done]),
                ("d", &[Busy, Busy, Unreachable, Done]),
            ],
            true,
        )
        .await;
        assert_eq!(
            summary,
            Summary {
                finished: 3,
                left: 1
            }
        );
        // Oldest first in every round; "b" only in the first, "c" once.
        assert_eq!(tried, ["a", "b", "c", "d", "a", "d", "a", "d", "a", "d"]);
        assert_eq!(log.iter().filter(|l| *l == "skip b").count(), 1);
        // Reset after "c" finished in the first round, doubled after.
        assert_eq!(
            waits,
            [5, 10, 20].map(Duration::from_secs).to_vec(),
            "{log:?}"
        );
    }

    #[tokio::test]
    async fn a_long_outage_waits_at_most_five_minutes_between_rounds() {
        let mut then = vec![Then::Unreachable; 12];
        then.push(Then::Done);
        let (summary, _, waits, _) = script(&[("a", &then)], true).await;
        assert_eq!(
            summary,
            Summary {
                finished: 1,
                left: 0
            }
        );
        assert_eq!(waits.len(), 12);
        assert_eq!(waits[6..], [MAX_WAIT; 6]);
        assert!(waits.windows(2).all(|w| w[0] <= w[1]));
    }

    #[tokio::test]
    async fn nothing_pending_ends_at_once() {
        let (summary, log, waits, _) = script(&[], true).await;
        assert_eq!(summary, Summary::default());
        assert!(log.is_empty() && waits.is_empty());
    }
}
