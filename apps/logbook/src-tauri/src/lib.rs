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
mod finisher;
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
use logbook_engine::llm;
use serde::Serialize;
#[cfg(not(target_os = "android"))]
use tauri::Emitter;
use tauri::{AppHandle, Manager, State};

use crate::finisher::{Finisher, Finishing};
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
    /// Pending meetings finished again until the server and brainy answer.
    finisher: Finisher,
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
    // A meeting stopped by a token or an address is tried with the new ones.
    app.finisher.start(&app.handle);
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
    let options = llm::Options {
        url: url.trim().to_string(),
        model: String::new(),
        key: key.or_else(|| Secret::ModelKey.get()),
        timeout: std::time::Duration::from_secs(15),
    };
    tokio::task::spawn_blocking(move || llm::models(&options))
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
    expected: Vec<String>,
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
            settings.publish(title, expected),
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
async fn take_screenshot(
    app: State<'_, App>,
    handle: AppHandle,
    kind: screenshot::Kind,
) -> Answer<Option<screenshot::Shot>> {
    let data = app.data.clone();
    tauri::async_runtime::spawn_blocking(move || screenshot::take(&handle, &data, kind))
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

/// The times go out on the displayed clock, as the page
/// has them: 00:00 at the first word (`Writer::shown`).
fn view(kept: history::Kept) -> MeetingView {
    let mut turns: Vec<Turn> = Vec::new();
    let shown = |t: f64| kept.writer.shown(t);
    for u in &kept.utterances {
        let label = kept
            .writer
            .label(&u.channel, u.speaker)
            .unwrap_or("Unknown speaker")
            .to_string();
        let phrase = Phrase {
            start: shown(u.start),
            end: shown(u.end),
            text: u.text.clone(),
        };
        match turns.last_mut() {
            Some(t) if t.label == label => t.phrases.push(phrase),
            _ => turns.push(Turn {
                label,
                start: shown(u.start),
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

/// The names offered for speaker `label` of kept meeting `id`, likeliest
/// first: none once its voice is no longer kept (the last meetings').
#[tauri::command]
fn speaker_candidates(app: State<App>, id: String, label: String) -> Answer<Vec<String>> {
    let kept = history::load(&app.data, &id).map_err(text)?;
    let taken: Vec<String> = kept.writer.speakers().into_iter().map(|s| s.2).collect();
    voices::candidates(&app.data, &id, &label, &taken).map_err(text)
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

/// Writes a kept meeting published without brainy to a new page, under
/// the parent page of the settings.
#[tauri::command]
async fn send_to_brainy(app: State<'_, App>, id: String) -> Answer<MeetingView> {
    let settings = app.settings();
    let brainy = settings
        .credentials()
        .any(None)
        .map_err(|_| "brainy needs its address and a token: set them in Settings".to_string())?;
    let parent = Some(settings.brainy.parent.trim()).filter(|p| !p.is_empty());
    let kept = meeting::send_to_brainy(&app.data, &id, &brainy, parent)
        .await
        .map_err(text)?;
    Ok(view(kept))
}

/// The notes of a kept meeting, edited after it.
#[tauri::command]
fn save_meeting_notes(app: State<App>, id: String, notes: String) -> Answer<()> {
    let mut kept = history::load(&app.data, &id).map_err(text)?;
    kept.notes = notes;
    history::save(&app.data, &kept).map_err(text)
}

/// Takes a meeting out of the list, into the trash until `purge_meeting`
/// (the undo's time is over) or the app's next start.
#[tauri::command]
fn delete_meeting(app: State<App>, id: String) -> Answer<()> {
    history::trash(&app.data, &id).map_err(text)
}

#[tauri::command]
fn restore_meeting(app: State<App>, id: String) -> Answer<()> {
    history::restore(&app.data, &id).map_err(text)
}

#[tauri::command]
fn purge_meeting(app: State<App>, id: String) -> Answer<()> {
    history::purge(&app.data, &id).map_err(text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingView {
    id: String,
    started: String,
    title: Option<String>,
    /// Seconds of the meeting's audio, every channel's longest.
    audio_s: f64,
    /// Where finishing it is, once a run has seen it.
    finishing: Option<Finishing>,
}

#[tauri::command]
fn list_pending(app: State<App>) -> Answer<Vec<PendingView>> {
    let mut out = Vec::new();
    for dir in Pending::list(&app.data).map_err(text)? {
        if let Ok(p) = Pending::open(&dir) {
            let samples = (0..p.meeting.channels.len())
                .map(|i| p.samples(i))
                .max()
                .unwrap_or(0);
            out.push(PendingView {
                finishing: app.finisher.state(&p.id()),
                id: p.id(),
                started: p.meeting.shown_started(),
                title: p.meeting.publish.title.clone(),
                audio_s: samples as f64 / logbook_engine::protocol::SAMPLE_RATE as f64,
            });
        }
    }
    Ok(out)
}

/// The meetings left pending finished, tried again until the server and
/// brainy answer (finisher.rs): Finish now, or Try again after a problem.
#[tauri::command]
fn finish_pending(app: State<'_, App>) {
    app.finisher.start(&app.handle);
}

/// A person known by voice as the voices page shows them: never their
/// prints, which stay in this process (a voiceprint is personal data).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Voice {
    name: String,
    /// Seconds of their voice, all prints together.
    seconds: f64,
    /// The meetings their prints come from (at most the latest 5).
    meetings: usize,
    /// The latest meeting they were heard in, recognized or named (its
    /// id, `YYYYMMDD-HHMMSS`; `Person::last_heard`).
    heard: String,
    /// No print has `GOOD_ENROLL` seconds: they may not be recognized
    /// every time (a person is matched by their best print).
    weak: bool,
}

#[tauri::command]
fn list_voices(app: State<App>) -> Answer<Vec<Voice>> {
    let lib = voices::Library::load(&app.data).map_err(text)?;
    Ok(lib
        .people
        .into_iter()
        .map(|p| {
            let mut meetings: Vec<&str> = p.prints.iter().map(|q| q.meeting.as_str()).collect();
            meetings.sort_unstable();
            meetings.dedup();
            Voice {
                seconds: p.prints.iter().map(|q| q.seconds).sum(),
                meetings: meetings.len(),
                heard: p.last_heard().to_string(),
                weak: p.prints.iter().all(|q| q.seconds < voices::GOOD_ENROLL),
                name: p.name,
            }
        })
        .collect())
}

/// Gives a person known by voice another name; refused when someone else
/// has it (`Library::rename`).
#[tauri::command]
fn rename_voice(app: State<App>, from: String, to: String) -> Answer<()> {
    voices::rename(&app.data, &from, &to).map_err(text)
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
            // Meetings deleted at the last run, their undo not waited for.
            history::empty_trash(&data);
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
                finisher: Finisher::default(),
                handle,
                logs: logs.clone(),
            });
            let app = tauri_app.state::<App>();
            app.preload();
            // What a meeting left pending at the last run needs: the
            // server, or the models here, and brainy.
            app.finisher.start(&app.handle);
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
            speaker_candidates,
            write_report,
            send_to_brainy,
            save_meeting_notes,
            delete_meeting,
            restore_meeting,
            purge_meeting,
            list_pending,
            finish_pending,
            list_voices,
            rename_voice,
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
