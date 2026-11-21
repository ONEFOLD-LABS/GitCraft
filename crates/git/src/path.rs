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
    fn to_path_joins_components() {
        let p = RepoPath::from_bytes(b"dir/sub/file.txt").to_path(Path::new("/repo"));
        assert_eq!(
            p,
            Path::new("/repo").join("dir").join("sub").join("file.txt")
        );
    }
}
