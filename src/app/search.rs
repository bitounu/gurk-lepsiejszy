//! In-memory conversation search. Preview navigation never edits the composer.
use super::{
    App,
    channel::ChannelPosition,
    window::{MessageWindow, PAGE},
};
use crate::{data::ChannelId, input::Input, search::SearchHit, storage::MessageId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::ops::Range;

#[derive(Clone, Debug)]
pub(crate) struct SearchFocus {
    pub channel_id: ChannelId,
    pub local_id: u64,
    pub query: String,
    pub range: Range<usize>,
    pub occurrence: usize,
}
pub(crate) struct MessageSearch {
    pub channel_id: ChannelId,
    pub input: Input,
    pub hits: Vec<SearchHit>,
    pub current: Option<usize>,
    pub error: Option<String>,
    original_position: ChannelPosition,
    original_focus: Option<SearchFocus>,
}

impl App {
    pub(super) fn jump_conversation_boundary(&mut self, newest: bool) {
        if self.is_help() || self.select_channel.is_shown {
            return;
        }
        let Some(channel) = self.selected_channel_id() else {
            return;
        };
        self.search_focus = None;
        let mut window = MessageWindow::new(channel);
        if newest {
            window.load_tail(&*self.storage, PAGE);
            self.positions.insert(channel, ChannelPosition::default());
        } else if let Some(first) = self.storage.oldest_message(channel) {
            window.load_around(&*self.storage, first.storage_id(), PAGE);
            self.positions.insert(
                channel,
                ChannelPosition {
                    selected: Some(first.storage_id()),
                    viewport_bottom: window.newest(),
                },
            );
        } else {
            self.positions.insert(channel, ChannelPosition::default());
        }
        self.window = Some(window);
        self.should_clear = true;
    }

    pub(super) fn open_message_search(&mut self) {
        if self.is_help() || self.select_channel.is_shown {
            return;
        }
        let Some(channel) = self.selected_channel_id() else {
            return;
        };
        self.message_search = Some(MessageSearch {
            channel_id: channel,
            input: Input::default(),
            hits: Vec::new(),
            current: None,
            error: None,
            original_position: self.positions.get(&channel).cloned().unwrap_or_default(),
            original_focus: self.search_focus.take(),
        });
        self.should_clear = true;
    }

    fn restore_search_position(&mut self, channel: ChannelId, mut position: ChannelPosition) {
        if self.selected_channel_id() != Some(channel) {
            return;
        }
        for anchor in [&mut position.selected, &mut position.viewport_bottom] {
            if anchor.is_some_and(|at| self.storage.message(MessageId::new(channel, at)).is_none())
            {
                *anchor = None;
            }
        }
        let mut window = MessageWindow::new(channel);
        if let Some(at) = position.selected.or(position.viewport_bottom) {
            window.load_around(&*self.storage, at, PAGE);
        } else {
            window.load_tail(&*self.storage, PAGE);
        }
        self.positions.insert(channel, position);
        self.window = Some(window);
        self.should_clear = true;
    }

    fn refresh_message_search(&mut self) {
        let Some(search) = self.message_search.as_ref() else {
            return;
        };
        let (channel, query) = (search.channel_id, search.input.data.clone());
        let result = self.storage.search_messages(channel, &query);
        let search = self.message_search.as_mut().unwrap();
        match result {
            Ok(hits) => {
                search.current = (!hits.is_empty()).then_some(0);
                search.hits = hits;
                search.error = None;
            }
            Err(error) => {
                search.current = None;
                search.hits.clear();
                search.error = Some(format!("Search failed: {error}"));
            }
        }
        if search.current.is_some() {
            self.focus_search_result();
        } else {
            let position = search.original_position.clone();
            self.search_focus = None;
            self.restore_search_position(channel, position);
        }
    }

    fn focus_search_result(&mut self) -> bool {
        let Some(search) = self.message_search.as_ref() else {
            return false;
        };
        let Some(hit) = search.current.and_then(|index| search.hits.get(index)) else {
            return false;
        };
        let Some(message) = self
            .storage
            .message(hit.message_id)
            .filter(|message| !message.deleted)
        else {
            self.message_search.as_mut().unwrap().error = Some(
                "Message is no longer available; change the query or move to another result".into(),
            );
            return false;
        };
        let body = crate::search::normalize_body(message.message.as_deref().unwrap_or_default());
        let range = crate::search::literal_regex(&search.input.data)
            .ok()
            .flatten()
            .and_then(|regex| {
                regex
                    .find_iter(&body)
                    .nth(hit.occurrence)
                    .map(|found| found.range())
            });
        let Some(range) = range else {
            self.message_search.as_mut().unwrap().error =
                Some("This occurrence changed; edit the query to refresh results".into());
            return false;
        };
        let focus = SearchFocus {
            channel_id: search.channel_id,
            local_id: message.storage_id(),
            query: search.input.data.clone(),
            range,
            occurrence: hit.occurrence,
        };
        let mut window = MessageWindow::new(focus.channel_id);
        window.load_around(&*self.storage, focus.local_id, PAGE);
        self.positions.insert(
            focus.channel_id,
            ChannelPosition {
                selected: Some(focus.local_id),
                viewport_bottom: Some(focus.local_id),
            },
        );
        self.window = Some(window);
        self.search_focus = Some(focus);
        self.should_clear = true;
        true
    }

    fn next_search_result(&mut self, previous: bool) {
        let Some(search) = self.message_search.as_mut() else {
            return;
        };
        let len = search.hits.len();
        if len == 0 {
            return;
        }
        let current = search.current.unwrap_or(0);
        search.current = Some(if previous {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        });
        self.focus_search_result();
    }

    pub(crate) fn paste_message_search(&mut self, text: &str) {
        if let Some(search) = self.message_search.as_mut() {
            for c in text.replace("\r\n", "\n").chars() {
                if !c.is_control() {
                    search.input.put_char(c);
                } else if c == '\n' || c == '\r' {
                    search.input.put_char(' ');
                }
            }
            self.refresh_message_search();
        }
    }

    pub(super) fn on_message_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let search = self.message_search.take().unwrap();
                self.search_focus = search.original_focus;
                self.restore_search_position(search.channel_id, search.original_position);
                return;
            }
            KeyCode::Enter => {
                if self.focus_search_result() {
                    self.message_search = None;
                    self.should_clear = true;
                } else if let Some(search) = self.message_search.as_mut() {
                    search.error = Some("No matching message; Esc cancels".into());
                }
                return;
            }
            KeyCode::Down => {
                self.next_search_result(false);
                return;
            }
            KeyCode::Up => {
                self.next_search_result(true);
                return;
            }
            KeyCode::F(3) => {
                self.next_search_result(key.modifiers.contains(KeyModifiers::SHIFT));
                return;
            }
            KeyCode::Char('n') if ctrl => {
                self.next_search_result(false);
                return;
            }
            KeyCode::Char('p') if ctrl => {
                self.next_search_result(true);
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return;
            }
            _ => {}
        }
        let input = &mut self.message_search.as_mut().unwrap().input;
        let before = input.data.clone();
        match key.code {
            KeyCode::Left => input.on_left(),
            KeyCode::Right => input.on_right(),
            KeyCode::Home => input.on_home(),
            KeyCode::End => input.on_end(),
            KeyCode::Backspace => input.on_backspace(),
            KeyCode::Delete => input.on_delete(),
            KeyCode::Char('u') if ctrl => input.on_delete_line(),
            KeyCode::Char('w') if ctrl => input.on_delete_word(),
            KeyCode::Char('a') if ctrl => input.on_home(),
            KeyCode::Char('e') if ctrl => input.on_end(),
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) || !c.is_ascii() =>
            {
                input.put_char(c)
            }
            _ => {}
        }
        if input.data != before {
            self.refresh_message_search();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::tests::test_app, data::Message};
    async fn populated_app() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::event::Event>,
        std::rc::Rc<std::cell::RefCell<Vec<Message>>>,
    ) {
        let (mut app, events, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let peer = channel.user().unwrap();
        for at in 1..=600 {
            app.storage
                .store_message(channel, &Message::text(peer, at, format!("filler {at}")));
        }
        app.storage.store_message(
            channel,
            &Message::text(peer, 1, "start żółw and ŻÓŁW end".into()),
        );
        app.storage
            .store_message(channel, &Message::text(peer, 590, "last żółw".into()));
        app.on_channel_changed();
        (app, events, sent)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn full_history_multiple_occurrences_enter_and_copy_anchor() {
        let (mut app, _events, sent) = populated_app().await;
        let channel = app.selected_channel_id().unwrap();
        assert!(app.window.as_ref().unwrap().get(1).is_none());
        app.on_key(KeyCode::Char('/').into()).await.unwrap();
        app.on_paste("żółw");
        assert_eq!(app.message_search.as_ref().unwrap().hits.len(), 3);
        assert_eq!(app.positions[&channel].selected, Some(1));
        let first_range = app.search_focus.as_ref().unwrap().range.clone();
        app.on_key(KeyCode::Down.into()).await.unwrap();
        assert!(app.search_focus.as_ref().unwrap().range.start > first_range.start);
        app.on_key(KeyCode::Down.into()).await.unwrap();
        assert_eq!(app.positions[&channel].selected, Some(590));
        app.on_key(KeyCode::Down.into()).await.unwrap();
        assert_eq!(app.message_search.as_ref().unwrap().current, Some(0));
        app.on_key(KeyCode::F(3).into()).await.unwrap();
        app.on_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT))
            .await
            .unwrap();
        assert_eq!(app.message_search.as_ref().unwrap().current, Some(0));
        app.on_key(KeyCode::Down.into()).await.unwrap();
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert!(app.message_search.is_none());
        assert_eq!(app.positions[&channel].selected, Some(1));
        assert!(app.search_focus.is_some());
        assert!(sent.borrow().is_empty());
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT))
            .await
            .unwrap();
        app.on_key(KeyCode::Char(' ').into()).await.unwrap();
        for _ in 0..4 {
            app.on_key(KeyCode::Right.into()).await.unwrap();
        }
        assert_eq!(app.text_selection.as_ref().unwrap().selected_text(), "ŻÓŁW");
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(app.search_focus.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_restores_position_draft_edit_files_and_literal_slashes() {
        let (mut app, _events, sent) = populated_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.on_pgup();
        let original = app.positions[&channel].clone();
        app.input.data = "draft https:".into();
        app.input.on_end();
        app.on_key(KeyCode::Char('/').into()).await.unwrap();
        assert!(app.message_search.is_none());
        assert_eq!(app.input.data, "draft https:/");
        app.pending_attachments.push("/tmp/pending.png".into());
        app.editing = Some(MessageId::new(channel, 0));
        let draft = app.input.data.clone();
        let cursor = app.input.cursor.clone();
        app.on_key(KeyCode::F(8).into()).await.unwrap();
        app.on_paste("żółw");
        assert_eq!(app.positions[&channel].selected, Some(1));
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert_eq!(app.positions[&channel].selected, original.selected);
        assert_eq!(
            app.positions[&channel].viewport_bottom,
            original.viewport_bottom
        );
        assert_eq!(app.input.data, draft);
        assert_eq!(app.input.cursor, cursor);
        assert_eq!(app.editing, Some(MessageId::new(channel, 0)));
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(sent.borrow().is_empty());
        app.on_key(KeyCode::F(8).into()).await.unwrap();
        app.on_paste("no such literal");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert!(app.message_search.is_some());
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(sent.borrow().is_empty());
        app.on_key(KeyCode::F(8).into()).await.unwrap();
        app.on_paste("żółw");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert!(app.message_search.is_none());
        assert_eq!(app.input.data, draft);
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn boundary_keys_jump_across_unloaded_history_and_tail_follows() {
        let (mut app, _events, sent) = populated_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert_eq!(app.positions[&channel].selected, Some(0));
        assert!(app.window.as_ref().unwrap().at_oldest());
        app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert_eq!(app.positions[&channel].selected, None);
        assert_eq!(app.positions[&channel].viewport_bottom, None);
        assert!(app.window.as_ref().unwrap().at_newest());
        let peer = channel.user().unwrap();
        app.store_message(channel, Message::text(peer, 601, "live".into()));
        assert_eq!(app.window.as_ref().unwrap().newest(), Some(601));
        assert!(sent.borrow().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_keeps_last_occurrence_visible_after_accepting_and_new_messages() {
        use ratatui::{Terminal, backend::TestBackend};
        let (mut app, _events, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let peer = channel.user().unwrap();
        app.store_message(
            channel,
            Message::text(
                peer,
                7,
                format!("{}needle at the end", "long line\n".repeat(200)),
            ),
        );
        app.on_key(KeyCode::Char('/').into()).await.unwrap();
        app.on_paste("needle");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        app.store_message(channel, Message::text(peer, 8, "new incoming".into()));
        let mut terminal = Terminal::new(TestBackend::new(65, 18)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("needle"), "{text}");
        assert_eq!(app.positions[&channel].selected, Some(7));
        app.store_edited_message(
            channel,
            7,
            Message::text(
                peer,
                9,
                format!("{}needle moved by remote edit", "new prefix\n".repeat(400)),
            ),
        )
        .unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let updated = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(updated.contains("needle"), "{updated}");
        assert!(sent.borrow().is_empty());
    }
}
