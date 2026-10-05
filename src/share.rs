//! Produce a checked snapshot and publish it as one envelope.
use crate::doctor;
use anyhow::{bail, Context};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct ShareOptions {
    pub output: PathBuf,
    pub force: bool,
    pub include_probe_history: bool,
    pub include_annotation_notes: bool,
}
pub struct ShareSummary {
    pub directory: PathBuf,
    pub files: usize,
    pub notes_requiring_review: usize,
}
pub enum ShareOutcome {
    Packaged(ShareSummary),
    Refused { flagged: usize },
}
struct Snapshot {
    path: PathBuf,
    bytes: String,
    annotations: bool,
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run(options: ShareOptions) -> anyhow::Result<ShareOutcome> {
    let root = crate::store::Store::resolve_root(None);
    let store = root.join("store");
    if !store.is_dir() {
        bail!("nothing to share: store does not exist");
    }
    if std::fs::symlink_metadata(&store)?.file_type().is_symlink() {
        bail!("share refuses a symlinked store");
    }
    let store = store.canonicalize()?;
    let output = resolved_output(&options.output)?;
    if output.starts_with(&store) || root.canonicalize()?.starts_with(&output) {
        bail!("share output overlaps the capture store");
    }
    validate_output(&output, options.force)?;

    // Take locked snapshots once, scan them, then export these same bytes.
    let mut snapshots = Vec::new();
    snapshot_tree(&store, Path::new(""), &options, &mut snapshots)?;
    if snapshots.is_empty() {
        bail!("the store contains no records to share");
    }
    let mut flagged = 0;
    let mut notes = 0;
    for snapshot in &snapshots {
        let (lines, count) = doctor::scan_body(&snapshot.bytes, snapshot.annotations);
        notes += count;
        flagged += usize::from(!lines.is_empty());
        for line in lines {
            eprintln!("{}:{line}", store.join(&snapshot.path).display());
        }
    }
    if flagged > 0 {
        return Ok(ShareOutcome::Refused { flagged });
    }

    let parent = output.parent().context("share output has no parent")?;
    std::fs::create_dir_all(parent).context("creating share parent")?;
    let stage = Staging(parent.join(format!(".mcpeval-share-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&stage.0).context("creating share staging directory")?;
    for snapshot in &snapshots {
        let destination = stage.0.join("store").join(&snapshot.path);
        std::fs::create_dir_all(destination.parent().expect("store file has a parent"))?;
        std::fs::write(destination, &snapshot.bytes).context("writing checked snapshot")?;
    }
    let notice = if options.include_annotation_notes {
        format!("ATTENTION: {notes} annotation note(s) explicitly included; review before sharing.")
    } else {
        "Annotation prose: excluded. Typed annotation metadata is retained.".into()
    };
    std::fs::write(
        stage.0.join("SHARE.md"),
        format!(
            "# mcpeval share envelope\n\nChecked JSONL snapshots from store/. \
         Excludes the fingerprint salt, index.db, and manifests.\n\n\
         Redaction sweep: clean ({} JSONL files scanned). This heuristic scan \
         does not prove arbitrary metadata is non-sensitive.\n\n{notice}\n\n\
         Keep this envelope separate from any file that contains the salt.\n",
            snapshots.len()
        ),
    )?;
    publish(&stage.0, &output, options.force)?;
    Ok(ShareOutcome::Packaged(ShareSummary {
        directory: options.output,
        files: snapshots.len(),
        notes_requiring_review: notes,
    }))
}

fn resolved_output(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    // symlink_metadata also detects dangling output symlinks.
    while match std::fs::symlink_metadata(ancestor) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    } {
        missing.push(
            ancestor
                .file_name()
                .context("invalid share output")?
                .to_owned(),
        );
        ancestor = ancestor.parent().context("invalid share output")?;
    }
    if missing.is_empty()
        && std::fs::symlink_metadata(ancestor)?
            .file_type()
            .is_symlink()
    {
        bail!("share refuses a symlinked output");
    }
    let mut resolved = ancestor.canonicalize()?;
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn validate_output(output: &Path, force: bool) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(output) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("share output must be a directory, not a symlink");
            }
            if !force && std::fs::read_dir(output)?.next().transpose()?.is_some() {
                bail!("share output is not empty; pass --force to replace it");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn snapshot_tree(
    root: &Path,
    relative: &Path,
    options: &ShareOptions,
    snapshots: &mut Vec<Snapshot>,
) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(root.join(relative))?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("share refuses symlinks in the store");
        }
        let path = relative.join(entry.file_name());
        if kind.is_dir() {
            if path == Path::new("probes") && !options.include_probe_history {
                continue;
            }
            snapshot_tree(root, &path, options, snapshots)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            let mut open = OpenOptions::new();
            open.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.custom_flags(nix::libc::O_NOFOLLOW);
            }
            let mut file = open.open(entry.path()).context("opening store snapshot")?;
            file.lock_shared().context("locking store snapshot")?;
            let mut body = String::new();
            file.read_to_string(&mut body)
                .context("reading store snapshot")?;
            file.unlock().context("unlocking store snapshot")?;
            let annotations = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("annotations-"));
            let mut bytes = String::new();
            for (index, line) in body.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let mut record: serde_json::Value = serde_json::from_str(line).map_err(|_| {
                    anyhow::anyhow!(
                        "invalid JSON record in {} line {}",
                        path.display(),
                        index + 1
                    )
                })?;
                if !record.is_object() {
                    bail!("store records must be JSON objects");
                }
                if annotations && !options.include_annotation_notes {
                    if let Some(note) = record.get_mut("note") {
                        *note = serde_json::json!("");
                    }
                }
                bytes.push_str(&serde_json::to_string(&record)?);
                bytes.push('\n');
            }
            snapshots.push(Snapshot {
                path,
                bytes,
                annotations,
            });
        }
    }
    Ok(())
}

fn publish(stage: &Path, output: &Path, force: bool) -> anyhow::Result<()> {
    validate_output(output, force)?;
    let backup = output
        .parent()
        .expect("validated output parent")
        .join(format!(".mcpeval-share-backup-{}", uuid::Uuid::new_v4()));
    let exists = output.try_exists()?;
    if exists {
        std::fs::rename(output, &backup).context("preserving prior envelope")?;
    }
    if let Err(error) = std::fs::rename(stage, output) {
        if exists {
            std::fs::rename(&backup, output).context("restoring prior envelope")?;
        }
        return Err(error).context("publishing share envelope");
    }
    if exists {
        std::fs::remove_dir_all(backup).context("removing replaced envelope")?;
    }
    Ok(())
}
