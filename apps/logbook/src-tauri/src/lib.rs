//! Logbook's desktop app: meetings recorded, transcribed and written up,
//! as Granola does, on crates/client (the CLI's engine). The pages
//! (React, `../src`) call the commands here; the meeting being recorded is
//! `recorder.rs`, the settings and their secrets `settings.rs`, the
//! library's messages `logs.rs`. The data folder is the CLI's (`logbook`
//! in the user's data folder): the two share the pending meetings and the
//! voices known by name.
//!
//! On Android (`android.rs`) the app records the microphone and sends it
//! to a server: no local models, its data in the app's own folder, the
//! recording kept alive by a foreground service, pending meetings finished
//! again until the server answers (docs/decisions/2026-10-29-android-app.md).

#[cfg(target_os = "android")]
mod android;
mod logs;
mod recorder;
#[cfg(not(target_os = "android"))]
mod screenshot;
mod settings;

use std::path::PathBuf;
#[cfg(not(target_os = "android"))]
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(not(target_os = "android"))]
use logbook_client::local;
use logbook_client::meeting::{self, Backend, Renamed};
use logbook_client::pending::Pending;
use logbook_client::{audio, history, remote, voices};
use logbook_engine::polish;
use serde::Serialize;
#[cfg(not(target_os = "android"))]
use tauri::Emitter;
use tauri::{AppHandle, Manager, State};

use crate::logs::Logs;
use crate::recorder::Recorder;
use crate::settings::{Backends, Mode, Secret, Secrets, Settings};

struct App {
    data: PathBuf,
    config: PathBuf,
    recorder: Recorder,
    backends: Mutex<Backends>,
    /// How the models of the local backend get ready (`models`); none
    /// with a server.
    #[cfg(not(target_os = "android"))]
    models: Arc<Mutex<Option<local::Status>>>,
    /// Pending meetings finished again until the server answers.
    #[cfg(target_os = "android")]
    finisher: android::Finisher,
    handle: AppHandle,
    logs: Logs,
}

impl App {
    fn settings(&self) -> Settings {
        Settings::load(&self.config)
    }

    /// The backend the settings name.
    fn backend(&self, settings: &Settings) -> Answer<Backend> {
        self.backends.lock().unwrap().get(settings).map_err(text)
    }

    /// Android has no models to load.
    #[cfg(target_os = "android")]
    fn preload(&self) {}

    /// The models loaded now, in the background, for the backend the
    /// settings name: a meeting started later does not wait for them.
    #[cfg(not(target_os = "android"))]
    fn preload(&self) {
        let Ok(backend) = self.backends.lock().unwrap().get(&self.settings()) else {
            return;
        };
        match backend {
            Backend::Local(local) => {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = local.load(1).await {
                        tracing::warn!("the models could not be loaded: {error:#}");
                    }
                });
            }
            Backend::Server(_) => {
                *self.models.lock().unwrap() = None;
                let _ = self.handle.emit("models", None::<local::Status>);
            }
        }
    }
}

/// A command's error, as the pages show it.
type Answer<T> = Result<T, String>;

fn text(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    settings: Settings,
    secrets: Secrets,
    /// Where meetings wait and voices are kept.
    data: String,
}

#[tauri::command]
fn get_settings(app: State<App>) -> SettingsView {
    SettingsView {
        settings: app.settings(),
        secrets: Secrets::read(),
        data: app.data.display().to_string(),
    }
}

/// Saves the settings; a secret given replaces the one kept
/// (empty removes it), one not given stays.
#[tauri::command]
fn save_settings(
    app: State<App>,
    settings: Settings,
    brainy_token: Option<String>,
    model_key: Option<String>,
    server_token: Option<String>,
) -> Answer<SettingsView> {
    if let Some(token) = server_token {
        Secret::ServerToken.set(&token).map_err(text)?;
    }
    if let Some(token) = brainy_token {
        Secret::BrainyToken.set(&token).map_err(text)?;
    }
    if let Some(key) = model_key {
        Secret::ModelKey.set(&key).map_err(text)?;
    }
    settings.save(&app.config).map_err(text)?;
    app.preload();
    Ok(get_settings(app))
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
fn models_state(app: State<App>) -> Option<local::Status> {
    app.models.lock().unwrap().clone()
}

/// No models on Android.
#[cfg(target_os = "android")]
#[tauri::command]
fn models_state() -> Option<()> {
    None
}

/// The models fetched and loaded again, after they failed to.
#[tauri::command]
fn load_models(app: State<App>) {
    app.preload();
}

#[tauri::command]
async fn list_devices() -> Answer<audio::Devices> {
    tokio::task::spawn_blocking(audio::device_names)
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

/// Whether logbook-server answers at `url` and takes the token: `token`
/// as typed and not saved yet, else the one kept.
#[tauri::command]
async fn check_server(url: String, token: Option<String>) -> Answer<()> {
    let token = token.or_else(|| Secret::ServerToken.get());
    remote::check(&remote::Server::new(url.trim()).with_token(token))
        .await
        .map_err(text)
}

/// The text models the API at `url` offers: `key` as typed and not saved
/// yet, else the one kept.
#[tauri::command]
async fn list_models(url: String, key: Option<String>) -> Answer<Vec<String>> {
    let options = polish::Options {
        url: url.trim().to_string(),
        model: String::new(),
        key: key.or_else(|| Secret::ModelKey.get()),
        timeout: std::time::Duration::from_secs(15),
    };
    tokio::task::spawn_blocking(move || polish::models(&options))
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

/// Whether brainy answers with the token, and the parent page exists.
#[tauri::command]
async fn check_brainy(app: State<'_, App>) -> Answer<()> {
    let settings = app.settings();
    let brainy = settings.credentials().any(None).map_err(text)?;
    brainy.check().await.map_err(text)?;
    let parent = settings.brainy.parent.trim();
    if !parent.is_empty() {
        brainy.resolve_page(parent).await.map_err(text)?;
    }
    Ok(())
}

#[tauri::command]
fn start_meeting(
    app: State<App>,
    handle: AppHandle,
    title: Option<String>,
    mode: Mode,
) -> Answer<recorder::State> {
    let mut settings = app.settings();
    if settings.audio.mode != mode {
        settings.audio.mode = mode;
        settings.save(&app.config).map_err(text)?;
    }
    let backend = app.backend(&settings)?;
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    app.recorder
        .start(
            handle,
            backend,
            app.data.clone(),
            settings.publish(title),
            settings.sources(),
            settings.echo(),
            settings.gain(),
        )
        .map_err(text)
}

#[tauri::command]
fn stop_meeting(app: State<App>, handle: AppHandle) {
    app.recorder.stop(&handle);
}

#[tauri::command]
fn set_notes(app: State<App>, part: usize, notes: String) {
    app.recorder.set_notes(part, notes);
}

/// A screenshot for the notes of the meeting being recorded.
#[cfg(not(target_os = "android"))]
#[tauri::command]
async fn take_screenshot(app: State<'_, App>, handle: AppHandle) -> Answer<screenshot::Shot> {
    let data = app.data.clone();
    tauri::async_runtime::spawn_blocking(move || screenshot::take(&handle, &data))
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

/// Android takes no screenshots.
#[cfg(target_os = "android")]
#[tauri::command]
fn take_screenshot() -> Answer<()> {
    Err("no screenshots on Android".into())
}

#[tauri::command]
fn meeting_state(app: State<App>) -> Option<recorder::State> {
    app.recorder.state()
}

#[tauri::command]
fn get_logs(app: State<App>) -> Vec<logs::Line> {
    app.logs.lines()
}

#[tauri::command]
fn list_meetings(app: State<App>) -> Answer<Vec<history::Summary>> {
    history::list(&app.data).map_err(text)
}

/// A kept meeting as the pages show it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingView {
    id: String,
    title: String,
    started: String,
    duration_s: f64,
    page: Option<String>,
    notes: String,
    report: Option<String>,
    report_error: Option<String>,
    warnings: Vec<String>,
    speakers: Vec<Speaker>,
    turns: Vec<Turn>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Speaker {
    label: String,
    /// The call's or the room's: a voice that can be named.
    nameable: bool,
}

/// One speaker's consecutive segments.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Turn {
    label: String,
    start: f64,
    phrases: Vec<Phrase>,
}

/// A segment of a turn, with its times: the page shows one where a long
/// pause falls inside a turn.
#[derive(Serialize)]
struct Phrase {
    start: f64,
    end: f64,
    text: String,
}

fn view(kept: history::Kept) -> MeetingView {
    let mut turns: Vec<Turn> = Vec::new();
    for u in &kept.utterances {
        let label = kept
            .writer
            .label(&u.channel, u.speaker)
            .unwrap_or("Unknown speaker")
            .to_string();
        let phrase = Phrase {
            start: u.start,
            end: u.end,
            text: u.text.clone(),
        };
        match turns.last_mut() {
            Some(t) if t.label == label => t.phrases.push(phrase),
            _ => turns.push(Turn {
                label,
                start: u.start,
                phrases: vec![phrase],
            }),
        }
    }
    MeetingView {
        speakers: kept
            .writer
            .speakers()
            .into_iter()
            .map(|(_, speaker, label)| Speaker {
                label,
                nameable: speaker.is_some(),
            })
            .collect(),
        turns,
        id: kept.id,
        title: kept.title,
        started: kept.started,
        duration_s: kept.duration_s,
        page: kept.page.map(|p| p.url),
        notes: kept.notes,
        report: kept.report,
        report_error: kept.report_error,
        warnings: kept.warnings,
    }
}

#[tauri::command]
fn get_meeting(app: State<App>, id: String) -> Answer<MeetingView> {
    history::load(&app.data, &id).map(view).map_err(text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenameView {
    renamed: Renamed,
    meeting: MeetingView,
}

/// Renames a speaker of a kept meeting: there, on its page, and in the
/// voices known by name.
#[tauri::command]
async fn rename_speaker(
    app: State<'_, App>,
    id: String,
    from: String,
    to: String,
) -> Answer<RenameView> {
    let settings = app.settings();
    let kept = history::load(&app.data, &id).map_err(text)?;
    let brainy = match &kept.page {
        Some(_) => Some(settings.credentials().any(None).map_err(text)?),
        None => None,
    };
    let renamed = meeting::rename_speaker(&app.data, &id, &from, &to, brainy.as_ref())
        .await
        .map_err(text)?;
    let meeting = history::load(&app.data, &id).map(view).map_err(text)?;
    Ok(RenameView { renamed, meeting })
}

/// Writes the report again (after a rename, or a failure), in place of
/// the last one on the page.
#[tauri::command]
async fn write_report(app: State<'_, App>, id: String) -> Answer<MeetingView> {
    let settings = app.settings();
    let backend = app.backend(&settings)?;
    let mut kept = history::load(&app.data, &id).map_err(text)?;
    let brainy = match &kept.page {
        Some(_) => Some(settings.credentials().any(None).map_err(text)?),
        None => None,
    };
    let images = history::images(&app.data, &id).map_err(text)?;
    meeting::write_report(&backend, brainy.as_ref(), &mut kept, &images).await;
    history::save(&app.data, &kept).map_err(text)?;
    Ok(view(kept))
}

/// The notes of a kept meeting, edited after it.
#[tauri::command]
fn save_meeting_notes(app: State<App>, id: String, notes: String) -> Answer<()> {
    let mut kept = history::load(&app.data, &id).map_err(text)?;
    kept.notes = notes;
    history::save(&app.data, &kept).map_err(text)
}

#[tauri::command]
fn delete_meeting(app: State<App>, id: String) -> Answer<()> {
    history::delete(&app.data, &id).map_err(text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingView {
    id: String,
    started: String,
    title: Option<String>,
}

#[tauri::command]
fn list_pending(app: State<App>) -> Answer<Vec<PendingView>> {
    let mut out = Vec::new();
    for dir in Pending::list(&app.data).map_err(text)? {
        if let Ok(p) = Pending::open(&dir) {
            out.push(PendingView {
                id: p.id(),
                started: p.meeting.started.clone(),
                title: p.meeting.publish.title.clone(),
            });
        }
    }
    Ok(out)
}

/// On Android, the meetings left pending finished, and tried again until
/// the server and brainy answer (`android::Finisher`): the app is the only
/// way to finish them there.
#[cfg(target_os = "android")]
#[tauri::command]
fn finish_pending(app: State<'_, App>) -> Answer<()> {
    let settings = app.settings();
    let backend = app.backend(&settings)?;
    app.finisher.start(
        app.handle.clone(),
        backend,
        app.data.clone(),
        settings.credentials(),
    );
    Ok(())
}

/// Finishes the meetings left pending (the server or brainy could not be
/// reached), one after the other; `meetings-changed` when done.
#[cfg(not(target_os = "android"))]
#[tauri::command]
async fn finish_pending(app: State<'_, App>, handle: AppHandle) -> Answer<()> {
    let settings = app.settings();
    let backend = app.backend(&settings)?;
    let credentials = settings.credentials();
    let data = app.data.clone();
    let dirs = Pending::list(&data).map_err(text)?;
    tauri::async_runtime::spawn(async move {
        for dir in dirs {
            let result = async {
                // One being recorded, or finished elsewhere, is left alone.
                let Some(mut pending) = Pending::claim(&dir)? else {
                    return Ok(None);
                };
                backend.prepare(false, 1).await?;
                meeting::finish(&backend, &mut pending, &credentials)
                    .await
                    .map(Some)
            }
            .await;
            match result {
                Ok(Some(_)) => tracing::info!("finished the pending meeting {}", dir.display()),
                Ok(None) => tracing::info!("{} is being recorded or finished", dir.display()),
                Err(error) => tracing::warn!("{error:#}"),
            }
        }
        let _ = handle.emit("meetings-changed", ());
    });
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Voice {
    name: String,
    prints: usize,
    seconds: f64,
}

#[tauri::command]
fn list_voices(app: State<App>) -> Answer<Vec<Voice>> {
    let lib = voices::Library::load(&app.data).map_err(text)?;
    Ok(lib
        .people
        .into_iter()
        .map(|p| Voice {
            seconds: p.prints.iter().map(|q| q.seconds).sum(),
            prints: p.prints.len(),
            name: p.name,
        })
        .collect())
}

#[tauri::command]
fn forget_voice(app: State<App>, name: String) -> Answer<()> {
    voices::forget(&app.data, &name).map_err(text)
}

/// Whether Android's battery optimization leaves the app alone (always on
/// the desktop).
#[tauri::command]
async fn background_state(handle: AppHandle) -> Answer<bool> {
    #[cfg(target_os = "android")]
    return android::unrestricted(&handle).await;
    #[cfg(not(target_os = "android"))]
    {
        let _ = handle;
        Ok(true)
    }
}

/// Android's dialog that exempts the app from battery optimization.
#[tauri::command]
async fn ask_background(handle: AppHandle) -> Answer<bool> {
    #[cfg(target_os = "android")]
    return android::ask_unrestricted(&handle, false).await;
    #[cfg(not(target_os = "android"))]
    {
        let _ = handle;
        Ok(true)
    }
}

/// Where meetings wait and voices are kept: the CLI's folder on the
/// desktop; the app's own on Android, where `dirs` has none.
fn data_folder(app: &tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "android")]
    return Ok(app.path().app_data_dir()?);
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(dirs::data_dir()
            .ok_or("no data folder for this user")?
            .join("logbook"))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logs = Logs::install();
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }));
    #[cfg(not(target_os = "android"))]
    let builder = builder
        .plugin(screenshot::plugin(|app| app.state::<App>().data.clone()))
        .register_uri_scheme_protocol("shot", |ctx, request| {
            screenshot::serve(&ctx.app_handle().state::<App>().data, &request)
        });
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_recording::init());
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |tauri_app| {
            logs.attach(tauri_app.handle().clone());
            let data = data_folder(tauri_app)?;
            let config = tauri_app.path().app_config_dir()?;
            Secret::open(&config);
            let handle = tauri_app.handle().clone();
            #[cfg(not(target_os = "android"))]
            let models = Arc::new(Mutex::new(None));
            #[cfg(not(target_os = "android"))]
            let backends = {
                let on_status: local::OnStatus = {
                    let models = models.clone();
                    let handle = handle.clone();
                    Arc::new(move |status: local::Status| {
                        *models.lock().unwrap() = Some(status.clone());
                        let _ = handle.emit("models", Some(status));
                    })
                };
                Backends::new(on_status)
            };
            #[cfg(target_os = "android")]
            let backends = Backends;
            tauri_app.manage(App {
                data,
                config,
                recorder: Recorder::default(),
                backends: Mutex::new(backends),
                #[cfg(not(target_os = "android"))]
                models,
                #[cfg(target_os = "android")]
                finisher: android::Finisher::default(),
                handle,
                logs: logs.clone(),
            });
            let app = tauri_app.state::<App>();
            app.preload();
            // What a meeting left pending at the last run needs: the server.
            #[cfg(target_os = "android")]
            if let Ok(backend) = app.backend(&app.settings()) {
                let settings = app.settings();
                app.finisher.start(
                    app.handle.clone(),
                    backend,
                    app.data.clone(),
                    settings.credentials(),
                );
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            models_state,
            load_models,
            list_devices,
            check_server,
            check_brainy,
            list_models,
            start_meeting,
            stop_meeting,
            set_notes,
            take_screenshot,
            meeting_state,
            get_logs,
            list_meetings,
            get_meeting,
            rename_speaker,
            write_report,
            save_meeting_notes,
            delete_meeting,
            list_pending,
            finish_pending,
            list_voices,
            forget_voice,
            background_state,
            ask_background,
        ])
        .build(tauri::generate_context!())
        .expect("the app could not start")
        .run(|_, _event| {
            // Tauri would call `process::exit`, which on Windows runs
            // libparakeet's teardown in the GPU driver (`parakeet::exit`).
            #[cfg(not(target_os = "android"))]
            if let tauri::RunEvent::Exit = _event {
                logbook_engine::parakeet::exit(0);
            }
        });
}
