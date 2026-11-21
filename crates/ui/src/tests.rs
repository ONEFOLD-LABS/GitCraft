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
        "Committen (im Commit-Formular)",
        "Zeilen im Diff wählen und vormerken",
        "Verwerfen …",
        "Löschen …",
        "Befehlsprotokoll ein/aus",
        "Neuer Branch",
        "Branch umbenennen",
        "Branch löschen?",
        "Commit auschecken?",
        "Commits würden unerreichbar",
        "Wechsel nicht möglich",
        "Änderungen parken (Stash)",
        "Stash löschen?",
        "Neuer Tag",
        "Rückgängig / Wiederholen (mit Rückfrage)",
        "Wiederholen?",
        "Rückgängig machen?",
        "Tag löschen?",
        "Die Nachricht fehlt.",
        "Was {b} seit der gemeinsamen Basis {basis} ändert (1 Datei)",
        "Was {b} seit der gemeinsamen Basis {basis} ändert ({n} Dateien)",
        "Der Name fehlt.",
        "Ein Name darf nicht mit „-“ beginnen.",
        "Ein Name darf keine Leerzeichen enthalten.",
        "Der Name ist unverändert.",
        "Löscht den Branch {name} mit 1 Commit, der auf keinem anderen Branch liegt:",
        "Löscht den Branch {name} mit {n} Commits, die auf keinem anderen Branch liegen:",
        "HEAD ist losgelöst und hält 1 Commit, der auf keinem Branch liegt. Nach dem Wechsel ist er nur noch über seine ID erreichbar:",
        "HEAD ist losgelöst und hält {n} Commits, die auf keinem Branch liegen. Nach dem Wechsel sind sie nur noch über ihre ID erreichbar:",
        "Verwirft die Änderungen an 1 Datei ({zeilen}).",
        "Verwirft die Änderungen an {n} Dateien ({zeilen}).",
        "Löscht 1 unversionierte Datei.",
        "Löscht {n} unversionierte Dateien.",
        "Hunk vormerken",
        "Hunk zurücknehmen",
        "Auswahl vormerken ({n})",
        "Auswahl zurücknehmen ({n})",
        "Diff oder Suche schließen",
        "Zoom",
        "Diese Übersicht",
        "Remote hinzufügen",
        "Remote bearbeiten",
        "Remote umbenennen",
        "Remote entfernen?",
        "Zugangsdaten aus der URL nehmen",
        "Unverschlüsselt: Daten (und Zugangsdaten) gehen im Klartext über das Netz.",
        "Vorspulen nicht möglich",
        "Branch zum ersten Mal hochladen",
        "Push abgelehnt",
        "Force-Push?",
        "Pull (nur Vorspulen)",
        "Pull mit Merge",
        "Pull mit Rebase",
    ]);
    texts.extend(network::PHASES);
    texts.extend(clone::CloneDialog::PROBLEMS);
    texts.extend(
        [
            gitcraft_git::UrlProblem::Empty,
            gitcraft_git::UrlProblem::LeadingDash,
            gitcraft_git::UrlProblem::Control,
            gitcraft_git::UrlProblem::Helper,
            gitcraft_git::UrlProblem::Scheme(String::new()),
        ]
        .iter()
        .map(remotes::problem_text),
    );
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

/// Focuses the text field labelled `label` and types `text`.
fn type_into(h: &mut Harness<'_, RepoWindow>, label: &str, text: &str) {
    use egui::accesskit::Role;
    let role = if h.query_by_role_and_label(Role::TextInput, label).is_some() {
        Role::TextInput
    } else {
        Role::MultilineTextInput
    };
    h.get_by_role_and_label(role, label).focus();
    h.step();
    h.get_by_role_and_label(role, label).type_text(text);
    h.step();
}

fn subjects_of(f: &Fixture) -> Vec<String> {
    let repo = Repository::open(&f.runner(), &f.repo()).unwrap();
    let mut log = repo.log().unwrap();
    log.next_page(100)
        .unwrap()
        .into_iter()
        .map(|c| c.subject)
        .collect()
}

/// Without an identity the form asks for it first; then a commit runs with Ctrl+Enter and the
/// form starts over.
#[test]
fn commit_form_sets_the_identity_and_commits() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    let mut h = harness(f.window());
    // The identity is read in the background when the repository opens.
    h.state()
        .session
        .as_ref()
        .unwrap()
        .submit(gitcraft_core::Command::Identity);
    settle(&mut h);
    type_into(&mut h, "Name", "Erika Muster");
    type_into(&mut h, "E-Mail", "erika@invalid");
    h.run_steps(2);
    h.get_by_label("Nur für dieses Repository").click();
    h.get_by_label("Speichern").click();
    settle(&mut h);
    assert!(h.state().commit.identity.as_ref().unwrap().is_complete());

    h.get_by_label("Alle vormerken").click();
    settle(&mut h);
    type_into(&mut h, "Zusammenfassung", "Erster Commit");
    type_into(&mut h, "Beschreibung (optional)", "Mit einer Beschreibung.");
    h.run_steps(2);
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("commit-form-dark.png"))
            .unwrap();
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(subjects_of(&f), ["Erster Commit"]);
    assert!(h.state().commit.summary.is_empty(), "the form starts over");
    assert!(h.state().status().unwrap().is_clean());
}

/// Amend loads the last message and replaces the commit.
#[test]
fn commit_form_amends_the_last_commit() {
    let f = Fixture::new();
    assert!(f.git(&["config", "user.name", "Test"]));
    assert!(f.git(&["config", "user.email", "t@invalid"]));
    f.write("a.txt", "1");
    f.commit_all("Tippfeler");
    f.write("b.txt", "2");
    let mut h = harness(f.window());
    h.state()
        .session
        .as_ref()
        .unwrap()
        .submit(gitcraft_core::Command::Identity);
    settle(&mut h);
    h.get_by_label("Alle vormerken").click();
    settle(&mut h);
    h.get_by_label("Letzten Commit ändern").click();
    settle(&mut h);
    assert_eq!(h.state().commit.summary, "Tippfeler");
    h.state_mut().commit.summary = "Tippfehler".into();
    h.run_steps(2);
    h.get_by_label("Commit ändern").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(subjects_of(&f), ["Tippfehler"]);
}

#[test]
fn render_commit_form_light() {
    let f = Fixture::new();
    assert!(f.git(&["config", "user.name", "Test"]));
    assert!(f.git(&["config", "user.email", "t@invalid"]));
    f.write("a.txt", "1");
    assert!(f.git(&["add", "a.txt"]));
    let mut w = f.window();
    w.commit.set_identity(gitcraft_git::Identity {
        name: Some("Test".into()),
        email: Some("t@invalid".into()),
    });
    w.commit.summary =
        "Eine Zusammenfassung, die deutlich länger ist als zweiundsiebzig Zeichen erlaubt".into();
    shot("commit-form-light", ThemePreference::Light, w);
}

/// Thirty lines; `changes` replaces line n with "n neu".
fn numbered(changes: &[usize]) -> String {
    (1..=30)
        .map(|n| {
            if changes.contains(&n) {
                format!("{n} neu\n")
            } else {
                format!("{n}\n")
            }
        })
        .collect()
}

/// A window showing the unstaged diff of a file with three hunks.
fn three_hunks() -> (Fixture, Harness<'static, RepoWindow>) {
    let f = Fixture::new();
    f.write("f.txt", &numbered(&[]));
    f.commit_all("Anfang");
    f.write("f.txt", &numbered(&[2, 15, 28]));
    let mut w = f.window();
    w.open_diff(
        DiffKey::Worktree {
            path: RepoPath::from_bytes(b"f.txt"),
            old: None,
            staged: false,
        },
        "f.txt".into(),
    );
    let mut h = harness(w);
    settle(&mut h);
    assert!(h.state().diff.as_ref().unwrap().prepared.is_some());
    (f, h)
}

fn index_text(f: &Fixture) -> String {
    let out = Command::new("git")
        .current_dir(f.repo())
        .args(["show", ":f.txt"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_hunk_button_stages_one_hunk() {
    let (f, mut h) = three_hunks();
    let buttons = h.get_all_by_label("Hunk vormerken").collect::<Vec<_>>();
    assert_eq!(buttons.len(), 3);
    buttons[1].click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(index_text(&f), numbered(&[15]));
}

/// Arrow keys pick changed lines, Shift extends, Enter stages the selection.
#[test]
fn keyboard_selects_and_stages_lines() {
    let (f, mut h) = three_hunks();
    // First changed line: "-2"; Shift+Down adds "+2 neu".
    h.key_press(egui::Key::ArrowDown);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowDown);
    h.run_steps(2);
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("diff-lines-dark.png"))
            .unwrap();
    }
    h.get_by_label("Auswahl vormerken (2)");
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(index_text(&f), numbered(&[2]));
}

/// Changed after display: nothing is applied and the user is told.
#[test]
fn a_stale_diff_is_not_applied() {
    let (f, mut h) = three_hunks();
    f.write("f.txt", &numbered(&[2, 15, 20, 28]));
    h.get_all_by_label("Hunk vormerken").next().unwrap().click();
    settle(&mut h);
    assert!(h.state().error().is_some_and(|e| e.contains("geändert")));
    assert_eq!(index_text(&f), numbered(&[]));
}

fn file_text(f: &Fixture, rel: &str) -> Option<String> {
    fs::read_to_string(f.repo().join(rel)).ok()
}

/// Discard from the context menu asks first (naming files and lines), then saves, discards and
/// offers to undo.
#[test]
fn discard_asks_first_and_can_be_undone() {
    let f = Fixture::new();
    f.write("a.txt", "1\n2\n");
    f.commit_all("Anfang");
    f.write("a.txt", "eins\n2\ndrei\n");
    let mut h = harness(f.window());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "a.txt")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Verwerfen …").click();
    settle(&mut h);
    h.get_by_label("Verwirft die Änderungen an 1 Datei (3 Zeilen).");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("discard-confirm-dark.png"))
            .unwrap();
    }
    // Cancel changes nothing.
    h.get_by_label("Abbrechen").click();
    h.run_steps(2);
    assert!(h.state().confirm.is_none());
    assert_eq!(file_text(&f, "a.txt").as_deref(), Some("eins\n2\ndrei\n"));

    h.get_by_role_and_label(egui::accesskit::Role::Button, "a.txt")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Verwerfen …").click();
    settle(&mut h);
    h.get_by_label("Verwerfen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(file_text(&f, "a.txt").as_deref(), Some("1\n2\n"));
    assert_eq!(h.state().discarded.len(), 1);
    h.get_by_label("1 Datei verworfen.");
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert_eq!(file_text(&f, "a.txt").as_deref(), Some("eins\n2\ndrei\n"));
}

/// An untracked file is deleted from its diff and comes back from "Zuletzt verworfen".
#[test]
fn delete_untracked_and_restore_from_the_list() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("Anfang");
    f.write("neu/notiz.txt", "wichtig");
    let mut w = f.window();
    w.open_diff(
        DiffKey::Untracked {
            path: RepoPath::from_bytes(b"neu/notiz.txt"),
        },
        "neu/notiz.txt".into(),
    );
    let mut h = harness(w);
    settle(&mut h);
    h.get_by_label("Löschen …").click();
    settle(&mut h);
    h.get_by_label("Löscht 1 unversionierte Datei.");
    h.get_by_label("Löschen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert!(!f.repo().join("neu").exists());
    h.get_by_label("Schließen").click();
    settle(&mut h);
    h.get_by_label("Zuletzt verworfen (1)").click();
    settle(&mut h);
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert_eq!(file_text(&f, "neu/notiz.txt").as_deref(), Some("wichtig"));
}

/// The command log lists the calls, filters writes and copies a line that runs in a shell.
#[test]
fn command_log_shows_and_copies_calls() {
    let f = Fixture::new();
    f.write("a b.txt", "1");
    let mut h = harness(f.window());
    h.get_by_label("Alle vormerken").click();
    settle(&mut h);
    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::L,
    );
    settle(&mut h);
    assert!(h.state().log_panel.open);
    // The line is the POSIX form; Windows copies the PowerShell one.
    #[cfg(unix)]
    {
        let add = "git -c core.fsmonitor=false --literal-pathspecs add --all --pathspec-from-file=- --pathspec-file-nul";
        h.get_by_role_and_label(egui::accesskit::Role::Button, add)
            .click();
        h.step();
        let copied = h
            .output()
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == add));
        assert!(copied, "the clicked line was copied");
    }
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("command-log-dark.png"))
            .unwrap();
    }
    h.get_by_label("Nur ändernde Befehle").click();
    settle(&mut h);
    assert!(
        h.query_by_label_contains("status --porcelain").is_none(),
        "reads are hidden"
    );
    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::L,
    );
    settle(&mut h);
    assert!(!h.state().log_panel.open);
}

fn head_branch(w: &RepoWindow) -> String {
    match &w.status().unwrap().head {
        gitcraft_git::Head::Branch { name, .. } => name.clone(),
        gitcraft_git::Head::Detached { .. } => "(losgelöst)".into(),
    }
}

/// main with two commits and identity set.
fn two_commit_fixture() -> Fixture {
    let f = Fixture::new();
    assert!(f.git(&["config", "user.name", "Test"]));
    assert!(f.git(&["config", "user.email", "t@invalid"]));
    f.write("a.txt", "1\n");
    f.commit_all("Eins");
    f.write("a.txt", "2\n");
    f.commit_all("Zwei");
    f
}

#[test]
fn toolbar_creates_and_checks_out_a_branch() {
    let f = two_commit_fixture();
    let mut h = harness(f.window());
    h.get_by_label("Branch").click();
    settle(&mut h);
    type_into(&mut h, "Name", "feature/neu");
    h.get_by_label("Anlegen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(head_branch(h.state()), "feature/neu");
}

#[test]
fn sidebar_switches_renames_and_deletes_with_restore() {
    let f = two_commit_fixture();
    assert!(f.git(&["checkout", "-q", "-b", "idee"]));
    f.write("idee.txt", "x");
    f.commit_all("Versuch");
    assert!(f.git(&["checkout", "-q", "main"]));
    let mut h = harness(f.window());
    // Double click switches.
    h.ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 5.0);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "idee")
        .click();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "idee")
        .click();
    settle(&mut h);
    assert_eq!(head_branch(h.state()), "idee");
    // Back to main from the context menu.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "main")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Auschecken").click();
    settle(&mut h);
    assert_eq!(head_branch(h.state()), "main");
    // Rename.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "idee")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Umbenennen …").click();
    settle(&mut h);
    h.state_mut().branch_dialog = Some(branches::Dialog::Rename {
        old: "idee".into(),
        name: "idee-2".into(),
        focus: false,
    });
    h.run_steps(2);
    h.get_by_label("Umbenennen").click();
    settle(&mut h);
    assert!(
        h.state()
            .snapshot
            .as_ref()
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "idee-2")
    );
    // Delete: the dialog names the commit only this branch has.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "idee-2")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Löschen …").click();
    settle(&mut h);
    h.get_by_label("Löscht den Branch idee-2 mit 1 Commit, der auf keinem anderen Branch liegt:");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("branch-delete-dark.png"))
            .unwrap();
    }
    h.get_by_label("Löschen").click();
    settle(&mut h);
    assert!(
        !h.state()
            .snapshot
            .as_ref()
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "idee-2")
    );
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert!(
        h.state()
            .snapshot
            .as_ref()
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "idee-2")
    );
}

/// Checking out a commit explains detached HEAD; leaving it with a new commit warns.
#[test]
fn detached_head_is_explained_and_leaving_it_warns() {
    let f = two_commit_fixture();
    let first = {
        let w = f.window();
        w.history.commits()[1].id.clone()
    };
    let mut h = harness(f.window());
    h.state_mut().branch_dialog = Some(branches::Dialog::Detach {
        id: first.clone(),
        label: "Eins".into(),
    });
    h.run_steps(2);
    h.get_by_label("Auschecken").click();
    settle(&mut h);
    assert_eq!(head_branch(h.state()), "(losgelöst)");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("detached-dark.png")).unwrap();
    }
    // A commit only the detached HEAD holds.
    f.write("b.txt", "x");
    f.commit_all("Experiment");
    h.state_mut().refresh();
    settle(&mut h);
    h.state_mut()
        .switch(branches::Target::Branch("main".into()));
    settle(&mut h);
    assert!(matches!(
        h.state().branch_dialog,
        Some(branches::Dialog::Leave { loss: Some(_), .. })
    ));
    h.get_by_label("Trotzdem wechseln").click();
    settle(&mut h);
    assert_eq!(head_branch(h.state()), "main");
}

/// A switch that would overwrite local changes stops and lists the files.
#[test]
fn a_switch_over_local_changes_lists_them() {
    let f = two_commit_fixture();
    assert!(f.git(&["branch", "alt", "HEAD~1"]));
    f.write("a.txt", "lokal\n");
    let mut h = harness(f.window());
    h.state_mut().switch(branches::Target::Branch("alt".into()));
    settle(&mut h);
    assert_eq!(
        h.state().branch_dialog,
        Some(branches::Dialog::Overwrite {
            files: vec!["a.txt".into()],
            target: Some(branches::Target::Branch("alt".into())),
        })
    );
    assert_eq!(head_branch(h.state()), "main");
}

fn stash_count(w: &RepoWindow) -> usize {
    w.snapshot.as_ref().map_or(0, |s| s.stashes.len())
}

/// Stash from the toolbar, look at it, pop it back; drop one and restore it.
#[test]
fn stash_park_show_pop_drop_restore() {
    let f = two_commit_fixture();
    f.write("a.txt", "geparkt\n");
    let mut h = harness(f.window());
    h.get_by_label("Stash").click();
    settle(&mut h);
    type_into(&mut h, "Nachricht (optional)", "Zwischenstand");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("stash-dark.png")).unwrap();
    }
    h.get_by_label("Stashen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(stash_count(h.state()), 1);
    assert_eq!(file_text(&f, "a.txt").as_deref(), Some("2\n"));

    // Shown like a commit.
    let stash = h.state().snapshot.as_ref().unwrap().stashes[0].clone();
    h.state_mut()
        .session
        .as_ref()
        .unwrap()
        .submit(gitcraft_core::Command::LoadCommit(stash.id.clone()));
    settle(&mut h);
    assert_eq!(h.state().selection, Selection::Commit(stash.id.clone()));
    // Its files load once it is selected.
    settle(&mut h);
    h.get_by_label("1 Datei geändert");

    h.get_by_label("Pop").click();
    settle(&mut h);
    assert_eq!(stash_count(h.state()), 0);
    assert_eq!(file_text(&f, "a.txt").as_deref(), Some("geparkt\n"));

    // Drop asks, then restore by id.
    assert!(f.git(&["stash", "push", "-q", "-m", "wegwerfen"]));
    h.state_mut().refresh();
    settle(&mut h);
    let stash = h.state().snapshot.as_ref().unwrap().stashes[0].clone();
    h.state_mut().branch_dialog = Some(branches::Dialog::DropStash {
        selector: stash.selector.clone(),
        id: stash.id.clone(),
        message: stash.message.clone(),
    });
    h.run_steps(2);
    h.get_by_label("Löschen").click();
    settle(&mut h);
    assert_eq!(stash_count(h.state()), 0);
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert_eq!(stash_count(h.state()), 1);
}

/// "Stashen und wechseln" parks the clashing changes and switches.
#[test]
fn stash_and_switch_from_the_overwrite_dialog() {
    let f = two_commit_fixture();
    assert!(f.git(&["branch", "alt", "HEAD~1"]));
    f.write("a.txt", "lokal\n");
    let mut h = harness(f.window());
    h.state_mut().switch(branches::Target::Branch("alt".into()));
    settle(&mut h);
    h.get_by_label("Stashen und wechseln").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(head_branch(h.state()), "alt");
    assert_eq!(stash_count(h.state()), 1);
}

fn tag_names(w: &RepoWindow) -> Vec<String> {
    w.snapshot
        .as_ref()
        .unwrap()
        .refs
        .iter()
        .filter(|r| r.kind == gitcraft_git::RefKind::Tag)
        .map(|r| r.name.clone())
        .collect()
}

/// An annotated tag from the history's context menu; deleting it asks and can be undone.
#[test]
fn tags_are_created_deleted_and_restored() {
    let f = two_commit_fixture();
    let mut h = harness(f.window());
    let head = h.state().history.commits()[0].id.clone();
    h.state_mut().branch_dialog = Some(tag_dialog(head.as_str().to_owned(), "Zwei".into()));
    h.run_steps(2);
    type_into(&mut h, "Name", "v1.0");
    h.get_by_label("Mit Nachricht (annotierter Tag)").click();
    h.run_steps(2);
    type_into(&mut h, "Nachricht", "Erste Version");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("tag-dark.png")).unwrap();
    }
    h.get_by_label("Anlegen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(tag_names(h.state()), ["v1.0"]);

    h.state_mut().branch_dialog = Some(branches::Dialog::DeleteTag {
        name: "v1.0".into(),
    });
    h.run_steps(2);
    h.get_by_label("Löschen").click();
    settle(&mut h);
    assert!(tag_names(h.state()).is_empty());
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert_eq!(tag_names(h.state()), ["v1.0"]);
}

/// Comparing HEAD with a diverged branch lists both sides and opens a file's diff.
#[test]
fn compare_with_a_branch() {
    let f = two_commit_fixture();
    assert!(f.git(&["checkout", "-q", "-b", "feature"]));
    f.write("neu.txt", "n\n");
    f.commit_all("Feature");
    assert!(f.git(&["checkout", "-q", "main"]));
    f.write("a.txt", "3\n");
    f.commit_all("Drei");
    let mut h = harness(f.window());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "feature")
        .click_secondary();
    h.run_steps(2);
    h.get_by_label("Mit HEAD vergleichen").click();
    settle(&mut h);
    h.get_by_label("Nur in main (1)");
    h.get_by_label("Nur in feature (1)");
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("compare-dark.png")).unwrap();
    }
    h.get_by_label("neu.txt").click();
    settle(&mut h);
    assert!(matches!(
        h.state().diff.as_ref().map(|d| &d.key),
        Some(DiffKey::Range { .. })
    ));
    assert!(h.state().diff.as_ref().unwrap().prepared.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().diff.is_none() && h.state().compare.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().compare.is_none());
}

fn head_id(f: &Fixture) -> String {
    let out = Command::new("git")
        .current_dir(f.repo())
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A commit made in GitCraft is undone softly (changes stay staged) and redone.
#[test]
fn undo_and_redo_a_commit() {
    let f = two_commit_fixture();
    let before = head_id(&f);
    f.write("b.txt", "neu\n");
    let mut h = harness(f.window());
    h.get_by_label("Alle vormerken").click();
    settle(&mut h);
    h.state_mut().commit.set_identity(gitcraft_git::Identity {
        name: Some("Test".into()),
        email: Some("t@invalid".into()),
    });
    type_into(&mut h, "Zusammenfassung", "Drei");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    settle(&mut h);
    let after = head_id(&f);
    assert_ne!(before, after);

    h.get_by_label("Rückgängig").click();
    settle(&mut h);
    h.get_by_label(&format!("git reset --soft {before}"));
    if let (Ok(img), Ok(dir)) = (h.render(), std::env::var("GITCRAFT_SHOTS")) {
        img.save(Path::new(&dir).join("undo-dark.png")).unwrap();
    }
    h.get_by_label("Rückgängig machen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(head_id(&f), before);
    assert_eq!(
        staged_paths(h.state()),
        ["b.txt"],
        "the changes stay staged"
    );

    h.key_press_modifiers(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    settle(&mut h);
    h.get_by_label("Wiederholen?");
    h.get_by_label("Erneut ausführen").click();
    settle(&mut h);
    assert_eq!(head_id(&f), after);
}

/// A new branch that was checked out: undo switches back and removes it.
#[test]
fn undo_a_new_branch() {
    let f = two_commit_fixture();
    let mut h = harness(f.window());
    h.get_by_label("Branch").click();
    settle(&mut h);
    type_into(&mut h, "Name", "versuch");
    h.get_by_label("Anlegen").click();
    settle(&mut h);
    assert_eq!(head_branch(h.state()), "versuch");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    settle(&mut h);
    h.get_by_label("Rückgängig machen").click();
    settle(&mut h);
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(head_branch(h.state()), "main");
    assert!(
        !h.state()
            .snapshot
            .as_ref()
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "versuch")
    );
}

fn remotes_of(f: &Fixture) -> String {
    let out = Command::new("git")
        .current_dir(f.repo())
        .args(["remote", "-v"])
        .env("GIT_CONFIG_GLOBAL", f.tmp.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Remotes are added from the sidebar (hostile URLs refused while typing), removed after a
/// confirmation and restored from the notice.
#[test]
fn remotes_are_added_removed_and_restored() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("Anfang");
    let mut h = harness(f.window());
    h.get_by_label("Remote hinzufügen …").click();
    settle(&mut h);
    type_into(&mut h, "Name", "origin");
    type_into(&mut h, "URL", "ext::sh -c touch% x");
    h.run_steps(2);
    h.get_by_label(
        "Transport-Helfer („ext::“, „fd::“ …) können beliebige Programme starten; GitCraft nimmt sie nicht an.",
    );
    assert!({
        use egui_kittest::kittest::NodeT;
        h.get_by_label("Hinzufügen").accesskit_node().is_disabled()
    });
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "URL")
        .focus();
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "URL")
        .type_text("http://example.org/r.git");
    h.run_steps(2);
    h.get_by_label("Unverschlüsselt: Daten (und Zugangsdaten) gehen im Klartext über das Netz.");
    h.get_by_label("Hinzufügen").hover();
    h.step();
    h.get_by_label("Hinzufügen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert!(remotes_of(&f).contains("origin\thttp://example.org/r.git"));

    h.get_by_label("origin").click_secondary();
    settle(&mut h);
    h.get_by_label("Entfernen …").click();
    settle(&mut h);
    assert!(
        matches!(
            h.state().remote_dialog,
            Some(remotes::Dialog::Remove { .. })
        ),
        "{:?}",
        h.state().remote_dialog
    );
    h.get_by_label("Entfernen").click();
    settle(&mut h);
    assert_eq!(remotes_of(&f), "");
    h.get_by_label("Wiederherstellen").click();
    settle(&mut h);
    assert!(remotes_of(&f).contains("origin\thttp://example.org/r.git"));
}

/// Remote branches grouped under their remotes; a URL with a password and an unencrypted one
/// in the warning colour.
#[test]
fn render_remotes() {
    let f = Fixture::new();
    f.write("a.txt", "1");
    f.commit_all("Anfang");
    let main = "refs/heads/main";
    for (remote, url) in [
        ("origin", "https://example.org/demo.git"),
        ("team/spiegel", "https://me:geheim@example.org/spiegel.git"),
        ("alt", "git://example.org/alt.git"),
    ] {
        assert!(f.git(&["remote", "add", remote, url]));
        for branch in ["main", "feature/x"] {
            let r = format!("refs/remotes/{remote}/{branch}");
            assert!(f.git(&["update-ref", &r, main]));
        }
    }
    shot("remotes-dark", ThemePreference::Dark, f.window());
    shot("remotes-light", ThemePreference::Light, f.window());
}

/// Runs git in `dir` (outside the repository) with the fixture's isolated configuration.
fn git_in(f: &Fixture, dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", f.tmp.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The repository pushed to a bare `up.git`, and a second clone `other` of it.
fn fixture_with_remote() -> Fixture {
    let f = Fixture::new();
    f.write("a.txt", "1\n");
    f.commit_all("Anfang");
    let up = f.tmp.path().join("up.git");
    git_in(
        &f,
        f.tmp.path(),
        &["init", "-q", "--bare", "-b", "main", up.to_str().unwrap()],
    );
    assert!(f.git(&["remote", "add", "origin", up.to_str().unwrap()]));
    assert!(f.git(&["push", "-q", "-u", "origin", "main"]));
    git_in(
        &f,
        f.tmp.path(),
        &["clone", "-q", up.to_str().unwrap(), "other"],
    );
    assert!(f.git(&["config", "user.name", "Test"]));
    assert!(f.git(&["config", "user.email", "test@invalid"]));
    f
}

fn push_from_other(f: &Fixture, file: &str) {
    let other = f.tmp.path().join("other");
    fs::write(other.join(file), file).unwrap();
    git_in(f, &other, &["add", "-A"]);
    git_in(f, &other, &["commit", "-q", "-m", file]);
    git_in(f, &other, &["push", "-q", "origin", "HEAD"]);
}

/// Fetch names what came in; Pull fast-forwards; when both sides moved on, the dialog explains
/// it and merges only on request.
#[test]
fn fetch_and_pull_from_the_toolbar() {
    crate::i18n::set_language(crate::i18n::Language::German);
    let f = fixture_with_remote();
    push_from_other(&f, "b.txt");
    push_from_other(&f, "c.txt");
    let mut h = harness(f.window());
    h.get_by_label("Fetch").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    let notice = h.state().notice.clone().map(|(text, _)| text);
    assert_eq!(
        notice.as_deref(),
        Some("Fetch fertig: origin/main +2 Commits.")
    );
    h.get_by_label("Pull").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert!(f.repo().join("c.txt").exists());

    push_from_other(&f, "d.txt");
    f.write("e.txt", "lokal");
    f.commit_all("lokal");
    h.state_mut().refresh();
    settle(&mut h);
    h.get_by_label("Pull").click();
    settle(&mut h);
    h.get_by_label("Vorspulen nicht möglich");
    assert!(!f.repo().join("d.txt").exists(), "nothing changed yet");
    h.get_by_label("Mergen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert!(f.repo().join("d.txt").exists() && f.repo().join("e.txt").exists());
}

#[test]
fn render_diverged_pull() {
    let f = fixture_with_remote();
    let mut w = f.window();
    w.remote_dialog = Some(remotes::Dialog::Diverged {
        branch: "main".into(),
        upstream: "origin/main".into(),
        ahead: 1,
        behind: 2,
    });
    shot("diverged-dark", ThemePreference::Dark, w);
}

fn up_tip(f: &Fixture, rev: &str) -> String {
    let out = Command::new("git")
        .current_dir(f.tmp.path().join("up.git"))
        .args(["rev-parse", "--verify", "--quiet", rev])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn local_head(f: &Fixture) -> String {
    let out = Command::new("git")
        .current_dir(f.repo())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The first push asks where to and sets the upstream; a rejected push is explained, and a
/// force push names the remote commits it removes before it runs.
#[test]
fn push_first_rejected_and_forced() {
    crate::i18n::set_language(crate::i18n::Language::German);
    let f = fixture_with_remote();
    assert!(f.git(&["switch", "-q", "-c", "neu"]));
    f.write("n.txt", "n");
    f.commit_all("neu");
    let mut h = harness(f.window());
    h.get_by_label("Push").click();
    settle(&mut h);
    h.get_by_label("Nach origin/neu pushen").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(up_tip(&f, "neu"), local_head(&f));

    assert!(f.git(&["switch", "-q", "main"]));
    push_from_other(&f, "fremd.txt");
    f.write("eigen.txt", "e");
    f.commit_all("eigen");
    h.state_mut().refresh();
    settle(&mut h);
    h.get_by_label("Push").click();
    settle(&mut h);
    h.get_by_label("Push abgelehnt");
    h.get_by_label("Force-Push …").click();
    settle(&mut h);
    h.get_by_label("Force-Push?");
    h.get_by_label_contains("Dieser Commit verschwindet vom Remote");
    h.get_by_label("Überschreiben").click();
    settle(&mut h);
    assert_eq!(h.state().error(), None);
    assert_eq!(up_tip(&f, "main"), local_head(&f));
}

#[test]
fn render_force_push() {
    let f = fixture_with_remote();
    let mut w = f.window();
    let id = w.history.commits()[0].id.clone();
    w.remote_dialog = Some(remotes::Dialog::Force {
        info: Some((
            "origin/main".into(),
            Some(id.clone()),
            gitcraft_git::Loss {
                commits: vec![gitcraft_git::LostCommit {
                    id,
                    subject: "Fremde Änderung".into(),
                }],
                total: 1,
            },
        )),
    });
    shot("force-push-light", ThemePreference::Light, w);
}

/// git's question reaches the window through the askpass bridge; the answer goes back, a
/// refused one as a refusal.
#[test]
fn git_prompts_are_answered_in_the_window() {
    crate::i18n::set_language(crate::i18n::Language::German);
    let f = Fixture::new();
    let mut h = harness(f.window());
    let ctx = h.ctx.clone();
    h.state_mut().use_askpass(&ctx, Path::new("gitcraft"));
    let env = h
        .state()
        .askpass
        .as_ref()
        .unwrap()
        .env(Path::new("gitcraft"));
    let get = |k: &str| {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .unwrap()
    };
    let (addr, token) = (
        get(gitcraft_core::askpass::HELPER_ADDR_ENV),
        get(gitcraft_core::askpass::HELPER_TOKEN_ENV),
    );
    let ask = |prompt: &'static str| {
        let (addr, token) = (addr.clone(), token.clone());
        std::thread::spawn(move || gitcraft_core::askpass::request(&addr, &token, prompt))
    };
    let asking = ask("Password for 'https://erika@example.org': ");
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.state().prompt.is_none() {
        assert!(Instant::now() < deadline, "no prompt");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label("Anmeldung");
    {
        use egui::accesskit::Role;
        let label = "Password for 'https://erika@example.org':";
        h.get_by_role_and_label(Role::PasswordInput, label).focus();
        h.step();
        h.get_by_role_and_label(Role::PasswordInput, label)
            .type_text("geheim");
        h.step();
    }
    h.get_by_label("OK").hover();
    h.step();
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(asking.join().unwrap().unwrap(), Some("geheim".into()));

    let asking = ask("Username for 'https://example.org': ");
    while h.state().prompt.is_none() {
        assert!(Instant::now() < deadline, "no prompt");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label("Abbrechen").click();
    h.run_steps(2);
    assert_eq!(asking.join().unwrap().unwrap(), None);
}

#[test]
fn render_host_key_prompt() {
    let f = Fixture::new();
    let mut w = f.window();
    w.prompts.push_back(gitcraft_core::askpass::Prompt {
        id: 1,
        text: String::new(),
        kind: gitcraft_core::askpass::PromptKind::HostKey {
            host: "example.org (192.0.2.1)".into(),
            fingerprint: "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU".into(),
        },
    });
    shot("host-key-dark", ThemePreference::Dark, w);
}

/// Cloning from the start screen: the folder name follows the URL, the clone opens with a
/// notice that names submodules it left out.
#[test]
fn clone_from_the_start_screen() {
    crate::i18n::set_language(crate::i18n::Language::German);
    let f = fixture_with_remote();
    fs::write(f.repo().join(".gitmodules"), "[submodule \"x\"]\n").unwrap();
    f.commit_all("Submodul-Eintrag");
    assert!(f.git(&["push", "-q", "origin", "main"]));
    let mut h = harness(RepoWindow::new(f.runner(), None));
    h.get_by_label("Repository klonen …").click();
    settle(&mut h);
    let up = f.tmp.path().join("up.git");
    type_into(&mut h, "URL", up.to_str().unwrap());
    h.run_steps(2);
    assert_eq!(h.state().clone.as_ref().unwrap().name, "up");
    h.state_mut().clone.as_mut().unwrap().parent = f.tmp.path().display().to_string();
    h.run_steps(2);
    h.get_by_label("Klonen").hover();
    h.step();
    h.get_by_label("Klonen").click();
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.state().session.is_none() {
        assert!(
            Instant::now() < deadline,
            "clone did not finish: {:?}",
            h.state().error()
        );
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(&mut h);
    assert!(f.tmp.path().join("up/a.txt").exists());
    let notice = h.state().notice.clone().map(|(t, _)| t).unwrap_or_default();
    assert!(notice.contains("Submodule"), "{notice}");
}

#[test]
fn render_clone_dialog() {
    let f = Fixture::new();
    let mut w = RepoWindow::new(f.runner(), None);
    let mut d = clone::CloneDialog::new(f.tmp.path());
    d.url = "http://example.org/team/projekt.git".into();
    d.name = "projekt".into();
    d.depth = "1".into();
    w.clone = Some(d);
    shot("clone-light", ThemePreference::Light, w);
}
