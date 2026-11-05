//! Local display overrides and their modal editor. Signal names remain untouched.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::{Command, MoveAmountText, MoveDirection};
use crate::input::Input;
use crate::storage::AliasTarget;

use super::App;

pub(crate) struct AliasEditor {
    pub target: AliasTarget,
    pub input: Input,
    pub error: Option<String>,
}

impl App {
    pub(crate) fn set_local_alias(
        &mut self,
        target: AliasTarget,
        text: &str,
    ) -> anyhow::Result<()> {
        let text = text.trim();
        anyhow::ensure!(
            !text.chars().any(char::is_control),
            "Alias must be a single line without control characters"
        );
        let alias = (!text.is_empty()).then_some(text);
        self.storage.set_alias(target, alias)?;
        if let Some(alias) = alias {
            self.local_aliases.insert(target, alias.to_owned());
        } else {
            self.local_aliases.remove(&target);
        }
        Ok(())
    }

    pub fn is_alias_editor_shown(&self) -> bool {
        self.alias_editor.is_some()
    }

    pub fn on_paste(&mut self, content: &str) {
        if self.spelling_dialog.is_some() {
            return;
        }
        if self.message_search.is_some() {
            self.paste_message_search(content);
            return;
        }
        if self.text_selection.is_some()
            || self.image_viewer.is_some()
            || self.send_job.is_some()
            || self.hidden_dialog.is_some()
            || self.forward_dialog.is_some()
        {
            return;
        }
        if let Some(picker) = self.file_picker.as_mut() {
            picker.paste(content);
            return;
        }
        if let Some(editor) = self.alias_editor.as_mut() {
            // Bracketed paste bypasses key handling; keep the alias a single line.
            let text = content.replace("\r\n", " ").replace(['\r', '\n'], " ");
            for c in text.chars().filter(|c| !c.is_control()) {
                editor.input.put_char(c);
            }
            editor.error = None;
        } else {
            for c in content.chars() {
                self.get_input().put_char(c);
            }
        }
    }

    pub(super) fn notification_origin(
        &self,
        channel_id: crate::data::ChannelId,
        sender: uuid::Uuid,
    ) -> String {
        let sender_name = self.name_by_id_cached(sender);
        match self.channel(channel_id) {
            Some(channel) if channel_id.is_user() => self.channel_name(channel).into_owned(),
            Some(channel) => format!("{sender_name} in {}", self.channel_name(channel)),
            None => sender_name,
        }
    }

    pub(super) fn edit_channel_alias(&mut self) {
        if let Some(id) = self.selected_channel_id() {
            self.open_alias_editor(AliasTarget::Channel(id));
        }
    }

    pub(super) fn edit_person_alias(&mut self) {
        let person = self
            .selected_message()
            .map(|message| message.from_id)
            .or_else(|| self.selected_channel_id().and_then(|id| id.user()));
        if let Some(id) = person {
            self.open_alias_editor(AliasTarget::Person(id));
        }
    }

    fn open_alias_editor(&mut self, target: AliasTarget) {
        let mut input = Input::default();
        if let Some(alias) = self.local_aliases.get(&target) {
            for c in alias.chars() {
                input.put_char(c);
            }
        }
        self.alias_editor = Some(AliasEditor {
            target,
            input,
            error: None,
        });
    }

    pub(super) fn on_alias_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.alias_editor = None;
                return;
            }
            KeyCode::Enter => {
                let editor = self.alias_editor.as_ref().expect("alias editor open");
                let target = editor.target;
                let text = editor.input.data.clone();
                match self.set_local_alias(target, &text) {
                    Ok(()) => self.alias_editor = None,
                    Err(error) => {
                        self.alias_editor.as_mut().expect("alias editor open").error =
                            Some(error.to_string())
                    }
                }
                return;
            }
            _ => {}
        }
        // Honor configured text editing shortcuts, swallowing conversation actions.
        let command = self.event_to_command(&key).cloned();
        let editor = self.alias_editor.as_mut().expect("alias editor open");
        let input = &mut editor.input;
        match command {
            Some(Command::MoveText(MoveDirection::Previous, MoveAmountText::Character)) => {
                input.on_left()
            }
            Some(Command::MoveText(MoveDirection::Next, MoveAmountText::Character)) => {
                input.on_right()
            }
            Some(Command::MoveText(MoveDirection::Previous, MoveAmountText::Word)) => {
                input.move_back_word()
            }
            Some(Command::MoveText(MoveDirection::Next, MoveAmountText::Word)) => {
                input.move_forward_word()
            }
            Some(Command::BeginningOfLine) => input.on_home(),
            Some(Command::EndOfLine) => input.on_end(),
            Some(Command::DeleteCharacter(MoveDirection::Previous)) => input.on_backspace(),
            Some(Command::DeleteCharacter(MoveDirection::Next)) => input.on_delete(),
            Some(Command::KillBackwardLine | Command::KillWholeLine) => input.on_delete_line(),
            Some(Command::KillLine) => input.on_delete_suffix(),
            Some(Command::KillWord) => input.on_delete_word(),
            Some(_) => return,
            None => match key.code {
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) && !c.is_control() =>
                {
                    input.put_char(c)
                }
                KeyCode::Left => input.on_left(),
                KeyCode::Right => input.on_right(),
                KeyCode::Home => input.on_home(),
                KeyCode::End => input.on_end(),
                KeyCode::Backspace => input.on_backspace(),
                KeyCode::Delete => input.on_delete(),
                _ => return,
            },
        }
        editor.error = None;
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::test_app;
    use crate::storage::AliasTarget;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn person_alias_equal_to_uuid_still_overrides_original_channel_name() {
        let (mut app, _, _) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let person = id.user().unwrap();
        app.set_local_alias(AliasTarget::Person(person), &person.to_string())
            .unwrap();
        assert_eq!(
            app.channel_name(app.channel(id).unwrap()),
            person.to_string()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_configuration_loads_with_persisted_aliases_without_rewriting_file() {
        let (mut app, _, _) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        let person = id.user().unwrap();
        app.set_local_alias(AliasTarget::Person(person), "Kolega")
            .unwrap();
        app.set_local_alias(AliasTarget::Channel(id), "Praca")
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gurk.toml");
        let legacy = "notifications = true\n[user]\nname = \"Legacy user\"\n";
        std::fs::write(&path, legacy).unwrap();
        let config = crate::config::Config::load(&path)
            .unwrap()
            .report_deprecated_keys();
        let (reloaded, _) = super::App::try_new(config, app.signal_manager, app.storage).unwrap();
        assert_eq!(reloaded.config.user.display_name, "Legacy user");
        assert_eq!(reloaded.name_by_id_cached(person), "Kolega");
        assert_eq!(
            reloaded.channel_name(reloaded.channel(id).unwrap()),
            "Praca"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), legacy);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mouse_cannot_change_conversation_or_draft_while_editing_alias() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;
        let (mut app, _, _) = test_app().await;
        let first = app.selected_channel_id().unwrap();
        let mut second = app.channel(first).unwrap().clone();
        second.id = crate::data::ChannelId::User(uuid::Uuid::new_v4());
        second.name = "Second conversation".into();
        app.store_channel(second);
        app.channels.state.select(Some(0));
        let selected = app.selected_channel_id().unwrap();
        app.input.put_char('x');
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 2, 2),
            (MouseEventKind::ScrollDown, 2, 2),
            (MouseEventKind::ScrollUp, 2, 2),
            (MouseEventKind::ScrollUp, 60, 2),
        ] {
            app.on_mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                Rect::new(0, 0, 80, 24),
            );
            assert_eq!(app.selected_channel_id(), Some(selected));
            assert!(app.selected_message().is_none());
            assert_eq!(app.input.data, "x");
        }
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        app.on_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, 80, 24),
        );
        assert_ne!(app.selected_channel_id(), Some(selected));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pasted_alias_is_single_line_and_preserves_message_paste_behavior() {
        let (mut app, _, _) = test_app().await;
        app.on_paste("draft\nsecond line");
        assert_eq!(app.input.data, "draft\nsecond line");
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        app.on_paste("Żółw\r\nz pracy 🐢");
        assert_eq!(app.get_input().data, "Żółw z pracy 🐢");
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.input.data, "draft\nsecond line");
        let id = app.selected_channel_id().unwrap();
        assert_eq!(
            app.channel_name(app.channel(id).unwrap()),
            "Żółw z pracy 🐢"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn aliases_reject_embedded_control_characters_without_changing_saved_name() {
        let (mut app, _, _) = test_app().await;
        let target = AliasTarget::Channel(app.selected_channel_id().unwrap());
        app.set_local_alias(target, "Old alias").unwrap();
        for invalid in ["two\nlines", "name\u{1b}[31m", "name\twith tab"] {
            assert!(app.set_local_alias(target, invalid).is_err());
        }
        assert_eq!(
            app.storage
                .aliases()
                .unwrap()
                .get(&target)
                .map(String::as_str),
            Some("Old alias")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn editor_uses_configured_shortcuts_and_preserves_multiline_mode() {
        use crate::command::get_keybindings;
        let (mut app, _, _) = test_app().await;
        let bindings =
            toml::from_str("[normal]\nctrl-r = \"edit_channel_alias\"\nctrl-e = \"end_of_line\"\n")
                .unwrap();
        app.mode_keybindings = get_keybindings(&bindings, true).unwrap();
        app.is_multiline_input = true;
        app.input.new_line();
        let draft = app.input.data.clone();
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        for c in "Alias".chars() {
            app.on_key(key(KeyCode::Char(c))).await.unwrap();
        }
        app.on_key(key(KeyCode::Home)).await.unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        app.on_key(key(KeyCode::Char('!'))).await.unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let id = app.selected_channel_id().unwrap();
        assert_eq!(app.channel_name(app.channel(id).unwrap()), "Alias!");
        assert_eq!(app.input.data, draft);
        assert!(app.is_multiline_input);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn notification_origin_uses_person_and_channel_aliases() {
        let (mut app, _, _) = test_app().await;
        let direct = app.selected_channel_id().unwrap();
        let person = direct.user().unwrap();
        app.set_local_alias(AliasTarget::Person(person), "Kolega")
            .unwrap();
        assert_eq!(app.notification_origin(direct, person), "Kolega");
        app.set_local_alias(AliasTarget::Channel(direct), "Praca")
            .unwrap();
        assert_eq!(app.notification_origin(direct, person), "Praca");
        let mut group = app.channel(direct).unwrap().clone();
        group.id = crate::data::ChannelId::Group([7; 32]);
        group.name = "Original Group".into();
        let group_id = group.id;
        app.store_channel(group);
        app.set_local_alias(AliasTarget::Channel(group_id), "Zespół")
            .unwrap();
        assert_eq!(
            app.notification_origin(group_id, person),
            "Kolega in Zespół"
        );
        app.set_local_alias(AliasTarget::Channel(group_id), "")
            .unwrap();
        assert_eq!(
            app.notification_origin(group_id, person),
            "Kolega in Original Group"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn editor_saves_unicode_without_touching_draft_or_sending_messages() {
        let (mut app, _, sent) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.input.put_char('x');
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        assert!(app.alias_editor.is_some());
        for c in "  Żółw z pracy 🐢  ".chars() {
            app.on_key(key(KeyCode::Char(c))).await.unwrap();
        }
        // Destructive conversation shortcuts must not escape the modal.
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.alias_editor.is_none());
        assert_eq!(
            app.channel_name(app.channel(channel).unwrap()),
            "Żółw z pracy 🐢"
        );
        assert_eq!(app.input.data, "x");
        assert!(sent.borrow().is_empty());
        assert_eq!(app.storage.messages_count_after(channel, 0), 0);
        assert_eq!(app.storage.messages(channel).count(), 1);
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        assert_eq!(app.get_input().data, "Żółw z pracy 🐢");
        app.get_input().take();
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.storage.aliases().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn editor_cancel_does_not_modify_alias_or_draft() {
        let (mut app, _, _) = test_app().await;
        let target = AliasTarget::Person(app.selected_channel_id().unwrap().user().unwrap());
        app.set_local_alias(target, "Old alias").unwrap();
        app.input.put_char('x');
        app.on_key(key(KeyCode::F(3))).await.unwrap();
        assert_eq!(app.alias_editor.as_ref().unwrap().target, target);
        app.get_input().take();
        app.get_input().put_char('y');
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        assert_eq!(
            app.local_aliases.get(&target).map(String::as_str),
            Some("Old alias")
        );
        assert_eq!(app.input.data, "x");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn person_editor_targets_selected_author_and_noops_without_target() {
        let (mut app, _, _) = test_app().await;
        app.on_pgup();
        let author = app.selected_message().unwrap().from_id;
        app.on_key(key(KeyCode::F(3))).await.unwrap();
        assert_eq!(
            app.alias_editor.as_ref().unwrap().target,
            AliasTarget::Person(author)
        );
        app.on_key(key(KeyCode::Esc)).await.unwrap();
        app.channels.state.select(None);
        app.on_key(key(KeyCode::F(3))).await.unwrap();
        assert!(app.alias_editor.is_none());
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        assert!(app.alias_editor.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn editor_save_failure_keeps_input_and_previous_alias() {
        let (mut app, _, _) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.set_local_alias(AliasTarget::Channel(channel), "Old alias")
            .unwrap();
        app.on_key(key(KeyCode::F(2))).await.unwrap();
        app.get_input().take();
        for c in "New alias".chars() {
            app.get_input().put_char(c);
        }
        app.storage = Box::new(crate::storage::ForgetfulStorage);
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let editor = app.alias_editor.as_ref().unwrap();
        assert_eq!(editor.input.data, "New alias");
        assert!(editor.error.is_some());
        assert_eq!(app.channel_name(app.channel(channel).unwrap()), "Old alias");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn channel_picker_searches_displayed_alias() {
        let (mut app, _, _) = test_app().await;
        let channel = app.selected_channel_id().unwrap();
        app.set_local_alias(AliasTarget::Channel(channel), "Project Phoenix")
            .unwrap();
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        for c in "Phoenix".chars() {
            app.on_key(key(KeyCode::Char(c))).await.unwrap();
        }
        assert_eq!(
            app.select_channel.filtered_names().collect::<Vec<_>>(),
            vec!["Project Phoenix"]
        );
    }
}
