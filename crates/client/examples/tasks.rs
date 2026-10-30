//! A kept meeting's tasks (and with `--report` its report) asked of a
//! language model again, from the transcript the client sends it: to
//! compare prompts on a real meeting. Prints the tasks as JSON.
//!
//!   LLM_URL=https://api.groq.com/openai/v1 LLM_MODEL=openai/gpt-oss-20b LLM_KEY=… \
//!     cargo run -p logbook-client --example tasks -- DATA MEETING_ID [--report]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use logbook_client::history;
use logbook_engine::protocol::{ReportRequest, TasksRequest};
use logbook_engine::{llm, report, tasks};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().context("DATA MEETING_ID [--report]")?);
    let id = args.next().context("DATA MEETING_ID [--report]")?;
    let with_report = args.next().as_deref() == Some("--report");
    let options = llm::Options {
        url: std::env::var("LLM_URL").context("LLM_URL")?,
        model: std::env::var("LLM_MODEL").context("LLM_MODEL")?,
        key: std::env::var("LLM_KEY").ok(),
        timeout: Duration::from_secs(llm::TIMEOUT),
    };
    let agent = llm::agent(&options);
    let kept = history::load(&data, &id)?;
    let request = TasksRequest {
        title: kept.title.clone(),
        notes: kept.notes.clone(),
        transcript: kept.transcript(),
        date: kept.started.get(..10).unwrap_or_default().to_string(),
    };
    let found = tasks::find(&agent, &options, &request)?;
    println!("{}", serde_json::to_string_pretty(&found)?);
    if with_report {
        let markdown = report::write(
            &agent,
            &options,
            &ReportRequest {
                title: request.title,
                notes: request.notes,
                transcript: request.transcript,
                tasks: found,
            },
        )?;
        println!("\n{markdown}");
    }
    Ok(())
}
