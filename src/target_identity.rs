//! Private launch-artifact binding for finding verification.
//! This observes executable and file-argument bytes, not dependency closure.
use anyhow::{bail, Context};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LaunchIdentity {
    executable: PathBuf,
    artifacts: Vec<(PathBuf, String)>,
}

fn digest(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path).context("opening verification launch artifact")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "verification launch artifact must be a regular file"
    );
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(crate) fn observe(command: &[String]) -> anyhow::Result<Option<LaunchIdentity>> {
    let Some(program) = command.first() else {
        return Ok(None);
    };
    let given = Path::new(program);
    let executable = if given.components().count() > 1 || given.is_absolute() {
        std::path::absolute(given).context("resolving verification executable")?
    } else {
        let mut resolved = None;
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            let path = directory.join(given);
            #[cfg(windows)]
            let path = if path.is_file() {
                path
            } else {
                path.with_extension("exe")
            };
            if path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if path.metadata()?.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                resolved = Some(std::path::absolute(path)?);
                break;
            }
        }
        resolved.context("verification executable is unavailable")?
    };
    // Preserve the invocation path: venv interpreters and other executables
    // may select their environment from argv[0]. Bind the referent separately.
    let mut paths = vec![executable
        .canonicalize()
        .context("resolving verification executable artifact")?];
    for argument in command.iter().skip(1) {
        let path = Path::new(argument);
        if path.is_file() {
            paths.push(path.canonicalize()?);
        }
    }
    paths.sort();
    paths.dedup();
    let artifacts = paths
        .into_iter()
        .map(|path| Ok((path.clone(), digest(&path)?)))
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(LaunchIdentity {
        executable,
        artifacts,
    }))
}

impl LaunchIdentity {
    pub(crate) fn pin(&self, command: &mut [String]) {
        command[0] = self.executable.to_string_lossy().into_owned();
    }
    pub(crate) fn unchanged(&self, command: &[String]) -> anyhow::Result<()> {
        if observe(command)?.as_ref() != Some(self) {
            bail!("verification launch artifacts changed; no lifecycle credit recorded");
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn pinning_an_interpreter_preserves_its_virtual_environment() {
        let root = std::env::temp_dir().join(format!("mcpeval-venv-{}", uuid::Uuid::new_v4()));
        let created = Command::new("python3")
            .args(["-m", "venv", "--without-pip", "--symlinks"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
        let invocation = root.join("bin/python");
        let mut command = vec![
            invocation.to_str().unwrap().to_owned(),
            "-c".into(),
            "import sys; print(sys.prefix)".into(),
        ];
        let identity = observe(&command).unwrap().unwrap();
        identity.pin(&mut command);
        let output = Command::new(&command[0])
            .args(&command[1..])
            .output()
            .unwrap();
        assert!(output.status.success());
        let observed = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let matches = observed.canonicalize().unwrap() == root.canonicalize().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            matches,
            "launch escaped the virtual environment: {observed:?}"
        );
    }
}
