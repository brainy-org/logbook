//! The pending meetings finished while the app is open, on the desktop as
//! on Android: one run at a time, trying again with backoff (client
//! `finishing.rs`, as `logbook finish --wait`) until only those waiting
//! cannot fix are left. Each meeting's state goes to the pages as it
//! changes (`finishing`), and is kept for a page that opens later
//! (`list_pending`).
//!
//! A run is started again, with the settings then, when it is asked for
//! while one goes on (Finish now, the settings saved): the running one
//! ends before its next meeting or at once if it is waiting, never in the
//! middle of one (a meeting half written to brainy is only ever taken up
//! by its next attempt).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use logbook_client::finishing::{self, Problem};
use logbook_client::meeting::{Outcome, Step, Steps};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

/// A pending meeting as the pages show it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finishing {
    pub id: String,
    /// Being finished now.
    pub active: bool,
    /// Held by another process: being recorded, or finished by `logbook
    /// finish`.
    pub busy: bool,
    /// Where it is (or was, when it stopped).
    pub step: Option<Step>,
    /// When the step started, in milliseconds since the Unix epoch.
    pub since: u64,
    pub problem: Option<Stopped>,
    /// When it is tried again, while waiting to be.
    pub retry_at: Option<u64>,
}

/// Why it stopped.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub kind: Problem,
    /// "server" or "brainy": who did not answer, or refused.
    pub who: &'static str,
    pub message: String,
    /// Tried again on its own.
    pub retried: bool,
}

#[derive(Clone, Default)]
pub struct Finisher(Arc<Inner>);

#[derive(Default)]
struct Inner {
    running: AtomicBool,
    /// Asked for while a run goes on: that run ends, then another starts.
    again: AtomicBool,
    wake: Notify,
    meetings: Mutex<HashMap<String, Finishing>>,
}

impl Finisher {
    /// A run, with the settings now; one going on ends first.
    pub fn start(&self, app: &AppHandle) {
        if self.0.running.swap(true, Ordering::SeqCst) {
            self.0.again.store(true, Ordering::SeqCst);
            self.0.wake.notify_one();
            return;
        }
        let (finisher, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            loop {
                finisher.0.again.store(false, Ordering::SeqCst);
                finisher.run(&app).await;
                if !finisher.0.again.load(Ordering::SeqCst) {
                    break;
                }
            }
            finisher.0.running.store(false, Ordering::SeqCst);
            // Asked for in between the check and the store: not lost.
            if finisher.0.again.swap(false, Ordering::SeqCst) {
                finisher.start(&app);
            }
        });
    }

    /// The state of the pending meeting `id`, if a run has seen it.
    pub fn state(&self, id: &str) -> Option<Finishing> {
        self.0.meetings.lock().unwrap().get(id).cloned()
    }

    async fn run(&self, app: &AppHandle) {
        let state = app.state::<crate::App>();
        let settings = state.settings();
        let backend = match state.backend(&settings) {
            Ok(backend) => backend,
            Err(error) => {
                tracing::warn!("pending meetings not finished: {error}");
                return;
            }
        };
        let data = state.data.clone();
        let mut progress = Progress {
            app: app.clone(),
            finisher: self.clone(),
        };
        let mut sleep = Wake(self.clone());
        let steps = |dir: &Path| {
            let (finisher, app, id) = (self.clone(), app.clone(), id(dir));
            Steps::new(move |step| finisher.step(&app, &id, step))
        };
        let result = finishing::finish_all(
            &backend,
            &data,
            None,
            &settings.credentials(),
            true,
            &mut sleep,
            &mut progress,
            steps,
        )
        .await;
        match result {
            Ok(summary) if summary.left > 0 => tracing::warn!(
                "{} pending meeting(s) could not be finished: see above",
                summary.left
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!("{error:#}"),
        }
        let _ = app.emit("meetings-changed", ());
    }

    /// Changes meeting `id`'s state and sends it to the pages.
    fn update(&self, app: &AppHandle, id: &str, change: impl FnOnce(&mut Finishing)) {
        let shown = {
            let mut meetings = self.0.meetings.lock().unwrap();
            let state = meetings.entry(id.to_string()).or_insert_with(|| Finishing {
                id: id.to_string(),
                ..Finishing::default()
            });
            change(state);
            state.clone()
        };
        let _ = app.emit("finishing", shown);
    }

    fn step(&self, app: &AppHandle, id: &str, step: Step) {
        self.update(app, id, |state| {
            let same = state
                .step
                .as_ref()
                .is_some_and(|s| std::mem::discriminant(s) == std::mem::discriminant(&step));
            if !same {
                state.since = now();
            }
            state.step = Some(step);
        });
    }
}

struct Progress {
    app: AppHandle,
    finisher: Finisher,
}

impl finishing::Progress for Progress {
    fn started(&mut self, dir: &Path) {
        self.finisher.update(&self.app, &id(dir), |state| {
            *state = Finishing {
                id: state.id.clone(),
                active: true,
                since: now(),
                ..Finishing::default()
            };
        });
    }

    fn finished(&mut self, dir: &Path, _outcome: &Outcome) {
        tracing::info!("finished the pending meeting {}", dir.display());
        self.finisher.0.meetings.lock().unwrap().remove(&id(dir));
        let _ = self.app.emit("meetings-changed", ());
    }

    fn busy(&mut self, dir: &Path) {
        self.finisher.update(&self.app, &id(dir), |state| {
            state.active = false;
            state.busy = true;
        });
    }

    fn failed(&mut self, dir: &Path, error: &anyhow::Error, retry: bool) {
        let then = if retry {
            "tried again later"
        } else {
            "left pending"
        };
        tracing::warn!("{}: {error:#} ({then})", dir.display());
        self.finisher.update(&self.app, &id(dir), |state| {
            // brainy is written to from the transcript on: what fails
            // there is brainy's (the report's model failing does not stop
            // a meeting).
            let who = match state.step {
                Some(Step::Publishing | Step::Report) => "brainy",
                _ => "server",
            };
            state.active = false;
            state.problem = Some(Stopped {
                kind: Problem::of(error),
                who,
                message: format!("{error:#}"),
                retried: retry,
            });
        });
    }

    fn waiting(&mut self, wait: Duration) {
        tracing::info!("pending meetings: trying again in {} s", wait.as_secs());
        let at = now() + wait.as_millis() as u64;
        let waiting: Vec<String> = {
            let meetings = self.finisher.0.meetings.lock().unwrap();
            meetings
                .values()
                .filter(|m| m.busy || m.problem.as_ref().is_some_and(|p| p.retried))
                .map(|m| m.id.clone())
                .collect()
        };
        for id in waiting {
            self.finisher
                .update(&self.app, &id, |state| state.retry_at = Some(at));
        }
    }

    fn stopped(&mut self) -> bool {
        self.finisher.0.again.load(Ordering::SeqCst)
    }
}

/// The wait between rounds, cut short when another run is asked for.
struct Wake(Finisher);

impl finishing::Sleep for Wake {
    async fn sleep(&mut self, wait: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = self.0.0.wake.notified() => {}
        }
    }
}

/// A pending folder's id (`Pending::id`), as `list_pending` names it.
fn id(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
