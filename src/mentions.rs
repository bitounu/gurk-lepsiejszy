//! Confirmed composer mentions and Signal's UTF-16 body ranges.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data::{AssociatedValue, BodyRange};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftMention {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub aci: Uuid,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageText {
    pub text: String,
    pub mentions: Vec<DraftMention>,
}

impl From<&str> for MessageText {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl From<String> for MessageText {
    fn from(text: String) -> Self {
        Self {
            text,
            mentions: Vec::new(),
        }
    }
}

fn valid(text: &str, mention: &DraftMention) -> bool {
    mention.start < mention.end
        && mention.text.starts_with('@')
        && text.get(mention.start..mention.end) == Some(mention.text.as_str())
}

/// Attachment syntax is meaningful only in user text, never inside confirmed labels.
pub(crate) fn attachment_matches<'a>(
    text: &'a str,
    mentions: &'a [DraftMention],
) -> impl Iterator<Item = regex::Match<'a>> + 'a {
    crate::util::ATTACHMENT_REGEX
        .find_iter(text)
        .filter(move |found| {
            found.as_str().starts_with("file://")
                && !mentions.iter().any(|mention| {
                    valid(text, mention)
                        && found.start() < mention.end
                        && mention.start < found.end()
                })
        })
}

impl MessageText {
    /// Preserve identities through the exact file:// extraction used by the composer.
    pub(crate) fn caption(mut self, cleaned: String) -> Self {
        let mut text = String::new();
        let mut mentions = Vec::new();
        let mut offset = 0;
        let mut keep = Vec::new();
        for found in attachment_matches(&self.text, &self.mentions) {
            keep.push(offset..offset + self.text[offset..found.start()].trim_end().len());
            offset = found.end();
        }
        keep.push(offset..self.text.len());
        for range in keep {
            for mention in &self.mentions {
                if valid(&self.text, mention)
                    && range.start <= mention.start
                    && mention.end <= range.end
                {
                    let mut mention = mention.clone();
                    mention.start = text.len() + mention.start - range.start;
                    mention.end = text.len() + mention.end - range.start;
                    mentions.push(mention);
                }
            }
            text.push_str(&self.text[range]);
        }
        let start_trimmed = text.trim_start().to_owned();
        reconcile_mentions(&text, &start_trimmed, &mut mentions);
        let trimmed = start_trimmed.trim_end().to_owned();
        reconcile_mentions(&start_trimmed, &trimmed, &mut mentions);
        self.mentions = if trimmed == cleaned {
            mentions
        } else {
            Vec::new()
        };
        self.text = cleaned;
        self
    }

    pub(crate) fn encode(mut self) -> (String, Vec<BodyRange>) {
        self.mentions.sort_by_key(|m| (m.start, m.end));
        let mut text = String::new();
        let mut ranges = Vec::new();
        let mut cursor = 0;
        let mut utf16 = 0;
        for mention in &self.mentions {
            if mention.start < cursor || !valid(&self.text, mention) {
                continue;
            }
            let plain = crate::emoji::replace_shortcodes(&self.text[cursor..mention.start]);
            utf16 += plain.encode_utf16().count();
            text.push_str(&plain);
            if let Some((start, end)) = u16::try_from(utf16)
                .ok()
                .and_then(|start| start.checked_add(1).map(|end| (start, end)))
            {
                text.push('\u{fffc}');
                ranges.push(BodyRange {
                    start,
                    end,
                    value: AssociatedValue::MentionUuid(mention.aci),
                });
                utf16 += 1;
            } else {
                text.push_str(&mention.text);
                utf16 += mention.text.encode_utf16().count();
            }
            cursor = mention.end;
        }
        text.push_str(&crate::emoji::replace_shortcodes(&self.text[cursor..]));
        (text, ranges)
    }
}

/// Preserve identities only when one contiguous edit leaves their label intact.
pub(crate) fn reconcile_mentions(old: &str, new: &str, mentions: &mut Vec<DraftMention>) {
    if old == new {
        mentions.retain(|m| valid(new, m));
        return;
    }
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    mentions.retain_mut(|mention| {
        if !valid(old, mention) {
            return false;
        }
        if old_end <= mention.start {
            // Inserting at the opening boundary moves the mention with its text.
            let Some(start) = mention
                .start
                .checked_sub(old_end)
                .and_then(|n| n.checked_add(new_end))
            else {
                return false;
            };
            let Some(end) = mention
                .end
                .checked_sub(old_end)
                .and_then(|n| n.checked_add(new_end))
            else {
                return false;
            };
            mention.start = start;
            mention.end = end;
        } else if prefix < mention.end {
            return false;
        }
        valid(new, mention)
    });
}

fn utf16_byte_offset(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units += ch.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}

pub(crate) fn expand(
    text: &str,
    ranges: &[BodyRange],
    name: impl Fn(Uuid) -> String,
) -> MessageText {
    let mut mentions = ranges
        .iter()
        .filter_map(|range| {
            let AssociatedValue::MentionUuid(aci) = &range.value else {
                return None;
            };
            if range.end.checked_sub(range.start) != Some(1) {
                return None;
            }
            let start = utf16_byte_offset(text, usize::from(range.start))?;
            let end = utf16_byte_offset(text, usize::from(range.end))?;
            (text.get(start..end) == Some("\u{fffc}")).then_some((start, end, *aci))
        })
        .collect::<Vec<_>>();
    mentions.sort_by_key(|&(start, end, _)| (start, end));
    let mut result = MessageText::default();
    let mut cursor = 0;
    for (start, end, aci) in mentions {
        if start < cursor {
            continue;
        }
        result.text.push_str(&text[cursor..start]);
        let label = format!("@{}", name(aci));
        let start = result.text.len();
        result.text.push_str(&label);
        result.mentions.push(DraftMention {
            start,
            end: result.text.len(),
            text: label,
            aci,
        });
        cursor = end;
    }
    result.text.push_str(&text[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mention(start: usize, text: &str, id: u128) -> DraftMention {
        DraftMention {
            start,
            end: start + text.len(),
            text: text.into(),
            aci: Uuid::from_u128(id),
        }
    }

    #[test]
    fn caption_extraction_preserves_mentions_across_multiple_removed_tokens_and_trim() {
        let text = "  file:///a @One file:///b and @Two  ";
        let one = text.find("@One").unwrap();
        let two = text.find("@Two").unwrap();
        let caption = MessageText {
            text: text.into(),
            mentions: vec![mention(one, "@One", 1), mention(two, "@Two", 2)],
        }
        .caption("@One and @Two".into());
        let (body, ranges) = caption.encode();
        assert_eq!(body, "\u{fffc} and \u{fffc}");
        assert_eq!(ranges.len(), 2);
        assert_eq!((ranges[1].start, ranges[1].end), (6, 7));
    }

    #[test]
    fn encodes_confirmed_mentions_with_utf16_offsets_and_plain_emoji() {
        let draft = MessageText {
            text: "😀 :smile: @é @é @typed".into(),
            mentions: vec![mention(13, "@é", 1), mention(17, "@é", 2)],
        };
        let (text, ranges) = draft.encode();
        assert_eq!(text, "😀 😄 \u{fffc} \u{fffc} @typed");
        assert_eq!(
            ranges.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(),
            [(6, 7), (8, 9)]
        );
        assert_eq!(
            ranges[1].value,
            AssociatedValue::MentionUuid(Uuid::from_u128(2))
        );
    }

    #[test]
    fn wire_range_roundtrip_has_length_one_at_nonzero_offset() {
        let (_, ranges) = MessageText {
            text: "😀 @name".into(),
            mentions: vec![mention(5, "@name", 1)],
        }
        .encode();
        let wire = presage::proto::BodyRange::from(&ranges[0]);
        assert_eq!(wire.start, Some(3));
        assert_eq!(wire.length, Some(1));
        assert_eq!(BodyRange::from_proto(wire), Some(ranges[0].clone()));
    }

    #[test]
    fn exclusive_end_overflow_preserves_literal_label() {
        for offset in [65535, 65536] {
            let text = format!("{}@name", "x".repeat(offset));
            let (encoded, ranges) = MessageText {
                mentions: vec![mention(offset, "@name", 1)],
                text: text.clone(),
            }
            .encode();
            assert_eq!(encoded, text);
            assert!(ranges.is_empty());
        }
    }

    #[test]
    fn unchanged_text_revalidates_existing_mention_labels() {
        let mut mentions = vec![
            mention(0, "@name", 1),
            mention(1, "@name", 2),
            mention(0, "@wrong", 3),
        ];
        reconcile_mentions("@name", "@name", &mut mentions);
        assert_eq!(mentions, [mention(0, "@name", 1)]);
    }

    #[test]
    fn invalid_and_overflow_mentions_remain_literal() {
        let (text, ranges) = MessageText {
            text: "é @name".into(),
            mentions: vec![mention(1, "@name", 1), mention(3, "@wrong", 2)],
        }
        .encode();
        assert_eq!(text, "é @name");
        assert!(ranges.is_empty());
        let text = format!("{}@name", "x".repeat(65536));
        let (encoded, ranges) = MessageText {
            mentions: vec![mention(65536, "@name", 1)],
            text: text.clone(),
        }
        .encode();
        assert_eq!(encoded, text);
        assert!(ranges.is_empty());
    }

    #[test]
    fn shortcode_in_mention_label_is_preserved_on_overflow() {
        let text = format!("{}@a:smile:", "x".repeat(65536));
        let (encoded, ranges) = MessageText {
            mentions: vec![mention(65536, "@a:smile:", 1)],
            text: text.clone(),
        }
        .encode();
        assert_eq!(encoded, text);
        assert!(ranges.is_empty());
    }

    #[test]
    fn edits_shift_boundaries_and_drop_edited_mentions() {
        for (new, expected) in [
            ("x @name", Some(2)),
            (" @name", Some(1)),
            ("@name!", Some(0)),
            ("@nXame", None),
            ("@Name", None),
            ("x @name!", None),
        ] {
            let mut mentions = vec![mention(0, "@name", 1)];
            reconcile_mentions("@name", new, &mut mentions);
            assert_eq!(mentions.first().map(|m| m.start), expected, "{new}");
        }
        // Separate composer edits preserve identity; one compound replacement is
        // conservatively treated as touching the enclosed mention.
        let mut mentions = vec![mention(0, "@name", 1)];
        reconcile_mentions("@name", "x @name", &mut mentions);
        reconcile_mentions("x @name", "x @name!", &mut mentions);
        assert_eq!(mentions, [mention(2, "@name", 1)]);
        let mut mentions = Vec::new();
        reconcile_mentions("", "@name", &mut mentions);
        assert!(mentions.is_empty());
    }

    #[test]
    fn unicode_edits_and_overlapping_or_out_of_bounds_drafts_are_safe() {
        let mut mentions = vec![mention(3, "@é", 1)];
        reconcile_mentions("é @é", "😀é @é", &mut mentions);
        assert_eq!(mentions, [mention(7, "@é", 1)]);
        let (text, ranges) = MessageText {
            text: "@same".into(),
            mentions: vec![
                mention(0, "@same", 1),
                mention(0, "@same", 2),
                DraftMention {
                    start: usize::MAX,
                    end: usize::MAX,
                    text: "@bad".into(),
                    aci: Uuid::nil(),
                },
            ],
        }
        .encode();
        assert_eq!(text, "\u{fffc}");
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn expands_wire_mentions_and_ignores_invalid_utf16_boundaries() {
        let ranges = vec![
            BodyRange {
                start: 3,
                end: 4,
                value: AssociatedValue::MentionUuid(Uuid::from_u128(1)),
            },
            BodyRange {
                start: 1,
                end: 2,
                value: AssociatedValue::MentionUuid(Uuid::from_u128(2)),
            },
        ];
        let draft = expand("😀 \u{fffc}!", &ranges, |_| "Zoë".into());
        assert_eq!(draft.text, "😀 @Zoë!");
        assert_eq!(draft.mentions, [mention(5, "@Zoë", 1)]);
    }
}
