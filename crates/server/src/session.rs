//! One live meeting: a thread that owns its channels.
//!
//! Audio, transcripts and the end of the audio all arrive in one inbox, so
//! the channels are touched by this thread alone. A tick wakes it when
//! nothing arrives, to send what diarization has meanwhile made ready.
//! With a final pass, the audio is also recorded, and handed to the
//! finalizer once the live transcript is complete.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use logbook_engine::Engine;
use logbook_engine::channel::Channel;
use logbook_engine::echo::EchoFilter;
use logbook_engine::parakeet::Transcript;
use logbook_engine::protocol::{ChannelConfig, ServerMessage};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::batcher::Job;
use crate::finalizer::FinalJob;
use crate::finals::Finals;
use crate::recording::Recording;

/// What every session shares.
pub struct Shared {
    pub engine: Arc<Engine>,
    pub batcher: Sender<Job>,
    pub finalizer: Sender<FinalJob>,
    pub finals: Arc<Finals>,
    /// Where sessions with a final pass record their audio.
    pub recordings: PathBuf,
}

pub enum Input {
    Audio(usize, Vec<f32>),
    Transcribed {
        channel: usize,
        id: u64,
        transcript: Transcript,
    },
    /// The client has no more audio.
    Stop,
    /// The connection is gone: nobody is left to send to.
    Closed,
}

const TICK: Duration = Duration::from_millis(100);

pub fn run(
    shared: &Shared,
    session: String,
    configs: Vec<ChannelConfig>,
    final_pass: bool,
    inbox: Receiver<Input>,
    reply: Sender<Input>,
    out: UnboundedSender<ServerMessage>,
) {
    let engine = &shared.engine;
    let batcher = &shared.batcher;
    let configs_echo: Vec<Option<String>> = configs.iter().map(|c| c.echo_of.clone()).collect();
    let mut recording = None;
    if final_pass {
        match Recording::create(&shared.recordings, &session, configs.len()) {
            Ok(r) => recording = Some(r),
            Err(error) => {
                let _ = out.send(ServerMessage::Error {
                    message: format!("could not record for the final pass: {error:#}"),
                });
                return;
            }
        }
    }
    // Why the recording stopped, if it did: the final pass is then refused.
    let mut recording_failed: Option<String> = None;
    let mut channels: Vec<Channel> = match configs
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, config)| engine.channel(i, config))
        .collect::<Result<_>>()
    {
        Ok(channels) => channels,
        Err(error) => {
            let _ = out.send(ServerMessage::Error {
                message: format!("{error:#}"),
            });
            return;
        }
    };
    // For each channel that may hear another again: that one's index and the filter.
    let mut echo: Vec<Option<(usize, EchoFilter)>> = Vec::new();
    for (i, config) in configs_echo.iter().enumerate() {
        let source = match config {
            None => None,
            Some(name) => match channels.iter().position(|c| c.name() == name) {
                Some(j) if j != i => Some((j, EchoFilter::default())),
                _ => {
                    let _ = out.send(ServerMessage::Error {
                        message: format!("echo_of names no other channel: {name:?}"),
                    });
                    return;
                }
            },
        };
        echo.push(source);
    }
    let names: Vec<&str> = channels.iter().map(|c| c.name()).collect();
    info!(session, channels = ?names, final_pass, "session started");
    let _ = out.send(ServerMessage::Ready);

    let submit = |clips: Vec<logbook_engine::channel::Clip>| {
        for clip in clips {
            let _ = batcher.send(Job {
                clip,
                reply: reply.clone(),
                queued: Instant::now(),
            });
        }
    };
    let mut stopping = false;
    loop {
        let input = match inbox.recv_timeout(TICK) {
            Ok(input) => Some(input),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let step = (|| -> Result<bool> {
            match input {
                Some(Input::Audio(index, pcm)) if !stopping => {
                    if let Some(r) = &mut recording
                        && let Err(error) = r.write(index, &pcm)
                    {
                        warn!(session, "recording stopped: {error:#}");
                        recording_failed = Some(format!("{error:#}"));
                        recording = None;
                    }
                    if let Some(channel) = channels.get_mut(index) {
                        submit(channel.feed(&pcm)?);
                    }
                }
                Some(Input::Audio(..)) | None => {}
                Some(Input::Transcribed {
                    channel,
                    id,
                    transcript,
                }) => {
                    if let Some(channel) = channels.get_mut(channel) {
                        channel.transcribed(id, transcript);
                    }
                }
                Some(Input::Stop) => {
                    if !stopping {
                        stopping = true;
                        for channel in &mut channels {
                            submit(channel.finish()?);
                        }
                    }
                }
                Some(Input::Closed) => return Ok(false),
            }
            for i in 0..channels.len() {
                for utterance in channels[i].ready()? {
                    for (source, filter) in echo.iter_mut().flatten() {
                        if *source == i {
                            filter.heard(&utterance);
                        }
                    }
                    match &mut echo[i] {
                        Some((_, filter)) => filter.hold(utterance),
                        None => {
                            let _ = out.send(ServerMessage::Utterance(utterance));
                        }
                    }
                }
            }
            for (source, filter) in echo.iter_mut().flatten() {
                for utterance in filter.release(channels[*source].settled()) {
                    let _ = out.send(ServerMessage::Utterance(utterance));
                }
            }
            Ok(true)
        })();
        match step {
            Ok(true) => {}
            Ok(false) => {
                info!(session, "session closed by the client");
                return;
            }
            Err(error) => {
                warn!("session failed: {error:#}");
                let _ = out.send(ServerMessage::Error {
                    message: format!("{error:#}"),
                });
                return;
            }
        }
        if stopping
            && channels.iter().all(Channel::idle)
            && echo.iter().flatten().all(|(_, f)| f.is_empty())
        {
            let audio = channels.iter().map(|c| c.time()).fold(0.0, f64::max);
            info!(
                session,
                audio_s = format!("{audio:.1}"),
                "live transcript done"
            );
            if !final_pass {
                let _ = out.send(ServerMessage::Done);
                return;
            }
            let files = match (recording.take(), recording_failed.take()) {
                (Some(r), None) => r.finish(),
                (_, Some(reason)) => Err(anyhow::anyhow!("the recording stopped: {reason}")),
                (None, None) => Err(anyhow::anyhow!("nothing was recorded")),
            };
            match files.and_then(|files| Ok((files, shared.finals.start(audio)?))) {
                Ok((files, id)) => {
                    let _ = out.send(ServerMessage::Finalizing {
                        audio_s: audio,
                        id: id.clone(),
                    });
                    let job = FinalJob {
                        label: session,
                        id,
                        channels: configs.into_iter().zip(files).collect(),
                        out: Some(out),
                    };
                    let _ = shared.finalizer.send(job);
                }
                Err(error) => {
                    let _ = out.send(ServerMessage::Error {
                        message: format!("no final pass: {error:#}"),
                    });
                }
            }
            return;
        }
    }
}
