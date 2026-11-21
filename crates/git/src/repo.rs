use std::path::{Path, PathBuf};

use crate::diff::{ChangedFile, FileDiff, parse_name_status, parse_numstat, whole_file_added};
use crate::log::{FORMAT, LogReader};
use crate::path::bytes_to_path;
use crate::refs::{REF_FORMAT, STASH_FORMAT};
use crate::{Commit, GitError, GitRunner, Oid, Ref, RepoPath, Stash, Status};
use std::ffi::OsString;

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

    /// Local branches, remote branches and tags, sorted by kind and name.
    pub fn refs(&self) -> Result<Vec<Ref>, GitError> {
        let out = self.runner.read_with(
            &self.workdir,
            &self.read_config,
            [
                "for-each-ref",
                REF_FORMAT,
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
        )?;
        let mut refs = Ref::parse_list(&out)?;
        refs.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        Ok(refs)
    }

    /// The stash entries, newest first.
    pub fn stashes(&self) -> Result<Vec<Stash>, GitError> {
        let out = self.runner.read_with(
            &self.workdir,
            &self.read_config,
            ["stash", "list", "-z", STASH_FORMAT],
        )?;
        Stash::parse_list(&out)
    }

    /// The full message of a commit.
    pub fn commit_message(&self, commit: &Commit) -> Result<String, GitError> {
        let out = self.runner.read_with(
            &self.workdir,
            &self.read_config,
            [
                "show",
                "-s",
                "--no-color",
                "--format=%B",
                commit.id.as_str(),
            ],
        )?;
        Ok(String::from_utf8_lossy(&out).trim_end().to_owned())
    }

    /// The files a commit changed, against its first parent (a root commit against nothing).
    pub fn commit_files(&self, commit: &Commit) -> Result<Vec<ChangedFile>, GitError> {
        let run = |format: &str| {
            let mut args = vec![
                "diff-tree",
                "-r",
                "-z",
                "--no-commit-id",
                "--find-renames",
                format,
            ];
            match commit.parents.first() {
                Some(parent) => args.push(parent.as_str()),
                None => args.push("--root"),
            }
            args.push(commit.id.as_str());
            self.runner
                .read_with(&self.workdir, &self.read_config, args)
        };
        let kinds = parse_name_status(&run("--name-status")?);
        parse_numstat(&run("--numstat")?, &kinds)
    }

    /// The diff of one file of a commit, against its first parent.
    pub fn commit_diff(&self, commit: &Commit, file: &ChangedFile) -> Result<FileDiff, GitError> {
        let mut args: Vec<OsString> = [
            "--literal-pathspecs",
            "diff-tree",
            "-p",
            "-r",
            "--no-color",
            "--no-commit-id",
            "--find-renames",
        ]
        .map(OsString::from)
        .to_vec();
        match commit.parents.first() {
            Some(parent) => args.push(parent.as_str().into()),
            None => args.push("--root".into()),
        }
        args.push(commit.id.as_str().into());
        args.push("--".into());
        args.extend(pathspec(file));
        let out = self
            .runner
            .read_with(&self.workdir, &self.read_config, args)?;
        Ok(FileDiff::parse(&out)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    /// The diff of a file in the working tree: index → working tree, or with `staged`
    /// HEAD → index.
    pub fn worktree_diff(
        &self,
        path: &RepoPath,
        old_path: Option<&RepoPath>,
        staged: bool,
    ) -> Result<FileDiff, GitError> {
        let mut args: Vec<OsString> = [
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
        ]
        .map(OsString::from)
        .to_vec();
        if staged {
            args.push("--cached".into());
        }
        args.push("--".into());
        args.push(path_arg(path));
        if let Some(old) = old_path {
            args.push(path_arg(old));
        }
        let out = self
            .runner
            .read_with(&self.workdir, &self.read_config, args)?;
        Ok(FileDiff::parse(&out)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    /// An untracked file shown as wholly added. Symbolic links are not followed (their target
    /// is shown); at most [`UNTRACKED_MAX`] bytes are read.
    pub fn untracked_diff(&self, path: &RepoPath) -> Result<FileDiff, GitError> {
        let full = path.to_path(&self.workdir);
        let io = |e: std::io::Error| GitError::Spawn(format!("{}: {e}", path.display()));
        let meta = std::fs::symlink_metadata(&full).map_err(io)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full).map_err(io)?;
            return Ok(whole_file_added(target.to_string_lossy().as_bytes()));
        }
        if !meta.is_file() {
            return Ok(FileDiff::default());
        }
        use std::io::Read;
        let mut content = Vec::new();
        std::fs::File::open(&full)
            .and_then(|f| f.take(UNTRACKED_MAX).read_to_end(&mut content))
            .map_err(io)?;
        let mut diff = whole_file_added(&content);
        diff.truncated |= meta.len() > UNTRACKED_MAX;
        Ok(diff)
    }

    /// The history of all branches, remote branches and tags (and a detached HEAD) in graph
    /// order, newest first, read in pages from one running `git log`. Empty for a repository
    /// without commits.
    pub fn log(&self) -> Result<LogReader, GitError> {
        let Some(revisions) = self.revisions()? else {
            return Ok(LogReader::new(None));
        };
        let mut args: Vec<OsString> = [
            "log",
            "-z",
            "--topo-order",
            "--no-color",
            "--decorate=no",
            FORMAT,
        ]
        .map(OsString::from)
        .to_vec();
        args.extend(revisions);
        let stream = self.runner.stream(&self.workdir, &self.read_config, args)?;
        Ok(LogReader::new(Some(stream)))
    }

    /// The revisions the history shows: all branches, remote branches and tags, and HEAD when
    /// it points at a commit. `None` for a repository without any commit.
    fn revisions(&self) -> Result<Option<Vec<OsString>>, GitError> {
        let head = match self.runner.read_with(
            &self.workdir,
            &self.read_config,
            ["rev-parse", "--verify", "--quiet", "HEAD"],
        ) {
            Ok(_) => true,
            Err(GitError::Failed { code: Some(1), .. }) => false,
            Err(e) => return Err(e),
        };
        if !head && self.refs()?.is_empty() {
            return Ok(None);
        }
        let mut args: Vec<OsString> = ["--branches", "--remotes", "--tags"]
            .map(OsString::from)
            .to_vec();
        if head {
            args.push("HEAD".into());
        }
        Ok(Some(args))
    }

    /// Commits of the history that match `text` (see [`SearchKind`]), streamed as they are
    /// found. The text is searched literally (no regular expressions) and never becomes an
    /// option: it is attached to its option (`--grep=…`, `-S…`) or given as a literal pathspec.
    pub fn search(&self, kind: SearchKind, text: &str) -> Result<SearchStream, GitError> {
        let Some(revisions) = self.revisions()? else {
            return Ok(SearchStream { stream: None });
        };
        let mut args: Vec<OsString> = vec![
            "--literal-pathspecs".into(),
            "log".into(),
            "-z".into(),
            "--format=%H".into(),
            "--no-color".into(),
        ];
        match kind {
            SearchKind::Message => {
                args.extend(["--regexp-ignore-case".into(), "--fixed-strings".into()]);
                args.push(format!("--grep={text}").into());
            }
            SearchKind::Author => {
                args.extend(["--regexp-ignore-case".into(), "--fixed-strings".into()]);
                args.push(format!("--author={text}").into());
            }
            SearchKind::Content => args.push(format!("-S{text}").into()),
            SearchKind::Path => {}
        }
        args.extend(revisions);
        if kind == SearchKind::Path {
            args.push("--".into());
            args.push(text.into());
        }
        let stream = self.runner.stream(&self.workdir, &self.read_config, args)?;
        Ok(SearchStream {
            stream: Some(stream),
        })
    }
}

/// What a history search looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    /// The commit message (case-insensitive).
    Message,
    /// Author name or e-mail (case-insensitive).
    Author,
    /// Commits that touched a path (a file or everything below a directory).
    Path,
    /// Commits that added or removed a string in a file's content (slow on large histories).
    Content,
}

/// Commit ids of a running search. Dropping it stops git.
pub struct SearchStream {
    stream: Option<crate::GitStream>,
}

impl SearchStream {
    /// Up to `n` further matches; fewer at the end (then [`SearchStream::is_done`]).
    pub fn next_batch(&mut self, n: usize) -> Result<Vec<Oid>, GitError> {
        let mut batch = Vec::new();
        while batch.len() < n {
            let Some(stream) = self.stream.as_mut() else {
                break;
            };
            match stream.next_record(0, 128)? {
                Some(rec) => {
                    let text = String::from_utf8_lossy(&rec);
                    let id = text.trim();
                    if !id.is_empty() {
                        batch.push(
                            Oid::parse(id)
                                .ok_or_else(|| GitError::Parse(format!("search: {id:?}")))?,
                        );
                    }
                }
                None => {
                    if let Some(stream) = self.stream.take() {
                        stream.finish()?;
                    }
                }
            }
        }
        Ok(batch)
    }

    pub fn is_done(&self) -> bool {
        self.stream.is_none()
    }
}

/// Largest untracked file read for display.
pub const UNTRACKED_MAX: u64 = 4 * 1024 * 1024;

/// A repository path as a command-line argument (after `--`, with `--literal-pathspecs`).
fn path_arg(path: &RepoPath) -> OsString {
    crate::path::bytes_to_path(path.as_bytes()).into_os_string()
}

/// The paths of a changed file: new path, and the old one for a rename.
fn pathspec(file: &ChangedFile) -> Vec<OsString> {
    let mut v = vec![path_arg(&file.path)];
    if let Some(old) = &file.old_path {
        v.push(path_arg(old));
    }
    v
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
