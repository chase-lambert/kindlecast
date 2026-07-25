use regex::Regex;
use std::sync::OnceLock;

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
pub fn strip_tags(html: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    TAG_RE
        .get_or_init(|| Regex::new("(?is)<[^>]+>").unwrap())
        .replace_all(html, " ")
        .to_string()
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
}
