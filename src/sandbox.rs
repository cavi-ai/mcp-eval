//! A write-contained launch for stdio servers under `score --sandbox`. A
//! fresh directory is the server's HOME, TMPDIR and working directory; the
//! server inherits only PATH and locale variables, so credentials in the
//! environment never reach it; file writes outside the directory are denied;
//! network access is denied unless allowed. Seatbelt (`sandbox-exec`)
//! enforces the policy on macOS; other platforms refuse the flag.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::Command;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The only variables the server inherits.
const INHERITED: [&str; 4] = ["PATH", "LANG", "LC_ALL", "LC_CTYPE"];

/// Device files any process may write: the null sink, the terminal, and the
/// DTrace helper that runtimes such as Node open at start.
const WRITABLE_DEVICES: [&str; 4] = ["/dev/null", "/dev/zero", "/dev/tty", "/dev/dtracehelper"];

#[derive(Debug)]
pub struct Sandbox {
    root: PathBuf,
    network: bool,
}

impl Sandbox {
    /// A new, empty sandbox directory under the system temporary directory,
    /// removed when the sandbox is dropped.
    pub fn create(network: bool) -> anyhow::Result<Self> {
        if !cfg!(target_os = "macos") || !Path::new(SANDBOX_EXEC).is_file() {
            bail!("--sandbox requires macOS sandbox-exec, which this platform does not provide");
        }
        let base = std::env::temp_dir()
            .canonicalize()
            .context("resolving the temporary directory")?;
        let root = base.join(format!("mcpeval-sandbox-{}", uuid::Uuid::new_v4().simple()));
        // The path is quoted into the policy; refuse one that would need escaping.
        if root.to_str().is_none_or(|text| text.contains(['"', '\\'])) {
            bail!("the temporary directory path cannot be used in a sandbox policy");
        }
        let sandbox = Self { root, network };
        for directory in ["home", "tmp", "work"] {
            let path = sandbox.root.join(directory);
            std::fs::create_dir_all(&path)
                .with_context(|| format!("creating {}", path.display()))?;
        }
        Ok(sandbox)
    }

    pub fn network(&self) -> bool {
        self.network
    }

    /// The Seatbelt policy: the last matching rule wins, so every write is
    /// denied and then the sandbox directory and device sinks are allowed.
    fn policy(&self) -> String {
        let mut writable = format!("(subpath \"{}\")", self.root.display());
        for device in WRITABLE_DEVICES {
            writable.push_str(&format!(" (literal \"{device}\")"));
        }
        writable.push_str(" (regex #\"^/dev/fd/\")");
        let mut policy =
            format!("(version 1)(allow default)(deny file-write*)(allow file-write* {writable})");
        if !self.network {
            policy.push_str("(deny network*)");
        }
        policy
    }

    /// The process that runs `program` with `args` inside the sandbox.
    pub(crate) fn command(&self, program: &str, args: &[String]) -> Command {
        let home = self.root.join("home");
        let mut command = Command::new(SANDBOX_EXEC);
        command
            .arg("-p")
            .arg(self.policy())
            .arg(program)
            .args(args)
            .env_clear()
            .current_dir(self.root.join("work"));
        for name in INHERITED {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("TMPDIR", self.root.join("tmp"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("HOME", home);
        command
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best effort: a server that outlived its client may still hold files.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_policy_contains_writes_and_denies_network_unless_allowed() {
        let closed = Sandbox::create(false).unwrap();
        let root = closed.root.clone();
        let output = closed
            .command(
                "/bin/sh",
                &[
                    "-c".into(),
                    "echo x > \"$HOME/inside\" && echo inside; \
                     echo x > \"$0\" 2>/dev/null && echo escaped || echo contained"
                        .into(),
                    std::env::temp_dir()
                        .join(format!("mcpeval-escape-{}", uuid::Uuid::new_v4().simple()))
                        .display()
                        .to_string(),
                ],
            )
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "inside\ncontained\n"
        );
        assert!(root.join("home/inside").is_file());
        assert!(closed.policy().ends_with("(deny network*)"));
        assert!(!Sandbox::create(true).unwrap().policy().contains("network"));
        drop(closed);
        assert!(!root.exists());
    }

    #[test]
    fn the_server_inherits_only_path_and_locale() {
        let set = [
            "HOME",
            "TMPDIR",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
        ];
        let allowed = |name: &str| INHERITED.contains(&name) || set.contains(&name);
        let dropped: Vec<String> = std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .filter(|name| !allowed(name))
            .collect();
        assert!(!dropped.is_empty(), "the test needs a variable to drop");
        let sandbox = Sandbox::create(false).unwrap();
        let output = sandbox.command("/usr/bin/env", &[]).output().unwrap();
        let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
            .collect();
        assert!(names.iter().all(|name| allowed(name)), "{names:?}");
        assert!(names.iter().any(|name| name == "HOME"), "{names:?}");
    }
}
