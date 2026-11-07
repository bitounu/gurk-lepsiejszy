//! An in-memory, grapheme-safe keyboard selection view.
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Clear, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CopyAction {
    None,
    Copy(String),
    Cancel,
}

pub(crate) struct TextSelection {
    text: String,
    boundaries: Vec<usize>,
    cursor: usize,
    anchor: Option<usize>,
    scroll: usize,
    width: usize,
    height: usize,
    desired_column: Option<usize>,
    error: Option<String>,
}

struct VisualLayout {
    positions: Vec<(usize, usize)>,
    rows: usize,
}

impl TextSelection {
    pub(crate) fn new(text: String) -> Self {
        let mut boundaries: Vec<_> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        boundaries.push(text.len());
        Self {
            text,
            boundaries,
            cursor: 0,
            anchor: None,
            scroll: 0,
            width: 80,
            height: 20,
            desired_column: None,
            error: None,
        }
    }

    pub(crate) fn set_cursor_to_byte(&mut self, byte: usize) {
        self.cursor = self
            .boundaries
            .partition_point(|&b| b <= byte.min(self.text.len()))
            .saturating_sub(1);
        self.desired_column = None;
    }

    pub(crate) fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub(crate) fn selected_text(&self) -> String {
        self.anchor
            .map(|anchor| {
                let (start, end) = (anchor.min(self.cursor), anchor.max(self.cursor));
                self.text[self.boundaries[start]..self.boundaries[end]].to_owned()
            })
            .unwrap_or_default()
    }

    fn len(&self) -> usize {
        self.boundaries.len() - 1
    }
    fn grapheme(&self, i: usize) -> &str {
        &self.text[self.boundaries[i]..self.boundaries[i + 1]]
    }
    fn is_newline(s: &str) -> bool {
        matches!(s, "\n" | "\r\n" | "\r")
    }
    fn display_width(s: &str) -> usize {
        if s == "\t" {
            4
        } else {
            UnicodeWidthStr::width(s).max(1)
        }
    }

    fn layout(&self) -> VisualLayout {
        let width = self.width.max(1);
        let (mut row, mut column) = (0, 0);
        let mut positions = vec![(0, 0); self.len() + 1];
        for (i, position) in positions.iter_mut().enumerate().take(self.len()) {
            let s = self.grapheme(i);
            let w = Self::display_width(s).min(width);
            // A newline after an exactly full row consumes the existing wrap,
            // rather than adding a spurious blank display row.
            if Self::is_newline(s) && column >= width {
                row += 1;
                column = 0;
                *position = (row, column);
                continue;
            }
            if column >= width || (!Self::is_newline(s) && column > 0 && column + w > width) {
                row += 1;
                column = 0;
            }
            *position = (row, column);
            if Self::is_newline(s) {
                row += 1;
                column = 0;
            } else {
                column += w;
            }
        }
        // A trailing insertion point needs its own visible cell when a row is full.
        if column >= width {
            row += 1;
            column = 0;
        }
        positions[self.len()] = (row, column);
        VisualLayout {
            positions,
            rows: row + 1,
        }
    }

    fn move_vertical(&mut self, delta: isize) {
        let layout = self.layout();
        let (row, column) = layout.positions[self.cursor];
        let wanted = *self.desired_column.get_or_insert(column);
        let target = row.saturating_add_signed(delta).min(layout.rows - 1);
        if target == row {
            return;
        }
        if let Some((i, _)) = layout
            .positions
            .iter()
            .enumerate()
            .filter(|(_, (r, _))| *r == target)
            .min_by_key(|(index, (_, c))| (c.abs_diff(wanted), std::cmp::Reverse(*index)))
        {
            self.cursor = i;
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> CopyAction {
        match key.code {
            KeyCode::Esc => return CopyAction::Cancel,
            KeyCode::Enter => {
                return CopyAction::Copy(self.selected_text());
            }
            KeyCode::Char(' ') => {
                self.anchor = if self.anchor.is_some() {
                    None
                } else {
                    Some(self.cursor)
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Up => {
                self.move_vertical(-1);
                return CopyAction::None;
            }
            KeyCode::Down => {
                self.move_vertical(1);
                return CopyAction::None;
            }
            KeyCode::PageUp => {
                self.move_vertical(-(self.height.max(1) as isize));
                return CopyAction::None;
            }
            KeyCode::PageDown => {
                self.move_vertical(self.height.max(1) as isize);
                return CopyAction::None;
            }
            KeyCode::Home => {
                while self.cursor > 0 && !Self::is_newline(self.grapheme(self.cursor - 1)) {
                    self.cursor -= 1;
                }
            }
            KeyCode::End => {
                while self.cursor < self.len() && !Self::is_newline(self.grapheme(self.cursor)) {
                    self.cursor += 1;
                }
            }
            _ => return CopyAction::None,
        }
        self.desired_column = None;
        CopyAction::None
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        let hint = self
            .error
            .as_deref()
            .unwrap_or("Space: select | Enter: copy | Esc: cancel");
        let block = Block::bordered().title("Copy text").title_bottom(hint);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.width = usize::from(inner.width).max(1);
        self.height = usize::from(inner.height);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let layout = self.layout();
        let cursor_row = layout.positions[self.cursor].0;
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        }
        if cursor_row >= self.scroll + self.height {
            self.scroll = cursor_row + 1 - self.height;
        }
        let range = self.anchor.map(|a| a.min(self.cursor)..a.max(self.cursor));
        for i in 0..=self.len() {
            let (row, column) = layout.positions[i];
            if row < self.scroll || row >= self.scroll + self.height || column >= self.width {
                continue;
            }
            let s = if i == self.len() {
                " "
            } else {
                self.grapheme(i)
            };
            let selected = range.as_ref().is_some_and(|r| r.contains(&i));
            let mut style = Style::default();
            if selected {
                style = style.fg(Color::Black).bg(Color::Yellow);
            }
            if i == self.cursor {
                style = style
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD);
            }
            let display = if Self::is_newline(s) {
                " ".to_owned()
            } else if s == "\t" {
                "    ".to_owned()
            } else if Self::display_width(s) > self.width {
                "�".to_owned()
            } else if UnicodeWidthStr::width(s) == 0 {
                format!(
                    "◌{}",
                    s.chars().filter(|c| !c.is_control()).collect::<String>()
                )
            } else {
                s.chars().filter(|c| !c.is_control()).collect()
            };
            frame.render_widget(
                Paragraph::new(display.clone()).style(style),
                Rect::new(
                    inner.x + column as u16,
                    inner.y + (row - self.scroll) as u16,
                    (Self::display_width(&display).max(1) as u16).min(inner.width - column as u16),
                    1,
                ),
            );
        }
        // Apply the cursor last: a newline at an exact wrap shares the next
        // line's first cell, which may have been painted after that newline.
        let (row, column) = layout.positions[self.cursor];
        if let Some(cell) = frame.buffer_mut().cell_mut((
            inner.x + column as u16,
            inner.y + (row - self.scroll) as u16,
        )) {
            cell.set_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    fn key(s: &mut TextSelection, code: KeyCode) -> CopyAction {
        s.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn copies_exact_graphemes_and_multiline_in_both_directions() {
        let text = "a\u{301}👨‍👩‍👧‍👦\n世界\t";
        let mut s = TextSelection::new(text.into());
        assert_eq!(s.len(), 6);
        key(&mut s, KeyCode::Char(' '));
        for _ in 0..6 {
            key(&mut s, KeyCode::Right);
        }
        assert_eq!(key(&mut s, KeyCode::Enter), CopyAction::Copy(text.into()));
        key(&mut s, KeyCode::Char(' '));
        key(&mut s, KeyCode::Char(' '));
        for _ in 0..6 {
            key(&mut s, KeyCode::Left);
        }
        assert_eq!(key(&mut s, KeyCode::Enter), CopyAction::Copy(text.into()));
        assert_eq!(key(&mut s, KeyCode::Esc), CopyAction::Cancel);
    }

    #[test]
    fn vertical_navigation_and_narrow_wrapping() {
        let mut s = TextSelection::new("a🙂b\ncd".into());
        s.width = 2;
        let l = s.layout();
        assert_eq!(l.positions[1], (1, 0));
        assert_eq!(l.positions[2], (2, 0));
        key(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 1);
        key(&mut s, KeyCode::Down);
        assert_eq!(s.cursor, 2);
        key(&mut s, KeyCode::End);
        assert_eq!(s.cursor, 3);
        key(&mut s, KeyCode::Home);
        assert_eq!(s.cursor, 0);
        s.width = 1;
        assert!(s.layout().rows >= 5);
    }

    #[test]
    fn toggle_clears_selection_and_empty_copy_is_safe() {
        let mut s = TextSelection::new("abc".into());
        key(&mut s, KeyCode::Char(' '));
        key(&mut s, KeyCode::Right);
        key(&mut s, KeyCode::Char(' '));
        assert_eq!(key(&mut s, KeyCode::Enter), CopyAction::Copy(String::new()));
    }

    #[test]
    fn vertical_moves_preserve_clamped_cursor_and_skip_shared_newline_position() {
        let mut s = TextSelection::new("ab\nc".into());
        s.width = 2;
        s.set_cursor_to_byte(3);
        key(&mut s, KeyCode::Down);
        key(&mut s, KeyCode::Char(' '));
        key(&mut s, KeyCode::Right);
        assert_eq!(s.selected_text(), "c");
        let mut s = TextSelection::new("ab\nc\nde".into());
        s.width = 2;
        s.set_cursor_to_byte(5);
        key(&mut s, KeyCode::Up);
        key(&mut s, KeyCode::Char(' '));
        key(&mut s, KeyCode::Right);
        assert_eq!(s.selected_text(), "c");
    }

    #[test]
    fn exact_width_newline_does_not_add_a_blank_row() {
        let mut s = TextSelection::new("ab\ncd".into());
        s.width = 2;
        let layout = s.layout();
        assert_eq!(layout.positions[2], (1, 0));
        assert_eq!(layout.positions[3], (1, 0));
        assert_eq!(layout.positions[4], (1, 1));
    }

    #[test]
    fn initial_byte_offset_clamps_to_grapheme_boundary() {
        let mut s = TextSelection::new("a\u{301}🙂z".into());
        s.set_cursor_to_byte(2);
        assert_eq!(s.cursor, 0);
        s.set_cursor_to_byte(4);
        assert_eq!(s.cursor, 1);
        s.set_cursor_to_byte(usize::MAX);
        assert_eq!(s.cursor, s.len());
    }

    #[test]
    fn tiny_framebuffers_do_not_panic() {
        use ratatui::{Terminal, backend::TestBackend};
        for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 3), (6, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut s = TextSelection::new("🙂a\u{301}\nend".into());
            terminal.draw(|f| s.draw(f, f.area())).unwrap();
        }
    }
}
