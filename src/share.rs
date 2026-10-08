//! Produce a checked snapshot and publish it as one envelope.
use crate::doctor;
use anyhow::{bail, Context};
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, Write};
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
#[derive(Default)]
struct SnapshotSummary {
    files: usize,
    flagged: usize,
    notes: usize,
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

    let parent = output.parent().context("share output has no parent")?;
    std::fs::create_dir_all(parent).context("creating share parent")?;
    let stage_path = parent.join(format!(".mcpeval-share-{}", uuid::Uuid::new_v4()));
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(&stage_path)
        .context("creating share staging directory")?;
    // Own cleanup only after successfully creating this directory.
    let stage = Staging(stage_path);
    let mut snapshots = SnapshotSummary::default();
    snapshot_tree(&store, Path::new(""), &options, &stage.0, &mut snapshots)?;
    if snapshots.files == 0 {
        bail!("the store contains no records to share");
    }
    if snapshots.flagged > 0 {
        return Ok(ShareOutcome::Refused {
            flagged: snapshots.flagged,
        });
    }
    let notice = if options.include_annotation_notes {
        format!(
            "ATTENTION: {} annotation note(s) explicitly included; review before sharing.",
            snapshots.notes
        )
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
            snapshots.files
        ),
    )?;
    publish(&stage.0, &output, options.force)?;
    Ok(ShareOutcome::Packaged(ShareSummary {
        directory: options.output,
        files: snapshots.files,
        notes_requiring_review: snapshots.notes,
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
    stage: &Path,
    snapshots: &mut SnapshotSummary,
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
            snapshot_tree(root, &path, options, stage, snapshots)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            let mut open = OpenOptions::new();
            open.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.custom_flags(nix::libc::O_NOFOLLOW);
            }
            let file = open.open(entry.path()).context("opening store snapshot")?;
            file.lock_shared().context("locking store snapshot")?;
            let mut reader = BufReader::new(file);
            let annotations = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("annotations-"));
            let destination = stage.join("store").join(&path);
            std::fs::create_dir_all(destination.parent().expect("store file has a parent"))?;
            let mut writer = BufWriter::new(std::fs::File::create(destination)?);
            let mut buffer = Vec::new();
            let mut source_line = 0;
            let mut normalized_line = 0;
            let mut flagged = false;
            loop {
                if crate::jsonl::read_line(&mut reader, &mut buffer)
                    .context("reading store snapshot")?
                    == 0
                {
                    break;
                }
                source_line += 1;
                let line = std::str::from_utf8(&buffer).context("store snapshot is not UTF-8")?;
                if line.trim().is_empty() {
                    continue;
                }
                let mut record: serde_json::Value = serde_json::from_str(line).map_err(|_| {
                    anyhow::anyhow!(
                        "invalid JSON record in {} line {}",
                        path.display(),
                        source_line
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
                let mut bytes = serde_json::to_string(&record)?;
                bytes.push('\n');
                anyhow::ensure!(
                    bytes.len() <= crate::jsonl::MAX_RECORD_BYTES,
                    "normalized journal record exceeds 4 MiB"
                );
                normalized_line += 1;
                // Scan exactly the normalized bytes to be published. Never
                // write a flagged record, even into the private staging tree.
                let (lines, notes) = doctor::scan_body(&bytes, annotations);
                snapshots.notes += notes;
                if lines.is_empty() {
                    writer
                        .write_all(bytes.as_bytes())
                        .context("writing checked snapshot")?;
                } else {
                    flagged = true;
                    eprintln!("{}:{normalized_line}", root.join(&path).display());
                }
            }
            writer.flush().context("writing checked snapshot")?;
            reader
                .into_inner()
                .unlock()
                .context("unlocking store snapshot")?;
            snapshots.files += 1;
            snapshots.flagged += usize::from(flagged);
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
