//! Input box

use unicode_width::UnicodeWidthStr;

use crate::cursor::Cursor;

/// Input box with data and a cursor
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct Input {
    pub data: String,
    pub cursor: Cursor,
    /// Original partial that started a tab-completion cycle (e.g., "th" for thumbsup).
    /// Set during cycling, cleared on any other input action.
    pub completion_partial: Option<String>,
    pub mentions: Vec<crate::mentions::DraftMention>,
    mention_text: String,
}

impl Input {
    /// Reconcile direct editor/spelling changes as well as normal keystrokes.
    pub fn sync_mentions(&mut self) {
        crate::mentions::reconcile_mentions(&self.mention_text, &self.data, &mut self.mentions);
        self.mention_text.clone_from(&self.data);
    }

    pub fn message_text(&mut self) -> crate::mentions::MessageText {
        self.sync_mentions();
        crate::mentions::MessageText {
            text: self.data.clone(),
            mentions: self.mentions.clone(),
        }
    }

    pub fn replace_text(&mut self, text: String) {
        self.sync_mentions();
        self.data = text;
        self.sync_mentions();
        let payload = self.message_text();
        self.set_message_text(payload);
    }

    pub fn set_message_text(&mut self, text: crate::mentions::MessageText) {
        self.data = text.text;
        self.mentions = text.mentions;
        self.mention_text.clone_from(&self.data);
        self.cursor = Cursor::default();
        while self.cursor.idx < self.data.len() {
            self.cursor.move_right(&self.data);
        }
        self.sync_mentions();
    }

    pub fn insert_mention(&mut self, range: std::ops::Range<usize>, name: &str, aci: uuid::Uuid) {
        self.sync_mentions();
        if self.data.get(range.clone()).is_none() {
            return;
        }
        let label = format!("@{name}");
        self.data.replace_range(range.clone(), &format!("{label} "));
        self.sync_mentions();
        self.mentions.push(crate::mentions::DraftMention {
            start: range.start,
            end: range.start + label.len(),
            text: label,
            aci,
        });
        self.cursor = Cursor::default();
        let end = range.start + name.len() + 2;
        while self.cursor.idx < end {
            self.cursor.move_right(&self.data);
        }
        self.completion_partial = None;
    }

    pub fn put_char(&mut self, c: char) {
        self.sync_mentions();
        self.cursor.put(c, &mut self.data);
        self.sync_mentions();
    }

    pub fn new_line(&mut self) {
        self.sync_mentions();
        self.cursor.new_line(&mut self.data);
        self.sync_mentions();
    }

    pub fn on_left(&mut self) {
        self.cursor.move_left(&self.data);
    }

    pub fn on_right(&mut self) {
        self.cursor.move_right(&self.data);
    }

    pub fn move_line_down(&mut self) {
        self.cursor.move_line_down(&self.data);
    }

    pub fn move_line_up(&mut self) {
        self.cursor.move_line_up(&self.data);
    }

    pub fn move_back_word(&mut self) {
        self.cursor.move_word_left(&self.data);
    }

    pub fn move_forward_word(&mut self) {
        self.cursor.move_word_right(&self.data);
    }

    pub fn on_home(&mut self) {
        self.cursor.start_of_line(&self.data);
    }

    pub fn on_end(&mut self) {
        self.cursor.end_of_line(&self.data);
    }

    pub fn on_backspace(&mut self) {
        self.sync_mentions();
        self.cursor.delete_backward(&mut self.data);
        self.sync_mentions();
    }

    pub fn on_delete(&mut self) {
        self.sync_mentions();
        self.cursor.delete_forward(&mut self.data);
        self.sync_mentions();
    }

    pub fn on_delete_line(&mut self) {
        self.sync_mentions();
        self.cursor.delete_line_backward(&mut self.data);
        self.sync_mentions();
    }

    pub fn on_delete_word(&mut self) {
        self.sync_mentions();
        self.cursor.delete_word_backward(&mut self.data);
        self.sync_mentions();
    }

    pub fn on_delete_suffix(&mut self) {
        self.sync_mentions();
        self.cursor.delete_suffix(&mut self.data);
        self.sync_mentions();
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn take(&mut self) -> String {
        self.cursor = Default::default();
        self.mentions.clear();
        self.mention_text.clear();
        self.completion_partial = None;
        std::mem::take(&mut self.data)
    }

    /// Convert only plain tokens, applying one edit at a time to preserve mentions.
    pub fn convert_ascii_emoticons(&mut self) {
        self.sync_mentions();
        let replacements = crate::emoji::ascii_emoticon_replacements(&self.data);
        let mut cursor = self.cursor.idx;
        let mut changed = false;
        for (range, emoji) in replacements.into_iter().rev() {
            if self
                .mentions
                .iter()
                .any(|m| range.start < m.end && m.start < range.end)
            {
                continue;
            }
            cursor = if cursor <= range.start {
                cursor
            } else if cursor >= range.end {
                cursor - range.len() + emoji.len()
            } else {
                range.start + emoji.len()
            };
            self.data.replace_range(range, emoji);
            self.sync_mentions();
            changed = true;
        }
        if changed {
            let before = &self.data[..cursor];
            self.cursor = Cursor::default();
            self.cursor.idx = cursor;
            self.cursor.line = before.chars().filter(|&c| c == '\n').count();
            self.cursor.col = before.rsplit('\n').next().unwrap_or_default().width();
            self.completion_partial = None;
        }
    }

    /// If the character just before the cursor is `:`, look backwards for a
    /// matching opening `:` and check if it's a valid emoji shortcode. If so,
    /// replace `:shortcode:` with the actual emoji and adjust cursor position.
    pub fn convert_emoji_on_colon(&mut self) {
        self.sync_mentions();
        self.convert_emoji_on_colon_inner();
        self.sync_mentions();
    }

    fn convert_emoji_on_colon_inner(&mut self) {
        let idx = self.cursor.idx;
        if idx < 1 || !self.data.is_char_boundary(idx) {
            return;
        }
        if &self.data[idx - 1..idx] != ":" {
            return;
        }

        let before = &self.data[..idx - 1];
        let open_pos = match before.rfind(':') {
            Some(p) => p,
            None => return,
        };

        // Only convert when the opening `:` starts a word, i.e. it is at the beginning of the input
        // or preceded by whitespace. This avoids mangling code such as `xtask::shell::foobar`.
        if let Some(prev) = self.data[..open_pos].chars().next_back()
            && !prev.is_whitespace()
        {
            return;
        }

        let shortcode = &self.data[open_pos + 1..idx - 1];
        let Some(emoji) = emojis::get_by_shortcode(shortcode) else {
            return;
        };
        let emoji_str = emoji.as_str();

        let old_width = self.data[open_pos..idx].width();
        let emoji_width = emoji_str.width();

        self.cursor.col = self.cursor.col.saturating_sub(old_width - emoji_width);
        self.cursor.idx = open_pos + emoji_str.len();
        self.data.replace_range(open_pos..idx, emoji_str);
    }

    /// If the cursor is inside a potential emoji shortcode (`:part`),
    /// complete or cycle to the next matching shortcode on each call.
    /// Returns true if a completion was made.
    pub fn try_complete_emoji(&mut self) -> bool {
        self.sync_mentions();
        let result = self.try_complete_emoji_inner();
        self.sync_mentions();
        result
    }

    fn try_complete_emoji_inner(&mut self) -> bool {
        let idx = self.cursor.idx;
        if idx < 1 {
            self.completion_partial = None;
            return false;
        }

        let before = &self.data[..idx];
        let open_pos = match before.rfind(':') {
            Some(p) => p,
            None => {
                self.completion_partial = None;
                return false;
            }
        };

        let current = &self.data[open_pos + 1..idx];
        if current.is_empty() {
            self.completion_partial = None;
            return false;
        }

        // Determine the base partial to match against.
        // If we're cycling (completion_partial is set and current text is
        // related), reuse the original partial. Otherwise start fresh.
        let partial = match &self.completion_partial {
            Some(prev) if current.starts_with(prev.as_str()) || prev.starts_with(current) => {
                prev.as_str()
            }
            _ => {
                self.completion_partial = Some(current.to_string());
                current
            }
        };

        let matches: Vec<&str> = emojis::iter()
            .flat_map(|e| e.shortcodes())
            .filter(|sc| sc.starts_with(partial))
            .collect();

        if matches.is_empty() {
            self.completion_partial = None;
            return false;
        }

        let cur_idx = matches.iter().position(|&sc| sc == current);
        let next_idx = match cur_idx {
            Some(i) => (i + 1) % matches.len(),
            None => 0,
        };

        let next = matches[next_idx];
        let old_width = current.width();
        let new_width = next.width();

        self.data.replace_range(open_pos + 1..idx, next);
        if new_width >= old_width {
            self.cursor.col += new_width - old_width;
        } else {
            self.cursor.col = self.cursor.col.saturating_sub(old_width - new_width);
        }
        self.cursor.idx = open_pos + 1 + next.len();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::Cursor;

    /// Type `s` character by character, mirroring the app's key dispatch: after every `:` we
    /// attempt an emoji conversion.
    fn type_str(input: &mut Input, s: &str) {
        for c in s.chars() {
            input.put_char(c);
            if c == ':' {
                input.convert_emoji_on_colon();
            }
        }
    }

    #[test]
    fn test_convert_emoji_basic() {
        let mut input = Input::default();
        type_str(&mut input, ":dog:");
        assert_eq!(input.data, "🐶");
        // idx = byte length of the emoji, col = display width, still on line 0
        assert_eq!(input.cursor, Cursor::new("🐶".len(), 0, 2));
    }

    #[test]
    fn test_convert_emoji_unknown_shortcode() {
        let mut input = Input::default();
        type_str(&mut input, ":nope:");
        assert_eq!(input.data, ":nope:");
        assert_eq!(input.cursor, Cursor::new(6, 0, 6));
    }

    #[test]
    fn test_convert_emoji_double_colon() {
        let mut input = Input::default();
        type_str(&mut input, "::");
        assert_eq!(input.data, "::");
        assert_eq!(input.cursor, Cursor::new(2, 0, 2));
    }

    #[test]
    fn test_convert_emoji_no_opening_colon() {
        let mut input = Input::default();
        type_str(&mut input, "dog:");
        assert_eq!(input.data, "dog:");
        assert_eq!(input.cursor, Cursor::new(4, 0, 4));
    }

    #[test]
    fn test_convert_emoji_with_leading_text() {
        let mut input = Input::default();
        type_str(&mut input, "hi :dog:");
        assert_eq!(input.data, "hi 🐶");
        assert_eq!(input.cursor, Cursor::new("hi 🐶".len(), 0, 5));
    }

    #[test]
    fn test_convert_emoji_requires_word_boundary() {
        // Opening `:` glued to a preceding word must not trigger conversion,
        // so paths like `xtask::shell::foobar` stay intact.
        let mut input = Input::default();
        type_str(&mut input, "a:dog:");
        assert_eq!(input.data, "a:dog:");
        assert_eq!(input.cursor, Cursor::new(6, 0, 6));

        let mut input = Input::default();
        type_str(&mut input, "xtask::shell::foobar");
        assert_eq!(input.data, "xtask::shell::foobar");
    }

    #[test]
    fn test_convert_emoji_after_newline() {
        // The start of a line counts as a word boundary.
        let mut input = Input::default();
        type_str(&mut input, "foo\n:dog:");
        assert_eq!(input.data, "foo\n🐶");
        assert_eq!(input.cursor, Cursor::new("foo\n🐶".len(), 1, 2));
    }

    #[test]
    fn test_convert_emoji_consecutive_needs_whitespace() {
        // Whitespace-separated shortcodes both convert.
        let mut input = Input::default();
        type_str(&mut input, ":dog: :cat:");
        assert_eq!(input.data, "🐶 🐱");
        assert_eq!(input.cursor, Cursor::new("🐶 🐱".len(), 0, 5));

        // Directly glued to the preceding emoji, the second one is not at a
        // word boundary and stays as text.
        let mut input = Input::default();
        type_str(&mut input, ":dog::cat:");
        assert_eq!(input.data, "🐶:cat:");
    }

    #[test]
    fn test_convert_emoji_in_middle_of_text() {
        // Cursor is moved back before the closing colon is typed, so text
        // trails the replacement range.
        let mut input = Input::default();
        type_str(&mut input, "x y");
        input.on_left();
        type_str(&mut input, ":dog:");
        assert_eq!(input.data, "x 🐶y");
        // cursor sits right after the emoji, before the trailing `y`
        assert_eq!(input.cursor, Cursor::new("x 🐶".len(), 0, 4));
    }

    /// Type `s` character by character without triggering emoji conversion, so a raw `:partial`
    /// shortcode can be built up for completion tests.
    fn put_str(input: &mut Input, s: &str) {
        for c in s.chars() {
            input.put_char(c);
        }
    }

    #[test]
    fn test_complete_emoji_unique_match() {
        // A partial with a single matching shortcode completes it fully.
        let mut input = Input::default();
        put_str(&mut input, ":thumbsu");
        assert!(input.try_complete_emoji());
        assert_eq!(input.data, ":thumbsup");
        // idx and col advance by the one extra character ("p").
        assert_eq!(input.cursor, Cursor::new(":thumbsup".len(), 0, 9));
        assert_eq!(input.completion_partial.as_deref(), Some("thumbsu"));
    }

    #[test]
    fn test_complete_emoji_cycles_matches() {
        // ":dog" matches ["dog", "dog2"]; current text "dog" is at index 0, so the first completion
        // moves to the next match.
        let mut input = Input::default();
        put_str(&mut input, ":dog");
        assert!(input.try_complete_emoji());
        assert_eq!(input.data, ":dog2");
        // The original partial is remembered across the cycle.
        assert_eq!(input.completion_partial.as_deref(), Some("dog"));

        // Cycling wraps back around to the first match.
        assert!(input.try_complete_emoji());
        assert_eq!(input.data, ":dog");
        assert_eq!(input.completion_partial.as_deref(), Some("dog"));
    }

    #[test]
    fn test_complete_emoji_partial_starts_first_match() {
        // "smil" is not itself a shortcode, so completion jumps to the first shortcode that starts
        // with it.
        let mut input = Input::default();
        put_str(&mut input, ":smil");
        assert!(input.try_complete_emoji());
        assert_eq!(input.data, ":smiley");
        assert_eq!(input.completion_partial.as_deref(), Some("smil"));
    }

    #[test]
    fn test_complete_emoji_no_match() {
        // No shortcode starts with "zzzz": nothing changes and state is reset.
        let mut input = Input::default();
        put_str(&mut input, ":zzzz");
        assert!(!input.try_complete_emoji());
        assert_eq!(input.data, ":zzzz");
        assert_eq!(input.completion_partial, None);
    }

    #[test]
    fn test_complete_emoji_no_colon() {
        // Without an opening colon there is nothing to complete.
        let mut input = Input::default();
        put_str(&mut input, "dog");
        assert!(!input.try_complete_emoji());
        assert_eq!(input.data, "dog");
        assert_eq!(input.completion_partial, None);
    }

    #[test]
    fn test_complete_emoji_empty_partial() {
        // A lone colon has an empty partial and does not complete.
        let mut input = Input::default();
        put_str(&mut input, ":");
        assert!(!input.try_complete_emoji());
        assert_eq!(input.data, ":");
        assert_eq!(input.completion_partial, None);
    }

    #[test]
    fn test_complete_emoji_empty_input() {
        let mut input = Input::default();
        assert!(!input.try_complete_emoji());
        assert_eq!(input.data, "");
        assert_eq!(input.completion_partial, None);
    }
}

#[cfg(test)]
mod mention_tests {
    use super::*;
    #[test]
    fn confirmed_mention_tracks_edits_and_take_clears_identity() {
        let mut input = Input::default();
        for c in "Hej @Bar".chars() {
            input.put_char(c);
        }
        let id = uuid::Uuid::new_v4();
        input.insert_mention(4..8, "Bartosz Kowalski", id);
        assert_eq!(input.data, "Hej @Bartosz Kowalski ");
        assert_eq!(input.mentions[0].aci, id);
        input.on_home();
        input.put_char('🙂');
        assert_eq!(input.mentions[0].start, 8);
        input.on_end();
        input.on_backspace(); // trailing space
        assert_eq!(input.mentions.len(), 1);
        input.on_backspace(); // edit name -> literal
        assert!(input.mentions.is_empty());
        input.take();
        assert!(input.mentions.is_empty());
    }
}

#[cfg(test)]
mod ascii_emoticon_tests {
    use super::*;

    #[test]
    fn ascii_emoticons_common_tokens_and_urls() {
        let mut input = Input::default();
        input.replace_text(
            ":) :-) ;) ;-) :D :-D :P :p :-P :-p :( :-( :/ :-/ :| :-| :O :-o :'( <3".into(),
        );
        input.convert_ascii_emoticons();
        assert_eq!(
            input.data,
            "🙂 🙂 😉 😉 😃 😃 😛 😛 😛 😛 🙁 🙁 😕 😕 😐 😐 😮 😮 😢 ❤️"
        );
        assert_eq!(input.cursor.col, input.data.width());
        input.replace_text("❤️ :)".into());
        input.convert_ascii_emoticons();
        assert_eq!(input.data, "❤️ 🙂");
        assert_eq!(input.cursor.col, 5);
        let original =
            "https://example.test/:/ C:/path x:) abc:| :)x :// :smile: `:)` \\:) e@:/ 🙂";
        input.replace_text(original.into());
        input.convert_ascii_emoticons();
        assert_eq!(input.data, original);
    }

    #[test]
    fn ascii_emoticons_preserve_confirmed_labels_and_unicode_cursor() {
        let mut input = Input::default();
        input.replace_text(":) @Bar :|".into());
        let id = uuid::Uuid::new_v4();
        input.insert_mention(3..7, "Bartosz :) Nowacki", id);
        input.convert_ascii_emoticons();
        assert_eq!(input.data, "🙂 @Bartosz :) Nowacki  😐");
        assert_eq!(input.mentions.len(), 1);
        assert_eq!(input.mentions[0].start, "🙂 ".len());
        assert_eq!(input.mentions[0].text, "@Bartosz :) Nowacki");
        assert_eq!(
            input.cursor,
            Cursor::new(
                input.data.find("  😐").unwrap() + 1,
                0,
                input.data[..input.data.find("  😐").unwrap() + 1].width()
            )
        );
        let (wire, ranges) = input.message_text().encode();
        assert_eq!(wire, "🙂 \u{fffc}  😐");
        assert_eq!((ranges[0].start, ranges[0].end), (3, 4));
        assert_eq!(
            ranges[0].value,
            crate::data::AssociatedValue::MentionUuid(id)
        );
        input.replace_text("Zażółć\n:)\t:/\n:|".into());
        input.on_home();
        let old = input.cursor.clone();
        input.convert_ascii_emoticons();
        assert_eq!(input.data, "Zażółć\n🙂\t😕\n😐");
        assert_eq!(
            input.cursor,
            Cursor::at(&input.data, "Zażółć\n🙂\t😕\n".len())
        );
        assert_eq!(input.cursor.line, old.line);
    }
}

#[cfg(test)]
mod ascii_heart_cursor_tests {
    use super::*;
    #[test]
    fn ascii_heart_conversion_navigation_and_deletion_keep_cursor_aligned() {
        let mut input = Input::default();
        input.replace_text("<3 ".into());
        input.convert_ascii_emoticons();
        assert_eq!(input.data, "❤️ ");
        for _ in 0..3 {
            input.on_backspace();
        }
        assert_eq!(input.data, "");
        assert_eq!(input.cursor, Cursor::begin());
        input.put_char('x');
        assert_eq!(input.cursor.col, 1);
        input.replace_text("prefix <3\nsuffix".into());
        input.convert_ascii_emoticons();
        input.on_home();
        input.on_left(); // newline -> previous line end
        assert_eq!(input.cursor.col, "prefix ❤️".width());
        input.on_left(); // variation selector
        assert_eq!(input.cursor.col, "prefix ❤".width());
        input.on_left(); // heart
        assert_eq!(input.cursor.col, "prefix ".width());
        input.on_right();
        input.on_right();
        assert_eq!(input.cursor.col, "prefix ❤️".width());
        input.on_home();
        assert_eq!(input.cursor.col, 0);
        input.on_end();
        assert_eq!(input.cursor.col, "prefix ❤️".width());
        input.on_delete_line();
        assert_eq!(input.data, "\nsuffix");
        assert_eq!(input.cursor, Cursor::begin());
    }
}
