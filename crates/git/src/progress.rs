//! Progress lines git writes to stderr with `--progress` (fetch, pull, push, clone):
//! `Receiving objects:  45% (450/1000), 1.20 MiB | 600 KiB/s`, `remote: Counting objects: 5,
//! done.`. Lines end with `\r` while a phase runs and with `\n` when it is done. The server's
//! lines (`remote: …`) may be in its language.

use std::sync::Arc;

/// One progress report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The phase as git names it (`Receiving objects`); from the server when `remote`.
    pub phase: String,
    pub remote: bool,
    /// Units done (objects, deltas).
    pub done: u64,
    /// Units in all, when git knows it.
    pub total: Option<u64>,
}

impl Progress {
    /// Parses one line; `None` for everything that is not a progress report (errors, ref
    /// updates, hints), which callers keep as git's message.
    pub fn parse(line: &str) -> Option<Progress> {
        let line = line.trim();
        let (remote, line) = match line.strip_prefix("remote:") {
            Some(rest) => (true, rest.trim_start()),
            None => (false, line),
        };
        let (phase, rest) = line.split_once(": ")?;
        if phase.is_empty()
            || !phase
                .chars()
                .all(|c| c.is_alphabetic() || c == ' ' || c == '-')
        {
            return None;
        }
        let rest = rest.trim_start();
        let (done, total) = if let Some(open) = rest.find('(') {
            // `45% (450/1000)…`
            let percent = rest[..open].trim();
            if !percent.ends_with('%') {
                return None;
            }
            let inner = &rest[open + 1..rest[open..].find(')')? + open];
            let (a, b) = inner.split_once('/')?;
            (a.trim().parse().ok()?, Some(b.trim().parse().ok()?))
        } else {
            // `5, done.` or `5`
            let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = &rest[number.len()..];
            if number.is_empty() || !(after.is_empty() || after.starts_with(',')) {
                return None;
            }
            (number.parse().ok()?, None)
        };
        Some(Progress {
            phase: phase.to_owned(),
            remote,
            done,
            total,
        })
    }

    /// Done as a fraction of the total (0.0–1.0), when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((self.done.min(total) as f64 / total as f64) as f32)
    }
}

/// Receives progress reports on the thread that reads git's stderr.
pub type OnProgress = Arc<dyn Fn(Progress) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        let p =
            Progress::parse("Receiving objects:  45% (450/1000), 1.20 MiB | 600 KiB/s   ").unwrap();
        assert_eq!(p.phase, "Receiving objects");
        assert!(!p.remote);
        assert_eq!((p.done, p.total), (450, Some(1000)));
        assert_eq!(p.fraction(), Some(0.45));
        let p = Progress::parse("remote: Counting objects: 100% (4/4), done.").unwrap();
        assert!(p.remote);
        assert_eq!((p.done, p.total), (4, Some(4)));
        let p = Progress::parse("remote: Enumerating objects: 5, done.").unwrap();
        assert_eq!((p.done, p.total), (5, None));
        assert_eq!(p.fraction(), None);
        let p = Progress::parse("remote: Zähle Objekte:  25% (1/4)").unwrap();
        assert_eq!(p.phase, "Zähle Objekte");
    }

    #[test]
    fn other_lines_are_not_progress() {
        for line in [
            "fatal: repository 'x' not found",
            "error: failed to push some refs to 'x'",
            "From /srv/up",
            " * [new branch]      main       -> origin/main",
            "   1111111..2222222  main       -> origin/main",
            "hint: Updates were rejected",
            "fatal: unable to access 'https://h/': Could not resolve host: h",
            "remote: Resolving deltas: x% (a/b)",
            "Total 3 (delta 0), reused 0 (delta 0), pack-reused 0",
            "",
        ] {
            assert_eq!(Progress::parse(line), None, "{line:?}");
        }
    }
}
