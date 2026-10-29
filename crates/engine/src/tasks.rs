//! The tasks agreed in a meeting, as data: what is to be done, who owns
//! it, when it is due and when in the meeting it was said, found by the
//! language model from the final transcript (docs/pipeline.md, "Tasks").
//! The client writes them as rows of a brainy collection (client
//! tasks.rs); decisions stay in the report.
//!
//! The transcript goes in parts of at most `report::PART` characters (two
//! to three hours), one request each, in order, whatever the meeting's
//! length: every request is bounded, and so is every answer (a part's
//! tasks fit the answer's limit, the whole of a day's would not). Each
//! part is given the tasks found in the parts before it, so that one
//! discussed across a cut is not listed twice; at most `MAX_TASKS` in all.
//!
//! The answer is JSON, read defensively: whatever surrounds it (a
//! `<think>` block, a code fence, a sentence) is dropped, an item without
//! a task is skipped, an owner who is not one of the transcript's
//! speakers is none, a due date that is not a date is none, and a task
//! without a time of the transcript is dropped: every row links to the
//! moment it was said, so that checking it costs a click. What the model
//! finds is a proposal: the rows are written "To confirm". A part whose answer cannot be read gives
//! no tasks and a log line; only a model that cannot be reached is an
//! error.

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::llm::{self, Options};
use crate::protocol::{Task, TasksRequest};
use crate::report::{self, MAX_NOTES, PART};

/// The most tasks a meeting gives: more is a model listing every sentence.
pub const MAX_TASKS: usize = 100;
/// The longest task kept, in characters.
const MAX_TASK: usize = 300;

const INSTRUCTIONS: &str = "\
You list the tasks people committed to in a meeting, from its transcript (and the notes the \
person recording it took, to understand it), for a task tracker. Each one becomes a row that \
a person will confirm or delete: list only what you are sure of. Missing a vague one is \
better than inventing one.

A task is an explicit commitment: a person and an action to do after the meeting. \
\"I'll send the quote\", \"Luca, can you check the contract by Friday?\" \"Sure.\", \
\"Anna prepares the slides\". Not a decision, not an idea, not a maybe (\"we could\", \
\"maybe I'll look\"), not a wish (\"we should someday\"), not a question left open, not \
something done during the meeting.

Answer with JSON only, nothing before or after it, in this shape:
{\"tasks\": [{\"task\": \"...\", \"owner\": \"...\", \"due\": \"YYYY-MM-DD\", \"time\": \"MM:SS\"}]}

- task: what is to be done, one short sentence starting with a verb, in the language of the \
transcript (an Italian meeting gets Italian tasks).
- owner: the person who will do it, only when the meeting says so or they clearly take it on \
themselves (\"I'll do it\" is the speaker's). It is who does it, not who speaks: when Anna \
says \"Luca will do it\", the owner is Luca. Write them exactly as the transcript labels \
that speaker (the bold name before a turn's time, \"Speaker 2\" included). null when no one \
is named, or when the person does not speak in the transcript.
- due: only when the meeting says when it is due and the date follows from the meeting's \
date given below, as YYYY-MM-DD (\"Friday\" is the first Friday after the meeting; \"end of \
the month\" its last day). null otherwise (\"soon\", \"next sprint\"): never guess one.
- time: the time printed at the start of the turn where it was committed to (MM:SS or \
H:MM:SS), exactly as in the transcript. A task must have one: one that is only in the notes \
is not listed.

Rules:
- Only what the transcript says: never invent tasks, names or dates.
- One entry per task, in the order they were agreed; the same task said twice is one entry.
- The transcript comes from speech recognition: read misheard words for what they must have been.
- {\"tasks\": []} when no one committed to anything.";

/// The tasks agreed in the meeting of `request`, in order. Blocks for
/// one request per part: run it off the async threads. An error only
/// when the model could not be asked; an answer it cannot read is no
/// tasks (and a warning).
pub fn find(agent: &llm::Agent, options: &Options, request: &TasksRequest) -> Result<Vec<Task>> {
    let transcript = request.transcript.trim();
    if transcript.is_empty() {
        return Ok(Vec::new());
    }
    let notes = request.notes.trim();
    let notes: String = notes.chars().take(MAX_NOTES).collect();
    let speakers = Speakers::of(transcript);
    let parts = report::parts(transcript, PART);
    let total = parts.len();
    if total > 1 {
        info!(
            parts = total,
            "the meeting is long: its tasks are found by part"
        );
    }
    let mut found: Vec<Task> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let user = message(request, &notes, &found, i, total, part);
        let answer = llm::complete(
            agent,
            options,
            report::chat(&options.model, INSTRUCTIONS, user),
        )
        .with_context(|| format!("the tasks of part {} of {total}", i + 1))?;
        match parse(&answer, &speakers) {
            Some(tasks) => {
                for task in tasks {
                    if found.len() >= MAX_TASKS {
                        break;
                    }
                    if !found.iter().any(|f| same(&f.task, &task.task)) {
                        found.push(task);
                    }
                }
            }
            None => warn!(
                part = i + 1,
                parts = total,
                "the model's tasks could not be read: none from this part ({})",
                excerpt(&answer)
            ),
        }
    }
    Ok(found)
}

/// What a part's request says: the meeting, its date, the notes, the
/// tasks already found, and the part.
fn message(
    request: &TasksRequest,
    notes: &str,
    found: &[Task],
    i: usize,
    total: usize,
    part: &str,
) -> String {
    let mut user = String::new();
    if !request.title.trim().is_empty() {
        user.push_str(&format!("Meeting: {}\n", request.title.trim()));
    }
    match weekday(&request.date) {
        Some(day) => user.push_str(&format!("Date: {} ({day})\n\n", request.date)),
        None => user.push_str("Date: unknown: give no due date\n\n"),
    }
    if notes.is_empty() {
        user.push_str("Notes: none.\n\n");
    } else {
        user.push_str(&format!("Notes:\n<notes>\n{notes}\n</notes>\n\n"));
    }
    if total == 1 {
        user.push_str(&format!("Transcript:\n<transcript>\n{part}\n</transcript>"));
        return user;
    }
    if !found.is_empty() {
        user.push_str(
            "Tasks already found in the parts before this one: do not list them again.\n",
        );
        for task in found {
            user.push_str(&format!("- {}\n", task.task));
        }
        user.push('\n');
    }
    user.push_str(&format!(
        "The meeting is long: this is part {} of {total} of its transcript.\n\
         <transcript>\n{part}\n</transcript>",
        i + 1
    ));
    user
}

/// The speakers' labels as the transcript's turns show them ("**Luca** ·
/// 00:04", unescaped), and the time of its last turn.
#[derive(Debug, Default)]
struct Speakers {
    labels: Vec<String>,
    end: f64,
}

impl Speakers {
    fn of(transcript: &str) -> Self {
        let mut out = Speakers::default();
        for line in transcript.lines() {
            let Some((label, time)) = turn(line) else {
                continue;
            };
            out.end = out.end.max(time);
            if !out.labels.contains(&label) {
                out.labels.push(label);
            }
        }
        out
    }

    /// The label `name` is, ignoring case and Markdown escapes.
    fn label(&self, name: &str) -> Option<String> {
        let name = unescape(name.trim().trim_matches('*').trim());
        self.labels
            .iter()
            .find(|l| l.to_lowercase() == name.to_lowercase())
            .cloned()
    }
}

/// A turn's heading line: its speaker's label and its time.
fn turn(line: &str) -> Option<(String, f64)> {
    let rest = line.trim().strip_prefix("**")?;
    let (label, time) = rest.rsplit_once("** · ")?;
    Some((unescape(label), seconds(time)?))
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// "12:34" or "1:02:03" (as the transcript prints times), in seconds.
fn seconds(text: &str) -> Option<f64> {
    let fields: Vec<&str> = text.trim().split(':').collect();
    if !(2..=3).contains(&fields.len()) {
        return None;
    }
    let mut total = 0u64;
    for (i, field) in fields.iter().enumerate() {
        if field.is_empty() || field.len() > 3 || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: u64 = field.parse().ok()?;
        if i > 0 && n >= 60 {
            return None;
        }
        total = total * 60 + n;
    }
    Some(total as f64)
}

/// The tasks in the model's `answer`; None when it holds no list of them.
fn parse(answer: &str, speakers: &Speakers) -> Option<Vec<Task>> {
    let items = items(answer)?;
    Some(
        items
            .iter()
            .filter_map(|item| task(item, speakers))
            .collect(),
    )
}

/// The list in the answer: `{"tasks": [...]}`, or a bare list, wherever
/// in the text it is.
fn items(answer: &str) -> Option<Vec<Value>> {
    let answer = match answer.rfind("</think>") {
        Some(i) => &answer[i + "</think>".len()..],
        None => answer,
    };
    let value: Value = serde_json::from_str(answer.trim()).ok().or_else(|| {
        // The outermost object or list, past a fence or a sentence.
        let start = answer.find(['{', '['])?;
        let end = answer.rfind(['}', ']'])?;
        serde_json::from_str(answer.get(start..=end)?).ok()
    })?;
    match value {
        Value::Array(items) => Some(items),
        Value::Object(mut map) => match map.remove("tasks") {
            Some(Value::Array(items)) => Some(items),
            _ => None,
        },
        _ => None,
    }
}

/// One item as a task, its fields checked; None without a task or a
/// time in the transcript.
fn task(item: &Value, speakers: &Speakers) -> Option<Task> {
    let text = |key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
    };
    let what = text("task").or_else(|| text("title"))?;
    let what: String = what.split_whitespace().collect::<Vec<_>>().join(" ");
    let what = match what.char_indices().nth(MAX_TASK) {
        Some((i, _)) => format!("{}…", &what[..i]),
        None => what,
    };
    let owner = text("owner").and_then(|o| speakers.label(o));
    let due = text("due").and_then(date);
    let at = match item.get("time") {
        Some(Value::String(s)) => seconds(s),
        Some(Value::Number(n)) => n.as_f64().filter(|s| s.is_finite() && *s >= 0.0),
        _ => None,
    }
    // A time past the last turn is not one of the transcript's.
    .filter(|&t| t <= speakers.end)?;
    Some(Task {
        task: what,
        owner,
        due,
        at,
    })
}

/// `YYYY-MM-DD` if `text` starts with a real date.
fn date(text: &str) -> Option<String> {
    let day = text.get(..10)?;
    weekday(day).map(|_| day.to_string())
}

/// The day of the week of `YYYY-MM-DD`; None if it is not a date.
fn weekday(date: &str) -> Option<&'static str> {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let s = date.get(range)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (y, m, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&m) || d < 1 || d > days[(m - 1) as usize] {
        return None;
    }
    // Sakamoto's: 0 is Sunday.
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let w = (y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d) % 7;
    Some(
        [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ][w as usize],
    )
}

/// Two tasks that say the same, ignoring case, spaces and punctuation.
fn same(a: &str, b: &str) -> bool {
    let key = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    key(a) == key(b)
}

/// The start of an answer, for a log line.
fn excerpt(answer: &str) -> String {
    let flat: String = answer.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    const TRANSCRIPT: &str = "**Marco** · 00:04\n\nI'll send the quote.\n\n\
        **Speaker 2 \\(Luca?\\)** · 12:30\n\nI check the contract by Friday.\n\n\
        **Anna\\_B** · 1:02:03\n\nOk.";

    fn speakers() -> Speakers {
        Speakers::of(TRANSCRIPT)
    }

    #[test]
    fn the_speakers_and_the_end_are_read_from_the_turns() {
        let s = speakers();
        assert_eq!(s.labels, ["Marco", "Speaker 2 (Luca?)", "Anna_B"]);
        assert_eq!(s.end, 3723.0);
        assert_eq!(s.label("marco").as_deref(), Some("Marco"));
        assert_eq!(s.label("**Anna\\_B**").as_deref(), Some("Anna_B"));
        assert_eq!(s.label("Luca"), None);
    }

    #[test]
    fn a_good_answer_gives_its_tasks_checked() {
        let answer = r#"{"tasks": [
            {"task": "Send  the quote", "owner": "Marco", "due": null, "time": "00:04"},
            {"task": "Check the contract", "owner": "Speaker 2 (Luca?)", "due": "2026-10-30", "time": "12:30"},
            {"task": "Book the room", "owner": "Giulia", "due": "next Friday", "time": "12:30"},
            {"task": "Call the bank", "owner": null, "due": "2026-02-30", "time": 3700},
            {"task": "Late", "time": "2:00:00"},
            {"task": "Not a time", "time": "9:99"},
            {"task": "From the notes", "time": null}
        ]}"#;
        let tasks = parse(answer, &speakers()).unwrap();
        assert_eq!(
            tasks[0],
            Task {
                task: "Send the quote".into(),
                owner: Some("Marco".into()),
                due: None,
                at: 4.0,
            }
        );
        assert_eq!(tasks[1].owner.as_deref(), Some("Speaker 2 (Luca?)"));
        assert_eq!(tasks[1].due.as_deref(), Some("2026-10-30"));
        assert_eq!(tasks[1].at, 750.0);
        // Not a speaker, not a date: dropped, the task kept.
        assert_eq!((tasks[2].owner.clone(), tasks[2].due.clone()), (None, None));
        // February has no 30th; a number of seconds is a time.
        assert_eq!((tasks[3].due.clone(), tasks[3].at), (None, 3700.0));
        // Past the last turn, not a time, no time: no link, no row.
        assert_eq!(tasks.len(), 4);
    }

    #[test]
    fn what_models_put_around_the_json_is_dropped() {
        let s = speakers();
        let fenced = "<think>hm {</think>\nHere they are:\n```json\n{\"tasks\": [{\"task\": \"Send the quote\", \"time\": \"00:04\"}]}\n```";
        assert_eq!(parse(fenced, &s).unwrap()[0].task, "Send the quote");
        let bare = "[{\"task\": \"Send the quote\", \"owner\": \"MARCO\", \"time\": \"0:04\"}]";
        assert_eq!(parse(bare, &s).unwrap()[0].owner.as_deref(), Some("Marco"));
        assert_eq!(parse("{\"tasks\": []}", &s).unwrap(), []);
    }

    #[test]
    fn a_bad_answer_gives_no_tasks() {
        let s = speakers();
        for bad in [
            "",
            "No tasks were agreed.",
            "{\"tasks\": \"none\"}",
            "{\"items\": [{\"task\": \"x\"}]}",
            "{\"tasks\": [{\"task\": \"x\"}",
            "42",
        ] {
            assert!(parse(bad, &s).is_none(), "{bad}");
        }
        // Items without a task are skipped, the others kept.
        let mixed = r#"{"tasks": [{"owner": "Marco"}, "send", {"task": ""}, {"task": "Ok", "time": "00:04"}]}"#;
        assert_eq!(parse(mixed, &s).unwrap().len(), 1);
    }

    #[test]
    fn dates_are_checked_and_named() {
        assert_eq!(weekday("2026-10-29"), Some("Thursday"));
        assert_eq!(weekday("2024-02-29"), Some("Thursday"));
        assert_eq!(weekday("2026-02-29"), None);
        assert_eq!(weekday("2026-13-01"), None);
        assert_eq!(weekday("29/10/2026"), None);
        assert_eq!(date("2026-11-06T17:00:00").as_deref(), Some("2026-11-06"));
        assert_eq!(seconds("1:02:03"), Some(3723.0));
        assert_eq!(seconds("12:30"), Some(750.0));
        assert_eq!(seconds("12"), None);
        assert_eq!(seconds("ab:cd"), None);
    }

    /// A model answering each request by what it was asked; the requests.
    fn model(
        answers: usize,
        answer: fn(&str) -> String,
    ) -> (Options, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let n = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_string)
                            })
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let content = answer(&request);
                let body = json!({"choices": [{"message": {"content": content}}]}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requests.push(request);
            }
            requests
        });
        let options = Options {
            url: format!("http://{address}/v1"),
            model: "m".into(),
            key: None,
            timeout: Duration::from_secs(5),
        };
        (options, server)
    }

    #[test]
    fn a_short_meeting_is_one_request_with_its_date() {
        let (options, server) = model(1, |_| {
            r#"{"tasks": [{"task": "Send the quote", "owner": "Marco", "time": "00:04"}]}"#.into()
        });
        let request = TasksRequest {
            title: "Weekly".into(),
            notes: "- quote".into(),
            transcript: TRANSCRIPT.into(),
            date: "2026-10-29".into(),
        };
        let tasks = find(&llm::agent(&options), &options, &request).unwrap();
        assert_eq!(tasks.len(), 1);
        let sent = server.join().unwrap();
        assert!(sent[0].contains("2026-10-29 (Thursday)"), "{}", sent[0]);
        assert!(sent[0].contains("- quote"));
    }

    #[test]
    fn a_long_meeting_is_asked_by_part_and_a_bad_part_gives_nothing() {
        // Three parts of at most 80 000 characters.
        let turn = format!("**Marco** · 00:04\n\n{}", "parola ".repeat(55));
        let transcript = vec![turn; 200_000 / 400].join("\n\n");
        let (options, server) = model(3, |request| {
            if request.contains("part 1 of 3") {
                r#"{"tasks": [{"task": "Send the quote", "owner": "Marco", "time": "00:04"}]}"#
                    .into()
            } else if request.contains("part 2 of 3") {
                "I could not find any.".into()
            } else {
                // The first part's task again, and a new one.
                r#"{"tasks": [{"task": "send the quote.", "time": "00:04"}, {"task": "Call the bank", "time": "00:04"}]}"#.into()
            }
        });
        let request = TasksRequest {
            title: String::new(),
            notes: String::new(),
            transcript,
            date: "2026-10-29".into(),
        };
        let tasks = find(&llm::agent(&options), &options, &request).unwrap();
        let names: Vec<&str> = tasks.iter().map(|t| t.task.as_str()).collect();
        assert_eq!(names, ["Send the quote", "Call the bank"]);
        let sent = server.join().unwrap();
        assert!(!sent[0].contains("already found"));
        assert!(sent[2].contains("already found") && sent[2].contains("- Send the quote"));
        assert!(sent.iter().all(|r| r.len() < 100_000));
    }

    #[test]
    fn an_unreachable_model_is_an_error() {
        let options = Options {
            url: "http://127.0.0.1:9/v1".into(),
            model: "m".into(),
            key: None,
            timeout: Duration::from_secs(2),
        };
        let request = TasksRequest {
            title: String::new(),
            notes: String::new(),
            transcript: TRANSCRIPT.into(),
            date: "2026-10-29".into(),
        };
        assert!(find(&llm::agent(&options), &options, &request).is_err());
    }
}
