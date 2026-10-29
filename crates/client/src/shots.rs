//! Screenshots in a meeting's notes. The app takes them while a meeting is
//! recorded into `<data>/screenshots` and puts a line in the notes that
//! names the file, `![Screenshot 10:32](screenshot-20261029-103205.png)`.
//! When the meeting ends, the files its notes name are copied into its
//! pending folder (`keep`), and from there to the history
//! (`<data>/meetings/<id>/`) once it is published. On the brainy page they
//! are image blocks, in the place of their lines (`pieces`): brainy's
//! Markdown keeps only absolute http(s) images, so a file is uploaded on
//! its own (`Brainy::upload_image`).
//!
//! A screenshot no meeting's notes name stays in `<data>/screenshots` until
//! the next recording starts (`clear`): a meeting of the series still
//! finishing may yet copy it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where the app writes the screenshots of the meeting being recorded.
pub fn staging(data: &Path) -> PathBuf {
    data.join("screenshots")
}

/// A screenshot's file name, made from when it was taken; `-2`, `-3`… for
/// more than one in a second.
pub fn new_name(dir: &Path, taken: chrono::DateTime<chrono::Local>) -> String {
    let stem = format!("screenshot-{}", taken.format("%Y%m%d-%H%M%S"));
    let mut name = format!("{stem}.png");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem}-{n}.png");
        n += 1;
    }
    name
}

/// Whether `name` can be a screenshot's file: a bare PNG name, nothing that
/// walks out of its folder.
pub fn is_name(name: &str) -> bool {
    name.ends_with(".png")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A part of Markdown that holds screenshots: text, or a line that is only
/// a screenshot.
#[derive(Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Image { name: String, caption: String },
}

/// The screenshot a line is, if it is one alone: `![caption](name.png)`,
/// or a list's item of it (`- ![caption](name.png)`), where a model may
/// put it.
fn image(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let line = ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| line.strip_prefix(m))
        .unwrap_or(line);
    let rest = line.trim_start().strip_prefix("![")?;
    let (caption, rest) = rest.split_once("](")?;
    let name = rest.strip_suffix(')')?;
    is_name(name).then(|| (name.to_string(), caption.to_string()))
}

/// The Markdown cut at its screenshots' lines.
pub fn pieces(markdown: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    for line in markdown.lines() {
        match image(line) {
            Some((name, caption)) => {
                if !text.trim().is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                text.clear();
                out.push(Piece::Image { name, caption });
            }
            None => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    if !text.trim().is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// The screenshots the Markdown shows, in its order.
pub fn names(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(image)
        .map(|(name, _)| name)
        .collect()
}

/// `report` with the screenshots of `notes` it left out added at its end:
/// the model is asked to keep them in place, and one that drops a
/// screenshot does not lose it.
pub fn with_missing(report: &str, notes: &str) -> String {
    let shown = names(report);
    let missing: Vec<String> = notes
        .lines()
        .filter(|l| image(l).is_some_and(|(name, _)| !shown.contains(&name)))
        .map(|l| l.trim().to_string())
        .collect();
    if missing.is_empty() {
        return report.to_string();
    }
    format!("{}\n\n{}\n", report.trim_end(), missing.join("\n\n"))
}

/// Copies the screenshots `notes` names from `from` into `to`; one that
/// is not there is left out with a warning (its line stays in the notes).
pub fn keep(from: &Path, to: &Path, notes: &str) -> Result<()> {
    for name in names(notes) {
        let source = from.join(&name);
        let target = to.join(&name);
        if target.exists() {
            continue;
        }
        if !source.exists() {
            tracing::warn!("the screenshot {name} is not in {}", from.display());
            continue;
        }
        std::fs::create_dir_all(to)?;
        std::fs::copy(&source, &target)
            .with_context(|| format!("could not copy {}", source.display()))?;
    }
    Ok(())
}

/// Removes the screenshots of earlier recordings.
pub fn clear(data: &Path) {
    let _ = std::fs::remove_dir_all(staging(data));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_cut_at_their_screenshots() {
        let notes =
            "- budget\n![Screenshot 10:32](screenshot-1.png)\n- dates\n  ![x](../etc.png)\n";
        assert_eq!(
            pieces(notes),
            [
                Piece::Text("- budget\n".into()),
                Piece::Image {
                    name: "screenshot-1.png".into(),
                    caption: "Screenshot 10:32".into()
                },
                Piece::Text("- dates\n  ![x](../etc.png)\n".into()),
            ]
        );
        assert_eq!(names(notes), ["screenshot-1.png"]);
    }

    #[test]
    fn a_screenshot_in_a_list_is_one() {
        let report = "- Point\n  - ![a](one.png)\n";
        assert_eq!(names(report), ["one.png"]);
        assert_eq!(with_missing(report, "![a](one.png)"), report);
    }

    #[test]
    fn a_report_keeps_every_screenshot() {
        let notes = "![a](one.png)\n- point\n![b](two.png)";
        let report = "## Notes\n\n- point\n\n![a](one.png)";
        assert_eq!(
            with_missing(report, notes),
            "## Notes\n\n- point\n\n![a](one.png)\n\n![b](two.png)\n"
        );
        assert_eq!(with_missing(report, "- point"), report);
    }

    #[test]
    fn a_name_stays_in_its_folder() {
        assert!(is_name("screenshot-20261029-103205-2.png"));
        assert!(!is_name("../x.png"));
        assert!(!is_name("a/b.png"));
        assert!(!is_name(".png"));
        assert!(!is_name("x.jpg"));
    }
}
