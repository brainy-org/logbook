//! Transcription for every session, in batches.
//!
//! One thread owns the calls to the ASR model. It takes the first phrase
//! waiting, adds whatever else is queued up to `max`, and transcribes them in
//! one call: while a batch runs, the next one fills up. No phrase waits for a
//! batch to fill, so a quiet server adds no latency and a busy one batches
//! more.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use logbook_engine::channel::Clip;
use logbook_engine::parakeet::{Model, SAMPLE_RATE};
use tracing::{info, warn};

use crate::session::Input;

pub struct Job {
    pub clip: Clip,
    pub reply: Sender<Input>,
    pub queued: Instant,
}

pub fn spawn(asr: Arc<Model>, max: usize) -> Sender<Job> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("batcher".into())
        .spawn(move || run(&asr, &rx, max.max(1)))
        .expect("could not start the batcher thread");
    tx
}

fn run(asr: &Model, rx: &Receiver<Job>, max: usize) {
    while let Ok(first) = rx.recv() {
        let mut jobs = vec![first];
        while jobs.len() < max {
            match rx.try_recv() {
                Ok(job) => jobs.push(job),
                Err(_) => break,
            }
        }
        let waited = jobs
            .iter()
            .map(|j| j.queued.elapsed())
            .max()
            .unwrap_or_default();
        let audio: usize = jobs.iter().map(|j| j.clip.samples.len()).sum();
        let clips: Vec<&[f32]> = jobs.iter().map(|j| j.clip.samples.as_slice()).collect();
        let started = Instant::now();
        let result = asr.transcribe(&clips);
        let took = started.elapsed();
        info!(
            phrases = jobs.len(),
            audio_s = format!("{:.1}", audio as f64 / SAMPLE_RATE as f64),
            compute_ms = took.as_millis() as u64,
            max_wait_ms = waited.as_millis() as u64,
            "batch"
        );
        match result {
            Ok(transcripts) => {
                for (job, transcript) in jobs.into_iter().zip(transcripts) {
                    let _ = job.reply.send(Input::Transcribed {
                        channel: job.clip.channel,
                        id: job.clip.id,
                        transcript,
                    });
                }
            }
            Err(error) => {
                warn!("a batch of {} phrases failed: {error:#}", jobs.len());
                for job in jobs {
                    let _ = job.reply.send(Input::Transcribed {
                        channel: job.clip.channel,
                        id: job.clip.id,
                        transcript: Default::default(),
                    });
                }
            }
        }
    }
}
