//! The askpass bridge end to end: git starts the GitCraft executable in helper mode, which asks
//! the (test's) window and hands the answers back to git.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gitcraft_core::askpass::{Askpass, PromptKind};

fn answer_next(a: &Askpass, expect: PromptKind, answer: &str) {
    let start = Instant::now();
    loop {
        if let Some(p) = a.poll().pop() {
            assert_eq!(p.kind, expect, "{}", p.text);
            a.answer(p.id, Some(answer.to_owned()));
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "git did not ask");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn git_asks_for_username_and_password_through_the_window() {
    let a = Askpass::start(Arc::new(|| {})).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("git");
    cmd.current_dir(tmp.path())
        .args(["credential", "fill"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", tmp.path().join("none"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in a.env(std::path::Path::new(env!("CARGO_BIN_EXE_gitcraft"))) {
        cmd.env(k, v);
    }
    let mut git = cmd.spawn().unwrap();
    git.stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=example.org\n\n")
        .unwrap();
    answer_next(&a, PromptKind::Username, "erika");
    answer_next(&a, PromptKind::Secret, "pa ss");
    let out = git.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("username=erika\n"), "{text}");
    assert!(text.contains("password=pa ss\n"), "{text}");
}

#[test]
fn a_refused_prompt_fails_git() {
    let a = Askpass::start(Arc::new(|| {})).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("git");
    cmd.current_dir(tmp.path())
        .args(["credential", "fill"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", tmp.path().join("none"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in a.env(std::path::Path::new(env!("CARGO_BIN_EXE_gitcraft"))) {
        cmd.env(k, v);
    }
    let mut git = cmd.spawn().unwrap();
    git.stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=example.org\n\n")
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(p) = a.poll().pop() {
            a.answer(p.id, None);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = git.wait_with_output().unwrap();
    assert!(!out.status.success());
}
