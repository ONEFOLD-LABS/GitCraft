//! Window tests against real temporary repositories. Render tests save PNGs with
//! `GITCRAFT_SHOTS=<dir>`; otherwise they only check that the window renders and is accessible.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use egui::ThemePreference;
use egui_kittest::Harness;
use gitcraft_git::{GitRunner, Repository};

use super::*;
use crate::history::History;

/// A temporary repository, isolated from the user's git configuration.
struct Fixture {
    tmp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let f = Fixture {
            tmp: tempfile::tempdir().unwrap(),
        };
        fs::write(f.tmp.path().join("gitconfig"), "").unwrap();
        fs::create_dir(f.repo()).unwrap();
        f.git(&["init", "-q", "-b", "main"]);
        f
    }

    fn repo(&self) -> PathBuf {
        self.tmp.path().join("demo-projekt")
    }

    fn git(&self, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(self.repo())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.tmp.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@invalid")
            .output()
            .unwrap()
            .status
            .success()
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.repo().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn commit_all(&self, message: &str) {
        assert!(self.git(&["add", "-A"]));
        assert!(self.git(&["commit", "-q", "-m", message]));
    }

    fn runner(&self) -> GitRunner {
        GitRunner::new()
            .env("GIT_CONFIG_GLOBAL", self.tmp.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
    }

    /// A window showing this repository, read synchronously.
    fn window(&self) -> RepoWindow {
        let runner = self.runner();
        let repo = Repository::open(&runner, &self.repo()).unwrap();
        let snapshot = worker::Snapshot::read(&repo).unwrap();
        let mut log = repo.log().unwrap();
        let mut commits = Vec::new();
        loop {
            let page = log.next_page(500).unwrap();
            if page.is_empty() {
                break;
            }
            commits.extend(page);
        }
        let mut w = RepoWindow::new(runner, None);
        w.history = History::from_commits(commits);
        w.repo = Some(repo);
        w.apply(snapshot);
        w
    }

    /// Every kind of row: conflict, staged, unstaged, rename, untracked, behind its upstream.
    fn busy_repository() -> Fixture {
        let f = Fixture::new();
        f.write("README.md", "Demo\n");
        f.write("src/main.rs", "fn main() {}\n");
        f.write("src/alt.rs", "// a module that will be renamed\n");
        f.write("docs/plan.md", "Plan\n");
        f.write("konflikt.txt", "Basis\n");
        f.commit_all("Anfang");
        assert!(f.git(&["checkout", "-q", "-b", "anderer"]));
        f.write("konflikt.txt", "anderer Stand\n");
        f.commit_all("anderer");
        assert!(f.git(&["checkout", "-q", "main"]));
        assert!(f.git(&["branch", "-q", "--set-upstream-to", "anderer"]));
        f.write("konflikt.txt", "main\n");
        f.commit_all("main");
        assert!(!f.git(&["merge", "-q", "anderer"]));
        f.write("src/main.rs", "fn main() { println!(\"hallo\"); }\n");
        assert!(f.git(&["mv", "src/alt.rs", "src/neu.rs"]));
        f.write("Cargo.toml", "[package]\n");
        assert!(f.git(&["add", "Cargo.toml"]));
        fs::remove_file(f.repo().join("docs/plan.md")).unwrap();
        f.write("notizen/Grüße aus Köln.txt", "neu\n");
        f
    }
}

fn shot(name: &str, preference: ThemePreference, mut w: RepoWindow) {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 640.0))
        .wgpu()
        .build_ui(move |ui| w.ui(ui));
    theme::install(&harness.ctx, preference);
    harness.run_steps(3);
    harness.check_accessibility();
    let Ok(img) = harness.render() else {
        eprintln!("no GPU renderer, image skipped");
        return;
    };
    if let Ok(dir) = std::env::var("GITCRAFT_SHOTS") {
        img.save(Path::new(&dir).join(format!("{name}.png")))
            .unwrap();
    }
}

#[test]
fn render_welcome() {
    // A runner that cannot start git: the footer stays empty, no error before opening.
    let w = RepoWindow::new(GitRunner::with_program("/nonexistent/git"), None);
    shot("welcome-dark", ThemePreference::Dark, w);
    let w = RepoWindow::new(GitRunner::with_program("/nonexistent/git"), None);
    shot("welcome-light", ThemePreference::Light, w);
}

#[test]
fn render_changes() {
    let f = Fixture::busy_repository();
    shot("changes-dark", ThemePreference::Dark, f.window());
    shot("changes-light", ThemePreference::Light, f.window());
}

#[test]
fn render_clean_and_unborn() {
    let f = Fixture::new();
    shot("unborn-dark", ThemePreference::Dark, f.window());
    f.write("a.txt", "1");
    f.commit_all("eins");
    shot("clean-light", ThemePreference::Light, f.window());
}

/// File names with characters no bundled font has (CJK, emoji, right-to-left, invisible
/// marks) render as boxes, as in the app (`MissingGlyphPolicy::Tofu`), and long names are cut.
#[test]
fn render_hostile_file_names() {
    let f = Fixture::new();
    for name in [
        "日本語/ファイル.md",
        "emoji 🦀🚀.txt",
        "עברית\u{202e}txt.exe",
        "zero\u{200b}width.txt",
        &format!("{}.txt", "sehr-langer-name-".repeat(12)),
    ] {
        f.write(name, "x");
    }
    let mut w = f.window();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(700.0, 400.0))
        .wgpu()
        .allow_missing_glyphs()
        .build_ui(move |ui| w.ui(ui));
    theme::install(&harness.ctx, ThemePreference::Dark);
    harness.run_steps(3);
    harness.check_accessibility();
    if let (Ok(img), Ok(dir)) = (harness.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("hostile-names-dark.png"))
            .unwrap();
    }
}

#[test]
fn render_error() {
    let f = Fixture::new();
    let mut w = f.window();
    w.error = Some(error_text(&GitError::NotARepository(PathBuf::from(
        "/home/nutzer/kein-repo",
    ))));
    shot("error-dark", ThemePreference::Dark, w);
}

#[test]
fn rows_group_conflicts_staged_unstaged_and_untracked() {
    let f = Fixture::busy_repository();
    let w = f.window();
    let sections: Vec<(&str, usize)> = w
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::Section { title, count } => Some((*title, *count)),
            Row::File { .. } => None,
        })
        .collect();
    assert_eq!(
        sections,
        [
            ("Konflikte", 1),
            ("Vorgemerkt", 2),
            ("Nicht vorgemerkt", 2),
            ("Unversioniert", 1)
        ]
    );
    assert!(w.rows.contains(&Row::File {
        letter: 'R',
        tone: Tone::Renamed,
        meaning: "umbenannt",
        path: "src/alt.rs -> src/neu.rs".into(),
    }));
    let up = w.status().map(upstream_text).unwrap();
    assert_eq!(up, "1 voraus, 1 zurück gegenüber anderer");
}

/// Opening goes through the worker: the window stays responsive and shows the result once git
/// has answered.
#[test]
fn open_runs_in_the_background() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    let mut w = RepoWindow::new(f.runner(), None);
    let ctx = egui::Context::default();
    w.open(f.repo().join("a.txt"), &ctx);
    assert!(w.is_busy());
    let deadline = Instant::now() + Duration::from_secs(20);
    while w.is_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        w.collect(&ctx);
    }
    assert!(!w.is_busy(), "git did not answer in time");
    assert_eq!(w.error(), None);
    assert_eq!(
        fs::canonicalize(w.repository().unwrap().workdir()).unwrap(),
        fs::canonicalize(f.repo()).unwrap()
    );
    assert_eq!(w.status().unwrap().untracked().count(), 1);
    // The history loads in the background as well.
    while w.history.is_loading() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        w.collect(&ctx);
    }
    assert!(!w.history.is_loading());
    assert!(w.history.commits().is_empty(), "no commits yet");
}

#[test]
fn opening_something_else_shows_an_error() {
    let f = Fixture::new();
    let plain = f.tmp.path().join("kein-repo");
    fs::create_dir(&plain).unwrap();
    let mut w = RepoWindow::new(f.runner(), None);
    let ctx = egui::Context::default();
    w.open(plain, &ctx);
    let deadline = Instant::now() + Duration::from_secs(20);
    while w.is_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        w.collect(&ctx);
    }
    assert!(w.error().is_some_and(|e| e.contains("kein Git-Repository")));
    assert!(w.repository().is_none());
}

/// Texts passed to `tr` through variables (section titles, meanings) are not seen by the
/// catalog scan in `i18n.rs`; check them here.
#[test]
fn texts_behind_variables_are_translated() {
    use gitcraft_git::Conflict as C;
    let mut texts = vec![
        "Konflikte",
        "Vorgemerkt",
        "Nicht vorgemerkt",
        "Unversioniert",
        "hinzugefügt",
        "geändert",
        "Dateityp geändert",
        "gelöscht",
        "umbenannt",
        "kopiert",
        "unversioniert",
        "Lokal",
        "Remote",
        "Tags",
    ];
    texts.extend(
        [
            C::BothModified,
            C::BothAdded,
            C::BothDeleted,
            C::AddedByUs,
            C::AddedByThem,
            C::DeletedByUs,
            C::DeletedByThem,
        ]
        .map(conflict_meaning),
    );
    let missing: Vec<&str> = texts
        .into_iter()
        .filter(|t| !i18n::has_english(t))
        .collect();
    assert!(missing.is_empty(), "missing in i18n/en.tsv: {missing:?}");
}

/// Names from the repository or a remote reach the window escaped, like file names.
#[test]
fn ref_names_paths_and_git_messages_are_escaped() {
    let head = head_text(&Head::Branch {
        name: "main\u{202e}txt".into(),
        commit: None,
    });
    assert!(head.contains("main\\u{202e}txt"), "{head}");
    let status = Status {
        head: Head::Branch {
            name: "main".into(),
            commit: None,
        },
        upstream: Some(gitcraft_git::Upstream {
            name: "origin/x\u{200b}y".into(),
            ahead_behind: None,
        }),
        files: Vec::new(),
    };
    assert!(upstream_text(&status).contains("x\\u{200b}y"));
    let err = error_text(&GitError::Failed {
        command: "status".into(),
        code: Some(128),
        stderr: "bad ref\nline\u{202e}2".into(),
    });
    assert!(err.contains("bad ref\\nline\\u{202e}2"), "{err}");
    let err = error_text(&GitError::NotARepository(PathBuf::from("/tmp/a\u{202e}b")));
    assert!(err.contains("a\\u{202e}b"), "{err}");
    assert_eq!(
        repo_name(Path::new("/x/repo\u{2066}name")),
        "repo\\u{2066}name"
    );
}

/// A history with branches, a merge, tags, a remote-like branch and work in progress: refs land
/// on their commits, the graph has two lanes, keys move the selection.
#[test]
fn history_with_branches_labels_and_keyboard() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("Anfang");
    assert!(f.git(&["tag", "-a", "v0.1", "-m", "erste Version"]));
    assert!(f.git(&["checkout", "-q", "-b", "feature"]));
    f.write("b.txt", "1");
    f.commit_all("Feature anlegen");
    f.write("b.txt", "2");
    f.commit_all("Feature ausbauen");
    assert!(f.git(&["checkout", "-q", "main"]));
    f.write("a.txt", "2");
    f.commit_all("Hauptlinie weiter");
    assert!(f.git(&[
        "merge",
        "-q",
        "--no-ff",
        "-m",
        "Feature zusammenführen",
        "feature"
    ]));
    assert!(f.git(&["branch", "experiment", "HEAD~1"]));
    f.write("a.txt", "dirty");

    let mut w = f.window();
    let commits = w.history.commits();
    assert_eq!(commits.len(), 5);
    assert_eq!(commits[0].subject, "Feature zusammenführen");
    let head = head_commit(w.status().unwrap()).unwrap();
    let labels: Vec<&str> = w.labels.of(&head).iter().map(|l| l.text.as_str()).collect();
    assert_eq!(labels, ["main"]);
    let tagged = commits.last().unwrap().id.clone();
    assert_eq!(w.labels.of(&tagged)[0].text, "v0.1");
    assert_eq!(
        w.selection,
        Selection::WorkInProgress,
        "dirty tree selects the work in progress"
    );

    w.move_selection(1);
    assert_eq!(w.selection, Selection::Commit(head.clone()));
    w.move_selection(100);
    assert_eq!(w.selection, Selection::Commit(tagged));
    w.move_selection(-1000);
    assert_eq!(w.selection, Selection::WorkInProgress);

    shot("history-dark", ThemePreference::Dark, f.window());
    shot("history-light", ThemePreference::Light, f.window());
    let mut w = f.window();
    w.select_commit(head);
    shot("history-commit-dark", ThemePreference::Dark, w);
}
