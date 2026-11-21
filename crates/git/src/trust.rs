//! What a repository could make git run (docs/security.md T1, issue #18): settings in its own
//! configuration (local and worktree scope, includes followed) that name a program, and the
//! hooks in its hooks directory. GitCraft switches such settings off for reads; before the first
//! write the user decides whether to trust them.

use std::path::{Path, PathBuf};

use crate::{GitError, Repository};

/// Hooks listed at most.
const MAX_HOOKS: usize = 200;

/// One setting from the repository's own configuration that makes git run a program.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgramSetting {
    /// As git prints it (section and name lower-case, subsection as written).
    pub key: String,
    pub value: String,
}

/// Everything a write in this repository could run beyond git itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustReport {
    /// Sorted by key.
    pub settings: Vec<ProgramSetting>,
    /// File names of executable hooks (samples left out), sorted.
    pub hooks: Vec<String>,
    /// Where git looks for hooks (`core.hooksPath` or `.git/hooks`).
    pub hooks_dir: PathBuf,
}

impl TrustReport {
    /// Nothing to trust: writes can run without asking.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.hooks.is_empty()
    }

    /// The report as stable text (one line per setting and hook), for a fingerprint that
    /// changes whenever what would run changes.
    pub fn canonical(&self) -> String {
        let mut text = String::new();
        for s in &self.settings {
            text.push_str(&format!("setting {:?} {:?}\n", s.key, s.value));
        }
        text.push_str(&format!("hooks-dir {:?}\n", self.hooks_dir));
        for h in &self.hooks {
            text.push_str(&format!("hook {h:?}\n"));
        }
        text
    }
}

impl Repository {
    /// Lists the program settings and hooks of this repository.
    pub fn trust_report(&self) -> Result<TrustReport, GitError> {
        let out = self.read(["config", "--show-scope", "--null", "--list"])?;
        let mut settings = parse_settings(&out)?;
        settings.sort();
        settings.dedup();
        let dir = self.read(["rev-parse", "--git-path", "hooks"])?;
        let dir = String::from_utf8_lossy(&dir)
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        let hooks_dir = if Path::new(&dir).is_absolute() {
            PathBuf::from(dir)
        } else {
            self.workdir().join(dir)
        };
        Ok(TrustReport {
            settings,
            hooks: hooks_in(&hooks_dir),
            hooks_dir,
        })
    }
}

/// Program settings of local and worktree scope from `config --show-scope --null --list`
/// (`scope NUL key LF value NUL`, a key without value has no LF).
fn parse_settings(out: &[u8]) -> Result<Vec<ProgramSetting>, GitError> {
    let mut fields = out.split(|b| *b == 0);
    let mut found = Vec::new();
    while let Some(scope) = fields.next() {
        if scope.is_empty() {
            continue;
        }
        let entry = fields
            .next()
            .ok_or_else(|| GitError::Parse("config list: scope without entry".into()))?;
        if scope != b"local" && scope != b"worktree" {
            continue;
        }
        let entry = String::from_utf8_lossy(entry);
        let (key, value) = entry.split_once('\n').unwrap_or((&entry, ""));
        if runs_program(key, value) {
            found.push(ProgramSetting {
                key: key.to_owned(),
                value: value.to_owned(),
            });
        }
    }
    Ok(found)
}

/// Whether git (or a command GitCraft runs) may start a program because of this setting.
fn runs_program(key: &str, value: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let Some((section, rest)) = k.split_once('.') else {
        return false;
    };
    let (sub, name) = match rest.rsplit_once('.') {
        Some((sub, name)) => (Some(sub), name),
        None => (None, rest),
    };
    match (section, sub, name) {
        (
            "core",
            None,
            "fsmonitor"
            | "hookspath"
            | "sshcommand"
            | "pager"
            | "editor"
            | "askpass"
            | "gitproxy"
            | "alternaterefscommand",
        ) => true,
        ("sequence", None, "editor") => true,
        ("diff", None, "external") => true,
        ("diff", Some(_), "command" | "textconv") => true,
        ("difftool" | "mergetool", Some(_), "cmd") => true,
        ("filter", Some(_), "clean" | "smudge" | "process") => true,
        ("merge", Some(_), "driver") => true,
        ("credential", _, "helper") => true,
        ("gpg", _, "program") => true,
        ("include", None, "path") | ("includeif", Some(_), "path") => true,
        ("uploadpack", None, "packobjectshook") => true,
        ("remote", Some(_), "uploadpack" | "receivepack" | "vcs") => true,
        ("protocol", Some("ext"), "allow") => value != "never",
        ("submodule", Some(_), "update") => value.starts_with('!'),
        ("pager", Some(_) | None, _) => true,
        ("browser", Some(_), "cmd" | "path") | ("web", None, "browser") => true,
        _ => false,
    }
}

/// Executable files in the hooks directory, without git's `.sample` files.
fn hooks_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut hooks: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.ends_with(".sample") && !name.starts_with('.') && is_executable(&e.path())
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .take(MAX_HOOKS)
        .collect();
    hooks.sort();
    hooks
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // Git for Windows runs hooks through its shell whatever their permissions.
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_keys_are_recognised() {
        for (k, v) in [
            ("core.fsmonitor", "x"),
            ("core.hookspath", ".husky"),
            ("core.sshcommand", "ssh -v"),
            ("diff.external", "x"),
            ("diff.Bin.textconv", "x"),
            ("difftool.my.cmd", "x"),
            ("filter.lfs.smudge", "x"),
            ("merge.ours.driver", "true"),
            ("credential.helper", "store"),
            ("credential.https://h.example.helper", "x"),
            ("gpg.program", "x"),
            ("gpg.ssh.program", "x"),
            ("include.path", "../x"),
            ("includeif.gitdir:~/w/.path", "../x"),
            ("remote.origin.uploadpack", "x"),
            ("protocol.ext.allow", "always"),
            ("submodule.s.update", "!rm -rf /"),
            ("pager.log", "x"),
            ("uploadpack.packobjectshook", "x"),
        ] {
            assert!(runs_program(k, v), "{k}");
        }
        for (k, v) in [
            ("core.bare", "false"),
            ("core.autocrlf", "true"),
            ("user.name", "x"),
            ("remote.origin.url", "https://x"),
            ("filter.lfs.required", "true"),
            ("protocol.ext.allow", "never"),
            ("submodule.s.update", "rebase"),
            ("diff.renames", "true"),
            ("notacore", "x"),
        ] {
            assert!(!runs_program(k, v), "{k}");
        }
    }

    #[test]
    fn only_local_and_worktree_scope_count() {
        let out = b"global\0core.pager\nless\0local\0core.hookspath\n.husky\0worktree\0core.sshcommand\nssh\0local\0core.bare\nfalse\0system\0credential.helper\nx\0local\0gpg.program\0";
        let s = parse_settings(out).unwrap();
        let keys: Vec<&str> = s.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["core.hookspath", "core.sshcommand", "gpg.program"]);
        assert_eq!(s[2].value, "");
        assert!(parse_settings(b"local").is_err(), "cut off after the scope");
    }

    #[test]
    fn the_canonical_text_names_everything() {
        let r = TrustReport {
            settings: vec![ProgramSetting {
                key: "core.pager".into(),
                value: "less\n-x".into(),
            }],
            hooks: vec!["pre-commit".into()],
            hooks_dir: PathBuf::from("/r/.git/hooks"),
        };
        assert_eq!(
            r.canonical(),
            "setting \"core.pager\" \"less\\n-x\"\nhooks-dir \"/r/.git/hooks\"\nhook \"pre-commit\"\n"
        );
        assert!(!r.is_empty());
        assert!(TrustReport::default().is_empty());
    }
}
