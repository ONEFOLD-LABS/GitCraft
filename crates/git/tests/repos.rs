//! The Git layer against real temporary repositories, isolated from the user's and the system's
//! git configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gitcraft_git::{
    Cancel, Change, CommandLog, Conflict, GitError, GitRunner, Head, Identity, NameError, RepoPath,
    Repository, State, Status,
};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    /// A fresh repository with branch `main` and no commits.
    fn new() -> Fixture {
        let f = Fixture {
            tmp: tempfile::tempdir().unwrap(),
        };
        fs::write(f.global_config(), "").unwrap();
        fs::create_dir(f.repo()).unwrap();
        f.git(&["init", "-q", "-b", "main"]);
        f
    }

    fn repo(&self) -> PathBuf {
        self.tmp.path().join("repo")
    }

    fn global_config(&self) -> PathBuf {
        self.tmp.path().join("gitconfig")
    }

    fn command(&self, dir: &Path) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", self.global_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@invalid");
        cmd
    }

    /// Runs git in the repository and expects success.
    fn git(&self, args: &[&str]) {
        let out = self.command(&self.repo()).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Runs git and returns whether it succeeded (for commands expected to fail, e.g. a merge
    /// with conflicts).
    fn git_may_fail(&self, args: &[&str]) -> bool {
        self.command(&self.repo())
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.repo().join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn runner(&self) -> GitRunner {
        GitRunner::new()
            .env("GIT_CONFIG_GLOBAL", self.global_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
    }

    fn open(&self) -> Repository {
        Repository::open(&self.runner(), &self.repo()).unwrap()
    }

    fn status(&self) -> Status {
        self.open().status().unwrap()
    }
}

fn names<'a>(
    it: impl Iterator<Item = (&'a gitcraft_git::FileChange, Change)>,
) -> Vec<(String, Change)> {
    it.map(|(f, c)| (f.path.display(), c)).collect()
}

#[test]
fn installed_git_is_supported() {
    assert!(GitRunner::new().version().is_ok());
}

#[test]
fn a_plain_directory_is_not_a_repository() {
    let f = Fixture::new();
    let plain = f.tmp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    assert!(matches!(
        Repository::open(&f.runner(), &plain),
        Err(GitError::NotARepository(_))
    ));
    assert!(matches!(
        Repository::open(&f.runner(), &f.tmp.path().join("missing")),
        Err(GitError::NotARepository(_))
    ));
}

#[test]
fn a_bare_repository_is_reported() {
    let f = Fixture::new();
    let bare = f.tmp.path().join("bare.git");
    let out = f
        .command(f.tmp.path())
        .args(["init", "-q", "--bare"])
        .arg(&bare)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(matches!(
        Repository::open(&f.runner(), &bare),
        Err(GitError::Bare(_))
    ));
}

#[test]
fn opening_a_subdirectory_or_file_finds_the_top_level() {
    let f = Fixture::new();
    f.write("a/b/file.txt", "x");
    let canon = |p: &Path| fs::canonicalize(p).unwrap();
    for start in [f.repo().join("a/b"), f.repo().join("a/b/file.txt")] {
        let repo = Repository::open(&f.runner(), &start).unwrap();
        assert_eq!(canon(repo.workdir()), canon(&f.repo()));
        assert_eq!(canon(repo.git_dir()), canon(&f.repo().join(".git")));
    }
}

#[test]
fn a_fresh_repository_has_an_unborn_branch() {
    let f = Fixture::new();
    let s = f.status();
    assert_eq!(
        s.head,
        Head::Branch {
            name: "main".into(),
            commit: None
        }
    );
    assert!(s.is_clean());
    assert_eq!(s.upstream, None);
}

#[test]
fn staged_unstaged_untracked_and_renamed_files() {
    let f = Fixture::new();
    f.write("keep.txt", "1\n");
    f.write("change.txt", "1\n");
    f.write("both.txt", "1\n");
    f.write("gone.txt", "1\n");
    f.write("old.txt", "some content that survives the rename\n");
    f.commit_all("first");

    f.write("change.txt", "2\n");
    f.write("both.txt", "2\n");
    f.git(&["add", "both.txt"]);
    f.write("both.txt", "3\n");
    fs::remove_file(f.repo().join("gone.txt")).unwrap();
    f.git(&["mv", "old.txt", "new.txt"]);
    f.write("added.txt", "new\n");
    f.git(&["add", "added.txt"]);
    f.write("deep/dir/untracked.txt", "u\n");
    f.write(".gitignore", "*.log\n");
    f.write("ignored.log", "i\n");

    let s = f.status();
    assert!(matches!(&s.head, Head::Branch { name, commit: Some(_) } if name == "main"));
    assert_eq!(
        names(s.staged()),
        [
            ("added.txt".into(), Change::Added),
            ("both.txt".into(), Change::Modified),
            ("new.txt".into(), Change::Renamed),
        ]
    );
    let renamed = s
        .files
        .iter()
        .find(|x| x.path.display() == "new.txt")
        .unwrap();
    assert_eq!(
        renamed.orig_path.as_ref().map(|p| p.display()),
        Some("old.txt".into())
    );
    assert_eq!(
        names(s.unstaged()),
        [
            ("both.txt".into(), Change::Modified),
            ("change.txt".into(), Change::Modified),
            ("gone.txt".into(), Change::Deleted),
        ]
    );
    let untracked: Vec<String> = s.untracked().map(|x| x.path.display()).collect();
    assert_eq!(untracked, [".gitignore", "deep/dir/untracked.txt"]);
    // Ignored files are not listed (no --ignored).
    assert!(!s.files.iter().any(|x| x.state == State::Ignored));
}

#[test]
fn unicode_and_spaces_in_file_names() {
    let f = Fixture::new();
    f.write("Grüße aus Köln.txt", "x");
    f.write("日本語/ファイル.md", "x");
    let s = f.status();
    let mut untracked: Vec<String> = s.untracked().map(|x| x.path.display()).collect();
    untracked.sort();
    assert_eq!(untracked, ["Grüße aus Köln.txt", "日本語/ファイル.md"]);
    let file = s.untracked().next().unwrap();
    assert!(file.path.to_path(f.open().workdir()).exists());
}

#[cfg(unix)]
#[test]
fn newline_in_a_file_name() {
    let f = Fixture::new();
    f.write("two\nlines.txt", "x");
    let s = f.status();
    let file = s.untracked().next().unwrap();
    assert_eq!(file.path.as_bytes(), b"two\nlines.txt");
    assert_eq!(file.path.display(), "two\\nlines.txt");
}

#[test]
fn detached_head() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("one");
    f.write("a.txt", "2");
    f.commit_all("two");
    f.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    assert!(matches!(f.status().head, Head::Detached { .. }));
}

#[test]
fn ahead_behind_and_gone_upstream() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("base");
    f.git(&["checkout", "-q", "-b", "feature"]);
    f.git(&["branch", "-q", "--set-upstream-to", "main"]);
    f.write("b.txt", "1");
    f.commit_all("on feature");
    f.git(&["checkout", "-q", "main"]);
    f.write("c.txt", "1");
    f.commit_all("on main 1");
    f.write("d.txt", "1");
    f.commit_all("on main 2");
    f.git(&["checkout", "-q", "feature"]);

    let up = f.status().upstream.unwrap();
    assert_eq!(up.name, "main");
    assert_eq!(up.ahead_behind, Some((1, 2)));

    f.git(&["branch", "-q", "-D", "main"]);
    let up = f.status().upstream.unwrap();
    assert_eq!(up.ahead_behind, None, "upstream branch is gone");
}

#[test]
fn merge_conflict() {
    let f = Fixture::new();
    f.write("a.txt", "base\n");
    f.commit_all("base");
    f.git(&["checkout", "-q", "-b", "other"]);
    f.write("a.txt", "other\n");
    f.commit_all("other");
    f.git(&["checkout", "-q", "main"]);
    f.write("a.txt", "main\n");
    f.commit_all("main");
    assert!(!f.git_may_fail(&["merge", "-q", "other"]));
    let s = f.status();
    let conflicts: Vec<_> = s.conflicts().map(|(x, c)| (x.path.display(), c)).collect();
    assert_eq!(conflicts, [("a.txt".into(), Conflict::BothModified)]);
}

/// A repository's `core.fsmonitor` names a program git would start for `status`. GitCraft
/// switches it off for every call, so opening a hostile repository runs nothing.
#[cfg(unix)]
#[test]
fn repository_config_cannot_start_a_program_on_status() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("base");
    let marker = f.tmp.path().join("fsmonitor-ran");
    let hook = f.tmp.path().join("hook.sh");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    f.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);

    f.status();
    assert!(!marker.exists(), "GitCraft must not run core.fsmonitor");

    // The check is meaningful: plain git does run the program.
    f.git(&["status", "--porcelain"]);
    assert!(marker.exists(), "plain git status should have run the hook");
}

/// A content filter from the repository's own configuration would run during `status` for
/// every stat-dirty file (all files of an extracted archive). GitCraft switches such filters off.
#[cfg(unix)]
#[test]
fn repository_filters_cannot_run_on_status() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write("a.txt", "hello\n");
    f.commit_all("base");
    let marker = f.tmp.path().join("filter-ran");
    let filter = f.tmp.path().join("filter.sh");
    fs::write(
        &filter,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
    f.write(".gitattributes", "* filter=evil\n");
    f.git(&["config", "filter.evil.clean", filter.to_str().unwrap()]);
    f.git(&["config", "filter.evil.process", filter.to_str().unwrap()]);
    // Make every tracked file stat-dirty, as after extracting an archive.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(f.repo().join("a.txt"))
        .unwrap()
        .set_modified(later)
        .unwrap();

    f.status();
    assert!(
        !marker.exists(),
        "GitCraft must not run the repository's filter"
    );

    // The check is meaningful: plain git runs the filter.
    f.git(&["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "plain git status should have run the filter"
    );
}

#[test]
fn filter_names_that_cannot_be_overridden_are_refused() {
    let f = Fixture::new();
    f.git(&["config", "filter.a=b.clean", "cat"]);
    assert!(matches!(
        Repository::open(&f.runner(), &f.repo()),
        Err(GitError::UnsafeConfig(_))
    ));
}

#[test]
fn environment_cannot_redirect_git_to_another_repository() {
    let a = Fixture::new();
    a.write("a.txt", "1");
    a.commit_all("base");
    // Against b's empty index, a.txt would show up as untracked.
    let b = Fixture::new();
    // GIT_DIR inherited from the parent process must not leak into GitCraft's calls: run the
    // check in a child of this test binary that inherits it.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "redirect_child",
            "--nocapture",
            "--include-ignored",
        ])
        .env("GITCRAFT_TEST_REPO", a.repo())
        .env("GITCRAFT_TEST_GLOBAL", a.global_config())
        .env("GIT_DIR", b.repo().join(".git"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Runs inside the child process of the test above, with `GIT_DIR` pointing at another
/// repository.
#[test]
#[ignore = "only runs as the child of environment_cannot_redirect_git_to_another_repository"]
fn redirect_child() {
    let (Some(repo), Some(global)) = (
        std::env::var_os("GITCRAFT_TEST_REPO"),
        std::env::var_os("GITCRAFT_TEST_GLOBAL"),
    ) else {
        return;
    };
    let runner = GitRunner::new()
        .env("GIT_CONFIG_GLOBAL", global)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let s = Repository::open(&runner, Path::new(&repo))
        .unwrap()
        .status()
        .unwrap();
    assert!(s.is_clean(), "status came from the GIT_DIR repository");
}

#[test]
fn log_of_a_fresh_repository_is_empty() {
    let f = Fixture::new();
    let mut log = f.open().log().unwrap();
    assert!(log.next_page(10).unwrap().is_empty());
    assert!(log.is_done());
}

#[test]
fn log_reads_pages_newest_first_with_merges() {
    let f = Fixture::new();
    for i in 0..5 {
        f.write("a.txt", &i.to_string());
        f.commit_all(&format!("c{i}"));
    }
    f.git(&["checkout", "-q", "-b", "side", "HEAD~2"]);
    f.write("b.txt", "side");
    f.commit_all("on side");
    f.git(&["checkout", "-q", "main"]);
    f.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);

    let mut log = f.open().log().unwrap();
    let mut all = Vec::new();
    loop {
        let page = log.next_page(2).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 2);
        all.extend(page);
    }
    assert!(log.is_done());
    assert_eq!(all.len(), 7);
    assert_eq!(all[0].subject, "merge side");
    assert_eq!(all[0].parents.len(), 2);
    assert_eq!(all.last().unwrap().subject, "c0");
    assert!(all.last().unwrap().parents.is_empty());
    // Topological order: every commit comes before its parents.
    for (i, c) in all.iter().enumerate() {
        for p in &c.parents {
            let at = all.iter().position(|x| &x.id == p).unwrap();
            assert!(at > i, "{} before its parent", c.subject);
        }
    }
}

#[test]
fn dropping_a_log_halfway_stops_git() {
    let f = Fixture::new();
    for i in 0..30 {
        f.write("a.txt", &i.to_string());
        f.commit_all(&format!("c{i}"));
    }
    let mut log = f.open().log().unwrap();
    assert_eq!(log.next_page(3).unwrap().len(), 3);
    drop(log); // must return promptly, not wait for the rest
}

#[test]
fn refs_and_stashes() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("base");
    f.git(&["tag", "-a", "v1", "-m", "release"]);
    f.git(&["tag", "light"]);
    f.git(&["branch", "feature"]);
    // A remote: a bare clone, fetched back as origin.
    let bare = f.tmp.path().join("origin.git");
    let out = f
        .command(f.tmp.path())
        .args(["clone", "-q", "--bare"])
        .arg(f.repo())
        .arg(&bare)
        .output()
        .unwrap();
    assert!(out.status.success());
    f.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
    f.git(&["fetch", "-q", "origin"]);
    f.git(&["branch", "-q", "--set-upstream-to", "origin/main"]);
    f.write("a.txt", "2");
    f.commit_all("ahead");
    f.write("a.txt", "dirty");
    f.git(&["stash", "push", "-q", "-m", "parked"]);

    let repo = f.open();
    let refs = repo.refs().unwrap();
    let names: Vec<(gitcraft_git::RefKind, &str)> =
        refs.iter().map(|r| (r.kind, r.name.as_str())).collect();
    use gitcraft_git::RefKind::*;
    assert_eq!(
        names,
        [
            (Local, "feature"),
            (Local, "main"),
            (Remote, "origin/feature"),
            (Remote, "origin/main"),
            (Tag, "light"),
            (Tag, "v1"),
        ]
    );
    let main = refs.iter().find(|r| r.name == "main").unwrap();
    assert!(main.is_head);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!(
        main.tracking,
        Some(gitcraft_git::Tracking::AheadBehind(1, 0))
    );
    let v1 = refs.iter().find(|r| r.name == "v1").unwrap();
    let light = refs.iter().find(|r| r.name == "light").unwrap();
    assert_eq!(
        v1.target, light.target,
        "annotated tag peeled to its commit"
    );

    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes.len(), 1);
    assert_eq!(stashes[0].selector, "stash@{0}");
    assert!(stashes[0].message.contains("parked"));

    // The log covers the remote branches and tags too, but not the stash commits.
    let mut log = repo.log().unwrap();
    let all = log.next_page(100).unwrap();
    assert_eq!(all.len(), 2);
    assert!(!all.iter().any(|c| c.id == stashes[0].id));
}

fn all_commits(repo: &Repository) -> Vec<gitcraft_git::Commit> {
    let mut log = repo.log().unwrap();
    let mut all = Vec::new();
    loop {
        let page = log.next_page(100).unwrap();
        if page.is_empty() {
            return all;
        }
        all.extend(page);
    }
}

#[test]
fn files_and_diffs_of_commits() {
    use gitcraft_git::{FileStatus, LineKind};
    let f = Fixture::new();
    f.write("a.txt", "one\ntwo\nthree\n");
    f.write("Grüße mit Leerzeichen.txt", "hallo\n");
    f.commit_all("root");
    f.write("a.txt", "one\nTWO\nthree\nfour\n");
    fs::write(f.repo().join("bin.dat"), [0u8, 1, 2, 3, 0]).unwrap();
    f.git(&["mv", "Grüße mit Leerzeichen.txt", "neu.txt"]);
    f.commit_all("second");
    f.git(&["checkout", "-q", "-b", "side", "HEAD~1"]);
    f.write("side.txt", "s\n");
    f.commit_all("side");
    f.git(&["checkout", "-q", "main"]);
    f.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);

    let repo = f.open();
    let all = all_commits(&repo);
    let by = |s: &str| all.iter().find(|c| c.subject == s).unwrap().clone();

    let root = by("root");
    let files = repo.commit_files(&root).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|x| x.status == FileStatus::Added));
    assert_eq!(repo.commit_message(&root).unwrap(), "root");

    let second = by("second");
    let files = repo.commit_files(&second).unwrap();
    let get = |name: &str| files.iter().find(|x| x.path.display() == name).unwrap();
    assert_eq!(get("a.txt").status, FileStatus::Modified);
    assert_eq!(get("a.txt").lines, Some((2, 1)));
    assert_eq!(get("bin.dat").lines, None, "binary");
    assert_eq!(get("bin.dat").status, FileStatus::Added);
    let renamed = get("neu.txt");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert_eq!(
        renamed.old_path.as_ref().unwrap().display(),
        "Grüße mit Leerzeichen.txt"
    );

    let diff = repo.commit_diff(&second, get("a.txt")).unwrap();
    assert_eq!((diff.added(), diff.removed()), (2, 1));
    let kinds: Vec<LineKind> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&LineKind::Removed) && kinds.contains(&LineKind::Added));
    assert!(repo.commit_diff(&second, get("bin.dat")).unwrap().binary);
    assert!(
        repo.commit_diff(&root, &repo.commit_files(&root).unwrap()[0])
            .unwrap()
            .new_file
    );

    // A merge is shown against its first parent: only what the side branch brought.
    let merge = by("merge");
    let files = repo.commit_files(&merge).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path.display(), "side.txt");
}

#[test]
fn diffs_of_the_working_tree() {
    let f = Fixture::new();
    // Names git would read as pathspec patterns: `[a].txt` is a glob that also matches `a.txt`.
    // `:(glob)*` is pathspec magic; Windows forbids `:` and `*` in file names, so it only exists
    // elsewhere.
    let magic_name = cfg!(not(windows)).then_some(":(glob)*");
    f.write("a.txt", "1\n2\n");
    f.write("[a].txt", "literal name\n");
    if let Some(m) = magic_name {
        f.write(m, "literal name\n");
    }
    f.commit_all("base");
    f.write("a.txt", "1\nzwei\n");
    f.git(&["add", "a.txt"]);
    f.write("a.txt", "1\nzwei\ndrei\n");
    f.write("[a].txt", "changed\n");
    if let Some(m) = magic_name {
        f.write(m, "changed\n");
    }
    f.write("neu.txt", "x\ny");

    let repo = f.open();
    let path = |s: &str| gitcraft_git::RepoPath::from_bytes(s.as_bytes());
    let staged = repo.worktree_diff(&path("a.txt"), None, true).unwrap();
    assert_eq!((staged.added(), staged.removed()), (1, 1));
    let unstaged = repo.worktree_diff(&path("a.txt"), None, false).unwrap();
    assert_eq!((unstaged.added(), unstaged.removed()), (1, 0));
    // Names that look like patterns are taken literally: only that file.
    let glob = repo.worktree_diff(&path("[a].txt"), None, false).unwrap();
    assert_eq!((glob.added(), glob.removed()), (1, 1));
    if let Some(m) = magic_name {
        let magic = repo.worktree_diff(&path(m), None, false).unwrap();
        assert_eq!((magic.added(), magic.removed()), (1, 1));
    }
    let untracked = repo.untracked_diff(&path("neu.txt")).unwrap();
    assert_eq!(untracked.added(), 2);
    assert!(untracked.hunks[0].lines[1].no_newline);
}

#[cfg(unix)]
#[test]
fn untracked_symlinks_are_not_followed() {
    let f = Fixture::new();
    let secret = f.tmp.path().join("outside.txt");
    fs::write(&secret, "secret outside the repository\n").unwrap();
    std::os::unix::fs::symlink(&secret, f.repo().join("link")).unwrap();
    let diff = f
        .open()
        .untracked_diff(&gitcraft_git::RepoPath::from_bytes(b"link"))
        .unwrap();
    let text: Vec<u8> = diff.hunks[0].lines[0].text.clone();
    assert_eq!(
        text,
        secret.to_string_lossy().as_bytes(),
        "shows the link target, not the content"
    );
}

/// The working-tree diff runs clean filters like status does; GitCraft switches them off.
#[cfg(unix)]
#[test]
fn repository_filters_cannot_run_on_diff() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write("a.txt", "hello\n");
    f.commit_all("base");
    let marker = f.tmp.path().join("filter-ran");
    let filter = f.tmp.path().join("filter.sh");
    fs::write(
        &filter,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
    f.write(".gitattributes", "* filter=evil\n");
    f.git(&["config", "filter.evil.clean", filter.to_str().unwrap()]);
    f.write("a.txt", "changed\n");
    let repo = f.open();
    repo.worktree_diff(&gitcraft_git::RepoPath::from_bytes(b"a.txt"), None, false)
        .unwrap();
    assert!(
        !marker.exists(),
        "GitCraft must not run the repository's filter on diff"
    );
}

#[test]
fn history_search() {
    use gitcraft_git::SearchKind::*;
    let f = Fixture::new();
    f.write("src/a.rs", "fn a() {}\n");
    f.commit_all("Parser anlegen");
    f.write("docs/plan.md", "Plan\n");
    f.commit_all("Plan schreiben (v1.*)");
    f.write("src/a.rs", "fn a() { let geheim = 42; }\n");
    f.commit_all("--all ist kein Schalter");
    let out = f
        .command(&f.repo())
        .args(["commit", "-q", "--allow-empty", "-m", "Leerer Commit"])
        .env("GIT_AUTHOR_NAME", "Zoe Andere")
        .env("GIT_AUTHOR_EMAIL", "zoe@invalid")
        .output()
        .unwrap();
    assert!(out.status.success());

    let repo = f.open();
    let all = all_commits(&repo);
    let subject =
        |id: &gitcraft_git::Oid| all.iter().find(|c| &c.id == id).unwrap().subject.clone();
    let find = |kind, text: &str| -> Vec<String> {
        let mut s = repo.search(kind, text).unwrap();
        let mut ids = Vec::new();
        loop {
            let batch = s.next_batch(2).unwrap();
            if batch.is_empty() {
                break;
            }
            ids.extend(batch);
        }
        assert!(s.is_done());
        let mut subjects: Vec<String> = ids.iter().map(subject).collect();
        subjects.sort();
        subjects
    };
    assert_eq!(
        find(Message, "PARSER"),
        ["Parser anlegen"],
        "case-insensitive"
    );
    assert_eq!(
        find(Message, "(v1.*)"),
        ["Plan schreiben (v1.*)"],
        "literal, not a regex"
    );
    assert_eq!(find(Message, "v1.+"), Vec::<String>::new());
    assert_eq!(
        find(Message, "--all"),
        ["--all ist kein Schalter"],
        "text is not an option"
    );
    assert_eq!(find(Author, "zoe"), ["Leerer Commit"]);
    assert_eq!(
        find(Path, "src"),
        ["--all ist kein Schalter", "Parser anlegen"]
    );
    assert_eq!(find(Path, "docs/plan.md"), ["Plan schreiben (v1.*)"]);
    assert_eq!(find(Content, "geheim"), ["--all ist kein Schalter"]);
    assert_eq!(
        find(Content, "-S"),
        Vec::<String>::new(),
        "a leading dash stays text"
    );
}

#[test]
fn search_in_an_empty_repository_finds_nothing() {
    let f = Fixture::new();
    let mut s = f
        .open()
        .search(gitcraft_git::SearchKind::Message, "x")
        .unwrap();
    assert!(s.next_batch(10).unwrap().is_empty());
    assert!(s.is_done());
}

fn paths(names: &[&str]) -> Vec<RepoPath> {
    names
        .iter()
        .map(|n| RepoPath::from_bytes(n.as_bytes()))
        .collect()
}

fn staged_names(s: &Status) -> Vec<String> {
    s.staged().map(|(f, _)| f.path.display()).collect()
}

#[test]
fn stage_and_unstage_paths() {
    let f = Fixture::new();
    f.write("a.txt", "1\n");
    f.write("b.txt", "1\n");
    f.write("gone.txt", "1\n");
    f.commit_all("first");
    f.write("a.txt", "2\n");
    f.write("b.txt", "2\n");
    f.write("new.txt", "n\n");
    fs::remove_file(f.repo().join("gone.txt")).unwrap();
    let repo = f.open();
    let c = Cancel::new();

    repo.stage(&paths(&["a.txt", "new.txt", "gone.txt"]), &c)
        .unwrap();
    let s = repo.status().unwrap();
    assert_eq!(staged_names(&s), ["a.txt", "gone.txt", "new.txt"]);
    assert!(s.unstaged().any(|(x, _)| x.path.display() == "b.txt"));

    repo.unstage(&paths(&["a.txt", "gone.txt"]), &c).unwrap();
    assert_eq!(staged_names(&repo.status().unwrap()), ["new.txt"]);
    assert_eq!(fs::read_to_string(f.repo().join("a.txt")).unwrap(), "2\n");

    repo.stage_all(&c).unwrap();
    assert_eq!(
        staged_names(&repo.status().unwrap()),
        ["a.txt", "b.txt", "gone.txt", "new.txt"]
    );
    repo.unstage_all(&c).unwrap();
    assert!(staged_names(&repo.status().unwrap()).is_empty());
    // An empty list changes nothing (git add --all without paths would stage everything).
    repo.stage(&[], &c).unwrap();
    assert!(staged_names(&repo.status().unwrap()).is_empty());
}

#[test]
fn unstage_on_an_unborn_branch() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.write("b.txt", "1");
    let repo = f.open();
    let c = Cancel::new();
    repo.stage_all(&c).unwrap();
    repo.unstage(&paths(&["a.txt"]), &c).unwrap();
    assert_eq!(staged_names(&repo.status().unwrap()), ["b.txt"]);
    repo.unstage_all(&c).unwrap();
    assert!(staged_names(&repo.status().unwrap()).is_empty());
    assert!(f.repo().join("a.txt").exists() && f.repo().join("b.txt").exists());
}

/// File names that look like options or pathspec magic are staged as the files they name and
/// nothing else (docs/security.md T2).
#[cfg(unix)]
#[test]
fn hostile_file_names_stay_file_names() {
    let f = Fixture::new();
    let hostile = [
        "--upload-pack=touch x",
        "-rf",
        ":(glob)*",
        "*",
        "a b",
        "new\nline",
        ":!a b",
    ];
    for n in hostile {
        f.write(n, "x");
    }
    f.write("bystander.txt", "x");
    let repo = f.open();
    let c = Cancel::new();
    for n in hostile {
        repo.stage(&paths(&[n]), &c).unwrap();
        let staged = staged_names(&repo.status().unwrap());
        assert!(
            staged.contains(&RepoPath::from_bytes(n.as_bytes()).display()),
            "{n:?} staged: {staged:?}"
        );
        assert!(
            !staged.contains(&"bystander.txt".to_owned()),
            "{n:?} staged more"
        );
        repo.unstage(&paths(&[n]), &c).unwrap();
        assert!(staged_names(&repo.status().unwrap()).is_empty(), "{n:?}");
    }
    assert!(!f.repo().join("x").exists(), "no program ran");
}

#[test]
fn branch_names_are_checked() {
    let f = Fixture::new();
    let repo = f.open();
    assert_eq!(repo.check_branch_name("feature/x-1"), Ok(()));
    assert_eq!(repo.check_branch_name(""), Err(NameError::Empty));
    assert_eq!(
        repo.check_branch_name("--upload-pack=touch x"),
        Err(NameError::LeadingDash)
    );
    assert_eq!(repo.check_branch_name("-rf"), Err(NameError::LeadingDash));
    assert_eq!(repo.check_branch_name("a b"), Err(NameError::Whitespace));
    assert_eq!(repo.check_branch_name("a\nb"), Err(NameError::Whitespace));
    for bad in [
        "a..b", "a~1", "x^", "c:d", "q?", "s*", "[x", "w\\x", "a@{1}", "x.lock", "x/", "/x", "@",
        "HEAD",
    ] {
        assert_eq!(
            repo.check_branch_name(bad),
            Err(NameError::Rules),
            "{bad:?}"
        );
    }
}

#[test]
fn every_call_is_logged_with_its_arguments() {
    let f = Fixture::new();
    f.write("a b.txt", "1");
    let log = Arc::new(CommandLog::new());
    let repo = Repository::open(&f.runner().with_log(Arc::clone(&log)), &f.repo()).unwrap();
    repo.status().unwrap();
    repo.stage(&paths(&["a b.txt"]), &Cancel::new()).unwrap();
    let entries = log.entries();
    let status = entries
        .iter()
        .find(|e| e.args.iter().any(|a| a == "status"))
        .unwrap();
    assert!(!status.write && status.code == Some(0));
    assert_eq!(status.args[..2], ["-c", "core.fsmonitor=false"]);
    let add = entries.last().unwrap();
    assert!(add.write);
    assert_eq!(
        add.posix_line(),
        "git -c core.fsmonitor=false --literal-pathspecs add --all --pathspec-from-file=- --pathspec-file-nul"
    );
    assert_eq!(add.cwd, repo.workdir());
}

#[test]
fn a_cancelled_write_does_not_start() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    let repo = f.open();
    let c = Cancel::new();
    c.cancel();
    assert_eq!(repo.stage_all(&c), Err(GitError::Cancelled));
    assert!(staged_names(&repo.status().unwrap()).is_empty());
}

#[test]
fn trust_report_lists_program_settings_and_hooks() {
    let f = Fixture::new();
    let repo = f.open();
    let clean = repo.trust_report().unwrap();
    assert!(
        clean.is_empty(),
        "a fresh repository has only sample hooks: {clean:?}"
    );

    f.git(&["config", "core.sshCommand", "ssh -i key"]);
    f.git(&["config", "diff.bin.textconv", "hexdump"]);
    f.git(&["config", "filter.x.clean", "tr a b"]);
    f.git(&["config", "core.autocrlf", "false"]);
    let extra = f.tmp.path().join("extra.cfg");
    fs::write(&extra, "[credential]\n\thelper = store\n").unwrap();
    f.git(&["config", "include.path", extra.to_str().unwrap()]);
    // The user's own (global) settings are not the repository's.
    fs::write(f.global_config(), "[core]\n\tpager = less\n").unwrap();
    let hooks = f.repo().join(".git/hooks");
    fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(hooks.join("pre-commit"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    let r = f.open().trust_report().unwrap();
    let keys: Vec<&str> = r.settings.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "core.sshcommand",
            "credential.helper",
            "diff.bin.textconv",
            "filter.x.clean",
            "include.path"
        ]
    );
    assert_eq!(r.hooks, ["pre-commit"]);
    assert!(r.hooks_dir.ends_with(".git/hooks"));

    // core.hooksPath moves the hooks directory.
    fs::create_dir(f.repo().join(".husky")).unwrap();
    f.git(&["config", "core.hooksPath", ".husky"]);
    let r = f.open().trust_report().unwrap();
    assert_eq!(r.hooks_dir, f.open().workdir().join(".husky"));
    assert!(r.hooks.is_empty());
    assert!(r.settings.iter().any(|s| s.key == "core.hookspath"));
}

fn subjects(repo: &Repository) -> Vec<String> {
    all_commits(repo).into_iter().map(|c| c.subject).collect()
}

/// A repository with an identity in its own configuration.
fn with_identity() -> (Fixture, Repository) {
    let f = Fixture::new();
    let repo = f.open();
    repo.set_identity("Test Person", "test@invalid", false, &Cancel::new())
        .unwrap();
    (f, repo)
}

#[test]
fn identity_is_read_and_set() {
    let f = Fixture::new();
    let repo = f.open();
    assert_eq!(repo.identity().unwrap(), Identity::default());
    assert!(!repo.identity().unwrap().is_complete());
    let c = Cancel::new();
    assert!(matches!(
        repo.set_identity("A\nB", "a@b", false, &c),
        Err(GitError::InvalidValue(_))
    ));
    assert!(matches!(
        repo.set_identity(" ", "a@b", false, &c),
        Err(GitError::InvalidValue(_))
    ));
    // A value that looks like an option is stored as the value.
    repo.set_identity("-n Name", "--global", false, &c).unwrap();
    let id = repo.identity().unwrap();
    assert_eq!(id.name.as_deref(), Some("-n Name"));
    assert_eq!(id.email.as_deref(), Some("--global"));
    assert_eq!(
        fs::read_to_string(f.global_config()).unwrap(),
        "",
        "only local"
    );
    repo.set_identity("Global", "g@invalid", true, &c).unwrap();
    assert!(
        fs::read_to_string(f.global_config())
            .unwrap()
            .contains("g@invalid")
    );
}

#[test]
fn commit_on_an_unborn_branch_and_after() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("a.txt", "1");
    repo.stage_all(&c).unwrap();
    repo.commit("Erster Commit\n\n#42 bleibt stehen\n\n\n", false, false, &c)
        .unwrap();
    let commits = all_commits(&repo);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].author.name, "Test Person");
    assert_eq!(
        repo.commit_message(&commits[0]).unwrap(),
        "Erster Commit\n\n#42 bleibt stehen",
        "lines starting with # are kept, trailing blank lines dropped"
    );
    assert!(repo.status().unwrap().is_clean());

    // Nothing staged: git refuses.
    assert!(matches!(
        repo.commit("Leer", false, false, &c),
        Err(GitError::Failed { .. })
    ));
    // An empty message is refused.
    f.write("a.txt", "2");
    repo.stage_all(&c).unwrap();
    assert!(matches!(
        repo.commit("  \n\n", false, false, &c),
        Err(GitError::Failed { .. })
    ));
    assert_eq!(subjects(&repo), ["Erster Commit"]);
}

#[cfg(unix)]
#[test]
fn hooks_run_and_can_be_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let (f, repo) = with_identity();
    let hook = f.repo().join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\necho 'Bitte erst testen' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let c = Cancel::new();
    f.write("a.txt", "1");
    repo.stage_all(&c).unwrap();
    match repo.commit("Mit Hook", false, false, &c) {
        Err(GitError::Failed { stderr, .. }) => assert!(stderr.contains("Bitte erst testen")),
        other => panic!("{other:?}"),
    }
    assert!(subjects(&repo).is_empty());
    repo.commit("Ohne Hook", false, true, &c).unwrap();
    assert_eq!(subjects(&repo), ["Ohne Hook"]);

    // A hook that passes: its output comes back.
    fs::write(&hook, "#!/bin/sh\necho 'alles gut' >&2\n").unwrap();
    f.write("a.txt", "2");
    repo.stage_all(&c).unwrap();
    let out = repo.commit("Zweiter", false, false, &c).unwrap();
    assert!(out.stderr.contains("alles gut"), "{out:?}");
}

#[test]
fn amend_message_and_content_and_pushed_warning() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    assert_eq!(repo.head_for_amend().unwrap(), None, "unborn");
    f.write("a.txt", "1");
    repo.stage_all(&c).unwrap();
    repo.commit("Tippfeler", false, false, &c).unwrap();
    assert_eq!(
        repo.head_for_amend().unwrap(),
        Some(("Tippfeler".to_owned(), false))
    );
    // Message only.
    repo.commit("Tippfehler", true, false, &c).unwrap();
    assert_eq!(subjects(&repo), ["Tippfehler"]);
    // Content.
    f.write("b.txt", "neu");
    repo.stage_all(&c).unwrap();
    repo.commit("Tippfehler", true, false, &c).unwrap();
    let commits = all_commits(&repo);
    assert_eq!(commits.len(), 1);
    let files: Vec<String> = repo
        .commit_files(&commits[0])
        .unwrap()
        .into_iter()
        .map(|x| x.path.display())
        .collect();
    assert_eq!(files, ["a.txt", "b.txt"]);

    // Once a remote-tracking branch has it, amending needs a force push.
    let remote = f.tmp.path().join("remote.git");
    let out = f
        .command(f.tmp.path())
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .output()
        .unwrap();
    assert!(out.status.success());
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    f.git(&["push", "-q", "origin", "main"]);
    assert_eq!(
        repo.head_for_amend().unwrap(),
        Some(("Tippfehler".to_owned(), true))
    );
}

/// Thirty lines; `changes` replaces line n (1-based) with "n neu".
fn numbered(changes: &[usize], eol: &str) -> String {
    (1..=30)
        .map(|n| {
            if changes.contains(&n) {
                format!("{n} neu{eol}")
            } else {
                format!("{n}{eol}")
            }
        })
        .collect()
}

fn hunk_fixture(eol: &str) -> (Fixture, Repository, RepoPath) {
    let (f, repo) = with_identity();
    f.write("f.txt", &numbered(&[], eol));
    repo.stage_all(&Cancel::new()).unwrap();
    repo.commit("Anfang", false, false, &Cancel::new()).unwrap();
    f.write("f.txt", &numbered(&[2, 15, 28], eol));
    (f, repo, RepoPath::from_bytes(b"f.txt"))
}

/// The changed lines (hunk, line) of hunk `h`.
fn hunk_lines(d: &gitcraft_git::FileDiff, h: usize) -> Vec<(usize, usize)> {
    d.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != gitcraft_git::LineKind::Context)
        .map(|(i, _)| (h, i))
        .collect()
}

fn staged_text(f: &Fixture) -> String {
    let out = f
        .command(&f.repo())
        .args(["show", ":f.txt"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn stage_first_middle_and_last_hunk() {
    for (h, line) in [(0, 2), (1, 15), (2, 28)] {
        let (f, repo, p) = hunk_fixture("\n");
        let shown = repo.worktree_diff(&p, None, false).unwrap();
        assert_eq!(shown.hunks.len(), 3);
        repo.stage_lines(
            &p,
            None,
            false,
            &shown,
            &hunk_lines(&shown, h),
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!(staged_text(&f), numbered(&[line], "\n"), "hunk {h}");
        // And back.
        let staged = repo.worktree_diff(&p, None, true).unwrap();
        repo.stage_lines(
            &p,
            None,
            true,
            &staged,
            &hunk_lines(&staged, 0),
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!(staged_text(&f), numbered(&[], "\n"));
    }
}

#[test]
fn hunks_of_crlf_files_keep_their_line_endings() {
    let (f, repo, p) = hunk_fixture("\r\n");
    let shown = repo.worktree_diff(&p, None, false).unwrap();
    repo.stage_lines(
        &p,
        None,
        false,
        &shown,
        &hunk_lines(&shown, 1),
        &Cancel::new(),
    )
    .unwrap();
    assert_eq!(staged_text(&f), numbered(&[15], "\r\n"));
}

#[test]
fn hunks_at_an_end_without_newline() {
    let (f, repo) = with_identity();
    f.write("f.txt", "a\nb\nc");
    repo.stage_all(&Cancel::new()).unwrap();
    repo.commit("Anfang", false, false, &Cancel::new()).unwrap();
    f.write("f.txt", "a\nb\nC");
    let p = RepoPath::from_bytes(b"f.txt");
    let shown = repo.worktree_diff(&p, None, false).unwrap();
    repo.stage_lines(
        &p,
        None,
        false,
        &shown,
        &hunk_lines(&shown, 0),
        &Cancel::new(),
    )
    .unwrap();
    assert_eq!(staged_text(&f), "a\nb\nC");
}

#[test]
fn a_file_changed_since_display_is_not_touched() {
    let (f, repo, p) = hunk_fixture("\n");
    let shown = repo.worktree_diff(&p, None, false).unwrap();
    f.write("f.txt", &numbered(&[2, 15, 20, 28], "\n"));
    assert_eq!(
        repo.stage_lines(
            &p,
            None,
            false,
            &shown,
            &hunk_lines(&shown, 0),
            &Cancel::new()
        ),
        Err(GitError::Stale)
    );
    assert_eq!(staged_text(&f), numbered(&[], "\n"));
}

#[test]
fn stage_single_lines() {
    let (f, repo) = with_identity();
    f.write("f.txt", "a\nb\nc\nd\n");
    repo.stage_all(&Cancel::new()).unwrap();
    repo.commit("Anfang", false, false, &Cancel::new()).unwrap();
    // One hunk: b removed, B1 and B2 added, d removed.
    f.write("f.txt", "a\nB1\nB2\nc\n");
    let p = RepoPath::from_bytes(b"f.txt");
    let c = Cancel::new();
    let shown = repo.worktree_diff(&p, None, false).unwrap();
    let pos = |text: &str, kind| {
        let i = shown.hunks[0]
            .lines
            .iter()
            .position(|l| l.text == text.as_bytes() && l.kind == kind)
            .unwrap();
        (0, i)
    };
    use gitcraft_git::LineKind::{Added, Removed};
    // Added only.
    repo.stage_lines(&p, None, false, &shown, &[pos("B2", Added)], &c)
        .unwrap();
    assert_eq!(staged_text(&f), "a\nb\nB2\nc\nd\n");
    repo.unstage_all(&c).unwrap();
    // Removed only.
    repo.stage_lines(&p, None, false, &shown, &[pos("d", Removed)], &c)
        .unwrap();
    assert_eq!(staged_text(&f), "a\nb\nc\n");
    repo.unstage_all(&c).unwrap();
    // Mixed: replace b with B1.
    repo.stage_lines(
        &p,
        None,
        false,
        &shown,
        &[pos("b", Removed), pos("B1", Added)],
        &c,
    )
    .unwrap();
    assert_eq!(staged_text(&f), "a\nB1\nc\nd\n");
    // Unstage just the B1 line again: b stays removed in the index.
    let staged = repo.worktree_diff(&p, None, true).unwrap();
    let b1 = staged.hunks[0]
        .lines
        .iter()
        .position(|l| l.text == b"B1")
        .unwrap();
    repo.stage_lines(&p, None, true, &staged, &[(0, b1)], &c)
        .unwrap();
    assert_eq!(staged_text(&f), "a\nc\nd\n");
}

fn read(f: &Fixture, rel: &str) -> Option<String> {
    fs::read_to_string(f.repo().join(rel)).ok()
}

#[test]
fn discard_and_restore_round_trip() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("a.txt", "1\n");
    f.write("b.txt", "1\n");
    f.write("gone.txt", "1\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Anfang", false, false, &c).unwrap();
    // a: unstaged only; b: staged change plus a further unstaged one; gone: deleted.
    f.write("a.txt", "2\n");
    f.write("b.txt", "staged\n");
    repo.stage(&paths(&["b.txt"]), &c).unwrap();
    f.write("b.txt", "staged and more\n");
    fs::remove_file(f.repo().join("gone.txt")).unwrap();

    let saved = repo
        .discard(&paths(&["a.txt", "b.txt", "gone.txt"]), &c)
        .unwrap();
    assert!(saved.starts_with("refs/gitcraft/discarded/"));
    assert_eq!(read(&f, "a.txt").as_deref(), Some("1\n"));
    assert_eq!(
        read(&f, "b.txt").as_deref(),
        Some("staged\n"),
        "back to the index"
    );
    assert_eq!(read(&f, "gone.txt").as_deref(), Some("1\n"));
    assert_eq!(
        staged_names(&repo.status().unwrap()),
        ["b.txt"],
        "index untouched"
    );

    // Survives garbage collection.
    f.git(&["gc", "-q", "--prune=now"]);
    let list = repo.discarded().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].files, paths(&["a.txt", "b.txt"]));
    assert_eq!(list[0].absent, paths(&["gone.txt"]));

    let before = repo.restore_discarded(&saved, &c).unwrap();
    assert_eq!(read(&f, "a.txt").as_deref(), Some("2\n"));
    assert_eq!(read(&f, "b.txt").as_deref(), Some("staged and more\n"));
    assert_eq!(read(&f, "gone.txt"), None, "the deletion is back too");
    let list = repo.discarded().unwrap();
    assert_eq!(
        list.len(),
        1,
        "the restored entry is gone, the state before it is kept"
    );
    assert_eq!(list[0].id, before);
}

#[test]
fn discard_single_lines() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("f.txt", "a\nb\nc\nd\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Anfang", false, false, &c).unwrap();
    f.write("f.txt", "a\nB\nc\nD\n");
    let p = RepoPath::from_bytes(b"f.txt");
    let shown = repo.worktree_diff(&p, None, false).unwrap();
    // Discard only the change of line 2 (the "-b" and "+B" lines).
    let picked: Vec<(usize, usize)> = shown.hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.text == b"b" || l.text == b"B")
        .map(|(i, _)| (0, i))
        .collect();
    assert_eq!(picked.len(), 2);
    let saved = repo.discard_lines(&p, &shown, &picked, &c).unwrap();
    assert_eq!(read(&f, "f.txt").as_deref(), Some("a\nb\nc\nD\n"));
    // Changed since display: refused.
    f.write("f.txt", "x\n");
    assert_eq!(
        repo.discard_lines(&p, &shown, &picked, &c),
        Err(GitError::Stale)
    );
    f.write("f.txt", "a\nb\nc\nD\n");
    repo.restore_discarded(&saved, &c).unwrap();
    assert_eq!(read(&f, "f.txt").as_deref(), Some("a\nB\nc\nD\n"));
}

#[cfg(unix)]
#[test]
fn delete_untracked_files_safely() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("tracked.txt", "t");
    f.write(".gitignore", "*.log\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Anfang", false, false, &c).unwrap();
    let outside = f.tmp.path().join("outside.txt");
    fs::write(&outside, "keep me").unwrap();
    f.write("deep/er/neu.txt", "neu");
    f.write("zwei\nzeilen.txt", "z");
    f.write("debug.log", "ignored");
    std::os::unix::fs::symlink(&outside, f.repo().join("link")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.write("run.sh", "#!/bin/sh\n");
        fs::set_permissions(f.repo().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Ignored and tracked files are refused, nothing is deleted.
    for refused in [
        &["debug.log"][..],
        &["tracked.txt"],
        &["deep/er/neu.txt", "debug.log"],
    ] {
        assert!(matches!(
            repo.delete_untracked(&paths(refused), &c),
            Err(GitError::UnsafePath(_))
        ));
    }
    assert!(f.repo().join("deep/er/neu.txt").exists());

    let names = ["deep/er/neu.txt", "zwei\nzeilen.txt", "link", "run.sh"];
    let saved = repo.delete_untracked(&paths(&names), &c).unwrap();
    for n in names {
        assert!(
            fs::symlink_metadata(f.repo().join(n)).is_err(),
            "{n:?} deleted"
        );
    }
    assert!(!f.repo().join("deep").exists(), "empty folders removed");
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "keep me",
        "link target untouched"
    );
    assert!(f.repo().join("debug.log").exists());

    repo.restore_discarded(&saved, &c).unwrap();
    assert_eq!(read(&f, "deep/er/neu.txt").as_deref(), Some("neu"));
    assert_eq!(read(&f, "zwei\nzeilen.txt").as_deref(), Some("z"));
    assert_eq!(fs::read_link(f.repo().join("link")).unwrap(), outside);
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(f.repo().join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "executable again");
}

#[test]
fn old_discarded_states_are_pruned() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("a.txt", "1");
    repo.stage_all(&c).unwrap();
    repo.commit("Anfang", false, false, &c).unwrap();
    f.write("a.txt", "2");
    repo.discard(&paths(&["a.txt"]), &c).unwrap();
    assert_eq!(repo.prune_discarded(3600, &c).unwrap(), 0);
    assert_eq!(repo.prune_discarded(-1, &c).unwrap(), 1);
    assert!(repo.discarded().unwrap().is_empty());
}

fn head_name(repo: &Repository) -> String {
    match repo.status().unwrap().head {
        Head::Branch { name, .. } => name,
        Head::Detached { .. } => "(detached)".into(),
    }
}

/// main with two commits.
fn two_commits() -> (Fixture, Repository) {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("a.txt", "1\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Eins", false, false, &c).unwrap();
    f.write("a.txt", "2\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Zwei", false, false, &c).unwrap();
    (f, repo)
}

#[test]
fn create_switch_rename_and_delete_branches() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    let first = all_commits(&repo)[1].id.clone();
    repo.create_branch("feature/x", first.as_str(), false, &c)
        .unwrap();
    assert_eq!(head_name(&repo), "main");
    repo.create_branch("work", "main", true, &c).unwrap();
    assert_eq!(head_name(&repo), "work");
    for (bad, problem) in [
        ("-rf", gitcraft_git::NameProblem::LeadingDash),
        ("a b", gitcraft_git::NameProblem::Whitespace),
        ("a..b", gitcraft_git::NameProblem::Rules),
    ] {
        assert!(matches!(
            repo.create_branch(bad, "main", false, &c),
            Err(GitError::InvalidName { problem: p, .. }) if p == problem
        ));
    }
    // Local changes travel along when they do not clash.
    f.write("neu.txt", "n");
    repo.switch_branch("feature/x", &c).unwrap();
    assert_eq!(head_name(&repo), "feature/x");
    assert!(f.repo().join("neu.txt").exists());
    repo.rename_branch("feature/x", "feature/y", &c).unwrap();
    assert_eq!(head_name(&repo), "feature/y");
    // The current branch cannot be deleted.
    assert!(repo.delete_branch("feature/y", &c).is_err());
    repo.switch_branch("main", &c).unwrap();
    // feature/y is contained in main: nothing would be lost.
    assert_eq!(repo.branch_loss("feature/y").unwrap().total, 0);
    let tip = repo.delete_branch("feature/y", &c).unwrap();
    assert_eq!(tip, first);
    // Restoring by its tip.
    repo.create_branch("feature/y", tip.as_str(), false, &c)
        .unwrap();
}

#[test]
fn switching_does_not_overwrite_local_changes() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    let first = all_commits(&repo)[1].id.clone();
    repo.create_branch("alt", first.as_str(), false, &c)
        .unwrap();
    f.write("a.txt", "lokal\n");
    assert_eq!(
        repo.switch_branch("alt", &c),
        Err(GitError::WouldOverwrite(vec!["a.txt".into()]))
    );
    assert_eq!(head_name(&repo), "main");
    assert_eq!(
        fs::read_to_string(f.repo().join("a.txt")).unwrap(),
        "lokal\n"
    );
}

#[test]
fn deleting_an_unmerged_branch_names_its_commits() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    repo.create_branch("idee", "main", true, &c).unwrap();
    for n in ["Versuch 1", "Versuch 2"] {
        f.write("idee.txt", n);
        repo.stage_all(&c).unwrap();
        repo.commit(n, false, false, &c).unwrap();
    }
    repo.switch_branch("main", &c).unwrap();
    let loss = repo.branch_loss("idee").unwrap();
    assert_eq!(loss.total, 2);
    let subjects: Vec<&str> = loss.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["Versuch 2", "Versuch 1"]);
    let tip = repo.delete_branch("idee", &c).unwrap();
    assert_eq!(tip, loss.commits[0].id);
}

#[test]
fn detached_head_and_what_leaving_it_loses() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    let first = all_commits(&repo)[1].id.clone();
    assert_eq!(repo.detached_loss().unwrap().total, 0, "on a branch");
    repo.switch_detached(&first, &c).unwrap();
    assert!(matches!(repo.status().unwrap().head, Head::Detached { .. }));
    assert_eq!(repo.detached_loss().unwrap().total, 0, "nothing new yet");
    f.write("b.txt", "x");
    repo.stage_all(&c).unwrap();
    repo.commit("Experiment", false, false, &c).unwrap();
    let loss = repo.detached_loss().unwrap();
    assert_eq!(loss.total, 1);
    assert_eq!(loss.commits[0].subject, "Experiment");
    // A branch keeps it.
    let here = loss.commits[0].id.clone();
    repo.create_branch("rettung", here.as_str(), false, &c)
        .unwrap();
    assert_eq!(repo.detached_loss().unwrap().total, 0);
}

#[test]
fn stash_push_show_apply_pop_drop_and_restore() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    assert!(
        repo.stash_push("leer", false, false, &c).unwrap().is_none(),
        "nothing to stash"
    );
    f.write("a.txt", "geparkt\n");
    f.write("neu.txt", "u");
    assert!(
        repo.stash_push("--force Versuch", false, false, &c)
            .unwrap()
            .is_some()
    );
    assert_eq!(fs::read_to_string(f.repo().join("a.txt")).unwrap(), "2\n");
    assert!(
        f.repo().join("neu.txt").exists(),
        "untracked stays without the option"
    );
    let list = repo.stashes().unwrap();
    assert_eq!(list.len(), 1);
    assert!(
        list[0].message.ends_with("--force Versuch"),
        "{}",
        list[0].message
    );
    // Shown like a commit against its base.
    let stash = repo.commit_by_id(&list[0].id).unwrap();
    let files: Vec<String> = repo
        .commit_files(&stash)
        .unwrap()
        .into_iter()
        .map(|x| x.path.display())
        .collect();
    assert_eq!(files, ["a.txt"]);

    repo.stash_apply("stash@{0}", &list[0].id, false, &c)
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.repo().join("a.txt")).unwrap(),
        "geparkt\n"
    );
    assert_eq!(repo.stashes().unwrap().len(), 1, "apply keeps it");
    repo.discard(&paths(&["a.txt"]), &c).unwrap();
    repo.stash_apply("stash@{0}", &list[0].id, true, &c)
        .unwrap();
    assert!(repo.stashes().unwrap().is_empty(), "pop drops it");

    // Untracked files and keep-index.
    f.write("b.txt", "vorgemerkt");
    repo.stage(&paths(&["b.txt"]), &c).unwrap();
    assert!(repo.stash_push("alles", true, true, &c).unwrap().is_some());
    assert!(!f.repo().join("neu.txt").exists(), "untracked included");
    assert!(
        f.repo().join("b.txt").exists(),
        "staged kept with --keep-index"
    );

    // A selector that moved (a newer stash took stash@{0}) is refused.
    let list = repo.stashes().unwrap();
    f.write("a.txt", "noch einer\n");
    repo.stash_push("neuer", false, false, &c).unwrap();
    assert_eq!(
        repo.stash_drop("stash@{0}", &list[0].id, &c),
        Err(GitError::Stale)
    );
    repo.stash_drop("stash@{0}", &repo.stashes().unwrap()[0].id, &c)
        .unwrap();
    // Drop and put back by id.
    repo.stash_drop("stash@{0}", &list[0].id, &c).unwrap();
    assert!(repo.stashes().unwrap().is_empty());
    repo.stash_store(&list[0].id, "alles", &c).unwrap();
    assert_eq!(repo.stashes().unwrap()[0].id, list[0].id);
    assert!(matches!(
        repo.stash_drop("stash@{0}; rm", &list[0].id, &c),
        Err(GitError::InvalidValue(_))
    ));
}

#[test]
fn a_conflicting_stash_apply_is_reported_and_kept() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    f.write("a.txt", "im Stash\n");
    repo.stash_push("", false, false, &c).unwrap();
    f.write("a.txt", "neu committet\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Drei", false, false, &c).unwrap();
    let id = repo.stashes().unwrap()[0].id.clone();
    let r = repo.stash_apply("stash@{0}", &id, true, &c);
    assert!(matches!(r, Err(GitError::Conflicts(_))), "{r:?}");
    assert_eq!(repo.stashes().unwrap().len(), 1, "kept after a conflict");
    assert_eq!(repo.status().unwrap().conflicts().count(), 1);
}

fn tags(repo: &Repository) -> Vec<String> {
    repo.refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == gitcraft_git::RefKind::Tag)
        .map(|r| r.name)
        .collect()
}

#[test]
fn tags_lightweight_annotated_delete_restore() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    let first = all_commits(&repo)[1].id.clone();
    repo.create_tag("v0.1", first.as_str(), None, false, &c)
        .unwrap();
    repo.create_tag(
        "v0.2",
        "HEAD",
        Some("Zweite Version\n\n#1 erledigt"),
        false,
        &c,
    )
    .unwrap();
    assert_eq!(tags(&repo), ["v0.1", "v0.2"]);
    let out = f
        .command(&f.repo())
        .args(["cat-file", "-p", "v0.2"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Zweite Version\n\n#1 erledigt"), "{text}");
    for bad in ["-x", "a b", "v1..2", ""] {
        assert!(matches!(
            repo.create_tag(bad, "HEAD", None, false, &c),
            Err(GitError::InvalidName { .. })
        ));
    }
    // An existing name is refused by git.
    assert!(repo.create_tag("v0.1", "HEAD", None, false, &c).is_err());

    let object = repo.delete_tag("v0.2", &c).unwrap();
    assert_eq!(tags(&repo), ["v0.1"]);
    repo.restore_tag("v0.2", &object, &c).unwrap();
    let out = f
        .command(&f.repo())
        .args(["cat-file", "-t", "v0.2"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "tag",
        "annotated again"
    );
    assert!(
        repo.restore_tag("v0.2", &object, &c).is_err(),
        "exists already"
    );
}

#[test]
fn compare_diverged_branches() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    repo.create_branch("feature", "main", true, &c).unwrap();
    f.write("feature.txt", "neu");
    repo.stage_all(&c).unwrap();
    repo.commit("Feature", false, false, &c).unwrap();
    repo.switch_branch("main", &c).unwrap();
    f.write("a.txt", "3\n");
    repo.stage_all(&c).unwrap();
    repo.commit("Drei auf main", false, false, &c).unwrap();

    let cmp = repo
        .compare("refs/heads/main", "refs/heads/feature")
        .unwrap();
    let subj = |v: &[gitcraft_git::Commit]| v.iter().map(|c| c.subject.clone()).collect::<Vec<_>>();
    assert_eq!(subj(&cmp.only_a), ["Drei auf main"]);
    assert_eq!(subj(&cmp.only_b), ["Feature"]);
    let files: Vec<String> = cmp.files.iter().map(|x| x.path.display()).collect();
    assert_eq!(
        files,
        ["feature.txt"],
        "from the merge base, not from main's tip"
    );
    let base = cmp.base.clone().unwrap();
    let d = repo
        .range_diff(base.as_str(), "refs/heads/feature", &cmp.files[0])
        .unwrap();
    assert_eq!((d.added(), d.new_file), (1, true));

    // No common history.
    f.git(&["checkout", "-q", "--orphan", "fremd"]);
    f.git(&["rm", "-rq", "--cached", "."]);
    f.write("x.txt", "x");
    f.git(&["add", "x.txt"]);
    f.git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@invalid",
        "commit",
        "-qm",
        "Fremd",
    ]);
    let cmp = repo.compare("refs/heads/main", "refs/heads/fremd").unwrap();
    assert_eq!(cmp.base, None);
    assert!(cmp.files.is_empty());
    assert_eq!(subj(&cmp.only_b), ["Fremd"]);
}

#[test]
fn undo_a_commit_and_redo_it() {
    let (f, repo) = two_commits();
    let c = Cancel::new();
    let before = repo.head_commit().unwrap();
    f.write("b.txt", "neu");
    repo.stage_all(&c).unwrap();
    repo.commit("Drei", false, false, &c).unwrap();
    let after = repo.head_commit().unwrap().unwrap();
    // Uncommitted work elsewhere stays untouched.
    f.write("a.txt", "lokal\n");
    repo.move_head(Some(&after), before.as_ref(), true, &c)
        .unwrap();
    assert_eq!(repo.head_commit().unwrap(), before);
    assert_eq!(
        staged_names(&repo.status().unwrap()),
        ["b.txt"],
        "changes stay staged"
    );
    assert_eq!(
        fs::read_to_string(f.repo().join("a.txt")).unwrap(),
        "lokal\n"
    );
    // Not where it was left: refused.
    assert_eq!(
        repo.move_head(Some(&after), before.as_ref(), true, &c),
        Err(GitError::Stale)
    );
    // Redo.
    repo.move_head(before.as_ref(), Some(&after), false, &c)
        .unwrap();
    assert_eq!(repo.head_commit().unwrap(), Some(after.clone()));
    assert_eq!(
        repo.head_pos().unwrap(),
        gitcraft_git::HeadPos::Branch("main".into())
    );
}

#[test]
fn undo_refuses_pushed_commits_and_handles_the_first_commit() {
    let (f, repo) = with_identity();
    let c = Cancel::new();
    f.write("a.txt", "1");
    repo.stage_all(&c).unwrap();
    repo.commit("Erster", false, false, &c).unwrap();
    let first = repo.head_commit().unwrap().unwrap();
    // Undo of the first commit: back to an unborn branch, the file stays staged.
    repo.move_head(Some(&first), None, true, &c).unwrap();
    assert_eq!(
        repo.head_pos().unwrap(),
        gitcraft_git::HeadPos::Unborn("main".into())
    );
    assert_eq!(staged_names(&repo.status().unwrap()), ["a.txt"]);
    repo.move_head(None, Some(&first), false, &c).unwrap();

    let remote = f.tmp.path().join("remote.git");
    assert!(
        f.command(f.tmp.path())
            .args(["init", "-q", "--bare"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    f.git(&["push", "-q", "origin", "main"]);
    assert_eq!(
        repo.move_head(Some(&first), None, true, &c),
        Err(GitError::Pushed)
    );
}

#[test]
fn refs_are_removed_only_where_they_were_left() {
    let (_f, repo) = two_commits();
    let c = Cancel::new();
    let head = repo.head_commit().unwrap().unwrap();
    repo.create_branch("tmp", head.as_str(), false, &c).unwrap();
    let first = all_commits(&repo)[1].id.clone();
    assert_eq!(
        repo.delete_ref_if("refs/heads/tmp", &first, &c),
        Err(GitError::Stale)
    );
    repo.delete_ref_if("refs/heads/tmp", &head, &c).unwrap();
    assert!(matches!(
        repo.delete_ref_if("refs/remotes/origin/x", &head, &c),
        Err(GitError::InvalidValue(_))
    ));
    let tag = repo.create_tag("t", "HEAD", Some("m"), false, &c).unwrap();
    assert_ne!(tag, head, "an annotated tag's own object");
    repo.delete_ref_if("refs/tags/t", &tag, &c).unwrap();
}

#[test]
fn remotes_are_added_renamed_edited_and_removed() {
    let f = Fixture::new();
    let repo = f.open();
    let cancel = Cancel::new();
    assert_eq!(repo.remotes().unwrap(), []);
    repo.add_remote("origin", "https://example.org/r.git", None, &cancel)
        .unwrap();
    repo.add_remote(
        "team.mirror",
        "/srv/mirror",
        Some("ssh://git@example.org/m"),
        &cancel,
    )
    .unwrap();
    let list = repo.remotes().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].name, "team.mirror");
    assert_eq!(list[1].push_url(), Some("ssh://git@example.org/m"));

    repo.rename_remote("origin", "upstream", &cancel).unwrap();
    repo.set_remote_urls("team.mirror", "/srv/other", None, &cancel)
        .unwrap();
    let list = repo.remotes().unwrap();
    assert_eq!(list[0].name, "upstream");
    assert_eq!(list[1].urls, ["/srv/other"]);
    assert!(list[1].push_urls.is_empty(), "separate push URL removed");

    let removed = repo.remove_remote("upstream", &cancel).unwrap();
    assert_eq!(removed.url(), Some("https://example.org/r.git"));
    assert_eq!(repo.remotes().unwrap().len(), 1);
    // Added back as it was.
    repo.add_remote("upstream", removed.url().unwrap(), None, &cancel)
        .unwrap();
    assert_eq!(repo.remotes().unwrap().len(), 2);
}

#[test]
fn hostile_remote_names_and_urls_are_refused() {
    let f = Fixture::new();
    let repo = f.open();
    let cancel = Cancel::new();
    for name in ["-x", "--upload-pack=touch x", "a b", "a..b", ""] {
        assert!(
            matches!(
                repo.add_remote(name, "/srv/r", None, &cancel),
                Err(GitError::InvalidName { .. })
            ),
            "{name:?}"
        );
    }
    for url in [
        "--upload-pack=touch x",
        "ext::sh -c touch% pwned",
        "ssh://-oProxyCommand=touch%20pwned/r",
    ] {
        assert!(
            matches!(
                repo.add_remote("origin", url, None, &cancel),
                Err(GitError::InvalidUrl { .. })
            ),
            "{url:?}"
        );
    }
    assert_eq!(repo.remotes().unwrap(), []);
    assert!(!f.repo().join("pwned").exists());
}

#[test]
fn credentials_in_urls_move_into_the_helper() {
    let f = Fixture::new();
    let repo = f.open();
    let cancel = Cancel::new();
    repo.add_remote(
        "origin",
        "https://me:s3cret@example.org/r.git",
        None,
        &cancel,
    )
    .unwrap();
    assert!(repo.remotes().unwrap()[0].has_credentials());
    // Without a helper nothing changes.
    assert_eq!(
        repo.move_credentials("origin", &cancel),
        Err(GitError::NoCredentialHelper)
    );
    assert!(repo.remotes().unwrap()[0].has_credentials());

    // A helper that records what it is asked to store.
    let store = f.tmp.path().join("stored");
    let helper = format!("store --file={}", store.display()).replace('\\', "/");
    f.git(&["config", "credential.helper", &helper]);
    repo.move_credentials("origin", &cancel).unwrap();
    let remote = &repo.remotes().unwrap()[0];
    assert_eq!(remote.urls, ["https://example.org/r.git"]);
    assert!(!remote.has_credentials());
    let stored = fs::read_to_string(&store).unwrap();
    assert!(stored.contains("me:s3cret@example.org"), "{stored}");
}

/// A repository with a commit on `main`, pushed to the bare `up.git`, and a second clone
/// `other` of it (for commits that arrive from elsewhere).
fn with_remote() -> Fixture {
    let f = Fixture::new();
    f.write("a.txt", "1\n");
    f.commit_all("Anfang");
    let up = f.tmp.path().join("up.git");
    let ok = f
        .command(f.tmp.path())
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&up)
        .status()
        .unwrap();
    assert!(ok.success());
    f.git(&["remote", "add", "origin", up.to_str().unwrap()]);
    f.git(&["push", "-q", "-u", "origin", "main"]);
    let ok = f
        .command(f.tmp.path())
        .args(["clone", "-q"])
        .arg(&up)
        .arg(f.tmp.path().join("other"))
        .status()
        .unwrap();
    assert!(ok.success());
    f
}

impl Fixture {
    fn in_other(&self, args: &[&str]) {
        let out = self
            .command(&self.tmp.path().join("other"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Commits `file` with `content` in `other` and pushes it.
    fn push_from_other(&self, file: &str, content: &str) {
        fs::write(self.tmp.path().join("other").join(file), content).unwrap();
        self.in_other(&["add", "-A"]);
        self.in_other(&["commit", "-q", "-m", file]);
        self.in_other(&["push", "-q", "origin", "HEAD"]);
    }

    fn head(&self) -> String {
        let out = self
            .command(&self.repo())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

fn no_progress() -> gitcraft_git::OnProgress {
    Arc::new(|_| {})
}

#[test]
fn fetch_reports_new_commits_per_branch_and_prunes() {
    let f = with_remote();
    f.push_from_other("b.txt", "2\n");
    f.push_from_other("c.txt", "3\n");
    f.in_other(&["push", "-q", "origin", "HEAD:refs/heads/feature"]);
    let repo = f.open();
    let cancel = Cancel::new();
    let updates = repo
        .fetch(Some("origin"), false, &cancel, no_progress())
        .unwrap();
    let main = updates.iter().find(|u| u.name == "origin/main").unwrap();
    assert_eq!(main.commits, Some(2));
    assert!(!main.forced);
    let feature = updates.iter().find(|u| u.name == "origin/feature").unwrap();
    assert_eq!(feature.old, None, "new branch");
    assert_eq!(repo.divergence().unwrap(), (0, 2));

    f.in_other(&["push", "-q", "origin", "--delete", "feature"]);
    let updates = repo.fetch(None, true, &cancel, no_progress()).unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].name, "origin/feature");
    assert_eq!(updates[0].new, None, "pruned");
    assert!(matches!(
        repo.fetch(Some("--upload-pack=touch x"), false, &cancel, no_progress()),
        Err(GitError::InvalidName { .. })
    ));
}

#[test]
fn pull_fast_forwards_and_explains_divergence() {
    let f = with_remote();
    f.push_from_other("b.txt", "2\n");
    let repo = f.open();
    let cancel = Cancel::new();
    repo.pull(gitcraft_git::PullMode::FastForward, &cancel, no_progress())
        .unwrap();
    assert!(f.repo().join("b.txt").exists());

    // Both sides move on: fast-forward refused before anything changes.
    f.push_from_other("c.txt", "3\n");
    f.write("d.txt", "4\n");
    f.commit_all("lokal");
    let head = f.head();
    // Even when the configuration asks for a rebase.
    f.git(&["config", "pull.rebase", "true"]);
    f.git(&["config", "user.name", "Test"]);
    f.git(&["config", "user.email", "test@invalid"]);
    repo.fetch(Some("origin"), false, &cancel, no_progress())
        .unwrap();
    assert_eq!(
        repo.pull(gitcraft_git::PullMode::FastForward, &cancel, no_progress()),
        Err(GitError::Diverged {
            ahead: 1,
            behind: 1
        })
    );
    assert_eq!(f.head(), head);
    // The explicit choice: a merge commit.
    repo.pull(gitcraft_git::PullMode::Merge, &cancel, no_progress())
        .unwrap();
    assert!(f.repo().join("c.txt").exists());
    assert_eq!(repo.divergence().unwrap(), (2, 0), "local commit and merge");
}

#[test]
fn pull_rebase_conflicts_and_missing_upstream() {
    let f = with_remote();
    f.push_from_other("a.txt", "remote\n");
    f.write("a.txt", "lokal\n");
    f.commit_all("lokal");
    let repo = f.open();
    let cancel = Cancel::new();
    assert!(matches!(
        repo.pull(gitcraft_git::PullMode::Rebase, &cancel, no_progress()),
        Err(GitError::MergeConflicts(_))
    ));
    assert!(f.git_may_fail(&["rebase", "--abort"]));

    f.git(&["switch", "-q", "-c", "solo"]);
    assert_eq!(
        repo.pull(gitcraft_git::PullMode::FastForward, &cancel, no_progress()),
        Err(GitError::NoUpstream)
    );
}

/// Cancelling a fetch that hangs (an ssh that never answers) stops it quickly and leaves the
/// repository as it was.
#[cfg(unix)]
#[test]
fn a_cancelled_fetch_leaves_the_repository_consistent() {
    let f = with_remote();
    f.git(&["remote", "add", "slow", "ssh://example.invalid/r.git"]);
    let runner = f
        .runner()
        .env("GIT_SSH_COMMAND", "sh -c 'sleep 30'")
        .env("GIT_SSH_VARIANT", "simple");
    let repo = Repository::open(&runner, &f.repo()).unwrap();
    let before = f.head();
    let cancel = Cancel::new();
    let later = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        later.cancel();
    });
    let start = std::time::Instant::now();
    assert_eq!(
        repo.fetch(Some("slow"), true, &cancel, no_progress()),
        Err(GitError::Cancelled)
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(f.head(), before);
    f.git(&["fsck", "--no-progress"]);
    assert!(repo.status().is_ok());
}
