//! Literal conversation search using normalized display text and UTF-8 byte offsets.
use std::ops::Range;

use crate::storage::MessageId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub message_id: MessageId,
    pub range: Range<usize>,
    pub occurrence: usize,
}

/// Empty queries have no matches; punctuation always has its literal meaning.
pub fn literal_regex(query: &str) -> anyhow::Result<Option<regex::Regex>> {
    if query.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .build()?,
    ))
}

/// Keep byte offsets aligned with displayed/copied text, including terminal sanitization.
pub fn normalize_body(body: &str) -> String {
    strip_ansi_escapes::strip_str(body)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '�'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_unicode_and_occurrences() {
        for query in ["%", "_", "[a].*", "żÓŁw", "👩‍💻"] {
            let body = format!("x{query} {query}");
            let regex = literal_regex(query).unwrap().unwrap();
            let matches: Vec<_> = regex.find_iter(&body).collect();
            assert_eq!(matches.len(), 2);
            assert_eq!(&body[matches[0].range()], query);
        }
        assert!(literal_regex("").unwrap().is_none());
        assert!(literal_regex("ŻÓŁW").unwrap().unwrap().is_match("żółw"));
    }

    #[test]
    fn normalization_matches_copy_text() {
        assert_eq!(
            normalize_body("\x1b[31mŻółw\x1b[0m\r\n👩‍💻\r\t\x07"),
            "Żółw\n👩‍💻"
        );
    }
}
