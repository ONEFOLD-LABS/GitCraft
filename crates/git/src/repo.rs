use std::path::{Path, PathBuf};

use crate::path::bytes_to_path;
use crate::{GitError, GitRunner, Status};

/// An opened Git working tree.
#[derive(Debug, Clone)]
pub struct Repository {
    runner: GitRunner,
    workdir: PathBuf,
    git_dir: PathBuf,
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
        Ok(Repository {
            runner: runner.clone(),
            workdir: bytes_to_path(top),
            git_dir,
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
        let out = self.runner.read(
            &self.workdir,
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

/// Removes the one trailing newline git prints after a path (the path itself may end in other
/// whitespace).
fn strip_newline(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}
