pub fn osc8_hyperlink(url: &str, label: &str) -> String {
    format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}

pub fn truncate_for_display(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_display_keeps_short_text() {
        assert_eq!(
            truncate_for_display("https://example.com/pr/1", 40),
            "https://example.com/pr/1"
        );
    }

    #[test]
    fn truncate_for_display_adds_ellipsis_for_long_text() {
        let value =
            "https://github.com/acme/repo/compare/main...very/long/branch/name/with/extra/segments";
        let out = truncate_for_display(value, 32);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 32);
    }
}
