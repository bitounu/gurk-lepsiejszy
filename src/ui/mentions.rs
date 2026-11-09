//! Compact contact picker over the bottom of the conversation.
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem},
};

pub(crate) fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(popup) = &app.mention_completion else {
        return;
    };
    if area.width < 4 || area.height < 3 {
        return;
    }
    let rows = popup
        .candidates
        .len()
        .min(6)
        .min(usize::from(area.height.saturating_sub(2)));
    if rows == 0 {
        return;
    }
    let height = rows as u16 + 2;
    let width = area.width.min(64);
    let rect = Rect::new(area.x, area.y + area.height - height, width, height);
    let first = popup.selected.saturating_sub(rows - 1);
    let items: Vec<_> = popup
        .candidates
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(index, c)| {
            let selected = index == popup.selected;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(format!(
                "{} {}",
                if selected { "›" } else { " " },
                c.label
            )))
            .style(style)
        })
        .collect();
    f.render_widget(Clear, rect);
    let title = if width >= 55 {
        " Contacts · Enter/Tab complete · ↑↓ · Esc dismiss "
    } else {
        " Contacts · Enter/Tab · Esc "
    };
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(list, rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[tokio::test(flavor = "multi_thread")]
    async fn popup_displays_selection_and_handles_tiny_terminals() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let id = uuid::Uuid::new_v4();
        app.storage.store_name(id, "Bartosz");
        for c in "@Bart".chars() {
            app.input.put_char(c);
        }
        app.refresh_mentions();
        let mut terminal = Terminal::new(TestBackend::new(70, 12)).unwrap();
        terminal.draw(|f| draw(f, &app, f.area())).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("Bartosz"));
        assert!(rendered.contains("Enter/Tab complete"));
        assert!(rendered.contains('›'));
        for (width, height) in [(1, 1), (3, 2), (4, 3), (8, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &app, f.area())).unwrap();
        }
    }
}
