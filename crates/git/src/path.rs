use std::fmt;
use std::path::{Path, PathBuf};

/// A path inside the repository as git reports it: relative to the top of the working tree,
/// `/`-separated, any bytes (file names need not be UTF-8).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        RepoPath(bytes.to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Text for display, escaped with [`crate::escape_bytes`]: invalid bytes as `\xNN`, control
    /// and invisible characters as `\n`, `\u{202e}`.
    pub fn display(&self) -> String {
        crate::escape_bytes(&self.0)
    }

    /// The path on disk below `workdir`.
    pub fn to_path(&self, workdir: &Path) -> PathBuf {
        let mut p = workdir.to_path_buf();
        for part in self.0.split(|b| *b == b'/') {
            p.push(bytes_to_path(part));
        }
        p
    }
}

impl RepoPath {
    /// The path on disk below `workdir` for an operation that changes or removes the file
    /// (docs/security.md T4). Refuses absolute paths, empty, `.` and `..` components, names
    /// Windows reserves (on Windows), and any path whose parent directories pass through a
    /// symbolic link. The last component itself is not resolved: a symbolic link there is
    /// handled as the link.
    pub fn checked_path(&self, workdir: &Path) -> Result<PathBuf, crate::GitError> {
        let bad = |why: &str| crate::GitError::UnsafePath(format!("{}: {why}", self.display()));
        if self.0.is_empty() {
            return Err(bad("empty"));
        }
        let parts: Vec<&[u8]> = self.0.split(|b| *b == b'/').collect();
        let mut path = workdir.to_path_buf();
        for (i, part) in parts.iter().enumerate() {
            match *part {
                b"" => return Err(bad("empty component or absolute path")),
                b"." | b".." => return Err(bad("relative component")),
                _ => {}
            }
            if cfg!(windows) && !windows_safe(part) {
                return Err(bad("name reserved on Windows"));
            }
            path.push(bytes_to_path(part));
            if i + 1 < parts.len()
                && std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(bad("a parent directory is a symbolic link"));
            }
        }
        Ok(path)
    }
}

/// A component Windows can use: no reserved device name (with or without extension), no
/// drive or stream colon, no backslash, no trailing dot or space.
fn windows_safe(part: &[u8]) -> bool {
    let name = String::from_utf8_lossy(part);
    if name.contains([':', '\\']) || name.ends_with(['.', ' ']) {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit()
        && stem.as_bytes()[3] != b'0');
    !reserved
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}

/// Bytes from git as a native path. Unix paths are bytes; on Windows git writes UTF-8.
pub(crate) fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_escapes_control_characters_and_keeps_unicode() {
        assert_eq!(
            RepoPath::from_bytes("a\nb ü.txt".as_bytes()).display(),
            "a\\nb ü.txt"
        );
        assert_eq!(RepoPath::from_bytes(b"x\xff").display(), "x\\xff");
    }

    #[test]
    fn display_shows_invisible_and_direction_characters() {
        let show = |s: &str| RepoPath::from_bytes(s.as_bytes()).display();
        assert_eq!(show("invoice\u{202e}txt.exe"), "invoice\\u{202e}txt.exe");
        assert_eq!(show("a\u{200b}b"), "a\\u{200b}b");
        assert_eq!(show("\u{feff}bom"), "\\u{feff}bom");
        assert_eq!(show("x\u{2067}y\u{2069}"), "x\\u{2067}y\\u{2069}");
        // Ordinary right-to-left text stays as it is.
        assert_eq!(show("עברית.txt"), "עברית.txt");
    }

    #[test]
    fn checked_paths_stay_inside_the_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        let w = dir.path();
        let check = |p: &str| RepoPath::from_bytes(p.as_bytes()).checked_path(w);
        assert_eq!(check("a/b.txt").unwrap(), w.join("a").join("b.txt"));
        for bad in ["", "/etc/passwd", "a//b", "../x", "a/../../x", "./a", "a/"] {
            assert!(check(bad).is_err(), "{bad:?}");
        }
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), w.join("link")).unwrap();
            assert!(check("link/file").is_err(), "through a symlinked directory");
            // The link itself is fine: it is removed as a link.
            assert_eq!(check("link").unwrap(), w.join("link"));
        }
    }

    #[test]
    fn windows_reserved_names() {
        for bad in [
            "CON", "nul.txt", "com1", "LPT9.log", "a:b", "x.", "y ", "c\\d", "Aux .c",
        ] {
            assert!(!windows_safe(bad.as_bytes()), "{bad:?}");
        }
        for good in ["console", "com10", "com0", "aux_file", "readme.md", "lpt"] {
            assert!(windows_safe(good.as_bytes()), "{good:?}");
        }
    }

    #[test]
    fn to_path_joins_components() {
        let p = RepoPath::from_bytes(b"dir/sub/file.txt").to_path(Path::new("/repo"));
        assert_eq!(
            p,
            Path::new("/repo").join("dir").join("sub").join("file.txt")
        );
    }
}
