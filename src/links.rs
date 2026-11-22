//! Bounded HTTP(S) links from plain post text, retaining byte ranges for mouse
//! hit testing. All URLs are validated independently of their displayed text.
use std::{collections::HashSet, ops::Range};

use url::Url;

const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_LINKS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// UTF-8 byte range in the original text, excluding surrounding punctuation.
    pub range: Range<usize>,
}

/// Find up to 32 visible links in the first 16 KiB of text. Repeated URLs retain
/// separate ranges so each rendered occurrence can be clicked.
pub fn find(text: &str) -> Vec<Link> {
    let mut limit = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    let visible = &text[..limit];
    let mut links = Vec::new();
    let mut consumed = 0;
    for (start, character) in visible.char_indices() {
        if start < consumed || !matches!(character, 'h' | 'H') {
            continue;
        }
        if visible[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let rest = &visible[start..];
        let scheme_length = if rest
            .as_bytes()
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case(b"https://"))
        {
            8
        } else if rest
            .as_bytes()
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case(b"http://"))
        {
            7
        } else {
            continue;
        };
        let mut end = limit;
        let mut invalid_control = false;
        for (offset, c) in visible[start + scheme_length..].char_indices() {
            if c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`') {
                end = start + scheme_length + offset;
                break;
            }
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                end = start + scheme_length + offset;
                invalid_control = true;
                break;
            }
        }
        consumed = end;
        // Never invent a shortened target when the bounded text window cuts
        // through a URL. A complete URL ending at the real end remains valid.
        if invalid_control || (end == limit && limit < text.len()) {
            continue;
        }
        end = trim_punctuation(text, start, end);
        let candidate = &text[start..end];
        let Ok(url) = Url::parse(candidate) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            continue;
        }
        links.push(Link {
            url: url.into(),
            range: start..end,
        });
        if links.len() == MAX_LINKS {
            break;
        }
    }
    links
}

/// Unique normalized URLs in their first-occurrence order, for a link picker.
pub fn extract(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    find(text)
        .into_iter()
        .filter_map(|link| seen.insert(link.url.clone()).then_some(link.url))
        .collect()
}

fn trim_punctuation(text: &str, start: usize, mut end: usize) -> usize {
    // Track balance once, keeping trimming linear even for pathological text
    // containing thousands of trailing closing delimiters.
    let mut balance = [0_i32; 3];
    for c in text[start..end].chars() {
        match c {
            '(' => balance[0] += 1,
            ')' => balance[0] -= 1,
            '[' => balance[1] += 1,
            ']' => balance[1] -= 1,
            '{' => balance[2] += 1,
            '}' => balance[2] -= 1,
            _ => {}
        }
    }
    while let Some(c) = text[start..end].chars().next_back() {
        let closing = match c {
            ')' => Some(0),
            ']' => Some(1),
            '}' => Some(2),
            _ => None,
        };
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= c.len_utf8();
        } else if let Some(index) = closing
            && balance[index] < 0
        {
            balance[index] += 1;
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_balanced_parentheses_and_ipv6_keep_their_targets() {
        assert_eq!(
            extract(
                "Read (https://en.wikipedia.org/wiki/Rust_(programming_language)). Then <http://[::1]:8080/path?q=yes&ok=1>!"
            ),
            [
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "http://[::1]:8080/path?q=yes&ok=1",
            ]
        );
        assert_eq!(
            extract("https://example.org/a)]},;!?"),
            ["https://example.org/a"]
        );
    }

    #[test]
    fn ranges_point_into_original_unicode_text_without_added_normalization_bytes() {
        let text = "🌍 café https://例え.テスト/写真?q=été. See HTTPS://EXAMPLE.ORG!";
        let links = find(text);
        assert_eq!(links.len(), 2);
        assert_eq!(
            &text[links[0].range.clone()],
            "https://例え.テスト/写真?q=été"
        );
        assert_eq!(&text[links[1].range.clone()], "HTTPS://EXAMPLE.ORG");
        assert!(links[0].url.starts_with("https://xn--r8jz45g.xn--zckzah/"));
        assert!(links[0].url.contains("%E5%86%99%E7%9C%9F"));
        assert_eq!(links[1].url, "https://example.org/");
    }

    #[test]
    fn repeated_targets_keep_click_ranges_but_picker_deduplicates() {
        let text = "https://example.org and HTTPS://EXAMPLE.ORG/ then https://example.net";
        assert_eq!(find(text).len(), 3);
        assert_eq!(
            extract(text),
            ["https://example.org/", "https://example.net/"]
        );
    }

    #[test]
    fn credentials_unsafe_schemes_control_characters_and_word_prefixes_are_rejected() {
        for text in [
            "file:///tmp/photo.png",
            "javascript:alert(1)",
            "https://user:secret@example.org/",
            "https://user@example.org/",
            "https://example.org/\x1b[31m",
            "https://example.org/\u{202e}misleading",
            "prefixhttps://example.org/",
            "https://",
            "https://example.org:99999/",
        ] {
            assert!(find(text).is_empty(), "accepted {text:?}");
        }
        assert_eq!(
            extract("see:https://example.org\nnext"),
            ["https://example.org/"]
        );
    }

    #[test]
    fn limits_are_utf8_safe_and_do_not_truncate_a_target_into_a_new_url() {
        let text = (0..100)
            .map(|n| format!("https://example.org/{n} "))
            .collect::<String>();
        assert_eq!(find(&text).len(), MAX_LINKS);
        let text = format!(
            "{} https://example.org/path",
            "é".repeat(MAX_TEXT_BYTES / 2 - 5)
        );
        assert!(find(&text).is_empty());
        let text = format!("{}https://example.org", "x".repeat(MAX_TEXT_BYTES));
        assert!(find(&text).is_empty());
        assert!(find(&"🌍".repeat(MAX_TEXT_BYTES)).is_empty());
    }
}
