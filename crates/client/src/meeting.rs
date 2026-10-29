//! A meeting from its first sample to its page: the audio to the backend
//! (logbook-server or this process) and, with a final pass, to a pending
//! folder (pending.rs); the final segments to the brainy page as they come;
//! at the end the final transcript completed, the report written, the
//! meeting kept in the history (history.rs) and the folder removed. On any
//! failure the folder stays, with whatever was reached, for `finish`.
//! The CLI and the desktop app both run meetings through here, one
//! recording as a series of them (series.rs).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Local};
use logbook_engine::protocol::{ChannelConfig, ReportRequest, SpeakerVoice, Utterance};
use serde::Serialize;
use tracing::{info, warn};

use crate::brainy::{Brainy, Page};
use crate::finishing::Transient;
#[cfg(feature = "local")]
use crate::local;
use crate::pending::{self, Meeting, Pending, Saved, Spool};
use crate::remote::{self, Audio, Finalized, KeptChannel, Live, Pass, Server};
use crate::{history, shots, transcript, voices};
#[cfg(feature = "local")]
use logbook_engine::final_pass::Recorded;
use logbook_engine::protocol::SAMPLE_RATE;

/// "Up to the end" for `published_until`: JSON has no infinity.
const END: f64 = f64::MAX;

/// What the person writes during the meeting; read when it ends.
pub type Notes = Arc<Mutex<String>>;

/// Where meetings are transcribed: logbook-server, or this process.
#[derive(Clone)]
pub enum Backend {
    Server(Server),
    /// Only with the `local` feature: not in the Android app, which links
    /// no model code.
    #[cfg(feature = "local")]
    Local(Arc<local::Local>),
}

impl Backend {
    /// Here, the models loaded before any audio is taken (on Linux the
    /// process may start again to find the CUDA libraries), for
    /// `sessions` at once; a clean-up or a report refused without a model.
    /// On a server, a token it refuses.
    #[cfg_attr(not(feature = "local"), allow(unused_variables))]
    pub async fn prepare(&self, model: bool, sessions: usize) -> Result<()> {
        self.check(model)?;
        match self {
            Backend::Server(server) => remote::check_token(server).await,
            #[cfg(feature = "local")]
            Backend::Local(local) => local.load(sessions).await,
        }
    }

    /// A clean-up or a report refused here without a model, before any
    /// audio is taken; the models are not loaded.
    #[cfg_attr(not(feature = "local"), allow(unused_variables))]
    pub fn check(&self, model: bool) -> Result<()> {
        match self {
            Backend::Server(_) => Ok(()),
            #[cfg(feature = "local")]
            Backend::Local(local) => local.check_model(model),
        }
    }

    /// A live session; `dir` is the meeting's pending folder, with a
    /// final pass: on a server, a session opened again after a lost
    /// connection takes the final pass up from the audio kept there.
    pub async fn live(
        &self,
        channels: Vec<ChannelConfig>,
        final_pass: Option<bool>,
        audio: Audio,
        dir: Option<&Path>,
        events: &mut impl remote::Events,
    ) -> Live {
        match self {
            Backend::Server(server) => {
                let kept = dir.map(|dir| {
                    let dir = dir.to_path_buf();
                    Box::new(move |i| pending::reader(&dir, i)) as remote::Kept
                });
                remote::live(server, channels, final_pass, audio, kept, events).await
            }
            #[cfg(feature = "local")]
            Backend::Local(local) => {
                let dir = dir
                    .map(Path::to_path_buf)
                    .unwrap_or_else(std::env::temp_dir);
                local.live(channels, final_pass, audio, &dir, events).await
            }
        }
    }

    /// The meeting's report: by the server's model, or here by the one
    /// configured.
    pub async fn report(&self, request: ReportRequest) -> Result<String> {
        match self {
            Backend::Server(server) => remote::report(server, &request).await,
            #[cfg(feature = "local")]
            Backend::Local(local) => local.report(request).await,
        }
    }
}

/// What happens once the meeting ends.
#[derive(Clone, Debug, Default)]
pub struct Publish {
    /// Read the whole meeting again once it ends (better speakers and
    /// phrases): implied by everything below.
    pub final_pass: bool,
    /// Have the final transcript cleaned up by the language model.
    pub polish: bool,
    /// Have the language model write the report.
    pub report: bool,
    /// Keep the meeting in the history once published.
    pub keep: bool,
    /// Write the final transcript as a page in brainy.
    pub brainy: bool,
    pub brainy_url: Option<String>,
    pub brainy_token: Option<String>,
    /// The page the meeting goes under: its address, id or path of titles.
    pub parent: Option<String>,
    pub title: Option<String>,
    /// The person behind the microphone.
    pub me: String,
}

impl Publish {
    pub fn final_pass(&self) -> bool {
        self.final_pass || self.brainy || self.report || self.keep
    }

    /// What is kept with a pending meeting: everything but the token.
    fn saved(&self) -> Saved {
        Saved {
            brainy: self.brainy,
            brainy_url: self.brainy_url.clone(),
            parent: self.parent.clone(),
            title: self.title.clone(),
            me: self.me.clone(),
            report: self.report,
            keep: self.keep,
        }
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            url: self.brainy_url.clone(),
            token: self.brainy_token.clone(),
        }
    }
}

/// The brainy address and token given now; the address falls back to the
/// one saved with the meeting, the token is never saved.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    pub url: Option<String>,
    pub token: Option<String>,
}

impl Credentials {
    /// None without brainy; an error if it lacks its address or token.
    fn brainy(&self, saved: &Saved) -> Result<Option<Brainy>> {
        if !saved.brainy {
            return Ok(None);
        }
        self.any(saved.brainy_url.as_deref()).map(Some)
    }

    /// brainy at the address given now or at `saved`.
    pub fn any(&self, saved: Option<&str>) -> Result<Brainy> {
        let url = self
            .url
            .as_deref()
            .or(saved)
            .context("brainy needs its address (BRAINY_URL)")?;
        let token = self
            .token
            .as_deref()
            .context("brainy needs a token (BRAINY_TOKEN)")?;
        Brainy::new(url, token)
    }
}

/// A meeting brought to its end.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    /// The pending folder's name: the id in the history and the voices.
    pub id: String,
    pub page: Option<Page>,
    /// The final transcript as Markdown, when it went to no page.
    pub transcript: Option<String>,
    pub report: Option<String>,
    /// Why the report could not be written; the meeting is published anyway.
    pub report_error: Option<String>,
    /// Nothing was said in it, and no note written: nothing was published
    /// and nothing kept (a stretch of silence a long recording was cut
    /// into, `meetings`).
    pub silent: bool,
}

/// The live session's events, and the final segments to the publisher.
struct Tee<'a, E> {
    events: &'a mut E,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Progress>>,
}

enum Progress {
    Segments {
        until: f64,
        utterances: Vec<Utterance>,
        voices: Vec<SpeakerVoice>,
    },
    FinalId(String),
}

impl<E: remote::Events> remote::Events for Tee<'_, E> {
    fn utterance(&mut self, u: Utterance) {
        self.events.utterance(u);
    }

    fn final_segments(
        &mut self,
        until: f64,
        utterances: Vec<Utterance>,
        voices: Vec<SpeakerVoice>,
    ) {
        if let Some(tx) = &self.progress {
            let _ = tx.send(Progress::Segments {
                until: until.min(END),
                utterances,
                voices,
            });
        }
    }

    fn finalizing(&mut self, audio_s: f64, id: &str) {
        self.events.finalizing(audio_s, id);
        if let Some(tx) = &self.progress {
            let _ = tx.send(Progress::FinalId(id.to_string()));
        }
    }

    fn connected(&mut self, up: bool) {
        self.events.connected(up);
    }
}

fn started(pending: &Pending) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(&pending.meeting.started)
        .context("meeting.json: started is not a date")
}

fn title(pending: &Pending) -> Result<String> {
    let started = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
    Ok(pending
        .meeting
        .publish
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("Meeting {started}")))
}

/// Writes the segments that start between what the page already has and
/// `limit` to the meeting's brainy page, creating the page first (with
/// the first segments, or at the end: a meeting in which nothing is said
/// makes no page until then). What is written is recorded in the pending
/// folder, so nothing is written twice, whatever fails.
async fn write_page(
    b: &Brainy,
    pending: &mut Pending,
    limit: f64,
    segments: &[Utterance],
) -> Result<()> {
    let saved = pending.meeting.publish.clone();
    let new: Vec<Utterance> = segments
        .iter()
        .filter(|u| u.start >= pending.meeting.published_until && u.start < limit)
        .cloned()
        .collect();
    if pending.meeting.page.is_none() && new.is_empty() && limit != END {
        pending.meeting.published_until = pending.meeting.published_until.max(limit);
        return pending.save();
    }
    if pending.meeting.page.is_none() {
        let started = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
        let page = b
            .create_page(
                &title(pending)?,
                saved.parent.as_deref(),
                &format!(
                    "Recording started {started} · the transcript follows the meeting, \
                     a minute or so behind\n\n---\n"
                ),
            )
            .await?;
        info!("brainy page: {}", page.url);
        pending.meeting.page = Some(page);
        pending.save()?;
    }
    let page = pending.meeting.page.clone().expect("created above");
    let mut writer = pending
        .meeting
        .writer
        .clone()
        .unwrap_or_else(|| transcript::Writer::new(&saved.me));
    if !new.is_empty() {
        let known = voices::Library::load(pending.data())?;
        let body = writer.body(&new, &pending.meeting.voices, &known);
        b.append(&page.id, &body).await?;
    }
    pending.meeting.writer = Some(writer);
    pending.meeting.published_until = pending.meeting.published_until.max(limit);
    pending.save()
}

/// The meeting's final segments as they come: kept in the pending folder,
/// and written to brainy. A failed write is tried again with the next
/// segments: the ones not written yet are read again from
/// `segments.jsonl`, where they are, never held here (brainy may fail for
/// hours). Gives the folder back when the meeting is over.
async fn publisher(
    mut pending: Pending,
    brainy: Option<Brainy>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Progress>,
) -> Pending {
    // Where in `segments.jsonl` the segments not on the page start, while
    // a write has failed.
    let mut unwritten: Option<u64> = None;
    while let Some(progress) = rx.recv().await {
        let written = async {
            match progress {
                Progress::FinalId(id) => pending.set_final_id(Some(id)),
                Progress::Segments {
                    until,
                    utterances,
                    voices,
                } => {
                    let at = pending.add_segments(&utterances)?;
                    if !voices.is_empty() {
                        pending.meeting.voices = voices;
                        pending.save()?;
                    }
                    pending.set_final_until(until)?;
                    match &brainy {
                        Some(b) => {
                            // `write_page` takes those from `published_until` on.
                            let segments = match unwritten {
                                Some(from) => pending.segments_from(from)?,
                                None => utterances,
                            };
                            let written = write_page(b, &mut pending, until, &segments).await;
                            unwritten = match written {
                                Ok(()) => None,
                                Err(_) => Some(unwritten.unwrap_or(at)),
                            };
                            written
                        }
                        None => Ok(()),
                    }
                }
            }
        }
        .await;
        if let Err(error) = written {
            warn!("{error:#} (tried again with the next segments)");
        }
    }
    pending
}

fn duration_s(finalized: &Finalized) -> f64 {
    finalized
        .utterances
        .iter()
        .map(|u| u.end)
        .fold(0.0, f64::max)
}

/// The final transcript, complete: the rest written to the brainy page and
/// its header line rewritten, or, without brainy, returned as Markdown.
async fn publish(
    pending: &mut Pending,
    brainy: &Credentials,
    finalized: &Finalized,
) -> Result<Option<String>> {
    if !finalized.voices.is_empty() {
        pending.meeting.voices = finalized.voices.clone();
    }
    let saved = pending.meeting.publish.clone();
    let started_text = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
    let duration_s = duration_s(finalized);
    let Some(b) = brainy.brainy(&saved)? else {
        let known = voices::Library::load(pending.data())?;
        let (markdown, writer) = transcript::markdown(&transcript::Meeting {
            utterances: &finalized.utterances,
            warnings: &finalized.warnings,
            me: &saved.me,
            started: &started_text,
            duration_s,
            voices: &pending.meeting.voices,
            known: &known,
        });
        pending.meeting.writer = Some(writer);
        pending.save()?;
        return Ok(Some(markdown));
    };
    write_page(&b, pending, END, &finalized.utterances).await?;
    let writer = pending.meeting.writer.clone().unwrap_or_default();
    let mut header = writer
        .header(&started_text, duration_s, &[])
        .trim()
        .to_string();
    if !finalized.warnings.is_empty() {
        header.push_str(&format!(" · Note: {}", finalized.warnings.join("; ")));
    }
    let page = pending.meeting.page.clone().expect("written above");
    b.rewrite_first_block(&page.id, &header).await?;
    info!("written to brainy: {}", page.url);
    Ok(None)
}

/// Brings a pending meeting to its end: its final pass (complete over the
/// session, the one the server is running, or a new one over the audio
/// kept here), then the transcript published, the report written, the
/// meeting kept in the history and the folder removed. On any failure
/// before the report the folder stays, with whatever was reached, for
/// `finish`.
pub async fn finish(
    backend: &Backend,
    pending: &mut Pending,
    brainy: &Credentials,
) -> Result<Outcome> {
    let kept = |error: anyhow::Error, dir: &Path| {
        error.context(format!(
            "the meeting is kept in {}; finishing it tries again",
            dir.display()
        ))
    };
    let finalized = match pending.final_result()? {
        Some(f) => f,
        None => match final_pass(backend, pending).await {
            Ok(f) => {
                pending.save_final(&f)?;
                f
            }
            Err(error) => return Err(kept(error, &pending.dir)),
        },
    };
    if finalized.utterances.is_empty()
        && pending.meeting.page.is_none()
        && pending.notes().trim().is_empty()
    {
        info!("nothing was said: nothing to publish");
        let id = pending.id();
        pending.remove()?;
        return Ok(Outcome {
            id,
            silent: true,
            ..Outcome::default()
        });
    }
    let transcript = match publish(pending, brainy, &finalized).await {
        Ok(transcript) => transcript,
        Err(error) => {
            let started_text = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
            let (markdown, _) = transcript::markdown(&transcript::Meeting {
                utterances: &finalized.utterances,
                warnings: &finalized.warnings,
                me: &pending.meeting.publish.me,
                started: &started_text,
                duration_s: duration_s(&finalized),
                voices: &pending.meeting.voices,
                known: &voices::Library::load(pending.data()).unwrap_or_default(),
            });
            let file = pending.save_transcript(&markdown)?;
            return Err(kept(
                error.context(format!(
                    "not written to brainy; the transcript is {}",
                    file.display()
                )),
                &pending.dir,
            ));
        }
    };
    if let Err(error) = remember_voices(pending) {
        warn!("the meeting's voices were not kept to be named: {error:#}");
    }
    let mut kept_meeting = history::Kept {
        id: pending.id(),
        title: title(pending)?,
        started: pending.meeting.started.clone(),
        duration_s: duration_s(&finalized),
        page: pending.meeting.page.clone(),
        writer: pending.meeting.writer.clone().unwrap_or_default(),
        utterances: finalized.utterances.clone(),
        warnings: finalized.warnings.clone(),
        notes: pending.notes(),
        report: None,
        report_blocks: Vec::new(),
        report_error: None,
    };
    let saved = pending.meeting.publish.clone();
    let page_brainy = match (&kept_meeting.page, saved.brainy) {
        (Some(_), true) => brainy.brainy(&saved)?,
        _ => None,
    };
    if saved.report {
        info!("writing the report…");
        write_report(
            backend,
            page_brainy.as_ref(),
            &mut kept_meeting,
            &pending.dir,
        )
        .await;
    }
    if kept_meeting.report.is_none()
        && !kept_meeting.notes.trim().is_empty()
        && let Some(b) = &page_brainy
    {
        // No report, or it failed: the notes go on the page as they were
        // written, in the report's place (a report written later
        // replaces them, since it holds them).
        let page = kept_meeting.page.clone().expect("checked above");
        let notes = format!("## Notes\n\n{}", kept_meeting.notes.trim());
        match insert_under_header(b, &page, &notes, &pending.dir).await {
            Ok(blocks) => kept_meeting.report_blocks = blocks,
            Err(error) => warn!("the notes were not written to the page: {error:#}"),
        }
    }
    if saved.keep {
        history::save(pending.data(), &kept_meeting)?;
        let images = history::images(pending.data(), &kept_meeting.id)?;
        shots::keep(&pending.dir, &images, &kept_meeting.notes)?;
    }
    pending.remove()?;
    Ok(Outcome {
        id: kept_meeting.id,
        page: kept_meeting.page,
        transcript,
        report: kept_meeting.report,
        report_error: kept_meeting.report_error,
        silent: false,
    })
}

/// `markdown` right under the page's header, its screenshots from
/// `images`.
async fn insert_under_header(
    b: &Brainy,
    page: &Page,
    markdown: &str,
    images: &Path,
) -> Result<Vec<String>> {
    let header = b.first_block(&page.id).await?;
    b.insert_with_images(&header, markdown, images).await
}

/// Writes the meeting's report, and puts it on its page under the header
/// in place of the last one. A failure is kept in `report_error`: the
/// meeting is published without it, and the report can be asked again.
/// The screenshots of the notes are in `images`; one the report leaves
/// out is added at its end.
pub async fn write_report(
    backend: &Backend,
    brainy: Option<&Brainy>,
    kept: &mut history::Kept,
    images: &Path,
) {
    let request = ReportRequest {
        title: kept.title.clone(),
        notes: kept.notes.clone(),
        transcript: kept.transcript(),
    };
    let written = async {
        let markdown = shots::with_missing(&backend.report(request).await?, &kept.notes);
        if let (Some(b), Some(page)) = (brainy, &kept.page) {
            if !kept.report_blocks.is_empty() {
                b.delete_blocks(&page.id, &kept.report_blocks).await?;
                kept.report_blocks.clear();
            }
            kept.report_blocks = insert_under_header(b, page, &markdown, images).await?;
        }
        anyhow::Ok(markdown)
    }
    .await;
    match written {
        Ok(markdown) => {
            kept.report = Some(markdown);
            kept.report_error = None;
        }
        Err(error) => {
            warn!("the report was not written: {error:#}");
            kept.report_error = Some(format!("{error:#}"));
        }
    }
}

/// A speaker renamed in a kept meeting.
#[derive(Clone, Debug, Serialize)]
pub struct Renamed {
    /// Its voice saved under the name, to name it in the next meetings;
    /// None when it could not be (`voice_error`).
    pub enrolled: Option<voices::Enrolled>,
    pub voice_error: Option<String>,
    /// Blocks of the brainy page that changed.
    pub page_blocks: usize,
}

/// Renames the speaker labelled `from` in kept meeting `id`: in the
/// history, in its page (with `brainy`), and saves its voice under `to`
/// when the meeting's voices are still kept and it spoke long enough.
pub async fn rename_speaker(
    data: &Path,
    id: &str,
    from: &str,
    to: &str,
    brainy: Option<&Brainy>,
) -> Result<Renamed> {
    let to = to.trim();
    if to.is_empty() {
        bail!("the name is empty");
    }
    let mut kept = history::load(data, id)?;
    if kept.writer.speakers().iter().any(|s| s.2 == to) {
        bail!("another speaker of this meeting is already {to}");
    }
    if !kept.writer.rename(from, to) {
        bail!("no speaker of this meeting is {from}");
    }
    let (enrolled, voice_error) = match voices::name(data, id, from, to) {
        Ok(enrolled) => (Some(enrolled), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    voices::relabel(data, id, from, to)?;
    let mut page_blocks = 0;
    if let (Some(b), Some(page)) = (brainy, &kept.page) {
        page_blocks = b.rename_speaker(&page.id, from, to).await?;
    }
    history::save(data, &kept)?;
    Ok(Renamed {
        enrolled,
        voice_error,
        page_blocks,
    })
}

/// The meeting's speakers, by their labels on the page, kept to be named
/// (voices.rs).
fn remember_voices(pending: &Pending) -> Result<()> {
    let Some(writer) = &pending.meeting.writer else {
        return Ok(());
    };
    let speakers = writer
        .labels()
        .into_iter()
        .filter_map(|(channel, speaker, label)| {
            let voice = pending
                .meeting
                .voices
                .iter()
                .find(|v| v.channel == channel && v.speaker == speaker)?;
            Some(voices::HeardSpeaker {
                label,
                voice: voice.clone(),
            })
        })
        .collect();
    let page = pending.meeting.page.as_ref().map(|p| p.url.clone());
    let heard = voices::heard(&pending.meeting.started, page, speakers);
    voices::remember(pending.data(), &pending.id(), &heard)
}

/// The final pass of a meeting whose live session did not complete it:
/// the one the server is running, or a new one over the audio kept here
/// from where its final segments stopped (`final_until`), never from the
/// start again. Its result follows the segments kept.
async fn final_pass(backend: &Backend, pending: &mut Pending) -> Result<Finalized> {
    let until = pending.meeting.final_until;
    if until == END {
        // Every segment came; only `final.json` was not written.
        return Ok(Finalized {
            utterances: pending.segments()?,
            voices: pending.meeting.voices.clone(),
            ..Finalized::default()
        });
    }
    let from = (until * SAMPLE_RATE as f64) as u64;
    let offset = from as f64 / SAMPLE_RATE as f64;
    let rest = |pending: &Pending, i: usize| pending.samples(i).saturating_sub(from);
    #[cfg_attr(not(feature = "local"), allow(clippy::infallible_destructuring_match))]
    let server = match backend {
        Backend::Server(server) => server,
        #[cfg(feature = "local")]
        Backend::Local(local) => {
            pending.set_final_id(None)?;
            info!("reading the meeting again from {offset:.0} s…");
            let meeting = &pending.meeting;
            let channels = meeting
                .channels
                .iter()
                .enumerate()
                .map(|(i, config)| Recorded {
                    config: config.clone(),
                    reader: pending.reader(i),
                    start: offset,
                    seconds: rest(pending, i) as f64 / SAMPLE_RATE as f64,
                })
                .collect();
            let result = local
                .final_pass(channels, offset, meeting.voices.clone(), meeting.polish)
                .await?;
            return merged(pending, result)?.context("the final pass started after the audio kept");
        }
    };
    if let Some(id) = pending.meeting.final_id.clone() {
        info!("waiting for the final pass…");
        match remote::poll(server, &id).await? {
            Pass::Done(f) => {
                let from = f.from;
                match merged(pending, f)? {
                    Some(f) => return Ok(f),
                    // The server kept the pass's last minutes only, and
                    // this folder's segments stop before them: as unknown.
                    None => warn!(
                        "the server kept the final pass from {from:.0} s only; \
                         the segments here stop at {until:.0} s"
                    ),
                }
            }
            Pass::Unknown => warn!("the server no longer has the final pass"),
            Pass::Failed(message) => warn!("{message}"),
        }
        pending.set_final_id(None)?;
    }
    let meeting = &pending.meeting;
    let audio = (0..meeting.channels.len())
        .map(|i| KeptChannel {
            reader: pending.reader(i),
            from,
            to: from + rest(pending, i),
        })
        .collect();
    let id = remote::upload(
        server,
        meeting.channels.clone(),
        meeting.polish,
        offset,
        meeting.voices.clone(),
        audio,
    )
    .await?;
    pending.set_final_id(Some(id.clone()))?;
    info!("reading the meeting again from {offset:.0} s…");
    match remote::poll(server, &id).await? {
        Pass::Done(f) => merged(pending, f)?.context("the final pass started after the audio sent"),
        // It restarted: once it is back, the audio goes again.
        Pass::Unknown => {
            Err(Transient("the server lost the final pass it had just started".into()).into())
        }
        Pass::Failed(message) => {
            pending.set_final_id(None)?;
            bail!("{message}")
        }
    }
}

/// A final pass's result after the segments kept: those before
/// `final_until` from the folder, the rest from the result (which may
/// start earlier: a pass taken up during the meeting). None when the
/// result starts after them (`from` past `final_until`): what was said in
/// between is in neither, and the audio kept must be read again.
fn merged(pending: &Pending, result: Finalized) -> Result<Option<Finalized>> {
    let until = pending.meeting.final_until;
    if result.from > until {
        return Ok(None);
    }
    let mut utterances: Vec<Utterance> = pending
        .segments()?
        .into_iter()
        .filter(|u| u.start < until)
        .collect();
    utterances.extend(result.utterances.into_iter().filter(|u| u.start >= until));
    let voices = if result.voices.is_empty() {
        pending.meeting.voices.clone()
    } else {
        result.voices
    };
    Ok(Some(Finalized {
        utterances,
        warnings: result.warnings,
        voices,
        from: 0.0,
    }))
}

/// A meeting being recorded: the audio of `audio` goes to the backend,
/// and, with a final pass, to a pending folder too, where the final
/// segments are kept as they come (and written to brainy). `notes` are
/// read once the final pass is done. Returns once the meeting is finished; None
/// without a final pass. A recording that may last for hours or months
/// goes through `meetings`, which cuts it into meetings.
pub async fn meeting(
    backend: &Backend,
    data: &Path,
    channels: Vec<ChannelConfig>,
    p: &Publish,
    audio: Audio,
    notes: Notes,
    events: &mut impl remote::Events,
) -> Result<Option<Outcome>> {
    meeting_at(
        backend,
        data,
        channels,
        p,
        audio,
        notes,
        Local::now(),
        events,
    )
    .await
}

/// `meeting`, whose first sample was taken at `started`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn meeting_at(
    backend: &Backend,
    data: &Path,
    channels: Vec<ChannelConfig>,
    p: &Publish,
    mut audio: Audio,
    notes: Notes,
    started: DateTime<Local>,
    events: &mut impl remote::Events,
) -> Result<Option<Outcome>> {
    let credentials = p.credentials();
    let mut spool: Option<Spool> = None;
    // The pending folder, where a local session records its final pass.
    let mut recordings = None;
    let mut publishing = None;
    let mut tee = Tee {
        events,
        progress: None,
    };
    if p.final_pass() {
        // A missing token or a parent that names no page fails now, not
        // at the end of the meeting. The parent is kept as its id: a page
        // renamed before `finish` still takes the meeting.
        let mut publish = p.saved();
        let brainy = credentials.brainy(&publish)?;
        if let (Some(b), Some(parent)) = (&brainy, &publish.parent) {
            publish.parent = Some(b.resolve_page(parent).await.context("the parent page")?);
        }
        let meeting = Meeting {
            started: started.to_rfc3339(),
            channels: channels.clone(),
            polish: p.polish,
            publish,
            final_id: None,
            page: None,
            writer: None,
            published_until: 0.0,
            final_until: 0.0,
            voices: Vec::new(),
        };
        let (created, writer) =
            Pending::create(data, &started.format("%Y%m%d-%H%M%S").to_string(), meeting)?;
        info!("keeping the meeting in {}", created.dir.display());
        recordings = Some(created.dir.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tee.progress = Some(tx);
        publishing = Some(tokio::spawn(publisher(created, brainy, rx)));
        spool = Some(writer);
    }
    // Every frame is written here before it goes to the backend.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tee_audio = tokio::spawn(async move {
        let mut failed = None;
        while let Some((channel, samples)) = audio.recv().await {
            if let Some(s) = &mut spool
                && let Err(error) = s.write(channel as usize, &samples)
            {
                warn!("could not keep the meeting's audio: {error:#}");
                failed = Some(error);
                spool = None;
            }
            if tx.send((channel, samples)).is_err() {
                break;
            }
        }
        match (failed, spool) {
            (Some(error), _) => Err(error),
            (None, Some(s)) => s.finish(),
            (None, None) => Ok(()),
        }
    });
    let final_pass = p.final_pass().then_some(p.polish);
    let live = backend
        .live(channels, final_pass, rx, recordings.as_deref(), &mut tee)
        .await;
    drop(tee);
    let kept = tee_audio.await?;
    let Some(publishing) = publishing else {
        if !live.intact {
            bail!("the connection was lost: the live transcript has gaps");
        }
        return Ok(None);
    };
    let mut pending = publishing.await?;
    let written = notes.lock().unwrap().clone();
    if !written.trim().is_empty() {
        pending.save_notes(&written)?;
        if let Err(error) = shots::keep(&shots::staging(data), &pending.dir, &written) {
            warn!("the notes' screenshots were not kept: {error:#}");
        }
    }
    if let Some(warnings) = live.finalized {
        let utterances = pending.segments()?;
        let voices = pending.meeting.voices.clone();
        pending.save_final(&Finalized {
            utterances,
            warnings,
            voices,
            from: 0.0,
        })?;
    } else if let Err(error) = kept {
        warn!("the final pass reads only what was kept before: {error:#}");
    }
    finish(backend, &mut pending, &credentials).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64) -> Utterance {
        Utterance {
            channel: "mic".into(),
            speaker: None,
            start,
            end: start + 2.0,
            text: format!("{start}"),
            words: Vec::new(),
        }
    }

    fn pending(name: &str) -> Pending {
        let root = std::env::temp_dir().join(format!("logbook-{name}-{}", std::process::id()));
        let meeting: Meeting = serde_json::from_value(serde_json::json!({
            "started": "2026-10-29T09:00:00+01:00",
            "channels": [{"name": "mic", "diarize": false}],
            "publish": {"brainy": false, "brainy_url": null, "parent": null, "title": null, "me": "Me"},
        }))
        .unwrap();
        Pending::create(&root, "m", meeting).unwrap().0
    }

    #[test]
    fn a_result_that_starts_after_the_segments_kept_is_a_gap() {
        let mut p = pending("gap");
        p.add_segments(&[seg(10.0), seg(50.0)]).unwrap();
        p.set_final_until(100.0).unwrap();
        // The server kept the pass from 80 s: it covers 100 s on.
        let covered = Finalized {
            utterances: vec![seg(85.0), seg(120.0), seg(700.0)],
            from: 80.0,
            ..Finalized::default()
        };
        let whole = merged(&p, covered).unwrap().unwrap();
        let starts: Vec<f64> = whole.utterances.iter().map(|u| u.start).collect();
        assert_eq!(starts, [10.0, 50.0, 120.0, 700.0]);
        assert_eq!(whole.from, 0.0);
        // From 400 s only: 100–400 s is in neither; the audio kept from
        // 100 s is read again instead.
        let gap = Finalized {
            utterances: vec![seg(420.0)],
            from: 400.0,
            ..Finalized::default()
        };
        assert!(merged(&p, gap).unwrap().is_none());
        // A server older than `from` sent everything (0).
        let old: Finalized =
            serde_json::from_str(r#"{"utterances":[],"warnings":[],"voices":[]}"#).unwrap();
        assert!(merged(&p, old).unwrap().is_some());
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn segments_not_on_the_page_are_read_again_from_the_folder() {
        let p = pending("unwritten");
        p.add_segments(&[seg(1.0), seg(2.0)]).unwrap();
        let at = p.add_segments(&[seg(3.0)]).unwrap();
        p.add_segments(&[seg(4.0), seg(5.0)]).unwrap();
        let again: Vec<f64> = p
            .segments_from(at)
            .unwrap()
            .iter()
            .map(|u| u.start)
            .collect();
        assert_eq!(again, [3.0, 4.0, 5.0]);
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }
}
