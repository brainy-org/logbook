//! Final passes by id, the ones live sessions started and the ones clients
//! uploaded (`POST /finals`), so that a client that lost its connection
//! fetches the result later (`GET /finals/{id}`).
//!
//! In memory: a restarted server forgets them, and the client, which keeps
//! its own copy of the meeting, uploads it again. A result is kept for
//! `KEEP` after it is ready, then dropped. The id is the only key to a
//! meeting's transcript: 128 random bits, never logged next to the text.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use logbook_engine::protocol::FinalState;

const KEEP: Duration = Duration::from_secs(3600);

#[derive(Default)]
pub struct Finals {
    entries: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    state: FinalState,
    /// When the result was ready; None while the pass runs.
    ready: Option<Instant>,
}

impl Finals {
    /// A new pass, running; returns its id.
    pub fn start(&self, audio_s: f64) -> Result<String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, e| e.ready.is_none_or(|t| t.elapsed() < KEEP));
        entries.insert(
            id.clone(),
            Entry {
                state: FinalState::Running { audio_s },
                ready: None,
            },
        );
        Ok(id)
    }

    pub fn finish(&self, id: &str, state: FinalState) {
        if let Some(entry) = self.entries.lock().unwrap().get_mut(id) {
            entry.state = state;
            entry.ready = Some(Instant::now());
        }
    }

    pub fn get(&self, id: &str) -> Option<FinalState> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(id)?;
        if entry.ready.is_some_and(|t| t.elapsed() >= KEEP) {
            return None;
        }
        Some(entry.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_is_found_by_its_id() {
        let finals = Finals::default();
        let id = finals.start(12.0).unwrap();
        assert_eq!(id.len(), 32);
        assert!(matches!(finals.get(&id), Some(FinalState::Running { .. })));
        finals.finish(
            &id,
            FinalState::Failed {
                message: "no".into(),
            },
        );
        assert!(matches!(finals.get(&id), Some(FinalState::Failed { .. })));
        assert!(finals.get("other").is_none());
        assert_ne!(finals.start(1.0).unwrap(), id);
    }
}
