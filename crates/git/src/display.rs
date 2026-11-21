//! Text from repositories (file names, ref names, paths, git's messages) made safe to show.
//!
//! Such text can contain anything: bytes that are not UTF-8, newlines and other control
//! characters, and invisible characters that hide text or reverse the order in which the rest of
//! a line is shown (`invoice\u{202e}txt.exe` would read as `invoiceexe.txt`). Shown escaped
//! instead: invalid bytes as `\xNN`, control and invisible characters as `\n`, `\u{202e}`, so two
//! different names never look the same and a row stays one line.

/// Escapes text for display (see the module documentation).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_escaped(&mut out, text);
    out
}

/// Like [`escape`], for bytes that may not be UTF-8.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        push_escaped(&mut out, chunk.valid());
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        if c.is_control() || is_invisible(c) {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
}

/// Characters that are not seen but change what is seen: format characters (Unicode category
/// Cf), line and paragraph separators, and the default-ignorable characters that render as
/// nothing (fillers, variation selectors, tags).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{034f}'
            | '\u{061c}'
            | '\u{0600}'..='\u{0605}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_direction_characters_are_escaped() {
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("invoice\u{202e}txt.exe"), "invoice\\u{202e}txt.exe");
        assert_eq!(escape("x\u{2067}y\u{2069}"), "x\\u{2067}y\\u{2069}");
        assert_eq!(escape("\u{feff}bom"), "\\u{feff}bom");
    }

    #[test]
    fn invisible_fillers_selectors_and_tags_are_escaped() {
        for c in [
            '\u{200b}',
            '\u{2028}',
            '\u{2029}',
            '\u{115f}',
            '\u{3164}',
            '\u{ffa0}',
            '\u{034f}',
            '\u{fe0f}',
            '\u{e0041}',
            '\u{180e}',
        ] {
            let shown = escape(&format!("a{c}b"));
            assert_ne!(shown, format!("a{c}b"), "{c:?} not escaped");
            assert!(shown.starts_with("a\\u{"), "{shown}");
        }
    }

    #[test]
    fn ordinary_text_in_any_script_stays() {
        for s in [
            "Grüße aus Köln.txt",
            "日本語/ファイル.md",
            "עברית.txt",
            "emoji 🦀.txt",
        ] {
            assert_eq!(escape(s), s);
        }
    }

    #[test]
    fn invalid_bytes_stay_distinguishable() {
        assert_eq!(escape_bytes(b"a\xff"), "a\\xff");
        assert_ne!(escape_bytes(b"a\xff"), escape_bytes(b"a\xfe"));
        assert_eq!(escape_bytes("ü\n".as_bytes()), "ü\\n");
    }
}
