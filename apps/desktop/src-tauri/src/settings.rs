//! What the app remembers: where meetings are transcribed, what is
//! recorded, where they go and which language model reads them, in
//! `settings.json` in the app's config folder. The brainy token and the
//! language model's key are not there: they are in the system's keychain
//! (`Secret`), and never go back to the pages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use logbook_client::aec::Gain;
use logbook_client::audio::{Echo, Source};
use logbook_client::local;
use logbook_client::meeting::{Backend, Credentials, Publish};
use logbook_client::remote::Server;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where meetings are transcribed: here, or on a logbook-server.
    pub transcription: Transcription,
    /// logbook-server's address, used with `Transcription::Server`.
    pub server_url: String,
    /// The person behind the microphone.
    pub me: String,
    pub audio: AudioSettings,
    pub brainy: BrainySettings,
    pub model: ModelSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transcription {
    Local,
    Server,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioSettings {
    /// Record the call (what the computer plays) as a second channel.
    pub system: bool,
    /// By name; None for the system's default.
    pub mic_device: Option<String>,
    pub system_device: Option<String>,
    /// The microphone does not hear the speakers: nothing cancelled.
    pub headphones: bool,
    /// Raise a quiet microphone's level (AGC2).
    pub mic_gain_auto: bool,
    /// Tell apart the people in the room on the microphone.
    pub diarize_mic: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BrainySettings {
    /// Write each meeting as a page in brainy.
    pub enabled: bool,
    pub url: String,
    /// The page the meetings go under: address, id or path of titles.
    pub parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelSettings {
    /// Clean the final transcript up (fillers, false starts).
    pub polish: bool,
    /// Write the report at the end (summary, notes, decisions, actions).
    pub report: bool,
    /// The model used without a server (with one, the server's is): an
    /// OpenAI-compatible API up to `/v1`, and the model's name.
    pub url: String,
    pub name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            transcription: Transcription::Local,
            server_url: "http://127.0.0.1:3300".into(),
            me: "Me".into(),
            audio: AudioSettings::default(),
            brainy: BrainySettings::default(),
            model: ModelSettings::default(),
        }
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            system: true,
            mic_device: None,
            system_device: None,
            headphones: false,
            mic_gain_auto: false,
            diarize_mic: false,
        }
    }
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            polish: false,
            report: true,
            url: "https://api.groq.com/openai/v1".into(),
            name: "qwen/qwen3.8-27b".into(),
        }
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(file(dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = file(dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
    }

    /// The language model here: only without a server, and only once set.
    pub fn local_config(&self) -> local::Config {
        let set = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        local::Config {
            models: None,
            polish_url: set(&self.model.url),
            polish_model: set(&self.model.name),
            polish_key: Secret::ModelKey.get(),
        }
    }

    pub fn server(&self) -> Option<Server> {
        (self.transcription == Transcription::Server && !self.server_url.trim().is_empty())
            .then(|| Server::new(self.server_url.trim()))
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            url: Some(self.brainy.url.trim().to_string()).filter(|u| !u.is_empty()),
            token: Secret::BrainyToken.get(),
        }
    }

    /// What happens to a meeting started now, titled `title`.
    pub fn publish(&self, title: Option<String>) -> Publish {
        let credentials = self.credentials();
        Publish {
            final_pass: true,
            polish: self.model.polish,
            report: self.model.report,
            keep: true,
            brainy: self.brainy.enabled,
            brainy_url: credentials.url,
            brainy_token: credentials.token,
            parent: Some(self.brainy.parent.trim().to_string()).filter(|p| !p.is_empty()),
            title,
            me: Some(self.me.trim().to_string())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "Me".into()),
        }
    }

    /// The channels to record: the microphone, and the call.
    pub fn sources(&self) -> Vec<(Source, Option<String>, bool)> {
        let mut sources = vec![(
            Source::Microphone,
            self.audio.mic_device.clone(),
            self.audio.diarize_mic,
        )];
        if self.audio.system {
            sources.push((Source::System, self.audio.system_device.clone(), true));
        }
        sources
    }

    pub fn echo(&self) -> Echo {
        if self.audio.system && !self.audio.headphones {
            Echo::Cancel
        } else {
            Echo::None
        }
    }

    pub fn gain(&self) -> Gain {
        if self.audio.mic_gain_auto {
            Gain::Auto
        } else {
            Gain::Off
        }
    }
}

/// What the pages are told of the secrets: whether they are set.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Secrets {
    pub brainy_token: bool,
    pub model_key: bool,
}

impl Secrets {
    pub fn read() -> Self {
        Self {
            brainy_token: Secret::BrainyToken.get().is_some(),
            model_key: Secret::ModelKey.get().is_some(),
        }
    }
}

/// A secret in the system's keychain, under the service "logbook", read
/// from there once per run: macOS may ask the person at every read (a
/// build signed otherwise than when they chose "Always"), and the
/// settings are read at every command.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Secret {
    BrainyToken,
    ModelKey,
}

/// What the keychain answered this run, by secret: None when unset or
/// refused (refused, it is not asked again until the app starts again).
static READ: Mutex<Option<HashMap<Secret, Option<String>>>> = Mutex::new(None);

impl Secret {
    fn entry(self) -> keyring::Result<keyring::Entry> {
        let name = match self {
            Secret::BrainyToken => "brainy-token",
            Secret::ModelKey => "model-key",
        };
        keyring::Entry::new("logbook", name)
    }

    pub fn get(self) -> Option<String> {
        let mut read = READ.lock().unwrap();
        read.get_or_insert_default()
            .entry(self)
            .or_insert_with(|| {
                self.entry()
                    .and_then(|e| e.get_password())
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .clone()
    }

    /// Sets it, or removes it when `value` is empty.
    pub fn set(self, value: &str) -> Result<()> {
        let entry = self.entry()?;
        let value = value.trim();
        if value.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            entry.set_password(value)?;
        }
        let set = Some(value.to_string()).filter(|v| !v.is_empty());
        READ.lock()
            .unwrap()
            .get_or_insert_default()
            .insert(self, set);
        Ok(())
    }
}

/// The backend the settings name: the same `Local` while its model
/// settings stay the same, so the models are loaded once.
pub struct Backends {
    local: Option<(String, Backend)>,
}

impl Backends {
    pub fn new() -> Self {
        Self { local: None }
    }

    pub fn get(&mut self, settings: &Settings) -> Backend {
        if let Some(server) = settings.server() {
            return Backend::Server(server);
        }
        let config = settings.local_config();
        let key = format!(
            "{:?}|{:?}|{}",
            config.polish_url,
            config.polish_model,
            config.polish_key.is_some()
        );
        match &self.local {
            Some((k, backend)) if *k == key => backend.clone(),
            _ => {
                let backend = Backend::Local(std::sync::Arc::new(local::Local::new(config)));
                self.local = Some((key, backend.clone()));
                backend
            }
        }
    }
}
