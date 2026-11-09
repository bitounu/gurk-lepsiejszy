use unicode_segmentation::UnicodeSegmentation;
#[cfg(test)]
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct Cursor {
    /// Position of the character as byte index
    pub idx: usize,
    /// Position vertically
    pub line: usize,
    /// Position horizontally
    pub col: usize,
    col_wanted: Option<usize>,
}

impl Cursor {
    #[cfg(test)]
    pub fn new(idx: usize, line: usize, col: usize) -> Self {
        Self {
            idx,
            line,
            col,
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub fn begin() -> Self {
        Default::default()
    }

    #[cfg(test)]
    pub fn end(text: &str) -> Self {
        let idx = snap_to_char(text, text.len());
        let (line, col) = calc_line_column(text, idx);
        Self {
            idx,
            line,
            col,
            col_wanted: None,
        }
    }

    #[cfg(test)]
    pub fn at(text: &str, idx: usize) -> Self {
        let idx = snap_to_char(text, idx);
        let (line, col) = calc_line_column(text, idx);
        Self {
            idx,
            line,
            col,
            col_wanted: None,
        }
    }

    fn refresh_position(&mut self, text: &str) {
        let prefix = &text[..self.idx];
        self.line = prefix.bytes().filter(|&b| b == b'\n').count();
        self.col = prefix.rsplit('\n').next().unwrap_or_default().width();
    }

    pub fn put(&mut self, c: char, text: &mut String) {
        text.insert(self.idx, c);
        self.idx += c.len_utf8();
        self.refresh_position(text);
        self.col_wanted = None;
    }

    pub fn new_line(&mut self, text: &mut String) {
        self.put('\n', text);
    }

    pub fn move_left(&mut self, text: &str) {
        if let Some((idx, _)) = text[..self.idx].char_indices().next_back() {
            self.idx = idx;
            self.refresh_position(text);
            self.col_wanted = None;
        }
    }

    pub fn move_right(&mut self, text: &str) {
        if let Some(c) = text[self.idx..].chars().next() {
            self.idx += c.len_utf8();
            self.refresh_position(text);
            self.col_wanted = None;
        }
    }

    pub fn delete_backward(&mut self, text: &mut String) {
        if let Some((idx, _)) = text[..self.idx].char_indices().next_back() {
            self.idx = idx;
            text.remove(idx);
            self.refresh_position(text);
            self.col_wanted = None;
        }
    }

    pub fn delete_forward(&mut self, text: &mut String) {
        if self.idx < text.len() {
            text.remove(self.idx);
            self.refresh_position(text);
            self.col_wanted = None;
        }
    }

    pub fn delete_line_backward(&mut self, text: &mut String) {
        let start = text[..self.idx].rfind('\n').map_or(0, |idx| idx + 1);
        if start == self.idx {
            self.delete_backward(text);
        } else {
            text.replace_range(start..self.idx, "");
            self.idx = start;
            self.refresh_position(text);
            self.col_wanted = None;
        }
    }

    pub fn delete_word_backward(&mut self, text: &mut String) {
        let end = self.idx;
        self.move_word_left(text);
        text.replace_range(self.idx..end, "");
    }

    pub fn move_line_down(&mut self, text: &str) {
        if let Some(end) = text[self.idx..].find('\n').map(|n| self.idx + n) {
            let start = end + 1;
            let end = text[start..].find('\n').map_or(text.len(), |n| start + n);
            let wanted = *self.col_wanted.get_or_insert(self.col);
            self.idx = start + column_index(&text[start..end], wanted);
        } else {
            self.idx = text.len();
        }
        self.refresh_position(text);
    }

    pub fn move_line_up(&mut self, text: &str) {
        if let Some(end) = text[..self.idx].rfind('\n') {
            let start = text[..end].rfind('\n').map_or(0, |n| n + 1);
            let wanted = *self.col_wanted.get_or_insert(self.col);
            self.idx = start + column_index(&text[start..end], wanted);
        } else {
            self.idx = 0;
        }
        self.refresh_position(text);
    }

    pub fn move_word_left(&mut self, text: &str) {
        let mut chars = text[..self.idx].chars().rev().peekable();

        while let Some(c) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            self.move_left(text);
            chars.next();
        }

        while let Some(c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            self.move_left(text);
            chars.next();
        }
    }

    pub fn move_word_right(&mut self, text: &str) {
        let mut chars = text[self.idx..].chars().peekable();

        while let Some(c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            self.move_right(text);
            chars.next();
        }

        while let Some(c) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            self.move_right(text);
            chars.next();
        }
    }

    pub fn start_of_line(&mut self, text: &str) {
        self.idx = text[..self.idx].rfind('\n').map_or(0, |n| n + 1);
        self.refresh_position(text);
        self.col_wanted = None;
    }

    pub fn end_of_line(&mut self, text: &str) {
        self.idx = text[self.idx..]
            .find('\n')
            .map_or(text.len(), |n| self.idx + n);
        self.refresh_position(text);
        self.col_wanted = None;
    }

    pub fn delete_suffix(&mut self, text: &mut String) {
        let end = text[self.idx..]
            .char_indices()
            .find(|&(_, c)| c == '\n')
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| text.len());
        if self.idx == end && end < text.len() {
            text.remove(end);
        } else {
            text.replace_range(self.idx..end, "");
        }
    }
}

/// Byte boundary at the requested display column, without splitting a grapheme.
fn column_index(line: &str, wanted: usize) -> usize {
    let mut col = 0;
    for (idx, grapheme) in line.grapheme_indices(true) {
        let next = col + grapheme.width();
        if next > wanted {
            return idx;
        }
        col = next;
    }
    line.len()
}

/// Snap the byte index `idx` to a char boundary in `s`.
///
/// The snapping is always done to left starting at `idx`.
#[cfg(test)]
fn snap_to_char(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        s.len()
    } else {
        while !s.is_char_boundary(idx) {
            if let Some(new_idx) = idx.checked_sub(1) {
                idx = new_idx
            } else {
                break;
            }
        }
        idx
    }
}

#[cfg(test)]
fn calc_line_column(s: &str, idx: usize) -> (usize, usize) {
    let prefix = &s[..idx];
    (
        prefix.bytes().filter(|&b| b == b'\n').count(),
        prefix.rsplit('\n').next().unwrap_or_default().width(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    use quickcheck::{Arbitrary, Gen};
    use quickcheck_macros::quickcheck;

    #[test]
    fn test_cursor_empty() {
        let cur = Cursor::end("");
        assert_eq!(cur, Cursor::begin());
    }

    #[test]
    fn test_cursor_one_line() {
        let cur = Cursor::end("Hello, world!");
        assert_eq!(cur, Cursor::new(13, 0, 13));
    }

    #[test]
    fn test_cursor_new_line() {
        let cur = Cursor::end("\n\n");
        assert_eq!(cur, Cursor::new(2, 2, 0));
    }

    #[test]
    fn test_cursor_multiple_lines() {
        let cur = Cursor::end("Hello\n\nWorld");
        assert_eq!(cur, Cursor::new(12, 2, 5));
    }

    #[test]
    fn test_cursor_at() {
        let text = "Hello\nWorld\n\n".to_string();
        assert_eq!(Cursor::at(&text, 0), Cursor::begin());
        assert_eq!(Cursor::at(&text, 0), Cursor::new(0, 0, 0));
        assert_eq!(Cursor::at(&text, 1), Cursor::new(1, 0, 1));
        assert_eq!(Cursor::at(&text, 5), Cursor::new(5, 0, 5));
        assert_eq!(Cursor::at(&text, 6), Cursor::new(6, 1, 0));
        assert_eq!(Cursor::at(&text, 6), Cursor::new(6, 1, 0));
        assert_eq!(Cursor::at(&text, 11), Cursor::new(11, 1, 5));
        assert_eq!(Cursor::at(&text, 12), Cursor::new(12, 2, 0));
        assert_eq!(Cursor::at(&text, 13), Cursor::new(13, 3, 0));
        assert_eq!(Cursor::at(&text, 100), Cursor::new(13, 3, 0));
    }

    #[test]
    fn test_move_word_left_right() {
        let text = "Hello\n  new🌍\n\nWorld";
        let mut cursor = Cursor::begin();

        let stops = vec![
            Cursor::new(0, 0, 0),
            Cursor::new(8, 1, 2),
            Cursor::new(17, 3, 0),
            Cursor::new(22, 3, 5),
        ];

        for stop in &stops {
            assert_eq!(stop, &cursor);
            cursor.move_word_right(text);
        }

        for stop in stops.iter().rev() {
            assert_eq!(stop, &cursor);
            cursor.move_word_left(text);
        }
    }

    #[test]
    fn test_delete_suffix() {
        let mut text = "Hello\n  new🌍\n\nWorld".to_string();
        let mut cursor = Cursor::begin();

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "\n  new🌍\n\nWorld");

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "  new🌍\n\nWorld");

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "\n\nWorld");

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "\nWorld");

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "World");

        cursor.delete_suffix(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "");
    }

    #[test]
    fn test_delete_forward() {
        let mut text = "Hello\n  new🌍\n\nWorld".to_string();
        let mut cursor = Cursor::begin();

        cursor.delete_forward(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "ello\n  new🌍\n\nWorld");

        cursor.delete_forward(&mut text);
        cursor.delete_forward(&mut text);
        cursor.delete_forward(&mut text);
        cursor.delete_forward(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "\n  new🌍\n\nWorld");

        cursor.delete_forward(&mut text);
        assert_eq!(cursor, Cursor::new(0, 0, 0));
        assert_eq!(text, "  new🌍\n\nWorld");

        for _ in 0..5 {
            cursor.move_right(&text);
        }
        assert_eq!(cursor, Cursor::new(5, 0, 5));
        cursor.delete_forward(&mut text);
        assert_eq!(cursor, Cursor::new(5, 0, 5));
        assert_eq!(text, "  new\n\nWorld");

        let mut cursor = Cursor::end(&text);
        let snapshot = text.clone();
        cursor.delete_forward(&mut text);
        assert_eq!(text, snapshot);
    }

    #[derive(Debug, Clone, Copy)]
    enum Operation {
        Left,
        Right,
        Down,
        Up,
        Put(char),
        DeleteBackward,
        DeleteForward,
    }

    impl Arbitrary for Operation {
        fn arbitrary(g: &mut Gen) -> Self {
            use Operation::*;

            let mut c = char::arbitrary(g);
            while c.width().unwrap_or(0) == 0 {
                c = char::arbitrary(g);
            }

            *g.choose(&[Left, Right, Up, Down, Put(c), DeleteBackward, DeleteForward])
                .unwrap()
        }
    }

    #[quickcheck]
    fn test_random_operations_sequence(operations: Vec<Operation>) -> bool {
        let mut text = "Hello\nnew🌍\n\nWorld".to_string();

        let calc_index_matrix = |text: &str| {
            let mut res: HashMap<(usize, usize), usize> = Default::default();

            let mut line = 0;
            let mut col = 0;

            for (idx, c) in text.char_indices() {
                res.insert((line, col), idx);
                if c == '\n' {
                    line += 1;
                    col = 0;
                }
                col += c.width().unwrap_or(0);
            }
            res
        };

        let mut cursor = Cursor::begin();
        let mut index_matrix = calc_index_matrix(&text);

        for op in operations {
            let is_last = cursor.idx == text.len();
            let is_entry = cursor.idx < text.len()
                && index_matrix.get(&(cursor.line, cursor.col)) == Some(&cursor.idx);
            if !(is_last || is_entry) {
                println!("is_last = {is_last}");
                println!("is_entry = {is_entry}");
                println!("{text:?}");
                println!("{index_matrix:?}");
                println!("cursor = {cursor:?}");
                println!("op = {op:?}");
                return false;
            }

            match op {
                Operation::Left => cursor.move_left(&text),
                Operation::Right => cursor.move_right(&text),
                Operation::Down => cursor.move_line_down(&text),
                Operation::Up => cursor.move_line_up(&text),
                Operation::Put(c) => {
                    cursor.put(c, &mut text);
                    index_matrix = calc_index_matrix(&text);
                }
                Operation::DeleteBackward => {
                    cursor.delete_backward(&mut text);
                    index_matrix = calc_index_matrix(&text);
                }
                Operation::DeleteForward => {
                    cursor.delete_forward(&mut text);
                    index_matrix = calc_index_matrix(&text);
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod emoji_column_tests {
    use super::*;
    #[test]
    fn vertical_motion_keeps_display_column_across_variation_selectors() {
        let text = "❤️x\nab\n❤️x";
        let mut cursor = Cursor::begin();
        cursor.move_right(text);
        cursor.move_right(text);
        assert_eq!(cursor.col, 2);
        cursor.move_line_down(text);
        assert_eq!(
            (cursor.idx, cursor.line, cursor.col),
            ("❤️x\nab".len(), 1, 2)
        );
        cursor.move_line_down(text);
        assert_eq!(
            (cursor.idx, cursor.line, cursor.col),
            ("❤️x\nab\n❤️".len(), 2, 2)
        );
        cursor.move_line_up(text);
        assert_eq!(
            (cursor.idx, cursor.line, cursor.col),
            ("❤️x\nab".len(), 1, 2)
        );
        cursor.move_line_up(text);
        assert_eq!((cursor.idx, cursor.line, cursor.col), ("❤️".len(), 0, 2));
    }
}
