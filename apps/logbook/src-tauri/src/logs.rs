//! crates/client's messages for the person (`tracing`, info and warn), to
//! the pages as `log` events, the last ones kept for a page that opens
//! later; and to stderr, for `tauri dev`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Messages kept for the pages.
const KEEP: usize = 200;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub warning: bool,
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

#[derive(Clone, Default)]
pub struct Logs {
    lines: Arc<Mutex<VecDeque<Line>>>,
    app: Arc<OnceLock<AppHandle>>,
}

impl Logs {
    /// Installs the subscriber; the pages get the lines once `attach`ed.
    pub fn install() -> Self {
        let logs = Logs::default();
        let filter = || {
            EnvFilter::try_from_env("LOGBOOK_LOG")
                .unwrap_or_else(|_| EnvFilter::new("logbook_client=info,logbook_app=info"))
        };
        let _ = tracing_subscriber::registry()
            .with(logs.clone().with_filter(filter()))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(false)
                    .with_filter(filter()),
            )
            .try_init();
        logs
    }

    pub fn attach(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    pub fn lines(&self) -> Vec<Line> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
}

impl<S: Subscriber> Layer<S> for Logs {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > Level::INFO {
            return;
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let line = Line {
            warning: level <= Level::WARN,
            message: message.0,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
        };
        {
            let mut lines = self.lines.lock().unwrap();
            if lines.len() == KEEP {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        if let Some(app) = self.app.get() {
            let _ = app.emit("log", line);
        }
    }
}
