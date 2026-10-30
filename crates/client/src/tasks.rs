//! The meeting's tasks as rows of a brainy collection, written with the
//! person's own token at the meeting's end (meeting.rs, `finish`): what
//! the language model found (engine tasks.rs) is a proposal, so every row
//! is "To confirm" and links to the moment of the meeting's page where it
//! was said; the person promotes it or deletes it in brainy
//! (docs/decisions/2026-10-29-tasks-as-collection-rows.md).
//!
//! Columns: the title (the task), Owner (text: the speaker's label),
//! Due (date), Status, Meeting (the page's address) and Moment (the
//! address of the turn, `#block-<id>`). A collection that lacks one gets
//! it (`extend_collection` only adds); one that has it with another type
//! is not written into (the row's text still says where it comes from).
//!
//! Exactly once: the tasks found are kept in the pending meeting
//! (`TaskLog`) before any row is written, and each row is recorded there
//! once brainy has it. `writing` is saved before a row is asked for: if
//! the process or the network fails before the answer, the next `finish`
//! looks the row up (by title and meeting) before writing it again. Only
//! the process holding the meeting's `.lock` writes.
//!
//! A series of meetings ("since last time: 3 closed, 2 open") would read
//! the collection's rows by their Meeting column for the series' earlier
//! pages; nothing here knows of series yet.

use anyhow::{Result, ensure};
use logbook_engine::protocol::Task;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::brainy::{Brainy, Collection, Page};
use crate::pending::Pending;

/// The status a row is written with: a proposal until a person says.
pub const TO_CONFIRM: &str = "To confirm";

/// The tasks found for a meeting and how far they are written.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskLog {
    /// What the model found, in order; written in that order.
    pub found: Vec<Task>,
    /// The rows written: `found[..rows.len()]` are in the collection.
    #[serde(default)]
    pub rows: Vec<Row>,
    /// A row was asked of brainy when this was saved, and its answer not
    /// recorded: it may be in the collection already.
    #[serde(default)]
    pub writing: bool,
}

/// A row written: its address, and the moment of the page it links to.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Row {
    pub url: String,
    pub moment: String,
}

/// A task and its row, as the app and the CLI show them.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Written {
    #[serde(flatten)]
    pub task: Task,
    pub row: Row,
}

impl TaskLog {
    pub fn new(found: Vec<Task>) -> Self {
        Self {
            found,
            ..Self::default()
        }
    }

    /// The tasks that are rows.
    pub fn written(&self) -> Vec<Written> {
        self.found
            .iter()
            .zip(&self.rows)
            .map(|(task, row)| Written {
                task: task.clone(),
                row: row.clone(),
            })
            .collect()
    }
}

/// Where rows go, and how one already there is found: brainy, or a test's.
pub(crate) trait Rows {
    /// The address of `task`'s row if a write of it reached the collection.
    async fn find(&self, task: &Task) -> Result<Option<String>>;
    /// Writes `task`'s row, linking to `moment`; its address.
    async fn create(&self, task: &Task, moment: &str) -> Result<String>;
    /// The address of the page's moment at `at` seconds.
    fn moment(&self, at: f64) -> String;
}

/// Writes the rows of the pending meeting's tasks not written yet, each
/// recorded as it is: what fails is taken up by the next call, none
/// twice. Nothing without tasks found.
pub(crate) async fn write_rows(pending: &mut Pending, rows: &impl Rows) -> Result<()> {
    ensure!(
        pending.held(),
        "the meeting's tasks are written only by the process holding it"
    );
    loop {
        let Some(log) = &pending.meeting.tasks else {
            return Ok(());
        };
        let Some(task) = log.found.get(log.rows.len()).cloned() else {
            return Ok(());
        };
        let moment = rows.moment(task.at);
        if log.writing
            && let Some(url) = rows.find(&task).await?
        {
            record(pending, Row { url, moment })?;
            continue;
        }
        if let Some(log) = &mut pending.meeting.tasks {
            log.writing = true;
        }
        pending.save()?;
        let url = rows.create(&task, &moment).await?;
        record(pending, Row { url, moment })?;
    }
}

fn record(pending: &mut Pending, row: Row) -> Result<()> {
    if let Some(log) = &mut pending.meeting.tasks {
        log.rows.push(row);
        log.writing = false;
    }
    pending.save()
}

/// The columns logbook writes: their names and the types it adds them as.
pub(crate) const OWNER: &str = "Owner";
const DUE: &str = "Due";
const STATUS: &str = "Status";
const MEETING: &str = "Meeting";
const MOMENT: &str = "Moment";

/// Which of logbook's columns a collection takes, by its properties.
#[derive(Debug, Default, PartialEq)]
struct Columns {
    /// The title property's name (rows have one, whatever it is called).
    title: String,
    owner: bool,
    due: bool,
    status: bool,
    meeting: bool,
    moment: bool,
}

impl Columns {
    fn of(collection: &Collection) -> Self {
        let kind = |name: &str| {
            collection
                .properties
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .map(|p| p.kind.as_str())
        };
        let link = |name| matches!(kind(name), Some("url" | "text"));
        Columns {
            title: collection
                .properties
                .iter()
                .find(|p| p.kind == "title")
                .map_or_else(|| "Name".to_string(), |p| p.name.clone()),
            owner: kind(OWNER) == Some("text"),
            due: kind(DUE) == Some("date"),
            status: matches!(kind(STATUS), Some("status" | "select")),
            meeting: link(MEETING),
            moment: link(MOMENT),
        }
    }

    /// The name of the collection's property `name`, as it is spelled
    /// there.
    fn name(collection: &Collection, name: &str) -> String {
        collection
            .properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map_or_else(|| name.to_string(), |p| p.name.clone())
    }
}

/// What the collection lacks of logbook's columns, as brainy's property
/// specs, and the options its Status lacks.
fn missing(collection: &Collection) -> (Vec<Value>, Value) {
    let has = |name: &str| {
        collection
            .properties
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(name))
    };
    let mut specs = Vec::new();
    for (name, kind) in [
        (OWNER, "text"),
        (DUE, "date"),
        (MEETING, "url"),
        (MOMENT, "url"),
    ] {
        if !has(name) {
            specs.push(json!({ "name": name, "type": kind }));
        }
    }
    let status = collection
        .properties
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(STATUS));
    let mut options = json!({});
    match status {
        None => {
            // Open is to do, Done complete; "To confirm", added after,
            // to do too (brainy puts an option added later there).
            specs.push(json!({ "name": STATUS, "type": "status", "options": ["Open", "Done"] }));
            options = json!({ STATUS: [TO_CONFIRM] });
        }
        Some(p)
            if matches!(p.kind.as_str(), "status" | "select")
                && !p.options.iter().any(|o| o.eq_ignore_ascii_case(TO_CONFIRM)) =>
        {
            options = json!({ p.name.clone(): [TO_CONFIRM] });
        }
        Some(_) => {}
    }
    (specs, options)
}

/// Rows of the collection `id` for the meeting at `page`, titled `title`.
pub(crate) struct BrainyRows<'a> {
    brainy: &'a Brainy,
    collection: String,
    columns: Columns,
    names: Names,
    page: Page,
    title: String,
    /// The page's turns, by time.
    turns: Vec<(String, f64)>,
}

/// The collection's spelling of logbook's columns.
struct Names {
    owner: String,
    due: String,
    status: String,
    meeting: String,
    moment: String,
}

impl<'a> BrainyRows<'a> {
    /// Gets the collection ready (its missing columns added) and reads
    /// the page's turns.
    pub(crate) async fn open(
        brainy: &'a Brainy,
        collection: &str,
        page: &Page,
        title: &str,
    ) -> Result<Self> {
        let mut read = brainy.collection(collection).await?;
        let (specs, options) = missing(&read);
        if !specs.is_empty() || options.as_object().is_some_and(|o| !o.is_empty()) {
            brainy
                .extend_collection(collection, &specs, &options)
                .await?;
            read = brainy.collection(collection).await?;
        }
        let columns = Columns::of(&read);
        for (ok, name) in [
            (columns.owner, OWNER),
            (columns.due, DUE),
            (columns.status, STATUS),
            (columns.meeting, MEETING),
            (columns.moment, MOMENT),
        ] {
            if !ok {
                tracing::warn!(
                    "the tasks' collection has a {name} property of another type: not written"
                );
            }
        }
        let names = Names {
            owner: Columns::name(&read, OWNER),
            due: Columns::name(&read, DUE),
            status: Columns::name(&read, STATUS),
            meeting: Columns::name(&read, MEETING),
            moment: Columns::name(&read, MOMENT),
        };
        Ok(Self {
            brainy,
            collection: collection.to_string(),
            columns,
            names,
            page: page.clone(),
            title: title.to_string(),
            turns: brainy.turns(&page.id).await?,
        })
    }

    fn properties(&self, task: &Task, moment: &str) -> Map<String, Value> {
        let mut out = Map::new();
        if self.columns.status {
            out.insert(self.names.status.clone(), json!(TO_CONFIRM));
        }
        if self.columns.owner
            && let Some(owner) = &task.owner
        {
            out.insert(self.names.owner.clone(), json!(owner));
        }
        if self.columns.due
            && let Some(due) = &task.due
        {
            out.insert(self.names.due.clone(), json!(due));
        }
        if self.columns.meeting {
            out.insert(self.names.meeting.clone(), json!(self.page.url));
        }
        if self.columns.moment {
            out.insert(self.names.moment.clone(), json!(moment));
        }
        out
    }
}

impl Rows for BrainyRows<'_> {
    async fn find(&self, task: &Task) -> Result<Option<String>> {
        let mut filters = vec![json!({
            "property": self.columns.title,
            "operator": "is",
            "value": task.task,
        })];
        if self.columns.meeting {
            filters.push(json!({
                "property": self.names.meeting,
                "operator": "is",
                "value": self.page.url,
            }));
        }
        self.brainy.find_row(&self.collection, &filters).await
    }

    async fn create(&self, task: &Task, moment: &str) -> Result<String> {
        let markdown = format!(
            "Proposed by logbook from [{}]({}), at [{}]({moment}): confirm it or delete it.",
            link_text(&self.title),
            self.page.id,
            clock(task.at),
        );
        self.brainy
            .create_row(
                &self.collection,
                &task.task,
                &self.properties(task, moment),
                &markdown,
            )
            .await
    }

    fn moment(&self, at: f64) -> String {
        moment(&self.page.url, &self.turns, at)
    }
}

/// The address of the turn being spoken at `at` (the last that starts by
/// then; the first, before it), or the page's when it has none.
fn moment(page: &str, turns: &[(String, f64)], at: f64) -> String {
    let turn = turns
        .iter()
        .rev()
        .find(|(_, t)| *t <= at + 0.5)
        .or_else(|| turns.first());
    match turn {
        Some((block, _)) => format!("{}#block-{block}", page.split('#').next().unwrap_or(page)),
        None => page.to_string(),
    }
}

/// A title as a link's text: brackets would end it.
fn link_text(title: &str) -> String {
    title.replace(['[', ']'], "")
}

fn clock(t: f64) -> String {
    let s = t as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::bail;

    use super::*;
    use crate::brainy::Property;
    use crate::pending::Meeting;

    fn task(name: &str, at: f64) -> Task {
        Task {
            task: name.into(),
            owner: None,
            due: None,
            at,
        }
    }

    fn pending(name: &str) -> Pending {
        let root = std::env::temp_dir().join(format!("logbook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let meeting: Meeting = serde_json::from_value(json!({
            "started": "2026-10-29T09:00:00+01:00",
            "channels": [{"name": "mic", "diarize": false}],
            "publish": {"brainy": true, "brainy_url": null, "parent": null, "title": null, "me": "Me"},
        }))
        .unwrap();
        let (mut p, _) = Pending::create(&root, "m", meeting).unwrap();
        p.meeting.tasks = Some(TaskLog::new(vec![
            task("Send the quote", 4.0),
            task("Check the contract", 750.0),
            task("Call the bank", 900.0),
        ]));
        p.save().unwrap();
        p
    }

    /// A collection in memory. `lost` fails the n-th create after the
    /// row is in (the answer lost on the way back); `refused` before.
    #[derive(Default)]
    struct Fake {
        rows: Mutex<Vec<String>>,
        creates: Mutex<usize>,
        finds: Mutex<usize>,
        lost: Option<usize>,
        refused: Option<usize>,
    }

    impl Rows for Fake {
        async fn find(&self, task: &Task) -> Result<Option<String>> {
            *self.finds.lock().unwrap() += 1;
            let rows = self.rows.lock().unwrap();
            Ok(rows
                .iter()
                .find(|r| **r == task.task)
                .map(|r| format!("row:{r}")))
        }

        async fn create(&self, task: &Task, _moment: &str) -> Result<String> {
            let n = {
                let mut creates = self.creates.lock().unwrap();
                *creates += 1;
                *creates
            };
            if self.refused == Some(n) {
                bail!("brainy is down");
            }
            self.rows.lock().unwrap().push(task.task.clone());
            if self.lost == Some(n) {
                bail!("timed out");
            }
            Ok(format!("row:{}", task.task))
        }

        fn moment(&self, at: f64) -> String {
            format!("page#{at}")
        }
    }

    fn written(dir: &std::path::Path) -> TaskLog {
        Pending::open(dir).unwrap().meeting.tasks.unwrap()
    }

    #[tokio::test]
    async fn rows_are_written_once_whatever_fails() {
        let mut p = pending("tasks-once");
        let dir = p.dir.clone();
        // The second row reaches brainy, its answer does not.
        let fake = Fake {
            lost: Some(2),
            ..Fake::default()
        };
        assert!(write_rows(&mut p, &fake).await.is_err());
        let log = written(&dir);
        assert_eq!(log.rows.len(), 1);
        assert!(log.writing, "the second row may be in");
        drop(p);

        // Finished again by another process: the second row is found, not
        // written again; the third is written.
        let mut again = Pending::claim(&dir).unwrap().unwrap();
        let fake = Fake {
            rows: Mutex::new(fake.rows.into_inner().unwrap()),
            refused: Some(1),
            ..Fake::default()
        };
        // brainy refuses the third: nothing more is in.
        assert!(write_rows(&mut again, &fake).await.is_err());
        assert_eq!(*fake.finds.lock().unwrap(), 1);
        assert_eq!(written(&dir).rows.len(), 2);
        // brainy is back: the third is looked up (not there) and written.
        let fake = Fake {
            rows: Mutex::new(fake.rows.into_inner().unwrap()),
            ..Fake::default()
        };
        write_rows(&mut again, &fake).await.unwrap();
        assert_eq!(
            *fake.rows.lock().unwrap(),
            ["Send the quote", "Check the contract", "Call the bank"]
        );
        let log = written(&dir);
        assert!(!log.writing);
        assert_eq!(log.rows[2].moment, "page#900");
        assert_eq!(log.written().len(), 3);

        // All written: another run asks nothing.
        let fake = Fake::default();
        write_rows(&mut again, &fake).await.unwrap();
        assert_eq!(*fake.creates.lock().unwrap(), 0);
        assert_eq!(*fake.finds.lock().unwrap(), 0);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[tokio::test]
    async fn only_the_holder_writes() {
        let p = pending("tasks-lock");
        let mut read = Pending::open(&p.dir).unwrap();
        let fake = Fake::default();
        assert!(write_rows(&mut read, &fake).await.is_err());
        assert_eq!(*fake.creates.lock().unwrap(), 0);
        let _ = std::fs::remove_dir_all(p.dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_row_links_to_the_turn_being_spoken() {
        let turns = vec![("a".to_string(), 4.0), ("b".to_string(), 700.0)];
        assert_eq!(
            moment("https://b/w/x/p", &turns, 750.0),
            "https://b/w/x/p#block-b"
        );
        assert_eq!(
            moment("https://b/w/x/p", &turns, 300.0),
            "https://b/w/x/p#block-a"
        );
        assert_eq!(
            moment("https://b/w/x/p", &turns, 0.0),
            "https://b/w/x/p#block-a"
        );
        assert_eq!(moment("https://b/w/x/p", &[], 10.0), "https://b/w/x/p");
    }

    #[test]
    fn a_row_links_to_its_turn_on_the_displayed_clock() {
        use crate::transcript::Writer;
        use crate::voices::Library;
        use logbook_engine::protocol::Utterance;
        let u = |speaker: u32, start: f64| Utterance {
            channel: "system".into(),
            speaker: Some(speaker),
            start,
            end: start + 5.0,
            text: format!("{start}"),
            words: Vec::new(),
        };
        // A meeting whose first word is 7 minutes after its cut.
        let utterances = [u(0, 432.3), u(1, 600.0), u(0, 1200.0)];
        let mut writer = Writer::new("Marco", 432.0);
        let page = writer.body(&utterances, &[], &Library::default());
        // The page's turns, as `Brainy::turns` reads them: by block.
        let turns: Vec<(String, f64)> = page
            .split("\n\n")
            .enumerate()
            .filter_map(|(i, b)| Some((i.to_string(), crate::brainy::turn_time(b)?)))
            .collect();
        assert_eq!(turns[0].1, 0.0);
        // The model reads the times the transcript prints (history's
        // `transcript`, the writer's `text`): the third turn's.
        let text = writer.text(&utterances, &[], &Library::default());
        let third = text.lines().filter(|l| l.starts_with("**")).nth(2).unwrap();
        assert_eq!(third, "**Speaker 1** · 12:48");
        let at = crate::brainy::turn_time(third).unwrap();
        assert_eq!(
            moment("https://b/p", &turns, at),
            format!("https://b/p#block-{}", turns[2].0)
        );
        // A task said just before the second turn links to the first.
        assert_eq!(moment("https://b/p", &turns, 160.0), "https://b/p#block-0");
    }

    fn property(name: &str, kind: &str, options: &[&str]) -> Property {
        Property {
            name: name.into(),
            kind: kind.into(),
            options: options.iter().map(|o| o.to_string()).collect(),
        }
    }

    #[test]
    fn a_collection_gets_the_columns_it_lacks() {
        let empty = Collection {
            name: "Tasks".into(),
            properties: vec![property("Name", "title", &[])],
        };
        let (specs, options) = missing(&empty);
        assert_eq!(specs.len(), 5);
        assert_eq!(options, json!({"Status": ["To confirm"]}));
        // A team's board: its Status takes the option; an Owner that is a
        // person is left alone, and not written.
        let board = Collection {
            name: "Board".into(),
            properties: vec![
                property("Task", "title", &[]),
                property("status", "status", &["Not started", "Done"]),
                property("Owner", "person", &[]),
                property("Due", "date", &[]),
                property("Meeting", "url", &[]),
                property("Moment", "url", &[]),
            ],
        };
        let (specs, options) = missing(&board);
        assert!(specs.is_empty());
        assert_eq!(options, json!({"status": ["To confirm"]}));
        let columns = Columns::of(&board);
        assert_eq!(columns.title, "Task");
        assert!(!columns.owner && columns.due && columns.status && columns.meeting);
    }
}
