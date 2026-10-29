//! The meeting's report, written by a language model from the final
//! transcript and the notes the person took during the meeting, as
//! Granola does: a summary, the notes completed from what was said, the
//! decisions and the action items (docs/pipeline.md, "Report").
//!
//! One request to the same OpenAI-compatible endpoint as the clean-up
//! (polish.rs), with the whole transcript, when it is up to
//! `ONE_REQUEST`: six to nine hours. A longer meeting is reported in
//! steps: the model takes detailed notes of each part of the transcript
//! (`PART`, two to three hours), in order, and writes the report from
//! them as from a transcript; notes too long for one request are taken
//! again, part by part, until they fit. Nothing is cut: every hour of the
//! meeting reaches the report. The transcript comes with the speakers'
//! names the client gave them (the server never sees the client's
//! voices), so the report names people as the page does.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tracing::info;

use crate::polish::{self, Options};
use crate::protocol::ReportRequest;

/// The longest transcript sent in one request, in characters: some
/// 90 000 tokens, six to nine hours of speech (a 28-minute meeting was
/// 16 800 characters). A longer one is reported in steps.
pub const ONE_REQUEST: usize = 320_000;
/// A part of a long transcript the model takes notes of: some 22 000
/// tokens, two to three hours, within the context of small models too.
const PART: usize = 80_000;
/// The longest notes sent.
pub const MAX_NOTES: usize = 40_000;

const INSTRUCTIONS: &str = "\
You write the report of a meeting from its transcript and from the notes the person \
recording it took during the meeting, as a careful assistant would for the minutes.

First decide which language most of the transcript is in. Write the whole report, headings \
included, in that language, whatever the language of these instructions or of the notes. \
Answer with Markdown only, in this order, leaving out a section with nothing to put in it. \
Each heading is the word in angle brackets in the transcript's language: an English meeting \
has \"## Summary\", \"## Notes\", \"## Decisions\", \"## Action items\"; an Italian one \
\"## Riepilogo\", \"## Note\", \"## Decisioni\", \"## Azioni\"; another language its own words.

## <Summary>
Three to six sentences: what the meeting was about and how it ended.

## <Notes>
Only when there are notes: the person's notes as they wrote them, in their order, in their \
words and in their Markdown: a heading stays a heading (one level below this section's), a \
paragraph a paragraph, a list a list; never turn them all into a list. After each point, a \
bulleted list of what the transcript says about it. A point the transcript says nothing \
about stays alone: never put a sentence under a point it \
does not concern, never the same thing under two points, never the transcript copied out. \
The notes say what mattered to the person: follow them. A line of the notes that is only \
an image, ![...](name.png), is a screenshot the person took at that point: copy each such line \
as it is, on a line of its own after the point it belongs to and its bullets, outside any list.

## <Decisions>
A bulleted list of what was decided.

## <Action items>
A task list (- [ ] ...), one per line, each starting with a verb, with who owns it \
(\" — Name\") and when it is due when the meeting says so.

Rules:
- Only what the transcript or the notes say: never invent names, numbers, dates or decisions.
- Call people as the transcript does. \"Speaker 2\" is someone whose name is not known: \
keep the label, unless the meeting itself makes their name certain.
- The transcript comes from speech recognition: read misheard words for what they must have been.
- No preamble, no closing remark, no title above the first heading.";

/// For a meeting too long for one request: notes of one part, from which
/// the report is written with the notes of the others.
const PART_INSTRUCTIONS: &str = "\
You take notes of one part of a long meeting, for whoever writes the meeting's report from \
the notes of all its parts. What you are given is the transcript of that part, or the notes \
already taken of several consecutive parts of it: keep their order and their detail.

First decide which language most of the text is in, and write the notes in that language, \
whatever the language of these instructions: an Italian meeting gets Italian notes. Answer \
with Markdown only: bullets in the \
order things were said, with the time where the text gives one. Keep:
- every subject discussed, with who said what about it, and how it ended;
- names, numbers, amounts, dates and places, exactly;
- every decision, with who made it;
- every task someone takes on or is given, with who and when it is due;
- questions left open;
- whatever touches the points of the person's notes, when there are notes.

Rules:
- Only what the text says: never invent names, numbers, dates or decisions.
- Call people as the text does; keep labels such as \"Speaker 2\".
- The transcript comes from speech recognition: read misheard words for what they must have been.
- At most about 1500 words. No preamble, no closing remark.";

/// The report's Markdown, from the model at `options`. Blocks for one
/// request, or one per part of a long meeting: run it off the async
/// threads.
pub fn write(agent: &polish::Agent, options: &Options, request: &ReportRequest) -> Result<String> {
    if request.transcript.trim().is_empty() {
        bail!("the meeting has no transcript to report on");
    }
    if request.notes.chars().count() > MAX_NOTES {
        bail!("the notes are longer than {MAX_NOTES} characters");
    }
    let mut source = Source::Transcript(request.transcript.trim().to_string());
    let mut round = 1;
    while source.text().chars().count() > ONE_REQUEST {
        let parts = parts(source.text(), PART);
        let total = parts.len();
        info!(
            round,
            parts = total,
            "the meeting is long: its report is written in steps"
        );
        let mut notes = Vec::with_capacity(total);
        for (i, part) in parts.iter().enumerate() {
            let user = format!(
                "{}Part {} of {total}:\n<part>\n{part}\n</part>",
                header(request),
                i + 1
            );
            let answer = polish::complete(
                agent,
                options,
                chat(&options.model, PART_INSTRUCTIONS, user),
            )
            .with_context(|| format!("the notes of part {} of {total}", i + 1))?;
            let taken = clean(&answer);
            if taken.is_empty() {
                bail!("the model wrote no notes for part {} of {total}", i + 1);
            }
            notes.push(format!("## Part {} of {total}\n\n{taken}", i + 1));
        }
        let shorter = Source::Notes(notes.join("\n\n"));
        if shorter.text().len() >= source.text().len() {
            bail!("the model's notes of the meeting are no shorter than what they were taken from");
        }
        source = shorter;
        round += 1;
    }
    let user = match &source {
        Source::Transcript(text) => {
            format!(
                "{}Transcript:\n<transcript>\n{text}\n</transcript>",
                header(request)
            )
        }
        Source::Notes(text) => format!(
            "{}The meeting was too long to give whole: these are detailed notes of its \
             consecutive parts, in order, taken from its transcript. Write the report from \
             them as from the transcript.\n\nTranscript notes:\n<transcript>\n{text}\n</transcript>",
            header(request)
        ),
    };
    let answer = polish::complete(agent, options, chat(&options.model, INSTRUCTIONS, user))?;
    let markdown = clean(&answer);
    if markdown.is_empty() {
        bail!("the model wrote an empty report");
    }
    Ok(markdown)
}

/// What the report is written from: the transcript, or notes of its parts.
enum Source {
    Transcript(String),
    Notes(String),
}

impl Source {
    fn text(&self) -> &str {
        match self {
            Source::Transcript(text) | Source::Notes(text) => text,
        }
    }
}

/// `text` in parts of at most `max` characters, cut between paragraphs
/// (a paragraph longer than that, between words).
fn parts(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut length = 0;
    for paragraph in text.split("\n\n") {
        let n = paragraph.chars().count();
        if length > 0 && length + 2 + n > max {
            out.push(std::mem::take(&mut current));
            length = 0;
        }
        if n > max {
            for word in paragraph.split(' ') {
                let w = word.chars().count();
                if length > 0 && length + 1 + w > max {
                    out.push(std::mem::take(&mut current));
                    length = 0;
                }
                if length > 0 {
                    current.push(' ');
                    length += 1;
                }
                current.push_str(word);
                length += w;
            }
            continue;
        }
        if length > 0 {
            current.push_str("\n\n");
            length += 2;
        }
        current.push_str(paragraph);
        length += n;
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// The meeting's title and the person's notes, before what is reported on.
fn header(request: &ReportRequest) -> String {
    let mut user = String::new();
    if !request.title.trim().is_empty() {
        user.push_str(&format!("Meeting: {}\n\n", request.title.trim()));
    }
    if request.notes.trim().is_empty() {
        user.push_str("Notes: none.\n\n");
    } else {
        user.push_str(&format!(
            "Notes:\n<notes>\n{}\n</notes>\n\n",
            request.notes.trim()
        ));
    }
    user
}

fn chat(model: &str, instructions: &str, user: String) -> Value {
    json!({
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": user},
        ],
    })
}

/// The Markdown past what models put around it: a `<think>` block, a
/// code fence around the whole answer.
fn clean(answer: &str) -> String {
    let answer = match answer.rfind("</think>") {
        Some(i) => &answer[i + "</think>".len()..],
        None => answer,
    };
    let answer = answer.trim();
    let unfenced = answer
        .strip_prefix("```markdown")
        .or_else(|| answer.strip_prefix("```md"))
        .or_else(|| answer.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"));
    unfenced.unwrap_or(answer).trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use super::*;

    #[test]
    fn what_models_put_around_the_report_is_dropped() {
        assert_eq!(
            clean("<think>hm</think>\n## Summary\nOk."),
            "## Summary\nOk."
        );
        assert_eq!(
            clean("```markdown\n## Summary\nOk.\n```"),
            "## Summary\nOk."
        );
        assert_eq!(clean("  ## Summary\nOk.  "), "## Summary\nOk.");
    }

    /// A model at a local address answering `answers` requests in turn,
    /// each by what it was asked; returns the requests.
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
                // Headers, then the body by Content-Length.
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
    fn notes_and_transcript_go_to_the_model() {
        let (options, server) = model(1, |_| "## Summary\nDone.".into());
        let request = ReportRequest {
            title: "Weekly".into(),
            notes: "- budget".into(),
            transcript: "**Luca** · 00:04\n\nIl budget è approvato.".into(),
        };
        let report = write(&polish::agent(&options), &options, &request).unwrap();
        assert_eq!(report, "## Summary\nDone.");
        let sent = server.join().unwrap();
        assert!(sent[0].contains("- budget"));
        assert!(sent[0].contains("Il budget"));
        let empty = ReportRequest {
            transcript: " ".into(),
            ..request
        };
        assert!(write(&polish::agent(&options), &options, &empty).is_err());
    }

    #[test]
    fn a_long_meeting_is_reported_from_notes_of_its_parts() {
        // Ten hours or so: 390 000 characters, in turns of 400.
        let turn = format!("**Luca** · 00:00\n\n{}", "parola ".repeat(55));
        let turns = 390_000 / turn.len();
        let transcript = vec![turn; turns].join("\n\n");
        assert!(transcript.chars().count() > ONE_REQUEST);
        // Five parts of at most 80 000, then the report.
        let (options, server) = model(6, |request| {
            if request.contains("<part>") {
                "- Luca spoke.".into()
            } else {
                "## Summary\nLong.".into()
            }
        });
        let request = ReportRequest {
            title: "Offsite".into(),
            notes: "- hiring".into(),
            transcript,
        };
        let report = write(&polish::agent(&options), &options, &request).unwrap();
        assert_eq!(report, "## Summary\nLong.");
        let sent = server.join().unwrap();
        assert!(sent[0].contains("Part 1 of 5") && sent[0].contains("- hiring"));
        assert!(sent[4].contains("Part 5 of 5"));
        assert!(sent[5].contains("## Part 5 of 5") && sent[5].contains("- Luca spoke."));
        assert!(!sent[5].contains("parola"));
    }

    #[test]
    fn parts_are_cut_between_paragraphs() {
        let text = "aaaa\n\nbbbb\n\ncccc";
        assert_eq!(parts(text, 10), ["aaaa\n\nbbbb", "cccc"]);
        assert_eq!(parts("one two three", 7), ["one two", "three"]);
        assert_eq!(parts(text, 100), [text]);
    }
}
