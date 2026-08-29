//! Package exactly the share-safe envelope for attaching to an issue.
//!
//! The privacy boundary is documented ("only `<MCPEVAL_HOME>/store/` is
//! safe to share"), but trust is stronger when the tool *produces* the
//! envelope instead of the operator hand-picking files. `mcpeval share`
//! copies the store subtree into a fresh directory, runs the redaction
//! sweep, and refuses to package anything the sweep flags. The salt,
//! manifests, and the SQLite index are outside the envelope by
//! construction: they are never copied.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::doctor;

pub struct ShareOptions {
    pub output: PathBuf,
    pub force: bool,
    pub include_probe_history: bool,
}

pub struct ShareSummary {
    pub directory: PathBuf,
    pub files: usize,
    pub notes_requiring_review: usize,
}

pub fn run(options: ShareOptions) -> anyhow::Result<ShareSummary> {
    let root = crate::store::Store::resolve_root(None);
    let store_dir = root.join("store");
    if !store_dir.is_dir() {
        bail!("nothing to share: {} does not exist", store_dir.display());
    }
    if options.output.exists() {
        let occupied = std::fs::read_dir(&options.output)?
            .filter_map(Result::ok)
            .next()
            .is_some();
        if occupied && !options.force {
            bail!(
                "{} is not empty; pass --force to replace it",
                options.output.display()
            );
        }
    } else {
        std::fs::create_dir_all(&options.output).context("creating share directory")?;
    }

    // Gate: the redaction sweep must come back clean before anything is
    // packaged. A finding means unredacted-looking text inside the store;
    // sharing it would defeat the boundary the envelope exists to honor.
    let report = doctor::check_redaction(&root)?;
    if !report.findings.is_empty() {
        for finding in &report.findings {
            eprintln!("{finding}");
        }
        bail!(
            "redaction sweep flagged {} file(s); run `mcpeval doctor --check-redaction`, \
             remove or fix the flagged records, and re-run `mcpeval share`",
            report.findings.len()
        );
    }

    let destination_store = options.output.join("store");
    let files = copy_tree(
        &store_dir,
        &destination_store,
        options.include_probe_history,
    )?;
    if files == 0 {
        bail!("the store contains no records to share");
    }

    let share_note = format!(
        "# mcpeval share envelope\n\n\
         This directory contains the share-safe subset of an MCPEVAL_HOME\n\
         capture root, produced by `mcpeval share`. Contents:\n\n\
         - `store/` — content-minimized JSONL records (the same boundary\n\
           documented in the README). Raw payloads, error prose, tool\n\
           descriptions, sessions, and credentials were never written.\n\n\
         Deliberately excluded (never copied):\n\n\
         - the fingerprint salt (`{}`) — makes template IDs invertible\n\
         - `index.db` and other derived databases\n\
         - manifest files, which may contain operational arguments\n\n\
         Redaction sweep: clean ({} JSONL files scanned).\n\
         {}\n\
         {}\n",
        crate::fingerprint::SALT_FILENAME,
        report.files,
        if report.notes_requiring_review > 0 {
            format!(
                "ATTENTION: {} annotation note(s) contain free-form agent prose — \
                 manually review or remove them before sharing (see doctor).",
                report.notes_requiring_review
            )
        } else {
            "Annotation notes: none present.".to_string()
        },
        "Always keep this envelope separate from any file that contains the salt."
    );
    std::fs::write(options.output.join("SHARE.md"), share_note).context("writing SHARE.md")?;

    Ok(ShareSummary {
        directory: options.output,
        files,
        notes_requiring_review: report.notes_requiring_review,
    })
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    include_probe_history: bool,
) -> anyhow::Result<usize> {
    std::fs::create_dir_all(destination)
        .with_context(|| format!("creating {}", destination.display()))?;
    let mut copied = 0usize;
    for entry in
        std::fs::read_dir(source).with_context(|| format!("reading {}", source.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == "probes" && !include_probe_history {
                // Trend history is share-safe but rarely relevant to a bug
                // report; keep the default envelope minimal.
                continue;
            }
            let target = destination.join(&name);
            std::fs::create_dir_all(&target).context("creating share subdirectory")?;
            copied += copy_tree(&path, &target, include_probe_history)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            std::fs::copy(&path, destination.join(&name))
                .with_context(|| format!("copying {}", path.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}
