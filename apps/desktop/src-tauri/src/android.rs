//! What the app does on Android only: a recording kept alive with the
//! screen off by the `recording` plugin's foreground service (its
//! permissions asked first, the battery optimization exemption asked
//! once), and the meetings left pending finished again and again until the
//! server and brainy answer: the app is the only way to finish them there,
//! no `logbook finish` on a phone (docs/decisions/2026-10-29-android-app.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use logbook_client::finishing;
use logbook_client::meeting::{Backend, Credentials, Outcome};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_recording::{PermissionState, RecordingExt};

use crate::Answer;

/// The plugin's calls block (a permission waits for the person): off the
/// async threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("{e}"))?
        .map_err(|e| anyhow!(e))
}

/// Before the microphone opens: its permission (and the notifications'),
/// the exemption from battery optimization asked the first time, and the
/// foreground service started, its Stop calling `on_stop`. While the app
/// is in the foreground: Android refuses a microphone service otherwise.
pub async fn begin(
    app: &AppHandle,
    title: &str,
    on_stop: impl Fn() + Send + Sync + 'static,
) -> Result<()> {
    let handle = app.clone();
    let permissions = blocking(move || handle.recording().request_permissions()).await?;
    if permissions.microphone != PermissionState::Granted {
        bail!("logbook may not use the microphone: allow it in Android's settings for the app");
    }
    if permissions.notifications != PermissionState::Granted {
        tracing::warn!(
            "notifications are off: the recording goes on, without its notification to stop it"
        );
    }
    let handle = app.clone();
    match blocking(move || handle.recording().ask_unrestricted(true)).await {
        Ok(true) => {}
        Ok(false) => tracing::warn!(
            "battery optimization may stop the recording with the screen off: \
             Settings, Background recording"
        ),
        Err(error) => tracing::warn!("{error:#}"),
    }
    let handle = app.clone();
    let title = if title.is_empty() { "Logbook" } else { title }.to_string();
    blocking(move || handle.recording().start(&title, on_stop)).await
}

/// The recording stopped: the notification says the meetings are being
/// finished, without its Stop.
pub fn finishing(app: &AppHandle) {
    if let Err(error) = app.recording().update("Finishing…", false) {
        tracing::warn!("{error}");
    }
}

/// The recording's meetings have all ended: the service stopped, the
/// locks released, and any meeting they left pending finished again.
pub fn end(app: &AppHandle) {
    if let Err(error) = app.recording().stop() {
        tracing::warn!("{error}");
    }
    let state = app.state::<crate::App>();
    if let Ok(backend) = state.backend(&state.settings()) {
        let credentials = state.settings().credentials();
        state
            .finisher
            .start(app.clone(), backend, state.data.clone(), credentials);
    }
}

pub async fn unrestricted(app: &AppHandle) -> Answer<bool> {
    let handle = app.clone();
    blocking(move || handle.recording().unrestricted())
        .await
        .map_err(|e| format!("{e:#}"))
}

pub async fn ask_unrestricted(app: &AppHandle, once: bool) -> Answer<bool> {
    let handle = app.clone();
    blocking(move || handle.recording().ask_unrestricted(once))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// The pending meetings finished, one run at a time, trying again with
/// backoff (client `finishing.rs`, as `logbook finish --wait`) until only
/// those waiting cannot fix are left. A run started while one goes on is
/// not started: the running one lists the folders again each round.
#[derive(Default)]
pub struct Finisher {
    running: Arc<AtomicBool>,
}

impl Finisher {
    pub fn start(&self, app: AppHandle, backend: Backend, data: PathBuf, brainy: Credentials) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = self.running.clone();
        tauri::async_runtime::spawn(async move {
            let mut progress = Progress { app: app.clone() };
            let result =
                finishing::finish_all(&backend, &data, None, &brainy, true, &mut progress).await;
            running.store(false, Ordering::SeqCst);
            match result {
                Ok(summary) if summary.left > 0 => tracing::warn!(
                    "{} pending meeting(s) could not be finished: see above",
                    summary.left
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!("{error:#}"),
            }
            let _ = app.emit("meetings-changed", ());
        });
    }
}

struct Progress {
    app: AppHandle,
}

impl finishing::Progress for Progress {
    fn finished(&mut self, dir: &Path, _outcome: &Outcome) {
        tracing::info!("finished the pending meeting {}", dir.display());
        let _ = self.app.emit("meetings-changed", ());
    }

    fn failed(&mut self, dir: &Path, error: &anyhow::Error, retry: bool) {
        let then = if retry {
            "tried again later"
        } else {
            "left pending"
        };
        tracing::warn!("{}: {error:#} ({then})", dir.display());
    }

    fn waiting(&mut self, wait: Duration) {
        tracing::info!("pending meetings: trying again in {} s", wait.as_secs());
    }
}
