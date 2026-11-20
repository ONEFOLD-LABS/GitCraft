use std::path::{Path, PathBuf};

use crate::path::bytes_to_path;
use crate::{GitError, GitRunner, Status};

/// An opened Git working tree.
#[derive(Debug, Clone)]
pub struct Repository {
    runner: GitRunner,
    workdir: PathBuf,
    git_dir: PathBuf,
    /// `-c` overrides for every read: neutralise programs the repository's own configuration
    /// names (see [`neutralised_filters`]).
    read_config: Vec<String>,
}

impl Repository {
    /// Opens the working tree that contains `path` (a directory or a file in it). Its top level
    /// becomes the working directory of every later call.
    pub fn open(runner: &GitRunner, path: &Path) -> Result<Repository, GitError> {
        let dir = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        if !dir.is_dir() {
            return Err(GitError::NotARepository(path.to_path_buf()));
        }
        let out = runner.read(
            dir,
            [
                "rev-parse",
                "--is-bare-repository",
                "--is-inside-work-tree",
                "--absolute-git-dir",
            ],
        )?;
        let mut lines = out.splitn(3, |b| *b == b'\n');
        let bare = lines.next() == Some(b"true");
        let inside = lines.next() == Some(b"true");
        let git_dir = lines
            .next()
            .map(strip_newline)
            .filter(|p| !p.is_empty())
            .map(bytes_to_path)
            .ok_or_else(|| GitError::Parse("rev-parse: no git dir".into()))?;
        if bare {
            return Err(GitError::Bare(git_dir));
        }
        if !inside {
            // E.g. inside the .git directory itself.
            return Err(GitError::NotARepository(path.to_path_buf()));
        }
        let top = runner.read(dir, ["rev-parse", "--show-toplevel"])?;
        let top = strip_newline(&top);
        if top.is_empty() {
            return Err(GitError::Parse("rev-parse: no top level".into()));
        }
        let workdir = bytes_to_path(top);
        let read_config = neutralised_filters(runner, &workdir)?;
        Ok(Repository {
            runner: runner.clone(),
            workdir,
            git_dir,
            read_config,
        })
    }

    /// Top level of the working tree.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The repository's git directory (for a linked worktree: its own one).
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Branch, upstream and all changed, conflicted and untracked paths (every untracked file,
    /// not only directories; renames detected).
    pub fn status(&self) -> Result<Status, GitError> {
        let out = self.runner.read_with(
            &self.workdir,
            &self.read_config,
            [
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
                "--find-renames",
            ],
        )?;
        Status::parse(&out)
    }
}

/// Overrides that switch off the content filters (`filter.<driver>.clean`, `.smudge`,
/// `.process`) defined in the repository's own configuration (local or worktree scope).
///
/// `git status` runs a clean filter for files whose stat data changed, which is every file of a
/// freshly extracted archive: a repository received as a zip could otherwise run a program as
/// soon as it is opened. Filters from the user's global or system configuration are the user's
/// own and stay active.
fn neutralised_filters(runner: &GitRunner, workdir: &Path) -> Result<Vec<String>, GitError> {
    let out = match runner.read(
        workdir,
        [
            "config",
            "--show-scope",
            "--name-only",
            "--get-regexp",
            r"^filter\.",
        ],
    ) {
        Ok(out) => out,
        // Exit code 1: no such keys.
        Err(GitError::Failed { code: Some(1), .. }) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut drivers: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let Some((scope, key)) = line.split_once('\t') else {
            continue;
        };
        if scope != "local" && scope != "worktree" {
            continue;
        }
        let Some(name) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
            .map(|(name, _)| name)
        else {
            continue;
        };
        // `-c` splits at the first `=`: such a name could not be overridden reliably.
        if name.contains('=') {
            return Err(GitError::UnsafeConfig(key.to_owned()));
        }
        if !drivers.iter().any(|d| d == name) {
            drivers.push(name.to_owned());
        }
    }
    Ok(drivers
        .iter()
        .flat_map(|d| {
            ["clean=", "smudge=", "process=", "required=false"].map(|v| format!("filter.{d}.{v}"))
        })
        .collect())
}

/// Removes the one trailing newline git prints after a path (the path itself may end in other
/// whitespace).
fn strip_newline(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}
