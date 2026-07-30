/// Format a byte count for progress and error messages.
pub fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = (1024 * 1024) as f64;
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Collapse HTML tags to spaces for plain-text length and snippets.
///
/// **Not a security boundary** — [`crate::sanitize`] is. This feeds only the
/// thread-heading snippet and the article husk-length heuristic, so the contract
/// is conservative: prose that merely *looks* like markup has to survive.
///
/// Two improvements over the `<[^>]+>` pattern it replaces. A `<` only opens a
/// tag when followed by a name, `/`, `!`, or `?`, so `2 < 3 > 1` stays intact
/// instead of losing its middle. And quoted attribute values are respected, so
/// `<a title="a > b">` ends at the right `>` rather than leaking `b">` into a
/// heading.
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match tag_end(after) {
            Some(end) if opens_tag(after) => {
                out.push(' ');
                rest = &after[end + 1..];
            }
            // Comparison operator, or an unterminated tag: keep it as text.
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether the byte after `<` can begin a tag name, closing tag, comment, or
/// processing instruction.
fn opens_tag(after: &str) -> bool {
    after
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'!' | b'?'))
}

/// Offset of the `>` closing a tag that began just after `<`, skipping `>`
/// inside quoted attribute values.
fn tag_end(after: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (index, &byte) in after.as_bytes().iter().enumerate() {
        match (quote, byte) {
            (Some(open), byte) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => return Some(index),
            (None, _) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_switches_units() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(20 * 1024 * 1024), "20.00 MiB");
    }

    #[test]
    fn strip_tags_removes_markup() {
        assert_eq!(strip_tags("<p>hi <b>there</b></p>").trim(), "hi  there");
    }

    #[test]
    fn strip_tags_keeps_comparison_operators() {
        // The pattern this replaced turned the first case into "2  1".
        assert_eq!(strip_tags("2 < 3 > 1"), "2 < 3 > 1");
        assert_eq!(strip_tags("a <= b and c < d"), "a <= b and c < d");
    }

    #[test]
    fn strip_tags_respects_quoted_attribute_values() {
        assert_eq!(strip_tags("<a title=\"a > b\">text</a>").trim(), "text");
        assert_eq!(strip_tags("<a title='a > b'>text</a>").trim(), "text");
    }

    #[test]
    fn strip_tags_keeps_unterminated_markup_as_text() {
        assert_eq!(strip_tags("before <p"), "before <p");
        assert_eq!(
            strip_tags("before <a title=\"unclosed"),
            "before <a title=\"unclosed"
        );
    }

    #[test]
    fn strip_tags_handles_closing_and_comment_starts() {
        assert_eq!(strip_tags("a</p>b").trim(), "a b");
        assert_eq!(strip_tags("a<!-- note -->b").trim(), "a b");
    }

    #[test]
    fn strip_tags_is_char_boundary_safe_on_multibyte_text() {
        let value = strip_tags("héllo <b>wörld</b> — ok");
        assert!(value.contains("héllo"));
        assert!(value.contains("wörld"));
        assert!(value.contains('—'));
    }
}
