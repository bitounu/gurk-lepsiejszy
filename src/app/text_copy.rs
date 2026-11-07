//! Keyboard text selection is a frozen RAM snapshot, separate from reply selection.
use super::App;
use crate::command::Command;
use crate::ui::text_selection::{CopyAction, TextSelection};
use crossterm::event::KeyEvent;

impl App {
    pub(super) fn open_text_selection(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let anchor = self
            .positions
            .get(&window.channel_id())
            .and_then(|pos| pos.selected.or(pos.viewport_bottom))
            .or_else(|| window.newest());
        let mut snapshot = String::new();
        let mut cursor = 0;
        for message in window
            .iter_rev_from(None)
            .rev()
            .filter(|message| !message.deleted)
        {
            let mut text = message.message.clone().unwrap_or_default();
            for file in &message.attachments {
                if !text.is_empty() {
                    text.push('\n');
                }
                if self.config.show_attachment_paths {
                    text.push_str(&format!("<file://{}>", file.filename.display()));
                } else {
                    text.push_str(
                        &file
                            .filename
                            .file_name()
                            .unwrap_or(file.filename.as_os_str())
                            .to_string_lossy(),
                    );
                }
            }
            let text = strip_ansi_escapes::strip_str(&text)
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
                .collect::<String>();
            if text.is_empty() {
                continue;
            }
            if !snapshot.is_empty() {
                snapshot.push_str("\n\n");
            }
            if Some(message.storage_id()) == anchor {
                let matched = self
                    .search_focus
                    .as_ref()
                    .filter(|focus| {
                        focus.channel_id == window.channel_id()
                            && focus.local_id == message.storage_id()
                    })
                    .and_then(|focus| {
                        crate::search::literal_regex(&focus.query)
                            .ok()
                            .flatten()
                            .and_then(|regex| {
                                regex
                                    .find_iter(&text)
                                    .nth(focus.occurrence)
                                    .map(|hit| hit.start())
                            })
                    });
                cursor = snapshot.len() + matched.unwrap_or(0);
            }
            snapshot.push_str(&text);
        }
        if snapshot.is_empty() {
            self.composer_error = Some("No message text to select".into());
            return;
        }
        let mut selection = TextSelection::new(snapshot);
        selection.set_cursor_to_byte(cursor);
        self.text_selection = Some(selection);
        self.should_clear = true;
    }

    pub(super) fn on_text_selection_key(&mut self, key: KeyEvent) {
        if matches!(self.event_to_command(&key), Some(Command::SelectText)) {
            self.text_selection = None;
            self.should_clear = true;
            return;
        }
        let action = self
            .text_selection
            .as_mut()
            .expect("text selection active")
            .on_key(key);
        match action {
            CopyAction::None => {}
            CopyAction::Cancel => {
                self.text_selection = None;
                self.should_clear = true;
            }
            CopyAction::Copy(text) => {
                if text.is_empty() {
                    self.text_selection
                        .as_mut()
                        .unwrap()
                        .set_error("Use Space to begin selection, then move the cursor".into());
                    return;
                }
                let result = self
                    .clipboard
                    .as_mut()
                    .ok_or_else(|| "System clipboard is unavailable in this session".to_owned())
                    .and_then(|clipboard| {
                        clipboard
                            .set_text(text)
                            .map_err(|error| format!("Cannot copy text: {error}"))
                    });
                match result {
                    Ok(()) => {
                        self.text_selection = None;
                        self.should_clear = true;
                    }
                    Err(error) => self.text_selection.as_mut().unwrap().set_error(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::test_app;
    use crate::data::Message;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    #[tokio::test(flavor = "multi_thread")]
    async fn selection_is_frozen_and_never_modifies_draft_or_sends() {
        let (mut app, _, sent) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let sender = id.user().unwrap();
        app.store_message(id, Message::text(sender, 0, "Żółw 👩‍💻\nsecond line".into()));
        let selected_id = app
            .storage
            .message_by_protocol(id, sender, 0)
            .unwrap()
            .storage_id();
        app.on_pgup();
        for c in "unsent draft".chars() {
            app.input.put_char(c);
        }
        app.pending_attachments.push("/tmp/keep-pending.txt".into());
        app.clipboard = None;
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT))
            .await
            .unwrap();
        assert!(app.text_selection.is_some());
        app.on_paste("do not paste");
        app.on_key(KeyCode::Char(' ').into()).await.unwrap();
        for _ in 0..4 {
            app.on_key(KeyCode::Right.into()).await.unwrap();
        }
        assert_eq!(app.text_selection.as_ref().unwrap().selected_text(), "Żółw");
        app.store_message(
            id,
            Message::text(sender, 999, "incoming while selecting".into()),
        );
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        app.on_key(KeyCode::F(4).into()).await.unwrap();
        terminal.backend_mut().resize(16, 6);
        terminal.autoresize().unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.text_selection.as_ref().unwrap().selected_text(), "Żółw");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert!(
            app.text_selection.is_some(),
            "clipboard failure must retain selection"
        );
        assert_eq!(app.input.data, "unsent draft");
        assert_eq!(app.pending_attachments.len(), 1);
        assert!(sent.borrow().is_empty());
        assert_eq!(app.positions[&id].selected, Some(selected_id));
        app.on_key(KeyCode::Esc.into()).await.unwrap();
        assert!(app.text_selection.is_none());
        assert_eq!(app.input.data, "unsent draft");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keyboard_selection_binding_is_configurable_and_excludes_hidden_channels() {
        let (mut app, _, _) = test_app().await;
        let config = toml::from_str("[normal]\nF9 = \"select_text\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&config, false).unwrap();
        app.on_key(KeyCode::F(9).into()).await.unwrap();
        assert!(app.text_selection.is_some());
        app.on_key(KeyCode::F(9).into()).await.unwrap();
        assert!(app.text_selection.is_none());
        app.hide_current_channel().unwrap();
        app.on_key(KeyCode::F(9).into()).await.unwrap();
        assert!(app.text_selection.is_none());
    }
}
