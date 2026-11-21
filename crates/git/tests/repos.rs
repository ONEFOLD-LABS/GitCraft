//! The Git layer against real temporary repositories, isolated from the user's and the system's
//! git configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gitcraft_git::{Change, Conflict, GitError, GitRunner, Head, Repository, State, Status};
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
    f.write("a.txt", "1\n2\n");
    f.write(":(glob)*", "literal name\n");
    f.commit_all("base");
    f.write("a.txt", "1\nzwei\n");
    f.git(&["add", "a.txt"]);
    f.write("a.txt", "1\nzwei\ndrei\n");
    f.write(":(glob)*", "changed\n");
    f.write("neu.txt", "x\ny");

    let repo = f.open();
    let path = |s: &str| gitcraft_git::RepoPath::from_bytes(s.as_bytes());
    let staged = repo.worktree_diff(&path("a.txt"), None, true).unwrap();
    assert_eq!((staged.added(), staged.removed()), (1, 1));
    let unstaged = repo.worktree_diff(&path("a.txt"), None, false).unwrap();
    assert_eq!((unstaged.added(), unstaged.removed()), (1, 0));
    // A name that looks like pathspec magic is taken literally: only that file.
    let magic = repo.worktree_diff(&path(":(glob)*"), None, false).unwrap();
    assert_eq!((magic.added(), magic.removed()), (1, 1));
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
