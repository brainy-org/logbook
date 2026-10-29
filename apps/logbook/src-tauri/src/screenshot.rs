//! Screenshots for the notes of the meeting being recorded, on the
//! desktop: the screen under the pointer, taken by the button on the
//! recorder (`take_screenshot`) or by `SHORTCUT` from any app while a
//! meeting is recorded (`screenshot` events). The file goes to
//! `<data>/screenshots` and the page puts its line in the notes; the
//! client takes it from there (crates/client `shots.rs`). The pages show
//! the files through the `shot` scheme (`serve`).
//!
//! macOS asks once for Screen Recording: refused, the screen comes back
//! without the other apps' windows.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use logbook_client::shots;
use serde::Serialize;
use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use xcap::image::imageops::{self, FilterType};

/// Takes a screenshot while a meeting is recorded, from any app.
pub const SHORTCUT: &str = "CmdOrCtrl+Shift+2";

/// A screenshot wider than this is scaled down: a 5K screen's PNG would
/// pass brainy's 10 MB for an upload.
const MAX_WIDTH: u32 = 2560;

/// Long enough for the window to be gone from the screen once hidden.
const HIDDEN: Duration = Duration::from_millis(300);

/// A screenshot taken: its file in `<data>/screenshots`, and the caption
/// of its line in the notes.
#[derive(Clone, Debug, Serialize)]
pub struct Shot {
    pub name: String,
    pub caption: String,
}

/// The screen under the pointer, the app's window out of it when it is in
/// front (the button was clicked, or the shortcut pressed from it).
pub fn take(app: &AppHandle, data: &Path) -> Result<Shot> {
    let window = app
        .get_webview_window("main")
        .filter(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false));
    if let Some(w) = &window {
        w.hide()?;
        std::thread::sleep(HIDDEN);
    }
    let image = capture(app);
    if let Some(w) = &window {
        let _ = w.show();
        let _ = w.set_focus();
    }
    let mut image = image?;
    if image.width() > MAX_WIDTH {
        let height = image.height() * MAX_WIDTH / image.width();
        image = imageops::resize(&image, MAX_WIDTH, height, FilterType::Triangle);
    }
    let dir = shots::staging(data);
    std::fs::create_dir_all(&dir)?;
    let taken = chrono::Local::now();
    let name = shots::new_name(&dir, taken);
    let path = dir.join(&name);
    image
        .save(&path)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(Shot {
        name,
        caption: format!("Screenshot {}", taken.format("%H:%M")),
    })
}

/// The monitor under the pointer, or the main one.
fn capture(app: &AppHandle) -> Result<xcap::image::RgbaImage> {
    let monitor = pointer(app)
        .and_then(|(x, y)| xcap::Monitor::from_point(x, y).ok())
        .or_else(|| {
            xcap::Monitor::all()
                .ok()?
                .into_iter()
                .find(|m| m.is_primary().unwrap_or(false))
        })
        .context("no screen to take")?;
    monitor.capture_image().context(
        "the screen could not be taken (on macOS: System Settings, Privacy, Screen Recording)",
    )
}

/// The pointer, in the units xcap places monitors in: points on macOS,
/// pixels elsewhere.
fn pointer(app: &AppHandle) -> Option<(i32, i32)> {
    let p = app.cursor_position().ok()?;
    let scale = if cfg!(target_os = "macos") {
        app.monitor_from_point(p.x, p.y)
            .ok()
            .flatten()
            .map_or(1.0, |m| m.scale_factor())
    } else {
        1.0
    };
    Some(((p.x / scale) as i32, (p.y / scale) as i32))
}

/// The shortcut's plugin: a screenshot each time it is pressed, told to
/// the recorder as `screenshot` (or `screenshot-failed`).
pub fn plugin(
    data: impl Fn(&AppHandle) -> std::path::PathBuf + Send + Sync + 'static,
) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, _, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let app = app.clone();
            let data = data(&app);
            tauri::async_runtime::spawn_blocking(move || match take(&app, &data) {
                Ok(shot) => {
                    let _ = app.emit("screenshot", shot);
                }
                Err(error) => {
                    tracing::warn!("{error:#}");
                    let _ = app.emit("screenshot-failed", format!("{error:#}"));
                }
            });
        })
        .build()
}

/// The shortcut is the app's while a meeting is recorded: it is not taken
/// from the other apps the rest of the time.
pub fn listen(app: &AppHandle) {
    let shortcut: Shortcut = SHORTCUT.parse().expect("a valid shortcut");
    if let Err(error) = app.global_shortcut().register(shortcut) {
        tracing::warn!("{SHORTCUT} takes no screenshot: {error}");
    }
}

pub fn unlisten(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
}

/// `shot://localhost/screenshots/<name>` and
/// `shot://localhost/meetings/<id>/<name>`: a screenshot of the meeting
/// being recorded, or of a kept one. Nothing else in the data folder.
pub fn serve(data: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let parts: Vec<&str> = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect();
    let path = match parts.as_slice() {
        ["screenshots", name] if shots::is_name(name) => Some(shots::staging(data).join(name)),
        ["meetings", id, name] if shots::is_name(name) => logbook_client::history::images(data, id)
            .ok()
            .map(|d| d.join(name)),
        _ => None,
    };
    match path.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => Response::builder()
            .header("Content-Type", "image/png")
            .body(bytes)
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap(),
    }
}
