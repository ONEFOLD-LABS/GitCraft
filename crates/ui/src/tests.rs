//! Window tests against real temporary repositories. Render tests save PNGs with
//! `GITCRAFT_SHOTS=<dir>`; otherwise they only check that the window renders and is accessible.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use egui::ThemePreference;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
        let (session, snapshot) =
            RepoSession::open(runner.clone(), &self.repo(), std::sync::Arc::new(|| {})).unwrap();
        let mut log = session.reader().log().unwrap();
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
        w.session = Some(session);
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
            Row::Section { title, count, .. } => Some((*title, *count)),
            Row::File { .. } => None,
        })
        .collect();
    assert_eq!(
        sections,
        [("Konflikte", 1), ("Nicht vorgemerkt", 3), ("Vorgemerkt", 2)]
    );
    let all = |title: &str| {
        w.rows.iter().find_map(|r| match r {
            Row::Section { title: t, all, .. } if *t == title => Some(all.clone()),
            _ => None,
        })
    };
    assert_eq!(
        all("Konflikte"),
        Some(None),
        "conflicts are not staged from here"
    );
    let p = |n: &str| RepoPath::from_bytes(n.as_bytes());
    assert_eq!(
        all("Nicht vorgemerkt"),
        Some(Some(StageOp::Stage(vec![
            p("docs/plan.md"),
            p("src/main.rs"),
            p("notizen/Grüße aus Köln.txt")
        ])))
    );
    // Unstaging a rename takes both paths back.
    assert_eq!(
        all("Vorgemerkt"),
        Some(Some(StageOp::Unstage(vec![
            p("Cargo.toml"),
            p("src/neu.rs"),
            p("src/alt.rs")
        ])))
    );
    assert!(w.rows.iter().any(|r| matches!(r, Row::File {
        letter: 'R',
        tone: Tone::Renamed,
        meaning: "umbenannt",
        path,
        ..
    } if path == "src/alt.rs -> src/neu.rs")));
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
        fs::canonicalize(w.workdir().unwrap()).unwrap(),
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
    assert!(w.workdir().is_none());
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
        "Vormerken",
        "Zurücknehmen",
        "Alle vormerken",
        "Alle zurücknehmen",
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
    texts.extend(toolbar::ACTIONS.map(|(_, label, _)| label));
    texts.extend(search::Kind::ALL.map(search::Kind::label));
    texts.extend([
        "Repository öffnen",
        "Status neu lesen",
        "Historie durchsuchen",
        "Nächster / vorheriger Treffer",
        "Commit wählen",
        "Commit-ID kopieren",
        "Diff oder Suche schließen",
        "Zoom",
        "Diese Übersicht",
    ]);
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
    let mut w = f.window();
    w.show_keys = true;
    shot("keys-light", ThemePreference::Light, w);
}

/// The settings window explains what the avatar switch sends; switching it saves the file and
/// turns the avatar loader on (the test never reaches the network: nothing is drawn while on).
#[test]
fn avatar_setting_is_saved_and_explained() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("eins");
    let dir = tempfile::tempdir().unwrap();
    let mut w = f.window();
    w.use_settings(Some(dir.path().to_path_buf()));
    assert!(!w.settings.avatars, "off by default");
    w.show_settings = true;
    shot("settings-dark", ThemePreference::Dark, w);

    let mut w = f.window();
    w.use_settings(Some(dir.path().to_path_buf()));
    w.settings.avatars = true;
    w.apply_settings();
    w.save_settings();
    let (saved, _) = crate::settings::Settings::load(dir.path());
    assert!(saved.avatars);
    let mut w = f.window();
    w.use_settings(Some(dir.path().to_path_buf()));
    assert!(w.settings.avatars, "loaded again");
}

/// Commit details list the changed files; a file's diff replaces the history, Esc returns.
#[test]
fn commit_files_and_diff_view() {
    let f = Fixture::new();
    f.write("src/main.rs", "fn main() {\n\tprintln!(\"hallo\");\n}\n");
    f.write("README.md", "Demo\n");
    f.commit_all("Anfang");
    f.write(
        "src/main.rs",
        "fn main() {\n\tprintln!(\"hallo welt\");\n\tprintln!(\"zweite Zeile\");\n}",
    );
    f.write("neu.txt", "neu\n");
    assert!(f.git(&["add", "-A"]));
    assert!(f.git(&[
        "commit",
        "-q",
        "-m",
        "Begrüßung erweitern\n\nDie Ausgabe nennt jetzt die Welt und\nbekommt eine zweite Zeile."
    ]));

    let mut w = f.window();
    let commit = w.history.commits()[0].clone();
    w.select_commit(commit.id.clone());
    let repo = Repository::open(&f.runner(), &f.repo()).unwrap();
    let files = repo.commit_files(&commit).unwrap();
    w.details = Some(Details {
        id: commit.id.clone(),
        message: Some(repo.commit_message(&commit).unwrap()),
        files: Some(files.clone()),
        error: None,
    });
    shot("commit-files-dark", ThemePreference::Dark, w);

    let main = files
        .iter()
        .find(|x| x.path.display() == "src/main.rs")
        .unwrap()
        .clone();
    let diff = repo.commit_diff(&commit, &main).unwrap();
    assert_eq!((diff.added(), diff.removed()), (3, 2));
    let mut w = f.window();
    w.select_commit(commit.id.clone());
    w.diff = Some(DiffState {
        key: DiffKey::Commit {
            commit: Box::new(commit.clone()),
            file: main.clone(),
        },
        title: "src/main.rs".into(),
        prepared: Some(Prepared::new("src/main.rs".into(), &diff)),
        error: None,
    });
    shot("diff-dark", ThemePreference::Dark, w);
    let mut w = f.window();
    w.diff = Some(DiffState {
        key: DiffKey::Untracked {
            path: gitcraft_git::RepoPath::from_bytes(b"x"),
        },
        title: "src/main.rs".into(),
        prepared: Some(Prepared::new("src/main.rs".into(), &diff)),
        error: None,
    });
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 640.0))
        .wgpu()
        .build_ui(move |ui| w.ui(ui));
    theme::install(&harness.ctx, ThemePreference::Light);
    harness.run_steps(2);
    if let (Ok(img), Ok(dir)) = (harness.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("diff-light.png")).unwrap();
    }
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
}

/// Every file row of the work in progress opens the right diff.
#[test]
fn work_in_progress_rows_open_their_diffs() {
    let f = Fixture::busy_repository();
    let w = f.window();
    let keys: Vec<(String, Option<DiffKey>)> = w
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::File { path, diff, .. } => Some((path.clone(), diff.clone())),
            Row::Section { .. } => None,
        })
        .collect();
    let get = |p: &str| keys.iter().find(|(x, _)| x == p).unwrap().1.clone();
    assert_eq!(
        get("konflikt.txt"),
        None,
        "conflicts wait for the conflict view"
    );
    assert!(matches!(
        get("Cargo.toml"),
        Some(DiffKey::Worktree { staged: true, .. })
    ));
    assert!(matches!(
        get("src/main.rs"),
        Some(DiffKey::Worktree { staged: false, .. })
    ));
    assert!(matches!(
        get("notizen/Grüße aus Köln.txt"),
        Some(DiffKey::Untracked { .. })
    ));
    assert!(matches!(
        get("src/alt.rs -> src/neu.rs"),
        Some(DiffKey::Worktree {
            old: Some(_),
            staged: true,
            ..
        })
    ));
}

/// A search dims the other commits, jumps to the first match, and F3-style navigation wraps
/// around; commit ids are matched by prefix without git.
#[test]
fn history_search_marks_and_jumps() {
    let f = Fixture::new();
    for (i, s) in [
        "Parser anlegen",
        "Plan schreiben",
        "Parser testen",
        "Aufräumen",
    ]
    .iter()
    .enumerate()
    {
        f.write("a.txt", &i.to_string());
        f.commit_all(s);
    }
    let mut w = f.window();
    let ctx = egui::Context::default();
    w.search.open();
    w.search.set_for_test(Some("parser"), None);
    w.search.run(&w.session.as_ref().unwrap().reader(), &ctx);
    let deadline = Instant::now() + Duration::from_secs(20);
    while w.search.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        w.collect(&ctx);
    }
    w.collect(&ctx);
    let subjects = |w: &RepoWindow| match &w.selection {
        Selection::Commit(id) => w.history.commit(id).unwrap().subject.clone(),
        _ => String::new(),
    };
    assert_eq!(w.match_position(), (Some(1), 2));
    assert_eq!(
        subjects(&w),
        "Parser testen",
        "jumped to the first (newest) match"
    );
    w.jump(true);
    assert_eq!(subjects(&w), "Parser anlegen");
    w.jump(true);
    assert_eq!(subjects(&w), "Parser testen", "wraps around");
    w.jump(false);
    assert_eq!(subjects(&w), "Parser anlegen");
    shot("search-dark", ThemePreference::Dark, w);

    let mut w = f.window();
    let prefix = w.history.commits()[3].id.short().to_owned();
    w.search.open();
    w.search.set_for_test(Some(&prefix), Some(search::Kind::Id));
    w.search.run(&w.session.as_ref().unwrap().reader(), &ctx);
    w.collect_search_now();
    assert_eq!(subjects(&w), "Parser anlegen");
    w.search.close();
    assert!(!w.search.active());
}

/// The help window (F1) links to the donation page.
#[test]
fn help_window_opens_the_donation_page() {
    let mut w = RepoWindow::new(GitRunner::with_program("/nonexistent/git"), None);
    w.show_keys = true;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 640.0))
        .build_ui(move |ui| w.ui(ui));
    harness.run_steps(2);
    harness.get_by_label("GitCraft unterstützen …").click();
    harness.step();
    let opened =
        harness.output().platform_output.commands.iter().any(
            |c| matches!(c, egui::OutputCommand::OpenUrl(u) if u.url == SPONSOR_URL && u.new_tab),
        );
    assert!(opened, "the donation page was not opened");
}

/// Runs the window until git has answered (at most 20 s).
fn settle(harness: &mut Harness<'_, RepoWindow>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.step();
        if !harness.state().is_busy() {
            break;
        }
        assert!(Instant::now() < deadline, "git did not answer in time");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(2);
}

fn harness(w: RepoWindow) -> Harness<'static, RepoWindow> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 640.0))
        .wgpu()
        .build_ui_state(|ui, w: &mut RepoWindow| w.ui(ui), w);
    h.run_steps(2);
    h
}

fn staged_paths(w: &RepoWindow) -> Vec<String> {
    w.status()
        .unwrap()
        .staged()
        .map(|(f, _)| f.path.display())
        .collect()
}

/// "Alle vormerken" and "Alle zurücknehmen" in the change list run git and show the result.
#[test]
fn sections_stage_and_unstage_all_their_files() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("Anfang");
    f.write("a.txt", "2");
    f.write("neu.txt", "n");
    let mut h = harness(f.window());
    h.get_by_label("Alle vormerken").click();
    settle(&mut h);
    assert_eq!(staged_paths(h.state()), ["a.txt", "neu.txt"]);
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("staged-dark.png")).unwrap();
    }
    h.get_by_label("Alle zurücknehmen").click();
    settle(&mut h);
    assert!(staged_paths(h.state()).is_empty());
    assert_eq!(h.state().error(), None);
}

/// A double click stages a single file; the button in the diff header unstages it again.
#[test]
fn files_are_staged_by_double_click_and_from_the_diff() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.write("b.txt", "1");
    f.commit_all("Anfang");
    f.write("a.txt", "2");
    f.write("b.txt", "2");
    let mut h = harness(f.window());
    // A harness step is 0.25 s of simulated time: allow the double click to span frames.
    h.ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 5.0);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "a.txt")
        .click();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "a.txt")
        .click();
    settle(&mut h);
    assert_eq!(staged_paths(h.state()), ["a.txt"]);
    // The click before the double click opened the diff of the unstaged change; it follows
    // the file to the staged side.
    assert!(matches!(
        h.state().diff.as_ref().map(|d| &d.key),
        Some(DiffKey::Worktree { staged: true, .. })
    ));
    h.get_by_label("Zurücknehmen").click();
    settle(&mut h);
    assert!(staged_paths(h.state()).is_empty());
}

/// A repository whose configuration runs programs asks before the first write; trusting it
/// runs the write and is remembered.
#[test]
fn the_first_write_asks_to_trust_the_repository() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    assert!(f.git(&["config", "core.hooksPath", "eigene-hooks"]));
    let dir = tempfile::tempdir().unwrap();
    let mut w = f.window();
    w.use_settings(Some(dir.path().to_path_buf()));
    let mut h = harness(w);
    h.get_by_label("Alle vormerken").click();
    h.run_steps(4);
    assert!(h.state().trust_prompt.is_some());
    h.get_by_label("Vertrauen");
    if let (Ok(img), Ok(shots)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&shots).join("trust-dark.png")).unwrap();
    }
    h.get_by_label("Vertrauen").click();
    settle(&mut h);
    assert_eq!(staged_paths(h.state()), ["a.txt"]);
    let (saved, _) = settings::Settings::load(dir.path());
    assert_eq!(saved.trusted.len(), 1);

    // Opened again: trusted without asking.
    let mut w = f.window();
    w.use_settings(Some(dir.path().to_path_buf()));
    let session = w.session.take().unwrap();
    let snapshot = std::sync::Arc::new(w.snapshot.as_deref().unwrap().clone());
    w.show_opened(session, snapshot, &egui::Context::default());
    assert!(w.session.as_ref().unwrap().is_trusted());
}

/// "Nur lesen" keeps the repository unchanged and stops asking.
#[test]
fn read_only_refuses_writes_with_an_explanation() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    assert!(f.git(&["config", "core.sshCommand", "ssh -v"]));
    let mut h = harness(f.window());
    h.get_by_label("Alle vormerken").click();
    h.run_steps(4);
    h.get_by_label("Nur lesen").click();
    h.run_steps(4);
    assert!(h.state().trust_prompt.is_none());
    h.get_by_label("Alle vormerken").click();
    h.run_steps(4);
    assert!(h.state().trust_prompt.is_none(), "no second question");
    assert!(
        h.state()
            .error()
            .is_some_and(|e| e.contains("nur zum Lesen"))
    );
    assert!(staged_paths(h.state()).is_empty());
}
