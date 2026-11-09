use std::{borrow::Cow, sync::LazyLock};

use regex::{Captures, Regex};

static REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([a-z1238+-][a-z0-9_-]*):").unwrap());

pub(crate) fn replace_shortcodes(text: &str) -> Cow<'_, str> {
    REGEX.replace_all(text, Replacer)
}

struct Replacer;

impl regex::Replacer for Replacer {
    fn replace_append(&mut self, caps: &Captures, dst: &mut String) {
        match emojis::get_by_shortcode(&caps[1]) {
            Some(emoji) => dst.push_str(emoji.as_str()),
            None => dst.push_str(&caps[1]),
        }
    }
}

/// Exact whitespace-delimited tokens only: URL/path fragments and backtick-wrapped tokens stay literal.
pub(crate) fn ascii_emoticon_replacements(
    text: &str,
) -> Vec<(std::ops::Range<usize>, &'static str)> {
    let mut offset = 0;
    text.split_inclusive(char::is_whitespace)
        .filter_map(|part| {
            let start = offset;
            offset += part.len();
            let token = part.trim_end_matches(char::is_whitespace);
            let emoji = match token {
                ":)" | ":-)" => "🙂",
                ";)" | ";-)" => "😉",
                ":D" | ":-D" => "😃",
                ":P" | ":p" | ":-P" | ":-p" => "😛",
                ":(" | ":-(" => "🙁",
                ":/" | ":-/" => "😕",
                ":|" | ":-|" => "😐",
                ":O" | ":o" | ":-O" | ":-o" => "😮",
                ":'(" | ":'-(" => "😢",
                "<3" => "❤️",
                _ => return None,
            };
            Some((start..start + token.len(), emoji))
        })
        .collect()
}
