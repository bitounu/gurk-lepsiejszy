use crossterm::event::{KeyCode, KeyEvent};
use uuid::Uuid;

use super::App;
use crate::data::{Channel, ChannelId};

pub(crate) struct HiddenDialog {
    pub(crate) entries: Vec<(ChannelId, String)>,
    pub(crate) selected: usize,
    pub(crate) error: Option<String>,
}

impl App {
    /// Mutate either channel cache, writing through to storage only on change.
    pub(super) fn modify_channel_by_id(
        &mut self,
        id: ChannelId,
        f: impl FnOnce(&mut Channel) -> bool,
    ) -> Option<bool> {
        if let Some(channel) = self.hidden_channel_data.get_mut(&id) {
            let changed = f(channel);
            if changed {
                self.storage.store_channel(channel);
            }
            Some(changed)
        } else {
            self.channels
                .modify_channel_by_id(&mut *self.storage, id, f)
        }
    }

    pub(crate) fn is_locally_muted(&self, channel: ChannelId, sender: Uuid) -> bool {
        self.hidden_channels.contains(&channel)
            || self.hidden_channels.contains(&ChannelId::User(sender))
            || self.channel(channel).is_some_and(|channel| channel.muted)
    }

    pub fn hide_current_channel(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.selected_channel_id() else {
            return Ok(());
        };
        // Save before hiding so a failed persistence operation leaves the active composer intact.
        self.persist_composer()?;
        self.storage.set_channel_hidden(id, true)?;
        self.hidden_channels.insert(id);
        let index = self.channels.state.selected().unwrap();
        let channel = self.channels.items.remove(index);
        self.hidden_channel_data.insert(id, channel);
        let next = if self.channels.items.is_empty() {
            None
        } else {
            Some(index.min(self.channels.items.len() - 1))
        };
        self.channels.state.select(next);
        self.swap_channel_draft(Some(id), self.selected_channel_id());
        self.on_channel_changed();
        // Invalidate any Ctrl-P snapshot containing the now-hidden conversation.
        self.select_channel = Default::default();
        self.persist_composer()?;
        Ok(())
    }

    fn hidden_entries(&self) -> Vec<(ChannelId, String)> {
        let mut entries: Vec<_> = self
            .hidden_channel_data
            .values()
            .map(|channel| {
                let kind = match channel.id {
                    ChannelId::User(_) => "person",
                    ChannelId::Group(_) => "group",
                };
                (
                    channel.id,
                    format!("{} [{kind}]", self.channel_name(channel)),
                )
            })
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        entries
    }

    pub fn open_hidden_channels(&mut self) {
        self.hidden_dialog = Some(HiddenDialog {
            entries: self.hidden_entries(),
            selected: 0,
            error: None,
        });
    }

    pub fn on_hidden_channels_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        let Some(dialog) = self.hidden_dialog.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc => self.hidden_dialog = None,
            KeyCode::Up => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Down => {
                dialog.selected = (dialog.selected + 1).min(dialog.entries.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                let Some(&(id, _)) = dialog.entries.get(dialog.selected) else {
                    return Ok(());
                };
                if let Err(error) = self.restore_hidden_channel(id) {
                    self.hidden_dialog.as_mut().unwrap().error = Some(format!("{error:#}"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn restore_hidden_channel(&mut self, id: ChannelId) -> anyhow::Result<()> {
        self.persist_composer()?;
        self.storage.set_channel_hidden(id, false)?;
        let old = self.selected_channel_id();
        self.hidden_channels.remove(&id);
        if let Some(channel) = self.hidden_channel_data.remove(&id) {
            self.channels.items.push(channel);
            self.channels
                .state
                .select(Some(self.channels.items.len() - 1));
        }
        self.sort_favorite_channels();
        self.swap_channel_draft(old, self.selected_channel_id());
        self.on_channel_changed();
        let entries = self.hidden_entries();
        if let Some(dialog) = self.hidden_dialog.as_mut() {
            dialog.entries = entries;
            dialog.selected = dialog.selected.min(dialog.entries.len().saturating_sub(1));
            dialog.error = None;
        }
        self.persist_composer()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_app;
    use super::*;
    use crate::storage::MessageId;

    #[tokio::test(flavor = "multi_thread")]
    async fn hiding_last_conversation_preserves_draft_and_incoming_messages() {
        let (mut app, _, _) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        app.input.put_char('ż');
        app.pending_attachments.push("/tmp/draft.png".into());
        app.hide_current_channel().unwrap();
        assert!(app.channels().is_empty());
        assert!(app.selected_channel_id().is_none());
        assert!(app.window.is_none());
        assert!(app.input.data.is_empty());
        assert!(app.pending_attachments.is_empty());
        assert!(app.storage.hidden_channels().unwrap().contains(&id));
        assert_eq!(app.storage.drafts().unwrap()[&id].text, "ż");
        assert_eq!(app.storage.last_draft_channel().unwrap(), None);
        let (mut app, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        assert!(app.channels().is_empty());
        assert!(app.window.is_none());
        assert!(app.input.data.is_empty());
        let unread_before = app.channel(id).unwrap().unread_messages;
        let mut incoming = app.storage.message(MessageId::new(id, 0)).unwrap();
        incoming.arrived_at = 10;
        incoming.from_id = Uuid::new_v4();
        app.add_message_to_channel(id, incoming);
        assert!(app.storage.message(MessageId::new(id, 10)).is_some());
        assert!(app.channels().is_empty());
        assert_eq!(app.channel(id).unwrap().unread_messages, unread_before + 1);
        app.open_hidden_channels();
        app.on_hidden_channels_key(KeyCode::Enter.into()).unwrap();
        assert_eq!(app.selected_channel_id(), Some(id));
        assert_eq!(app.input.data, "ż");
        assert_eq!(
            app.pending_attachments,
            vec![std::path::PathBuf::from("/tmp/draft.png")]
        );
        assert!(app.hidden_dialog.as_ref().unwrap().entries.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_people_mute_other_conversations_and_restart_stays_hidden() {
        let (mut app, _, _) = test_app().await;
        let hidden = app.selected_channel_id().unwrap();
        let person = hidden.user().unwrap();
        let mut visible = app.channel(hidden).unwrap().clone();
        visible.id = ChannelId::Group([3; 32]);
        let other = visible.id;
        app.store_channel(visible);
        app.hide_current_channel().unwrap();
        assert!(app.is_locally_muted(hidden, Uuid::new_v4()));
        assert!(app.is_locally_muted(other, person));
        assert!(!app.is_locally_muted(other, Uuid::new_v4()));
        let (restored, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        assert!(restored.channel(hidden).is_some());
        assert!(restored.channels().iter().all(|c| c.id != hidden));
        assert_eq!(restored.selected_channel_id(), Some(other));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn page_down_clears_selection_and_viewport_at_newest() {
        let (mut app, _, sent) = test_app().await;
        let id = app.selected_channel_id().unwrap();
        app.on_pgdn();
        assert!(app.positions.entry(id).or_default().selected.is_none());
        app.positions.get_mut(&id).unwrap().selected = Some(0);
        app.positions.get_mut(&id).unwrap().viewport_bottom = Some(0);
        app.on_pgdn();
        let pos = &app.positions[&id];
        assert!(pos.selected.is_none());
        assert!(pos.viewport_bottom.is_none());
        app.input.put_char('a');
        app.send_input(id);
        assert!(sent.borrow()[0].quote.is_none());
        app.on_pgup();
        let selected = app.positions[&id].selected.unwrap();
        app.input.put_char('b');
        app.send_input(id);
        assert_eq!(
            sent.borrow()[1].quote.as_ref().unwrap().arrived_at,
            selected
        );
    }
}
