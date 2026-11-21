//! Remotes (issue #45): list, add, rename, remove, change URLs; check URLs before git sees them
//! and hide credentials embedded in them (docs/security.md T6, T7).
//!
//! Only `https`, `ssh`, `git`, `file` and local paths are accepted; git's transport helpers
//! (`ext::`, `fd::`, `<helper>::`) never are. Plain `http` and `git://` are accepted but not
//! encrypted, which the UI says.

use crate::branch::NameProblem;
use crate::{Cancel, GitError, Repository};

/// A configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// `remote.<name>.url`, in configuration order (git fetches from the first).
    pub urls: Vec<String>,
    /// `remote.<name>.pushurl`; empty when pushes go to the fetch URL.
    pub push_urls: Vec<String>,
}

impl Remote {
    /// The URL git fetches from.
    pub fn url(&self) -> Option<&str> {
        self.urls.first().map(String::as_str)
    }

    /// The URL git pushes to (a separate push URL, or the fetch URL).
    pub fn push_url(&self) -> Option<&str> {
        self.push_urls.first().map(String::as_str).or(self.url())
    }

    /// A URL of this remote carries a password (or token) in plain text.
    pub fn has_credentials(&self) -> bool {
        self.urls
            .iter()
            .chain(&self.push_urls)
            .any(|u| credentials(u).is_some())
    }
}

/// How a URL reaches the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    Https,
    /// Not encrypted.
    Http,
    /// `ssh://…` or `user@host:path`.
    Ssh,
    /// `git://`: not encrypted, not authenticated.
    Git,
    /// `file://…`
    File,
    /// A path on this computer.
    Local,
}

impl UrlKind {
    /// Whether the connection is encrypted (or never leaves the computer).
    pub fn is_encrypted(self) -> bool {
        !matches!(self, UrlKind::Http | UrlKind::Git)
    }
}

/// Why a URL is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlProblem {
    Empty,
    /// Starts with `-`, or its host does (would read as an option of ssh).
    LeadingDash,
    /// Contains control characters (a newline would split git's credential protocol).
    Control,
    /// A transport helper (`ext::…`, `fd::…`): runs arbitrary programs.
    Helper,
    /// A scheme GitCraft does not offer.
    Scheme(String),
}

/// Checks a remote URL; returns how it connects.
pub fn check_url(url: &str) -> Result<UrlKind, UrlProblem> {
    if url.trim().is_empty() {
        return Err(UrlProblem::Empty);
    }
    if url.starts_with('-') {
        return Err(UrlProblem::LeadingDash);
    }
    if url.chars().any(char::is_control) {
        return Err(UrlProblem::Control);
    }
    // `<transport>::<address>` (git's syntax for remote helpers).
    if let Some((helper, _)) = url.split_once("::")
        && !helper.is_empty()
        && helper
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return Err(UrlProblem::Helper);
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "https" => UrlKind::Https,
            "http" => UrlKind::Http,
            "ssh" | "git+ssh" | "ssh+git" => UrlKind::Ssh,
            "git" => UrlKind::Git,
            "file" => return Ok(UrlKind::File),
            other => return Err(UrlProblem::Scheme(other.to_owned())),
        };
        let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if host.starts_with('-') {
            return Err(UrlProblem::LeadingDash);
        }
        return Ok(kind);
    }
    if let Some(host) = scp_host(url) {
        if host.starts_with('-') {
            return Err(UrlProblem::LeadingDash);
        }
        return Ok(UrlKind::Ssh);
    }
    Ok(UrlKind::Local)
}

/// The host of an scp-like address (`user@host:path`), as git reads one: a colon before any
/// slash. A single letter before the colon is a drive (`C:\repo`), not a host.
fn scp_host(url: &str) -> Option<&str> {
    let colon = url.find(':')?;
    if url[..colon].contains('/') || url[..colon].contains('\\') {
        return None;
    }
    let before = &url[..colon];
    if before.len() == 1 && before.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
    // `[host]:path` (IPv6 or a host with a colon).
    Some(host.trim_start_matches('['))
}

/// Splits `scheme://userinfo@rest` into its parts (`None` without a userinfo).
fn userinfo(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest[..end].rfind('@')?;
    Some((scheme, &rest[..at], &rest[at + 1..]))
}

/// The URL as GitCraft shows it: user name and password of `http(s)` URLs (which are often a
/// token) and the password of other URLs replaced by `***`.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, info, rest)) = userinfo(url) else {
        return url.to_owned();
    };
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") {
        return format!("{scheme}://***@{rest}");
    }
    match info.split_once(':') {
        Some((user, _)) => format!("{scheme}://{user}:***@{rest}"),
        None => url.to_owned(),
    }
}

/// Credentials written into an `http(s)` URL, for handing them to git's credential helper.
#[derive(Clone, PartialEq, Eq)]
pub struct UrlCredentials {
    pub protocol: String,
    pub host: String,
    pub username: String,
    pub password: String,
    /// The URL without them.
    pub bare_url: String,
}

impl std::fmt::Debug for UrlCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret (logs, test failures).
        write!(f, "UrlCredentials({}://***@{})", self.protocol, self.host)
    }
}

/// The user name and password of an `http(s)` URL that carries a password; `None` otherwise.
pub fn credentials(url: &str) -> Option<UrlCredentials> {
    let (scheme, info, rest) = userinfo(url)?;
    let protocol = scheme.to_ascii_lowercase();
    if protocol != "https" && protocol != "http" {
        return None;
    }
    let (user, password) = info.split_once(':')?;
    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    Some(UrlCredentials {
        protocol: protocol.clone(),
        host: host.to_owned(),
        username: percent_decode(user)?,
        password: percent_decode(password)?,
        bare_url: format!("{scheme}://{rest}"),
    })
}

/// Decodes `%XX` escapes (git does the same with credentials in URLs); `None` when the result
/// is not valid UTF-8 or contains control characters.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    (!s.chars().any(char::is_control)).then_some(s)
}

fn invalid_url(url: &str, problem: UrlProblem) -> GitError {
    GitError::InvalidUrl {
        url: redact_url(url),
        problem,
    }
}

impl Repository {
    /// The configured remotes in configuration order
    /// (`config -z --get-regexp ^remote\..*\.(url|pushurl)$`).
    pub fn remotes(&self) -> Result<Vec<Remote>, GitError> {
        let out = match self.read([
            "config",
            "-z",
            "--get-regexp",
            r"^remote\..*\.(url|pushurl)$",
        ]) {
            Ok(out) => out,
            // Exit code 1: no remote configured.
            Err(GitError::Failed { code: Some(1), .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(parse_remotes(&out))
    }

    fn remote_name_ok(&self, name: &str) -> Result<(), GitError> {
        let problem = if name.is_empty() {
            Some(NameProblem::Empty)
        } else if name.starts_with('-') {
            Some(NameProblem::LeadingDash)
        } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            Some(NameProblem::Whitespace)
        } else {
            // git's own rule for remote names.
            let full = format!("refs/remotes/{name}/test");
            match self.read(["check-ref-format", full.as_str()]) {
                Ok(_) => None,
                Err(GitError::Failed { code: Some(1), .. }) => Some(NameProblem::Rules),
                Err(e) => return Err(e),
            }
        };
        match problem {
            Some(problem) => Err(GitError::InvalidName {
                name: name.to_owned(),
                problem,
            }),
            None => Ok(()),
        }
    }

    fn url_ok(url: &str) -> Result<(), GitError> {
        check_url(url).map(|_| ()).map_err(|p| invalid_url(url, p))
    }

    /// Adds a remote (`remote add`), with a separate push URL when given (`remote set-url
    /// --push`). Nothing is fetched.
    pub fn add_remote(
        &self,
        name: &str,
        url: &str,
        push_url: Option<&str>,
        cancel: &Cancel,
    ) -> Result<(), GitError> {
        self.remote_name_ok(name)?;
        Self::url_ok(url)?;
        if let Some(p) = push_url {
            Self::url_ok(p)?;
        }
        self.write(
            ["remote", "add", "--end-of-options", name, url],
            None,
            cancel,
        )?;
        if let Some(p) = push_url {
            self.write(
                ["remote", "set-url", "--push", "--end-of-options", name, p],
                None,
                cancel,
            )?;
        }
        Ok(())
    }

    /// Renames a remote with its remote-tracking branches and upstream settings
    /// (`remote rename`).
    pub fn rename_remote(&self, old: &str, new: &str, cancel: &Cancel) -> Result<(), GitError> {
        self.remote_name_ok(old)?;
        self.remote_name_ok(new)?;
        self.write(
            ["remote", "rename", "--end-of-options", old, new],
            None,
            cancel,
        )?;
        Ok(())
    }

    /// Removes a remote and its remote-tracking branches (`remote remove`); returns what it
    /// was, so that it can be added again.
    pub fn remove_remote(&self, name: &str, cancel: &Cancel) -> Result<Remote, GitError> {
        self.remote_name_ok(name)?;
        let remote = self
            .remotes()?
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| GitError::InvalidValue(name.to_owned()))?;
        self.write(["remote", "remove", "--end-of-options", name], None, cancel)?;
        Ok(remote)
    }

    /// Sets the fetch URL and the push URL (`remote set-url [--push]`); `push_url: None`
    /// removes a separate push URL, so that pushes go to the fetch URL.
    pub fn set_remote_urls(
        &self,
        name: &str,
        url: &str,
        push_url: Option<&str>,
        cancel: &Cancel,
    ) -> Result<(), GitError> {
        self.remote_name_ok(name)?;
        Self::url_ok(url)?;
        if let Some(p) = push_url {
            Self::url_ok(p)?;
        }
        let current = self
            .remotes()?
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| GitError::InvalidValue(name.to_owned()))?;
        if current.url() != Some(url) {
            self.write(
                ["remote", "set-url", "--end-of-options", name, url],
                None,
                cancel,
            )?;
        }
        match push_url {
            Some(p) if current.push_urls.first().map(String::as_str) != Some(p) => {
                self.write(
                    ["remote", "set-url", "--push", "--end-of-options", name, p],
                    None,
                    cancel,
                )?;
            }
            None if !current.push_urls.is_empty() => {
                let key = format!("remote.{name}.pushurl");
                self.write(
                    ["config", "--unset-all", "--end-of-options", key.as_str()],
                    None,
                    cancel,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Hands passwords written into the remote's URLs to the configured credential helper
    /// (`credential approve`, on stdin) and removes them from the URLs. Fails with
    /// [`GitError::NoCredentialHelper`] (nothing changed) when no helper is configured.
    pub fn move_credentials(&self, name: &str, cancel: &Cancel) -> Result<(), GitError> {
        self.remote_name_ok(name)?;
        let helpers = match self.read(["config", "--get-all", "credential.helper"]) {
            Ok(out) => out,
            Err(GitError::Failed { code: Some(1), .. }) => Vec::new(),
            Err(e) => return Err(e),
        };
        if String::from_utf8_lossy(&helpers).trim().is_empty() {
            return Err(GitError::NoCredentialHelper);
        }
        let remote = self
            .remotes()?
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| GitError::InvalidValue(name.to_owned()))?;
        let bare = |u: &String| credentials(u).map_or_else(|| u.clone(), |c| c.bare_url);
        for url in remote.urls.iter().chain(&remote.push_urls) {
            if let Some(c) = credentials(url) {
                let input = format!(
                    "protocol={}\nhost={}\nusername={}\npassword={}\n\n",
                    c.protocol, c.host, c.username, c.password
                );
                self.write(["credential", "approve"], Some(input.as_bytes()), cancel)?;
            }
        }
        let url = remote.urls.first().map(bare).unwrap_or_default();
        let push = remote.push_urls.first().map(bare);
        self.set_remote_urls(name, &url, push.as_deref(), cancel)
    }
}

/// Parses `config -z --get-regexp` output (`key\nvalue\0`). The name is what lies between
/// `remote.` and the last `.url`/`.pushurl`, so names with dots come out whole.
fn parse_remotes(out: &[u8]) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(rec);
        let Some((key, value)) = text.split_once('\n') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("remote.") else {
            continue;
        };
        let (name, push) = if let Some(n) = rest.strip_suffix(".pushurl") {
            (n, true)
        } else if let Some(n) = rest.strip_suffix(".url") {
            (n, false)
        } else {
            continue;
        };
        let i = match remotes.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                remotes.push(Remote {
                    name: name.to_owned(),
                    urls: Vec::new(),
                    push_urls: Vec::new(),
                });
                remotes.len() - 1
            }
        };
        let list = if push {
            &mut remotes[i].push_urls
        } else {
            &mut remotes[i].urls
        };
        list.push(value.to_owned());
    }
    // A remote with only a push URL is not usable for fetching; git lists it anyway.
    remotes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_url_kinds() {
        for (url, kind) in [
            ("https://example.org/r.git", UrlKind::Https),
            ("HTTPS://example.org/r.git", UrlKind::Https),
            ("http://example.org/r.git", UrlKind::Http),
            ("ssh://git@example.org:22/r.git", UrlKind::Ssh),
            ("git+ssh://example.org/r", UrlKind::Ssh),
            ("git@example.org:team/r.git", UrlKind::Ssh),
            ("example.org:r.git", UrlKind::Ssh),
            ("[::1]:r.git", UrlKind::Ssh),
            ("git://example.org/r.git", UrlKind::Git),
            ("file:///srv/r.git", UrlKind::File),
            ("/srv/r.git", UrlKind::Local),
            ("../r", UrlKind::Local),
            ("C:\\repos\\r", UrlKind::Local),
            ("c:/repos/r", UrlKind::Local),
            ("./a:b", UrlKind::Local),
            ("my repos/r", UrlKind::Local),
        ] {
            assert_eq!(check_url(url), Ok(kind), "{url}");
        }
        assert!(!UrlKind::Http.is_encrypted() && !UrlKind::Git.is_encrypted());
        assert!(UrlKind::Https.is_encrypted() && UrlKind::Ssh.is_encrypted());
    }

    #[test]
    fn refused_urls() {
        for (url, problem) in [
            ("", UrlProblem::Empty),
            ("  ", UrlProblem::Empty),
            ("--upload-pack=touch x", UrlProblem::LeadingDash),
            ("-rf", UrlProblem::LeadingDash),
            ("ssh://-oProxyCommand=x/r", UrlProblem::LeadingDash),
            ("ssh://user@-oProxyCommand=x/r", UrlProblem::LeadingDash),
            ("-oProxyCommand=x:r", UrlProblem::LeadingDash),
            ("git@-host:r", UrlProblem::LeadingDash),
            ("ext::sh -c touch% /tmp/x", UrlProblem::Helper),
            ("fd::17", UrlProblem::Helper),
            ("persistent-https::x", UrlProblem::Helper),
            ("https://example.org/r\n", UrlProblem::Control),
            ("ftp://example.org/r", UrlProblem::Scheme("ftp".into())),
        ] {
            assert_eq!(check_url(url), Err(problem), "{url:?}");
        }
    }

    #[test]
    fn credentials_are_redacted() {
        assert_eq!(
            redact_url("https://me:s3cret@example.org/r.git"),
            "https://***@example.org/r.git"
        );
        assert_eq!(
            redact_url("https://ghp_token@example.org/r.git"),
            "https://***@example.org/r.git"
        );
        assert_eq!(
            redact_url("ssh://git:pw@example.org/r"),
            "ssh://git:***@example.org/r"
        );
        assert_eq!(
            redact_url("ssh://git@example.org/r"),
            "ssh://git@example.org/r"
        );
        assert_eq!(redact_url("git@example.org:r.git"), "git@example.org:r.git");
        assert_eq!(
            redact_url("https://example.org/a@b"),
            "https://example.org/a@b",
            "an @ in the path is no user"
        );
    }

    #[test]
    fn credentials_are_extracted_and_never_printed() {
        let c = credentials("https://me%40work:p%3Ass@example.org:8443/r.git").unwrap();
        assert_eq!(c.username, "me@work");
        assert_eq!(c.password, "p:ss");
        assert_eq!(c.host, "example.org:8443");
        assert_eq!(c.bare_url, "https://example.org:8443/r.git");
        assert!(!format!("{c:?}").contains("p:ss"));
        assert_eq!(credentials("https://token@example.org/r"), None);
        assert_eq!(credentials("ssh://git:pw@example.org/r"), None);
        assert_eq!(
            credentials("https://me:a%0Ab@example.org/r"),
            None,
            "a newline would split git's credential protocol"
        );
    }

    #[test]
    fn remote_list() {
        let out = b"remote.origin.url\nhttps://example.org/r.git\0remote.a.b.url\n/srv/a\0remote.a.b.pushurl\nssh://h/a\0remote.origin.url\n/second\0";
        let r = parse_remotes(out);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "origin");
        assert_eq!(r[0].urls, ["https://example.org/r.git", "/second"]);
        assert_eq!(r[0].push_url(), Some("https://example.org/r.git"));
        assert_eq!(r[1].name, "a.b");
        assert_eq!(r[1].push_url(), Some("ssh://h/a"));
        assert!(!r[1].has_credentials());
    }
}
