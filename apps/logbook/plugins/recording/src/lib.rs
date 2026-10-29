//! What keeps logbook's Android app recording with the screen off: a
//! foreground service of type microphone with its notification, a partial
//! wake lock and a Wi-Fi lock (Kotlin, `android/`), the microphone's and
//! the notifications' permissions, and the exemption from battery
//! optimization. The app's Rust calls it as a recording starts and once
//! its meetings have finished; the pages never do (no commands).
//!
//! On the desktop it is not registered and does nothing.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "io.github.brainy_org.logbook.recording";

/// The Kotlin side, once registered.
pub struct Recording<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    _runtime: std::marker::PhantomData<fn() -> R>,
}

/// A permission's state, as Android's plugin answers it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionState {
    Granted,
    Denied,
    Prompt,
    PromptWithRationale,
}

#[derive(Debug, Deserialize)]
pub struct Permissions {
    pub microphone: PermissionState,
    pub notifications: PermissionState,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Background {
    unrestricted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Start {
    title: String,
    on_stop: tauri::ipc::Channel<serde_json::Value>,
}

#[derive(Serialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Update<'a> {
    text: &'a str,
    stoppable: bool,
}

#[derive(Serialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Ask {
    once: bool,
}

#[derive(Serialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct Request {
    permissions: Vec<&'static str>,
}

type Result<T> = std::result::Result<T, String>;

#[cfg(target_os = "android")]
impl<R: Runtime> Recording<R> {
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        payload: impl Serialize,
    ) -> Result<T> {
        self.handle
            .run_mobile_plugin(command, payload)
            .map_err(|e| e.to_string())
    }

    /// Asks for the microphone and the notifications (once each: Android
    /// answers at once afterwards). Blocks until the person answers: not
    /// on the main thread.
    pub fn request_permissions(&self) -> Result<Permissions> {
        self.call(
            "requestPermissions",
            Request {
                permissions: vec!["microphone", "notifications"],
            },
        )
    }

    /// The foreground service, its notification titled `title`, and the
    /// locks; `on_stop` is called when the notification's Stop is pressed.
    /// While the app is in the foreground.
    pub fn start(&self, title: &str, on_stop: impl Fn() + Send + Sync + 'static) -> Result<()> {
        let on_stop = tauri::ipc::Channel::new(move |_| {
            on_stop();
            Ok(())
        });
        self.call(
            "start",
            Start {
                title: title.to_string(),
                on_stop,
            },
        )
    }

    /// The notification's line; `stoppable`: with its Stop button.
    pub fn update(&self, text: &str, stoppable: bool) -> Result<()> {
        self.call("update", Update { text, stoppable })
    }

    /// The service stopped, the locks released.
    pub fn stop(&self) -> Result<()> {
        self.call("stop", ())
    }

    /// Whether Android's battery optimization leaves the app alone.
    pub fn unrestricted(&self) -> Result<bool> {
        self.call::<Background>("background", ())
            .map(|b| b.unrestricted)
    }

    /// The system's dialog to exempt the app from battery optimization;
    /// with `once`, only if never asked. Answers with the state before it.
    pub fn ask_unrestricted(&self, once: bool) -> Result<bool> {
        self.call::<Background>("askBackground", Ask { once })
            .map(|b| b.unrestricted)
    }
}

#[cfg(not(target_os = "android"))]
impl<R: Runtime> Recording<R> {
    pub fn request_permissions(&self) -> Result<Permissions> {
        Ok(Permissions {
            microphone: PermissionState::Granted,
            notifications: PermissionState::Granted,
        })
    }

    pub fn start(&self, _title: &str, _on_stop: impl Fn() + Send + Sync + 'static) -> Result<()> {
        Ok(())
    }

    pub fn update(&self, _text: &str, _stoppable: bool) -> Result<()> {
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        Ok(())
    }

    pub fn unrestricted(&self) -> Result<bool> {
        Ok(true)
    }

    pub fn ask_unrestricted(&self, _once: bool) -> Result<bool> {
        Ok(true)
    }
}

/// `app.recording()`.
pub trait RecordingExt<R: Runtime> {
    fn recording(&self) -> &Recording<R>;
}

impl<R: Runtime, T: Manager<R>> RecordingExt<R> for T {
    fn recording(&self) -> &Recording<R> {
        self.state::<Recording<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("recording")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let recording = Recording {
                handle: api
                    .register_android_plugin(PLUGIN_IDENTIFIER, "RecordingPlugin")
                    .map_err(|e| e.to_string())?,
            };
            #[cfg(not(target_os = "android"))]
            let recording = {
                let _ = api;
                Recording::<R> {
                    _runtime: std::marker::PhantomData,
                }
            };
            app.manage(recording);
            Ok(())
        })
        .build()
}
