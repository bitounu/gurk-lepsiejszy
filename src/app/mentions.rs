//! Explicit contact selection for composer mentions.
use super::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};
use uuid::Uuid;

pub(crate) struct MentionCandidate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
}
pub(crate) struct MentionCompletion {
    pub range: Range<usize>,
    pub candidates: Vec<MentionCandidate>,
    pub selected: usize,
    text: String,
    cursor: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::tests::test_app, storage::AliasTarget};
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn completion_selects_a_contact_without_sending() {
        let (mut app, _events, sent) = test_app().await;
        let id = Uuid::new_v4();
        app.storage.store_name(id, "Bartosz");
        for c in "@Barto".chars() {
            app.on_key(key(KeyCode::Char(c))).await.unwrap();
        }
        app.refresh_mentions();
        assert_eq!(
            app.mention_completion.as_ref().unwrap().candidates[0].id,
            id
        );
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.input.data, "@Bartosz ");
        assert_eq!(app.input.mentions[0].aci, id);
        assert!(sent.borrow().is_empty());
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn aliases_duplicates_dismissal_and_guards() {
        let (mut app, _events, sent) = test_app().await;
        let ids = [Uuid::new_v4(), Uuid::new_v4()];
        for id in ids {
            app.storage.store_name(id, "Bartosz");
        }
        for c in "@Bart".chars() {
            app.input.put_char(c);
        }
        app.refresh_mentions();
        let popup = app.mention_completion.as_ref().unwrap();
        assert_eq!(popup.candidates.len(), 2);
        assert_ne!(popup.candidates[0].label, popup.candidates[1].label);
        assert!(app.on_mention_key(key(KeyCode::Esc)));
        app.refresh_mentions();
        assert!(app.mention_completion.is_none());
        assert_eq!(app.input.data, "@Bart");
        app.input.take();
        app.set_local_alias(AliasTarget::Person(ids[0]), "Friend")
            .unwrap();
        for c in "@Fri".chars() {
            app.input.put_char(c);
        }
        assert!(app.on_mention_key(key(KeyCode::Tab)));
        assert_eq!(app.input.data, "@Friend ");
        app.input.take();
        for c in "email@Bart".chars() {
            app.input.put_char(c);
        }
        app.refresh_mentions();
        assert!(app.mention_completion.is_none());
        app.input.take();
        for c in "@Bart".chars() {
            app.input.put_char(c);
        }
        app.display_help = true;
        app.refresh_mentions();
        assert!(app.mention_completion.is_none());
        assert!(sent.borrow().is_empty());
    }
}

fn clean(text: &str) -> String {
    // Strip full escape sequences as well as controls before displaying or inserting names.
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

impl App {
    pub(crate) fn refresh_mentions(&mut self) {
        self.input.sync_mentions();
        if self.display_help
            || self.select_channel.is_shown
            || self.file_picker.is_some()
            || self.image_viewer.is_some()
            || self.forward_dialog.is_some()
            || self.hidden_dialog.is_some()
            || self.alias_editor.is_some()
            || self.spelling_dialog.is_some()
            || self.text_selection.is_some()
            || self.send_job.is_some()
            || self.message_search.is_some()
        {
            self.mention_completion = None;
            return;
        }
        let Some(channel) = self.selected_channel_id() else {
            self.mention_completion = None;
            return;
        };
        let cursor = self.input.cursor.idx;
        let fingerprint = (channel, self.input.data.clone(), cursor);
        if self.mention_dismissed.as_ref() == Some(&fingerprint) {
            self.mention_completion = None;
            return;
        }
        self.mention_dismissed = None;
        let Some(before) = self.input.data.get(..cursor) else {
            self.mention_completion = None;
            return;
        };
        let Some(start) = before.rfind('@') else {
            self.mention_completion = None;
            return;
        };
        let boundary = before[..start]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || "([{\"'".contains(c));
        let prefix = &before[start + 1..];
        if !boundary
            || prefix
                .chars()
                .any(|c| c.is_whitespace() || c == '@' || c.is_control())
            || self
                .input
                .mentions
                .iter()
                .any(|m| start >= m.start && start < m.end)
        {
            self.mention_completion = None;
            return;
        }
        let prefix = prefix.to_lowercase();
        let members: HashSet<_> = self
            .selected_channel()
            .and_then(|c| c.group_data.as_ref())
            .map(|g| g.members.iter().copied().collect())
            .unwrap_or_default();
        let mut names: BTreeMap<_, _> = self.storage.names().collect();
        let cache = self.names_cache.take();
        if let Some(cache) = &cache {
            for (id, name) in cache {
                names.entry(*id).or_insert_with(|| name.clone());
            }
        }
        self.names_cache.replace(cache);
        for channel in self.all_channels() {
            if let Some(id) = channel.id.user() {
                names.entry(id).or_insert_with(|| channel.name.clone());
            }
        }
        for id in members.iter().copied().chain(std::iter::once(self.user_id)) {
            names
                .entry(id)
                .or_insert_with(|| self.name_by_id_cached(id));
        }
        let mut candidates: Vec<_> = names
            .into_iter()
            .filter_map(|(id, original)| {
                if self
                    .hidden_channels
                    .contains(&crate::data::ChannelId::User(id))
                {
                    return None;
                }
                let cached = self.name_by_id_cached(id);
                let name = clean(if cached == id.to_string() {
                    &original
                } else {
                    &cached
                });
                let matches = |s: &str| {
                    s.to_lowercase()
                        .split_whitespace()
                        .any(|part| part.starts_with(&prefix))
                };
                (!name.is_empty() && (prefix.is_empty() || matches(&name) || matches(&original)))
                    .then(|| MentionCandidate {
                        id,
                        label: name.clone(),
                        name,
                    })
            })
            .collect();
        candidates.sort_by_key(|c| (!members.contains(&c.id), c.name.to_lowercase(), c.id));
        let mut counts = BTreeMap::new();
        for c in &candidates {
            *counts.entry(c.name.to_lowercase()).or_insert(0) += 1;
        }
        for c in &mut candidates {
            if counts[&c.name.to_lowercase()] > 1 {
                c.label = format!("{} · {}", c.name, c.id);
            }
        }
        if candidates.is_empty() {
            self.mention_completion = None;
            return;
        }
        let selected = self
            .mention_completion
            .as_ref()
            .filter(|p| p.text == self.input.data && p.cursor == cursor)
            .and_then(|p| p.candidates.get(p.selected))
            .and_then(|previous| candidates.iter().position(|c| c.id == previous.id))
            .unwrap_or(0);
        self.mention_completion = Some(MentionCompletion {
            range: start..cursor,
            candidates,
            selected,
            text: self.input.data.clone(),
            cursor,
        });
    }

    pub(crate) fn on_mention_key(&mut self, key: KeyEvent) -> bool {
        self.refresh_mentions();
        let allowed = key.modifiers.is_empty()
            || (key.code == KeyCode::BackTab && key.modifiers == KeyModifiers::SHIFT);
        if !allowed {
            return false;
        }
        let Some(popup) = self.mention_completion.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Enter | KeyCode::Tab => {
                let candidate = &popup.candidates[popup.selected];
                self.input
                    .insert_mention(popup.range.clone(), &candidate.name, candidate.id);
                self.mention_completion = None;
                self.mention_dismissed = None;
            }
            KeyCode::Down => popup.selected = (popup.selected + 1) % popup.candidates.len(),
            KeyCode::Up | KeyCode::BackTab => {
                popup.selected =
                    (popup.selected + popup.candidates.len() - 1) % popup.candidates.len()
            }
            KeyCode::Esc => {
                self.mention_dismissed = self
                    .selected_channel_id()
                    .map(|id| (id, self.input.data.clone(), self.input.cursor.idx));
                self.mention_completion = None;
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod sending_tests {
    use super::*;
    use crate::{app::tests::test_app, data::AssociatedValue, storage::AliasTarget};
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    async fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c))).await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepted_alias_sends_identity_and_utf16_range_without_local_label() {
        let (mut app, _events, sent) = test_app().await;
        let id = Uuid::new_v4();
        app.storage.store_name(id, "Bartosz Original");
        app.set_local_alias(AliasTarget::Person(id), "Bartosz Local Alias")
            .unwrap();
        type_text(&mut app, "😀 @Barto").await;
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.input.data, "😀 @Bartosz Local Alias ");
        assert!(sent.borrow().is_empty());
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message.as_deref(), Some("😀 \u{fffc}"));
        assert_eq!(sent[0].body_ranges.len(), 1);
        let range = &sent[0].body_ranges[0];
        assert_eq!((range.start, range.end), (3, 4));
        assert_eq!(range.value, AssociatedValue::MentionUuid(id));
        assert!(!sent[0].message.as_ref().unwrap().contains("Alias"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_name_down_selects_the_second_identity_and_multiple_mentions_send() {
        let (mut app, _events, sent) = test_app().await;
        let ids = [Uuid::from_u128(101), Uuid::from_u128(102)];
        for id in ids {
            app.storage.store_name(id, "Bartosz");
        }
        type_text(&mut app, "@Barto").await;
        app.on_key(key(KeyCode::Down)).await.unwrap();
        app.on_key(key(KeyCode::Tab)).await.unwrap();
        assert_eq!(app.input.mentions[0].aci, ids[1]);
        type_text(&mut app, "and @Barto").await;
        app.on_key(key(KeyCode::Tab)).await.unwrap();
        assert_eq!(app.input.mentions[1].aci, ids[0]);
        app.on_key(key(KeyCode::Enter)).await.unwrap();
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message.as_deref(), Some("\u{fffc} and \u{fffc}"));
        assert_eq!(sent[0].body_ranges.len(), 2);
        for (range, (start, id)) in sent[0].body_ranges.iter().zip([(0, ids[1]), (6, ids[0])]) {
            assert_eq!((range.start, range.end), (start, start + 1));
            assert_eq!(range.value, AssociatedValue::MentionUuid(id));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mentions_survive_channel_draft_swaps_and_storage_reload() {
        let (mut app, _, _) = test_app().await;
        let first = app.selected_channel_id().unwrap();
        let mut other = app.channel(first).unwrap().clone();
        other.id = crate::data::ChannelId::User(Uuid::new_v4());
        let second = other.id;
        app.store_channel(other);
        let id = Uuid::new_v4();
        app.storage.store_name(id, "Bartosz");
        type_text(&mut app, "@Barto").await;
        app.on_key(key(KeyCode::Tab)).await.unwrap();
        let mentions = app.input.mentions.clone();
        app.channels
            .state
            .select(app.channels.items.iter().position(|c| c.id == second));
        app.swap_channel_draft(Some(first), Some(second));
        assert!(app.input.mentions.is_empty());
        type_text(&mut app, "another draft").await;
        app.persist_composer().unwrap();
        assert_eq!(app.storage.drafts().unwrap()[&first].mentions, mentions);
        let (mut restored, _) = App::try_new(app.config, app.signal_manager, app.storage).unwrap();
        restored
            .channels
            .state
            .select(restored.channels.items.iter().position(|c| c.id == first));
        restored.swap_channel_draft(Some(second), Some(first));
        assert_eq!(restored.input.data, "@Bartosz ");
        assert_eq!(restored.input.mentions, mentions);
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread")]
    async fn staged_attachment_caption_keeps_confirmed_mention_identity() {
        let (mut app, mut events, sent) = crate::app::tests::test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let id = Uuid::new_v4();
        app.storage.store_name(id, "Bartosz");
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"attachment").unwrap();
        app.stage_attachment_for(channel, file.path().to_owned())
            .unwrap();
        for c in "@Barto".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .await
                .unwrap();
        }
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .await
            .unwrap();
        assert!(sent.borrow().is_empty());
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while sent.borrow().is_empty() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message.as_deref(), Some("\u{fffc}"));
        assert_eq!(
            sent[0].body_ranges[0].value,
            crate::data::AssociatedValue::MentionUuid(id)
        );
        assert_eq!(
            (sent[0].body_ranges[0].start, sent[0].body_ranges[0].end),
            (0, 1)
        );
        assert!(app.input.is_empty());
        assert!(app.pending_attachments.is_empty());
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;
    use crate::{
        data::{AssociatedValue, BodyRange},
        storage::{AliasTarget, MessageId},
    };
    #[tokio::test(flavor = "multi_thread")]
    async fn editing_wire_mention_expands_name_and_preserves_identity_when_appending() {
        let (mut app, _events, sent) = crate::app::tests::test_app().await;
        let channel = app.selected_channel_id().unwrap();
        let id = Uuid::new_v4();
        app.set_local_alias(AliasTarget::Person(id), "Bartosz Alias")
            .unwrap();
        let mut message = app.storage.message(MessageId::new(channel, 0)).unwrap();
        message.arrived_at = crate::util::utc_now_timestamp_msec();
        message.message = Some("😀 \u{fffc}".into());
        message.body_ranges = vec![BodyRange {
            start: 3,
            end: 4,
            value: AssociatedValue::MentionUuid(id),
        }];
        app.store_message(channel, message);
        app.on_key(KeyCode::PageUp.into()).await.unwrap();
        app.on_key(KeyCode::Char('e').into()).await.unwrap();
        assert!(app.is_editing());
        assert_eq!(app.input.data, "😀 @Bartosz Alias");
        assert_eq!(app.input.mentions[0].aci, id);
        app.input.put_char('!');
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message.as_deref(), Some("😀 \u{fffc}!"));
        assert_eq!(
            sent[0].body_ranges[0].value,
            AssociatedValue::MentionUuid(id)
        );
    }
}

#[cfg(test)]
mod file_label_regressions {
    use super::*;
    use crate::{data::AssociatedValue, signal::test::SignalManagerMock};

    #[tokio::test(flavor = "multi_thread")]
    async fn confirmed_file_like_contact_labels_never_become_attachment_commands() {
        let label_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(label_file.path(), b"PRIVATE CONTACT LABEL FILE").unwrap();
        let staged_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(staged_file.path(), b"EXPLICITLY STAGED FILE").unwrap();
        for label in [
            "Bartosz file:notes".to_owned(),
            format!("Bartosz file://{}", label_file.path().display()),
        ] {
            for staged in [false, true] {
                let (mut app, mut events, _) = crate::app::tests::test_app().await;
                let mock = SignalManagerMock::new();
                let files = mock.sent_files.clone();
                let sent = mock.sent_messages.clone();
                app.signal_manager = Box::new(mock);
                let id = Uuid::new_v4();
                app.storage.store_name(id, &label);
                if staged {
                    app.stage_attachment_for(
                        app.selected_channel_id().unwrap(),
                        staged_file.path().to_owned(),
                    )
                    .unwrap();
                }
                for c in "@Barto".chars() {
                    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                        .await
                        .unwrap();
                }
                app.on_key(KeyCode::Tab.into()).await.unwrap();
                assert_eq!(app.input.data, format!("@{label} "));
                app.on_key(KeyCode::Enter.into()).await.unwrap();
                if staged {
                    tokio::time::timeout(std::time::Duration::from_secs(3), async {
                        while sent.borrow().is_empty() {
                            app.handle_event(events.recv().await.unwrap()).unwrap();
                        }
                    })
                    .await
                    .unwrap();
                } else {
                    assert!(
                        app.send_job.is_none(),
                        "label started attachment preparation: {label}"
                    );
                }
                let sent = sent.borrow();
                assert_eq!(sent.len(), 1, "label={label}, staged={staged}");
                assert_eq!(sent[0].message.as_deref(), Some("\u{fffc}"));
                assert_eq!(sent[0].body_ranges.len(), 1);
                assert_eq!(
                    sent[0].body_ranges[0].value,
                    AssociatedValue::MentionUuid(id)
                );
                let files = files.borrow();
                assert_eq!(files.len(), 1);
                if staged {
                    assert_eq!(files[0].len(), 1);
                    assert_eq!(files[0][0].2, b"EXPLICITLY STAGED FILE");
                } else {
                    assert!(
                        files[0].is_empty(),
                        "contact label uploaded a file: {label}"
                    );
                }
            }
        }
    }
}
