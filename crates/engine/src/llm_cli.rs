//! The language model of a coding agent's CLI, run as a subprocess: Claude
//! Code (`claude -p`) or Codex (`codex exec`), signed in with the person's
//! own account, for whoever has a subscription and no API key (llm.rs
//! picks it by `Provider`).
//!
//! Each request is one process, in an empty folder of its own, with no
//! tools, no MCP servers, no settings or hooks of the user's (Claude) and a
//! read-only sandbox (Codex): it answers the prompt and nothing else. The
//! instructions go in a file (Claude's `--system-prompt-file`) or at the
//! head of the prompt (Codex has no system prompt), the prompt on stdin:
//! nothing long goes on the command line, which a `.cmd` on Windows would
//! refuse.
//!
//! Whether the CLI is signed in is asked of the CLI itself (`claude auth
//! status`, `codex login status`: exit 0 when signed in), before a meeting
//! is recorded and at the server's start, so that a report does not fail
//! only at the meeting's end. A desktop app started from the Finder has
//! only the system's PATH: the CLI is looked for in the folders its
//! installers use too, and those folders are given to it (Codex from npm
//! is a node script).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// A coding agent's CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
}

/// How long asking whether the CLI is signed in may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// The longest error text kept from a CLI's output.
const MAX_ERROR: usize = 2000;

impl Cli {
    fn name(self) -> &'static str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        }
    }

    fn product(self) -> &'static str {
        match self {
            Cli::Claude => "Claude Code",
            Cli::Codex => "Codex",
        }
    }

    /// The variable naming the binary, for one installed elsewhere.
    pub fn variable(self) -> &'static str {
        match self {
            Cli::Claude => "LOGBOOK_CLAUDE_BIN",
            Cli::Codex => "LOGBOOK_CODEX_BIN",
        }
    }

    /// What signs the CLI in, for whoever reads the error.
    fn login(self) -> &'static str {
        match self {
            Cli::Claude => {
                "run `claude auth login` in a terminal (on a server, `claude setup-token` \
                 and CLAUDE_CODE_OAUTH_TOKEN)"
            }
            Cli::Codex => "run `codex login` in a terminal",
        }
    }

    /// The model names offered when none is listed: Claude's aliases;
    /// Codex has no listing (its default model, or a name typed).
    pub fn models(self) -> Vec<String> {
        match self {
            Cli::Claude => ["haiku", "opus", "sonnet"].map(String::from).to_vec(),
            Cli::Codex => Vec::new(),
        }
    }
}

/// Folders the CLIs' installers put them in, beyond PATH: the native
/// installer's, npm's, Homebrew's.
fn known_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(home) = &home {
        for dir in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
        ] {
            dirs.push(home.join(dir));
        }
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs
}

/// Where `cli` is: its variable, else PATH, else the known folders.
pub fn binary(cli: Cli) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(cli.variable()).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd"]
            .iter()
            .map(|ext| format!("{}.{ext}", cli.name()))
            .collect()
    } else {
        vec![cli.name().to_string()]
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(known_dirs())
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
        .with_context(|| {
            format!(
                "the {} CLI ({}) is not installed on this computer, or not where it is looked \
                 for (PATH, ~/.local/bin, /opt/homebrew/bin, /usr/local/bin): install {}, or \
                 set {} to its path",
                cli.product(),
                cli.name(),
                cli.product(),
                cli.variable()
            )
        })
}

/// A command for `bin`, its folder and the known ones on its PATH.
fn command(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    let mut dirs: Vec<PathBuf> = bin.parent().map(Path::to_path_buf).into_iter().collect();
    dirs.extend(known_dirs());
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Ok(path) = std::env::join_paths(dirs) {
        command.env("PATH", path);
    }
    command
}

/// What a process wrote and how it ended.
#[derive(Debug)]
struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// `command` run with `input` on its stdin, killed after `timeout`.
fn run(mut command: Command, input: &[u8], timeout: Duration, what: &str) -> Result<Output> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {what}"))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let input = input.to_vec();
    // Written while the output is read: a prompt larger than the pipe
    // would wait for a reader otherwise.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(Box::new(child.stdout.take().context("no stdout")?));
    let stderr = read(Box::new(child.stderr.take().context("no stderr")?));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{what} did not answer within {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = writer.join();
    Ok(Output {
        success: status.success(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// An error if `cli` is missing or not signed in, saying how to sign in.
pub fn check(cli: Cli) -> Result<()> {
    let bin = binary(cli)?;
    let mut command = command(&bin);
    match cli {
        Cli::Claude => command.args(["auth", "status"]),
        Cli::Codex => command.args(["login", "status"]),
    };
    let output = run(command, b"", CHECK_TIMEOUT, cli.name())?;
    if !output.success {
        bail!(
            "{} is not signed in on this computer: {}",
            cli.product(),
            cli.login()
        );
    }
    Ok(())
}

/// A folder of one request's own, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "logbook-llm-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not make {}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The answer of `cli`'s model (`model`, or the CLI's default when empty)
/// to `user`, told `instructions`. Blocks for as long as the model writes.
pub fn complete(
    cli: Cli,
    model: &str,
    timeout: Duration,
    instructions: &str,
    user: &str,
) -> Result<String> {
    let bin = binary(cli)?;
    let scratch = Scratch::new()?;
    let answer = match cli {
        Cli::Claude => claude(&bin, &scratch.0, model, timeout, instructions, user),
        Cli::Codex => codex(&bin, &scratch.0, model, timeout, instructions, user),
    };
    // A failure for want of a sign-in says so, and how to sign in.
    answer.map_err(|error| match check(cli) {
        Err(signed_out) => signed_out,
        Ok(()) => error,
    })
}

#[derive(Deserialize)]
struct ClaudeResult {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
}

fn claude(
    bin: &Path,
    dir: &Path,
    model: &str,
    timeout: Duration,
    instructions: &str,
    user: &str,
) -> Result<String> {
    let system = dir.join("instructions.md");
    std::fs::write(&system, instructions)?;
    let mut command = command(bin);
    command.current_dir(dir).args([
        "-p",
        "--output-format",
        "json",
        "--no-session-persistence",
        // A model answering, nothing more: no tools, MCP servers, slash
        // commands, nor the user's settings and hooks.
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--setting-sources",
        "",
    ]);
    command.arg("--system-prompt-file").arg(&system);
    if !model.is_empty() {
        command.args(["--model", model]);
    }
    let output = run(command, user.as_bytes(), timeout, "claude")?;
    let parsed: Option<ClaudeResult> = serde_json::from_str(output.stdout.trim()).ok();
    match parsed {
        Some(ClaudeResult {
            is_error: false,
            result: Some(text),
        }) if output.success => Ok(text),
        Some(ClaudeResult {
            result: Some(text), ..
        }) => bail!("claude failed: {}", tail(&text)),
        _ => bail!("claude failed: {}", tail(&failure(&output))),
    }
}

fn codex(
    bin: &Path,
    dir: &Path,
    model: &str,
    timeout: Duration,
    instructions: &str,
    user: &str,
) -> Result<String> {
    let answer = dir.join("answer.md");
    let mut command = command(bin);
    command
        .current_dir(dir)
        .args([
            "exec",
            "--skip-git-repo-check",
            "--ephemeral",
            "--color",
            "never",
            "--sandbox",
            "read-only",
        ])
        .arg("--cd")
        .arg(dir)
        .arg("--output-last-message")
        .arg(&answer);
    if !model.is_empty() {
        command.args(["--model", model]);
    }
    command.arg("-");
    // No system prompt in codex exec: the instructions head the prompt.
    let prompt = format!("<instructions>\n{instructions}\n</instructions>\n\n{user}");
    let output = run(command, prompt.as_bytes(), timeout, "codex")?;
    if !output.success {
        bail!("codex failed: {}", tail(&failure(&output)));
    }
    std::fs::read_to_string(&answer).context("codex wrote no answer")
}

/// What a failed process said: its stderr, else its stdout.
fn failure(output: &Output) -> String {
    let said = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    match said.trim() {
        "" => "it exited with no message".into(),
        text => text.to_string(),
    }
}

/// The end of `text`, where a CLI's error is.
fn tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= MAX_ERROR {
        return text.to_string();
    }
    let start: String = text.chars().skip(count - MAX_ERROR).collect();
    format!("…{start}")
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A fake CLI: a shell script at a path of its own.
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_prompt_larger_than_a_pipe_goes_in_and_the_answer_comes_back() {
        let scratch = Scratch::new().unwrap();
        let bin = script(&scratch.0, "wc -c");
        let big = "x".repeat(300_000);
        let output = run(command(&bin), big.as_bytes(), Duration::from_secs(10), "wc").unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.trim(), "300000");
    }

    #[test]
    fn a_process_past_its_time_is_stopped() {
        let scratch = Scratch::new().unwrap();
        let bin = script(&scratch.0, "sleep 30");
        let started = Instant::now();
        let error = run(command(&bin), b"", Duration::from_millis(300), "sleep").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(error.to_string().contains("did not answer"), "{error}");
    }

    #[test]
    fn claude_s_answer_and_its_errors_are_read() {
        let scratch = Scratch::new().unwrap();
        let ok = script(
            &scratch.0,
            r#"cat > /dev/null; printf '%s\n' '{"type":"result","is_error":false,"result":"Summary\nOk."}'"#,
        );
        let answer = claude(&ok, &scratch.0, "", Duration::from_secs(10), "i", "u").unwrap();
        assert_eq!(answer, "Summary\nOk.");
        let refused = script(
            &scratch.0,
            r#"cat > /dev/null; echo '{"is_error":true,"result":"Not logged in"}'; exit 1"#,
        );
        let error = claude(&refused, &scratch.0, "", Duration::from_secs(10), "i", "u")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Not logged in"), "{error}");
    }

    #[test]
    fn codex_s_answer_is_its_last_message() {
        let scratch = Scratch::new().unwrap();
        // The file after --output-last-message gets the prompt it read.
        let bin = script(
            &scratch.0,
            r#"while [ "$1" != "--output-last-message" ]; do shift; done; cat > "$2""#,
        );
        let answer = codex(
            &bin,
            &scratch.0,
            "",
            Duration::from_secs(10),
            "Be brief.",
            "Hi",
        )
        .unwrap();
        assert!(answer.starts_with("<instructions>\nBe brief.\n</instructions>"));
        assert!(answer.ends_with("Hi"));
    }
}
