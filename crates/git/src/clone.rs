//! Clone (issue #46): a repository from a checked URL into a new or empty folder, with
//! progress and cancellation. What a failed or cancelled clone left behind is removed, but only
//! what GitCraft created: a folder it made, or the contents of a folder that was empty.
//! Submodules are not cloned along (Git's default); the caller can say whether there are any.

use std::path::{Path, PathBuf};

use crate::network::network;
use crate::{Cancel, GitError, GitRunner, OnProgress, check_url, redact_url};

/// What to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneOptions {
    pub url: String,
    /// The new working tree (created; or an existing empty folder).
    pub dir: PathBuf,
    /// Check out this branch (or tag) instead of the remote's default.
    pub branch: Option<String>,
    /// Only the newest commits (a shallow clone).
    pub depth: Option<u32>,
}

/// The folder name `git clone` would choose: the URL's last part without `.git`.
pub fn clone_dir_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        .collect();
    if name.is_empty() || name == "." || name == ".." {
        "repository".to_owned()
    } else {
        name
    }
}

/// How the target folder was found.
enum Target {
    Created,
    WasEmpty,
}

impl GitRunner {
    /// Clones `options.url` into `options.dir` (`clone --progress [--branch=…] [--depth=…]
    /// --end-of-options <url> <dir>`). Fails with [`GitError::UnsafePath`] when the folder exists
    /// and is not empty. On failure or cancellation what the clone created is removed.
    pub fn clone_repository(
        &self,
        options: &CloneOptions,
        cancel: &Cancel,
        on_progress: OnProgress,
    ) -> Result<PathBuf, GitError> {
        let url = options.url.trim();
        check_url(url).map_err(|problem| GitError::InvalidUrl {
            url: redact_url(url),
            problem,
        })?;
        if let Some(b) = &options.branch
            && (b.is_empty() || b.starts_with('-') || b.chars().any(char::is_control))
        {
            return Err(GitError::InvalidValue(b.clone()));
        }
        if options.depth == Some(0) {
            return Err(GitError::InvalidValue("0".into()));
        }
        let dir = &options.dir;
        let parent = dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| GitError::UnsafePath(dir.display().to_string()))?;
        if !parent.is_dir() {
            return Err(GitError::UnsafePath(parent.display().to_string()));
        }
        let target = match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(dir).map_err(|e| GitError::Spawn(e.to_string()))?;
                Target::Created
            }
            Err(e) => return Err(GitError::Spawn(e.to_string())),
            Ok(meta) if meta.is_dir() && is_empty(dir)? => Target::WasEmpty,
            Ok(_) => return Err(GitError::UnsafePath(dir.display().to_string())),
        };
        let branch = options.branch.as_ref().map(|b| format!("--branch={b}"));
        let depth = options.depth.map(|d| format!("--depth={d}"));
        let mut args: Vec<&std::ffi::OsStr> = vec!["clone".as_ref(), "--progress".as_ref()];
        if let Some(b) = &branch {
            args.push(b.as_ref());
        }
        if let Some(d) = &depth {
            args.push(d.as_ref());
        }
        args.extend(["--end-of-options".as_ref(), url.as_ref(), dir.as_os_str()]);
        let result = self.write_progress(parent, &[], args, cancel, on_progress, &[]);
        match result {
            Ok(_) => Ok(dir.clone()),
            Err(e) => {
                // Remove only what the clone made.
                match target {
                    Target::Created => {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                    Target::WasEmpty => {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                let _ = match entry.file_type() {
                                    Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path),
                                    _ => std::fs::remove_file(&path),
                                };
                            }
                        }
                    }
                }
                Err(network(e))
            }
        }
    }
}

fn is_empty(dir: &Path) -> Result<bool, GitError> {
    let mut entries = std::fs::read_dir(dir).map_err(|e| GitError::Spawn(e.to_string()))?;
    Ok(entries.next().is_none())
}

/// Whether a working tree declares submodules (which a clone leaves empty).
pub fn has_submodules(workdir: &Path) -> bool {
    workdir.join(".gitmodules").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_from_urls() {
        for (url, name) in [
            ("https://example.org/team/projekt.git", "projekt"),
            ("https://example.org/team/projekt/", "projekt"),
            ("git@example.org:team/werkzeug.git", "werkzeug"),
            ("example.org:solo", "solo"),
            ("/srv/repos/alt.git", "alt"),
            ("C:\\repos\\neu", "neu"),
            ("https://example.org/", "example.org"),
            ("https://example.org/..", "repository"),
            ("", "repository"),
        ] {
            assert_eq!(clone_dir_name(url), name, "{url}");
        }
    }
}
