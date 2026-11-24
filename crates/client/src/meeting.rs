//! A meeting from its first sample to its page: the audio to the backend
//! (logbook-server or this process) and, with a final pass, to a pending
//! folder (pending.rs), with the final segments as they come; at the end
//! the final transcript completed, the report written and put on a brainy
//! page (never the transcript), the meeting kept in the history
//! (history.rs) and the folder removed. On any
//! failure the folder stays, with whatever was reached, for `finish`.
//! The CLI and the desktop app both run meetings through here, one
//! recording as a series of them (series.rs).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Local};
use logbook_engine::protocol::{
    ChannelConfig, PassPhase, PassProgress, ReportRequest, SpeakerVoice, Utterance,
};
use serde::Serialize;
use tracing::{info, warn};

use crate::brainy::{Brainy, Page};
use crate::finishing::{self, Transient};
#[cfg(feature = "local")]
use crate::local;
use crate::pending::{self, Meeting, Pending, Saved, Spool};
use crate::remote::{self, Audio, Finalized, KeptChannel, Live, Pass, Server};
use crate::{history, shots, transcript, voices};
#[cfg(feature = "local")]
use logbook_engine::final_pass::Recorded;
use logbook_engine::protocol::SAMPLE_RATE;

/// "Up to the end" for `final_until`: JSON has no infinity.
const END: f64 = f64::MAX;

/// What the person writes and changes during the meeting; read when it
/// ends.
pub type Notes = Arc<Mutex<Written>>;

#[derive(Clone, Debug, Default)]
pub struct Written {
    pub notes: String,
    /// A title given during the meeting, in place of the one it started
    /// with.
    pub title: Option<String>,
}

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
    /// `sessions` at once; a report refused without a model.
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

    /// A report refused here without a model, before any
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
        final_pass: bool,
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
    /// Have the language model write the report.
    pub report: bool,
    /// Keep the meeting in the history once published.
    pub keep: bool,
    /// Write the meeting as a page in brainy: its report (or its notes
    /// without one), never the transcript.
    pub brainy: bool,
    pub brainy_url: Option<String>,
    pub brainy_token: Option<String>,
    /// The page the meeting goes under: its address, id or path of titles.
    pub parent: Option<String>,
    pub title: Option<String>,
    /// The person behind the microphone.
    pub me: String,
    /// The people expected, among the voices known by name: the
    /// meeting's speakers are named from them only (voices.rs). None: from
    /// every voice known.
    pub expected: Vec<String>,
}

impl Publish {
    pub fn final_pass(&self) -> bool {
        self.final_pass || self.brainy || self.report || self.keep
    }

    /// Whether a language model is needed at the end: the report.
    pub fn model(&self) -> bool {
        self.report
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
            expected: self.expected.clone(),
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
    /// The final transcript as Markdown: it goes to no page.
    pub transcript: Option<String>,
    pub report: Option<String>,
    /// Why the report could not be written; the meeting is published anyway.
    pub report_error: Option<String>,
    /// Nothing was said in it, and no note written: nothing was published
    /// and nothing kept (a stretch of silence a long recording was cut
    /// into, `meetings`).
    pub silent: bool,
}

/// Where a meeting being finished is, for whoever shows it (the app's
/// pending meetings, `logbook finish`). `done` is from 0 to 1; `audio_s`
/// is the audio the step goes through.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "stage", rename_all = "camelCase")]
pub enum Step {
    /// The audio kept here going to the server.
    #[serde(rename_all = "camelCase")]
    Sending { done: f64, audio_s: f64 },
    /// On the server, behind other meetings' passes.
    Queued,
    /// The pass reading every channel once to find its pauses.
    #[serde(rename_all = "camelCase")]
    Reading { done: f64, audio_s: f64 },
    /// The pass, range by range; `done` None from a server older than
    /// the progress.
    #[serde(rename_all = "camelCase")]
    Transcribing { done: Option<f64>, audio_s: f64 },
    /// The page made in brainy.
    Publishing,
    /// The report written.
    Report,
}

impl Step {
    /// The pass of a whole meeting as the server or this process says it.
    pub(crate) fn of_pass(audio_s: f64, progress: Option<PassProgress>) -> Self {
        match progress {
            None => Step::Transcribing {
                done: None,
                audio_s,
            },
            Some(p) => match p.phase {
                PassPhase::Queued => Step::Queued,
                PassPhase::Reading => Step::Reading {
                    done: p.done,
                    audio_s,
                },
                PassPhase::Transcribing => Step::Transcribing {
                    done: Some(p.done),
                    audio_s,
                },
            },
        }
    }
}

/// Who is told the steps: nobody, or a function called at each (a few
/// times a second at most).
#[derive(Clone, Default)]
pub struct Steps(Option<Arc<dyn Fn(Step) + Send + Sync>>);

impl Steps {
    pub fn new(f: impl Fn(Step) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    pub(crate) fn tell(&self, step: Step) {
        if let Some(f) = &self.0 {
            f(step);
        }
    }
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

/// The meeting's first sample: where it was cut from the recording.
fn started(pending: &Pending) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(&pending.meeting.started)
        .context("meeting.json: started is not a date")
}

/// Where the meeting's displayed clock starts, in seconds of the meeting:
/// the writer's offset, fixed when the page was created; before any
/// writer, where it will be for `utterances` (`first_word`). A page made
/// before the offset was kept has 0.
fn offset(pending: &Pending, utterances: &[Utterance]) -> f64 {
    match (&pending.meeting.writer, &pending.meeting.page) {
        (Some(writer), _) => writer.offset(),
        (None, Some(_)) => 0.0,
        (None, None) => transcript::first_word(utterances),
    }
}

/// The displayed start: the first sample, `offset` seconds on.
fn shown(pending: &Pending, offset: f64) -> Result<DateTime<FixedOffset>> {
    Ok(started(pending)? + chrono::Duration::milliseconds((offset * 1000.0) as i64))
}

fn minute(at: &DateTime<FixedOffset>) -> String {
    at.format("%Y-%m-%d %H:%M").to_string()
}

/// The title asked for, or "Meeting <displayed start>".
fn title(pending: &Pending, shown: &DateTime<FixedOffset>) -> String {
    pending
        .meeting
        .publish
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("Meeting {}", minute(shown)))
}

/// Where `new_page` creates the page: brainy, or a test's.
pub(crate) trait Pages {
    async fn create_page(&self, title: &str, parent: Option<&str>, markdown: &str) -> Result<Page>;
}

impl Pages for Brainy {
    async fn create_page(&self, title: &str, parent: Option<&str>, markdown: &str) -> Result<Page> {
        Brainy::create_page(self, title, parent, markdown).await
    }
}

/// The page of `kept` under `parent` (an id; the top without one), with
/// its header only: the report, or the notes without one, go under it
/// (`put_report`). The transcript never goes to brainy: it stays in the
/// history. A parent brainy no longer has does not hold the meeting: the
/// page goes among the person's own pages.
async fn new_page(b: &impl Pages, kept: &history::Kept, parent: Option<&str>) -> Result<Page> {
    let started = DateTime::parse_from_rfc3339(&kept.started)
        .with_context(|| format!("the meeting's start {} is not a time", kept.started))?;
    let mut header = kept
        .writer
        .header(&minute(&started), kept.duration_s, &[])
        .trim()
        .to_string();
    if !kept.warnings.is_empty() {
        header.push_str(&format!(" · Note: {}", kept.warnings.join("; ")));
    }
    let page = match b.create_page(&kept.title, parent, &header).await {
        Err(error) if parent.is_some() && page_gone(&error) => {
            warn!("{error:#}: the meeting's page is made at the top instead");
            b.create_page(&kept.title, None, &header).await?
        }
        created => created?,
    };
    info!("written to brainy: {}", page.url);
    Ok(page)
}

/// The voices a pending meeting's speakers are named from: its expected
/// people's, or every one known.
fn known(pending: &Pending) -> Result<voices::Library> {
    Ok(voices::Library::load(pending.data())?.only(&pending.meeting.publish.expected))
}

/// brainy answered that the page is not there: deleted, or in its trash.
fn page_gone(error: &anyhow::Error) -> bool {
    error.chain().any(|c| {
        c.downcast_ref::<finishing::HttpStatus>()
            .is_some_and(|e| e.status == 404)
    })
}

/// The meeting's final segments as they come, kept in the pending folder.
/// Gives the folder back when the meeting is over.
async fn publisher(
    mut pending: Pending,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Progress>,
) -> Pending {
    while let Some(progress) = rx.recv().await {
        let kept = (|| match progress {
            Progress::FinalId(id) => pending.set_final_id(Some(id)),
            Progress::Segments {
                until,
                utterances,
                voices,
            } => {
                pending.add_segments(&utterances)?;
                if !voices.is_empty() {
                    pending.meeting.voices = voices;
                    pending.save()?;
                }
                pending.set_final_until(until)
            }
        })();
        if let Err(error) = kept {
            warn!("{error:#}");
        }
    }
    pending
}

/// From the first word (`offset`) to the last.
fn duration_s(finalized: &Finalized, offset: f64) -> f64 {
    let end = finalized
        .utterances
        .iter()
        .map(|u| u.end)
        .fold(0.0, f64::max);
    (end - offset).max(0.0)
}

/// The final transcript as Markdown; its writer (the speakers' names, the
/// displayed clock) is kept with the meeting from here on.
fn transcript_of(pending: &mut Pending, finalized: &Finalized) -> Result<String> {
    if !finalized.voices.is_empty() {
        pending.meeting.voices = finalized.voices.clone();
    }
    let known = known(pending)?;
    let offset = offset(pending, &finalized.utterances);
    let (markdown, writer) = transcript::markdown(&transcript::Meeting {
        utterances: &finalized.utterances,
        warnings: &finalized.warnings,
        me: &pending.meeting.publish.me,
        started: &minute(&shown(pending, offset)?),
        duration_s: duration_s(finalized, offset),
        offset,
        voices: &pending.meeting.voices,
        known: &known,
    });
    pending.meeting.writer = Some(writer);
    pending.save()?;
    Ok(markdown)
}

/// Brings a pending meeting to its end: its final pass (complete over the
/// session, the one the server is running, or a new one over the audio
/// kept here), then the report written, the page made in brainy (its
/// header and the report, never the transcript), the meeting kept in the
/// history and the folder removed. On any failure before the page the
/// folder stays, with whatever was reached, for `finish`.
pub async fn finish(
    backend: &Backend,
    pending: &mut Pending,
    brainy: &Credentials,
    steps: &Steps,
) -> Result<Outcome> {
    let kept = |error: anyhow::Error, dir: &Path| {
        error.context(format!(
            "the meeting is kept in {}; finishing it tries again",
            dir.display()
        ))
    };
    let finalized = match pending.final_result()? {
        Some(f) => f,
        None => match final_pass(backend, pending, steps).await {
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
    let transcript = transcript_of(pending, &finalized)?;
    // The writer is there by now: its offset is the meeting's displayed
    // start from here on.
    let offset = offset(pending, &finalized.utterances);
    let shown = shown(pending, offset)?;
    let mut kept_meeting = history::Kept {
        id: pending.id(),
        title: title(pending, &shown),
        started: shown.to_rfc3339(),
        duration_s: duration_s(&finalized, offset),
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
    // Not asked again of a meeting finished again (`report.md`).
    let kept_report = pending.report();
    if saved.report {
        info!("writing the report…");
        steps.tell(Step::Report);
    }
    let report = match (saved.report, kept_report) {
        (false, _) => None,
        (true, Some(markdown)) => Some(Ok(markdown)),
        (true, None) => Some(report_markdown(backend, &kept_meeting).await),
    };
    if let Some(Ok(markdown)) = &report
        && let Err(error) = pending.save_report(markdown)
    {
        warn!("the report was not kept: {error:#}");
    }
    let page_brainy = brainy.brainy(&saved)?;
    if let Some(b) = &page_brainy
        && pending.meeting.page.is_none()
    {
        steps.tell(Step::Publishing);
        match new_page(b, &kept_meeting, saved.parent.as_deref()).await {
            // Kept at once: a meeting finished again does not make a
            // second page.
            Ok(page) => {
                pending.meeting.page = Some(page);
                pending.save()?;
            }
            Err(error) => {
                let file = pending.save_transcript(&transcript)?;
                return Err(kept(
                    error.context(format!(
                        "not written to brainy; the transcript is {}",
                        file.display()
                    )),
                    &pending.dir,
                ));
            }
        }
    }
    kept_meeting.page = pending.meeting.page.clone();
    if let Err(error) = remember_voices(pending, &shown) {
        warn!("the meeting's voices were not kept to be named: {error:#}");
    }
    let labels: Vec<String> = kept_meeting
        .writer
        .speakers()
        .into_iter()
        .map(|s| s.2)
        .collect();
    if let Err(error) = voices::heard_in(pending.data(), &pending.id(), &labels) {
        warn!("when the people were last heard was not kept: {error:#}");
    }
    let page_brainy = page_brainy.filter(|_| kept_meeting.page.is_some());
    match report {
        Some(Ok(markdown)) => {
            put_report(
                page_brainy.as_ref(),
                &mut kept_meeting,
                markdown,
                &pending.dir,
            )
            .await
        }
        Some(Err(error)) => {
            warn!("the report was not written: {error:#}");
            kept_meeting.report_error = Some(format!("{error:#}"));
        }
        None => {}
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
        transcript: Some(transcript),
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
    match report_markdown(backend, kept).await {
        Ok(markdown) => put_report(brainy, kept, markdown, images).await,
        Err(error) => {
            warn!("the report was not written: {error:#}");
            kept.report_error = Some(format!("{error:#}"));
        }
    }
}

/// The report the model writes of `kept`, with the screenshots of its
/// notes it left out.
async fn report_markdown(backend: &Backend, kept: &history::Kept) -> Result<String> {
    let request = ReportRequest {
        title: kept.title.clone(),
        notes: kept.notes.clone(),
        transcript: kept.transcript(),
    };
    Ok(shots::with_missing(
        &backend.report(request).await?,
        &kept.notes,
    ))
}

/// The report on the page, in place of the last one, and in `kept`.
async fn put_report(
    brainy: Option<&Brainy>,
    kept: &mut history::Kept,
    markdown: String,
    images: &Path,
) {
    let written = async {
        if let (Some(b), Some(page)) = (brainy, &kept.page) {
            if !kept.report_blocks.is_empty() {
                b.delete_blocks(&page.id, &kept.report_blocks).await?;
                kept.report_blocks.clear();
            }
            kept.report_blocks = insert_under_header(b, page, &markdown, images).await?;
        }
        anyhow::Ok(())
    }
    .await;
    match written {
        Ok(()) => {
            kept.report = Some(markdown);
            kept.report_error = None;
        }
        Err(error) => {
            warn!("the report was not written: {error:#}");
            kept.report_error = Some(format!("{error:#}"));
        }
    }
}

/// Writes kept meeting `id`, published without brainy, to a new page
/// under `parent` (a reference `resolve_page` reads; the top without
/// one): its header and under it its report, or its notes without one,
/// as `finish` puts them; the transcript stays in the history. The page is kept with the meeting once made: a report that fails
/// to go on it leaves `report_error`, and "write the report" puts it
/// there.
pub async fn send_to_brainy(
    data: &Path,
    id: &str,
    b: &Brainy,
    parent: Option<&str>,
) -> Result<history::Kept> {
    let mut kept = history::load(data, id)?;
    if kept.page.is_some() {
        bail!("this meeting has a brainy page already");
    }
    let parent = match parent {
        Some(parent) => Some(b.resolve_page(parent).await.context("the parent page")?),
        None => None,
    };
    let page = new_page(b, &kept, parent.as_deref()).await?;
    kept.page = Some(page.clone());
    history::save(data, &kept)?;
    let images = history::images(data, id)?;
    match kept.report.clone() {
        Some(report) => put_report(Some(b), &mut kept, report, &images).await,
        None if !kept.notes.trim().is_empty() => {
            let notes = format!("## Notes\n\n{}", kept.notes.trim());
            match insert_under_header(b, &page, &notes, &images).await {
                Ok(blocks) => kept.report_blocks = blocks,
                Err(error) => warn!("the notes were not written to the page: {error:#}"),
            }
        }
        None => {}
    }
    history::save(data, &kept)?;
    Ok(kept)
}

/// A speaker renamed in a kept meeting.
#[derive(Clone, Debug, Serialize)]
pub struct Renamed {
    /// Its voice saved under the name, to name it in the next meetings;
    /// None when it could not be (`voice_error`), or may not be theirs
    /// (`voice_doubt`: saved only if the person says so, `save_voice`).
    pub enrolled: Option<voices::Enrolled>,
    pub voice_error: Option<String>,
    pub voice_doubt: Option<String>,
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
    let (mut enrolled, mut voice_error, mut voice_doubt) = (None, None, None);
    match voices::name(data, id, from, to, false) {
        Ok(e) => enrolled = Some(e),
        Err(error) => match error.downcast_ref::<voices::Doubted>() {
            Some(doubt) => voice_doubt = Some(doubt.to_string()),
            None => voice_error = Some(format!("{error:#}")),
        },
    }
    voices::relabel(data, id, from, to)?;
    let mut page_blocks = 0;
    if let (Some(b), Some(page)) = (brainy, &kept.page) {
        page_blocks = b.rename_speaker(&page.id, from, to).await?;
    }
    history::save(data, &kept)?;
    Ok(Renamed {
        enrolled,
        voice_error,
        voice_doubt,
        page_blocks,
    })
}

/// Gives kept meeting `id` a new title: there and on its page. An empty
/// one is the title a meeting given none gets, "Meeting <displayed start>".
/// The page is renamed first: a title brainy refused is not kept here. A
/// page no longer there (deleted, or in brainy's trash, which takes no
/// changes) does not hold the meeting's title: it is renamed here alone,
/// its link kept, since a page in the trash can come back.
pub async fn rename_meeting(
    data: &Path,
    id: &str,
    title: &str,
    brainy: Option<&Brainy>,
) -> Result<history::Kept> {
    let mut kept = history::load(data, id)?;
    let title = match title.trim() {
        "" => {
            let started = DateTime::parse_from_rfc3339(&kept.started)
                .with_context(|| format!("the meeting's start {} is not a time", kept.started))?;
            format!("Meeting {}", minute(&started))
        }
        t => t.to_string(),
    };
    if title == kept.title {
        return Ok(kept);
    }
    if let (Some(b), Some(page)) = (brainy, &kept.page) {
        match b.rename_page(&page.id, &title).await {
            Err(error) if page_gone(&error) => {
                warn!("{error:#}: the meeting is renamed, its page is not")
            }
            renamed => renamed?,
        }
    }
    kept.title = title;
    history::save(data, &kept)?;
    Ok(kept)
}

/// The meeting's speakers, by their labels on the page, kept to be named
/// (voices.rs).
fn remember_voices(pending: &Pending, shown: &DateTime<FixedOffset>) -> Result<()> {
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
    let heard = voices::heard(&shown.to_rfc3339(), page, speakers);
    voices::remember(pending.data(), &pending.id(), &heard)
}

/// The final pass of a meeting whose live session did not complete it:
/// the one the server is running, or a new one over the audio kept here
/// from where its final segments stopped (`final_until`), never from the
/// start again. Its result follows the segments kept.
async fn final_pass(backend: &Backend, pending: &mut Pending, steps: &Steps) -> Result<Finalized> {
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
                .final_pass(channels, offset, meeting.voices.clone(), steps)
                .await?;
            return merged(pending, result)?.context("the final pass started after the audio kept");
        }
    };
    if let Some(id) = pending.meeting.final_id.clone() {
        info!("waiting for the final pass…");
        match remote::poll(server, &id, steps).await? {
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
        offset,
        meeting.voices.clone(),
        audio,
        steps,
    )
    .await?;
    pending.set_final_id(Some(id.clone()))?;
    info!("reading the meeting again from {offset:.0} s…");
    match remote::poll(server, &id, steps).await? {
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
/// segments are kept as they come. `notes` are
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
        // As the library spells them. A name it no longer knows (forgotten
        // during a recording of many meetings) is left out, not a meeting
        // that fails to start: the CLI and the app check them first.
        let known = voices::Library::load(data)?;
        publish
            .expected
            .retain(|n| match known.expected(std::slice::from_ref(n)) {
                Ok(_) => true,
                Err(error) => {
                    warn!("{error:#}: not expected");
                    false
                }
            });
        publish.expected = known.expected(&publish.expected)?;
        let brainy = credentials.brainy(&publish)?;
        if let (Some(b), Some(parent)) = (&brainy, &publish.parent) {
            publish.parent = Some(b.resolve_page(parent).await.context("the parent page")?);
        }
        let meeting = Meeting {
            started: started.to_rfc3339(),
            channels: channels.clone(),
            publish,
            final_id: None,
            page: None,
            writer: None,
            final_until: 0.0,
            voices: Vec::new(),
        };
        let (created, writer) =
            Pending::create(data, &started.format("%Y%m%d-%H%M%S").to_string(), meeting)?;
        info!("keeping the meeting in {}", created.dir.display());
        recordings = Some(created.dir.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tee.progress = Some(tx);
        publishing = Some(tokio::spawn(publisher(created, rx)));
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
    let final_pass = p.final_pass();
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
    if !written.notes.trim().is_empty() {
        pending.save_notes(&written.notes)?;
        if let Err(error) = shots::keep(&shots::staging(data), &pending.dir, &written.notes) {
            warn!("the notes' screenshots were not kept: {error:#}");
        }
    }
    if let Some(title) = written.title {
        pending.meeting.publish.title = Some(title).filter(|t| !t.trim().is_empty());
        pending.save()?;
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
    finish(backend, &mut pending, &credentials, &events.steps())
        .await
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_reads_as_the_pages_expect() {
        // apps/logbook/src/lib/api.ts, `Step`.
        let json = |step| serde_json::to_string(&step).unwrap();
        assert_eq!(
            json(Step::Sending {
                done: 0.25,
                audio_s: 60.0
            }),
            r#"{"stage":"sending","done":0.25,"audioS":60.0}"#
        );
        assert_eq!(json(Step::Queued), r#"{"stage":"queued"}"#);
        assert_eq!(
            json(Step::of_pass(60.0, None)),
            r#"{"stage":"transcribing","done":null,"audioS":60.0}"#
        );
        assert_eq!(json(Step::Publishing), r#"{"stage":"publishing"}"#);
    }

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

    fn said(channel: &str, speaker: Option<u32>, start: f64) -> Utterance {
        Utterance {
            channel: channel.into(),
            speaker,
            ..seg(start)
        }
    }

    /// brainy in memory: the pages created, under which parent and with
    /// what; a parent "gone" was deleted.
    #[derive(Default)]
    struct Fake {
        created: Mutex<Vec<(String, Option<String>, String)>>,
    }

    impl Pages for Fake {
        async fn create_page(
            &self,
            title: &str,
            parent: Option<&str>,
            markdown: &str,
        ) -> Result<Page> {
            if parent == Some("gone") {
                return Err(finishing::HttpStatus {
                    status: 404,
                    message: "brainy answered 404: no page gone".into(),
                }
                .into());
            }
            self.created.lock().unwrap().push((
                title.into(),
                parent.map(Into::into),
                markdown.into(),
            ));
            Ok(Page {
                id: "p".into(),
                url: "https://b/p".into(),
            })
        }
    }

    /// The meeting as `finish` keeps it, without its report.
    fn kept_of(p: &Pending, finalized: &Finalized) -> history::Kept {
        let offset = offset(p, &finalized.utterances);
        let shown = shown(p, offset).unwrap();
        history::Kept {
            id: p.id(),
            title: title(p, &shown),
            started: shown.to_rfc3339(),
            duration_s: duration_s(finalized, offset),
            page: None,
            writer: p.meeting.writer.clone().unwrap(),
            utterances: finalized.utterances.clone(),
            warnings: finalized.warnings.clone(),
            notes: String::new(),
            report: None,
            report_blocks: Vec::new(),
            report_error: None,
        }
    }

    #[tokio::test]
    async fn the_page_has_the_header_and_never_the_transcript() {
        // Cut at 09:00; the first word at 07:12.4.
        let mut p = pending("first-word");
        let dir = p.dir.clone();
        let finalized = Finalized {
            utterances: vec![
                said("system", Some(0), 432.4),
                said("mic", None, 440.0),
                said("system", Some(1), 4100.0),
            ],
            warnings: vec!["the call was silent for a while".into()],
            ..Finalized::default()
        };
        let transcript = transcript_of(&mut p, &finalized).unwrap();
        assert!(
            transcript.contains("**Speaker 1** · 00:00\n\n432.4\n\n**Me** · 00:08"),
            "{transcript}"
        );
        // The clock is kept in meeting.json with the writer.
        let saved = Pending::open(&dir).unwrap();
        assert_eq!(saved.meeting.writer.as_ref().unwrap().offset(), 432.0);
        assert_eq!(saved.meeting.shown_started(), "2026-10-29T09:07:12+01:00");
        let kept = kept_of(&p, &finalized);
        // From the first word (432 s) to the last's end (4102 s).
        assert_eq!(kept.duration_s, 3670.0);
        let fake = Fake::default();
        new_page(&fake, &kept, Some("parent")).await.unwrap();
        let (title, parent, markdown) = fake.created.lock().unwrap()[0].clone();
        assert_eq!(title, "Meeting 2026-10-29 09:07");
        assert_eq!(parent.as_deref(), Some("parent"));
        assert_eq!(
            markdown,
            "Recorded 2026-10-29 09:07 · 1 h 1 min · Speaker 1, Me, Speaker 2 · \
             Note: the call was silent for a while"
        );
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[tokio::test]
    async fn a_deleted_parent_puts_the_page_at_the_top() {
        let mut p = pending("deleted-parent");
        let finalized = Finalized {
            utterances: vec![said("mic", Some(0), 10.0)],
            ..Finalized::default()
        };
        transcript_of(&mut p, &finalized).unwrap();
        let fake = Fake::default();
        new_page(&fake, &kept_of(&p, &finalized), Some("gone"))
            .await
            .unwrap();
        assert_eq!(fake.created.lock().unwrap()[0].1, None);
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_meeting_of_notes_only_starts_at_its_first_sample() {
        let mut p = pending("notes-only");
        transcript_of(&mut p, &Finalized::default()).unwrap();
        let kept = kept_of(&p, &Finalized::default());
        assert_eq!(kept.title, "Meeting 2026-10-29 09:00");
        assert_eq!(offset(&p, &[]), 0.0);
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_meeting_saved_before_the_offset_keeps_its_clock() {
        // meeting.json of a page written before the offset was kept.
        let mut p = pending("old-offset");
        let old: Meeting = serde_json::from_value(serde_json::json!({
            "started": "2026-10-29T09:00:00+01:00",
            "channels": [{"name": "mic", "diarize": false}],
            "publish": {"brainy": true, "brainy_url": null, "parent": null, "title": null, "me": "Me"},
            "page": {"id": "p", "url": "https://b/p"},
            "writer": {"me": "Me", "names": [], "known": [], "previous": null},
            "published_until": 600.0,
        }))
        .unwrap();
        p.meeting = old;
        assert_eq!(offset(&p, &[seg(432.0)]), 0.0);
        assert_eq!(p.meeting.shown_started(), "2026-10-29T09:00:00+01:00");
        // A page with no writer kept: 0 as well, as its turns were written.
        p.meeting.writer = None;
        assert_eq!(offset(&p, &[seg(432.0)]), 0.0);
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }

    #[tokio::test]
    async fn a_kept_meeting_is_renamed_and_an_empty_title_is_its_start() {
        let data = std::env::temp_dir().join(format!("logbook-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let kept = history::Kept {
            id: "20261027-091500".into(),
            title: "Weekly".into(),
            started: "2026-10-27T09:15:00+01:00".into(),
            duration_s: 5.0,
            page: None,
            writer: transcript::Writer::new("Me", 0.0),
            utterances: Vec::new(),
            warnings: Vec::new(),
            notes: String::new(),
            report: None,
            report_blocks: Vec::new(),
            report_error: None,
        };
        history::save(&data, &kept).unwrap();
        let renamed = rename_meeting(&data, &kept.id, "  Budget review ", None)
            .await
            .unwrap();
        assert_eq!(renamed.title, "Budget review");
        assert_eq!(
            history::load(&data, &kept.id).unwrap().title,
            "Budget review"
        );
        let renamed = rename_meeting(&data, &kept.id, " ", None).await.unwrap();
        assert_eq!(renamed.title, "Meeting 2026-10-27 09:15");
        let _ = std::fs::remove_dir_all(&data);
    }
}
