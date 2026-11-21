//! New versions of the desktop app, from the GitHub releases of
//! brainy-org/logbook (`plugins.updater` in tauri.conf.json): a
//! `latest.json` that `scripts/release.sh` attaches to each release, with
//! the macOS bundle and the Windows installer it names, each signed with
//! the updater's key (its public half in tauri.conf.json; its private half
//! only on the Mac that releases, `~/.config/logbook/updater.key`). A draft
//! release is not `latest`: nobody is offered it until it is published.
//!
//! Looked for shortly after the start and every few hours; the page shows
//! the version found (`update` event, `update_state`) and the person
//! chooses when to install it. Installing restarts the app, so it is
//! refused while a meeting is recorded or finished: a pending meeting
//! interrupted otherwise is taken up at the next start
//! (docs/decisions/2026-11-21-the-app-updates-itself.md).
//!
//! On Android there is nothing here: the APK is installed by hand.

use serde::Serialize;

/// A version newer than this one, found on GitHub.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    pub version: String,
    pub current: String,
    /// The release's notes.
    pub notes: Option<String>,
}

/// The download of an update being installed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u64,
    /// None when the server does not say.
    pub total: Option<u64>,
}

#[cfg(not(target_os = "android"))]
pub use desktop::*;

#[cfg(not(target_os = "android"))]
mod desktop {
    use std::sync::Mutex;
    use std::time::Duration;

    use anyhow::{Result, bail};
    use tauri::{AppHandle, Emitter, Manager};
    use tauri_plugin_updater::UpdaterExt;
    use tracing::{info, warn};

    use super::{Available, Progress};

    /// The first look, once the app has settled.
    const FIRST: Duration = Duration::from_secs(20);
    /// The next ones.
    const EVERY: Duration = Duration::from_secs(6 * 3600);

    /// The newest version found, if newer than this one.
    #[derive(Default)]
    pub struct Updates {
        found: Mutex<Option<Available>>,
    }

    impl Updates {
        pub fn found(&self) -> Option<Available> {
            self.found.lock().unwrap().clone()
        }
    }

    /// Looks for a new version now and then, for as long as the app runs.
    pub fn watch(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST).await;
            loop {
                if let Err(error) = check(&app).await {
                    // Offline, or GitHub down: the next look will do.
                    warn!("could not look for a new version: {error:#}");
                }
                tokio::time::sleep(EVERY).await;
            }
        });
    }

    /// The newest version, if newer than this one; the page hears it.
    pub async fn check(app: &AppHandle) -> Result<Option<Available>> {
        let update = match app.updater()?.check().await {
            Ok(update) => update,
            // GitHub answered, with no latest.json: no release is published
            // yet (drafts are not `latest`), so nothing is newer. Offline
            // is another error, and stays one.
            Err(tauri_plugin_updater::Error::ReleaseNotFound) => None,
            Err(error) => return Err(error.into()),
        };
        let found = update.map(|update| Available {
            version: update.version,
            current: update.current_version,
            notes: update.body,
        });
        if let Some(found) = &found {
            info!(version = found.version, "a new version is available");
        }
        *app.state::<Updates>().found.lock().unwrap() = found.clone();
        let _ = app.emit("update", &found);
        Ok(found)
    }

    /// The new version downloaded, installed and started in place of this
    /// one; refused while a meeting is recorded or finished. On Windows the
    /// installer closes the app itself.
    pub async fn install(app: &AppHandle) -> Result<()> {
        if app.state::<crate::App>().recorder.state().is_some() {
            bail!("a meeting is being recorded: install the new version once it is finished");
        }
        let Some(update) = app.updater()?.check().await? else {
            bail!("there is no newer version");
        };
        info!(version = update.version, "installing the new version");
        let mut done = 0u64;
        update
            .download_and_install(
                |chunk, total| {
                    done += chunk as u64;
                    let _ = app.emit("update-progress", Progress { done, total });
                },
                || {},
            )
            .await?;
        relaunch(app)
    }

    /// The new version started in place of this one. Not `app.restart()`:
    /// it starts the new process only after the exit event, where the app
    /// ends with `parakeet::exit` (lib.rs), which never returns, so the app
    /// quit and nothing came back. A detached shell waits for this process
    /// to be gone (a second instance started before would hand over to it
    /// and quit: single-instance), then opens the bundle again.
    #[cfg(target_os = "macos")]
    fn relaunch(app: &AppHandle) -> Result<()> {
        use anyhow::Context;
        use std::process::{Command, Stdio};
        let exe = std::env::current_exe()?;
        let bundle = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .context("the app is not in an .app bundle: start it again by hand")?;
        Command::new("/bin/sh")
            .args([
                "-c",
                r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#,
                "sh",
            ])
            .arg(std::process::id().to_string())
            .arg(bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start the new version")?;
        let _ = app;
        logbook_engine::parakeet::exit(0)
    }

    /// Windows' installer closes and starts the app itself; elsewhere
    /// Tauri's restart.
    #[cfg(not(target_os = "macos"))]
    fn relaunch(app: &AppHandle) -> Result<()> {
        app.restart()
    }
}
