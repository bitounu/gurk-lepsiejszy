//! Explicit forwarding creates a fresh, unquoted message without source metadata.
use super::App;
use crate::{
    data::{ChannelId, Message},
    event::Event,
    storage::MessageId,
};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use tokio::sync::Semaphore;

static FORWARD_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub(crate) struct ForwardDialog {
    pub entries: Vec<(ChannelId, String)>,
    pub selected: usize,
    pub error: Option<String>,
    pub preparing: bool,
    pub source: MessageId,
    message: Message,
    generation: u64,
}

impl App {
    pub(super) fn open_forward_dialog(&mut self) {
        if self.editing.is_some() {
            return;
        }
        let Some(source_channel) = self.selected_channel_id() else {
            return;
        };
        let Some(message) = self
            .selected_message()
            .filter(|message| !message.deleted)
            .map(|message| message.into_owned())
        else {
            return;
        };
        let mut entries: Vec<_> = self
            .channels
            .items
            .iter()
            .filter(|channel| channel.id != source_channel)
            .map(|channel| (channel.id, self.channel_name(channel).into_owned()))
            .collect();
        entries.sort_by(|a, b| {
            a.1.to_lowercase()
                .cmp(&b.1.to_lowercase())
                .then_with(|| a.1.cmp(&b.1))
        });
        self.forward_generation = self.forward_generation.wrapping_add(1);
        self.forward_dialog = Some(ForwardDialog {
            entries,
            selected: 0,
            error: None,
            preparing: false,
            source: MessageId::new(source_channel, message.storage_id()),
            message,
            generation: self.forward_generation,
        });
    }

    pub(super) fn on_forward_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.forward_dialog = None;
            return;
        }
        let Some(dialog) = self.forward_dialog.as_mut() else {
            return;
        };
        if dialog.preparing {
            return;
        }
        match key.code {
            KeyCode::Up => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Down => {
                dialog.selected = (dialog.selected + 1).min(dialog.entries.len().saturating_sub(1))
            }
            KeyCode::Home => dialog.selected = 0,
            KeyCode::End => dialog.selected = dialog.entries.len().saturating_sub(1),
            KeyCode::Enter => {
                let Some(&(target, _)) = dialog.entries.get(dialog.selected) else {
                    return;
                };
                if dialog.message.attachment_downloads.pending > 0
                    || dialog.message.attachment_downloads.failed > 0
                {
                    dialog.error =
                        Some("Download all attachments successfully before forwarding".into());
                    return;
                }
                if dialog.message.message.as_deref().is_none_or(str::is_empty)
                    && dialog.message.attachments.is_empty()
                {
                    dialog.error =
                        Some("This message has no text or attachments to forward".into());
                    return;
                }
                let paths: Vec<PathBuf> = dialog
                    .message
                    .attachments
                    .iter()
                    .map(|file| file.filename.clone())
                    .collect();
                let generation = dialog.generation;
                dialog.preparing = true;
                dialog.error = None;
                let tx = self.event_tx.clone();
                let clean = self.config.clean_image_metadata;
                tokio::spawn(async move {
                    let result = match FORWARD_SLOTS.clone().acquire_owned().await {
                        Ok(permit) => tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            super::composer::prepare_attachments(&paths, clean)
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.into())),
                        Err(error) => Err(error.into()),
                    };
                    let _ = tx.send(Event::ForwardPrepared {
                        generation,
                        target,
                        result,
                    });
                });
            }
            _ => {}
        }
    }

    pub(super) fn finish_forward(
        &mut self,
        generation: u64,
        target: ChannelId,
        result: anyhow::Result<Vec<(presage::libsignal_service::sender::AttachmentSpec, Vec<u8>)>>,
    ) {
        let Some(dialog) = self
            .forward_dialog
            .as_mut()
            .filter(|dialog| dialog.generation == generation)
        else {
            return;
        };
        dialog.preparing = false;
        let mut files = match result {
            Ok(files) => files,
            Err(error) => {
                dialog.error = Some(format!("Cannot forward: {error:#}"));
                return;
            }
        };
        for ((spec, _), original) in files.iter_mut().zip(&dialog.message.attachments) {
            if spec.content_type == "application/octet-stream"
                && original.content_type.parse::<mime_guess::Mime>().is_ok()
            {
                spec.content_type = original.content_type.clone();
            }
        }
        let source = dialog.source;
        let raw = dialog.message.message.clone().unwrap_or_default();
        let ranges = dialog.message.body_ranges.clone();
        let text = crate::mentions::expand(&raw, &ranges, |id| self.name_by_id_cached(id));
        if self
            .storage
            .message(source)
            .is_none_or(|message| message.deleted)
        {
            self.forward_dialog.as_mut().unwrap().error =
                Some("Source message was removed; forwarding canceled".into());
            return;
        }
        if !self
            .channels
            .items
            .iter()
            .any(|channel| channel.id == target && channel.id != source.channel_id)
        {
            return;
        }
        let mut text = Some(text);
        for files in crate::signal::attachment_batch::compatible_batches(files) {
            // Fresh messages, never inheriting the selected quote, edit state or source metadata.
            self.queue_payload_part(target, text.take().unwrap_or_default(), files, None, None);
        }
        self.bubble_up_channel(target);
        self.forward_dialog = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::test_app;
    use crate::signal::{Attachment, test::SignalManagerMock};
    use crossterm::event::KeyModifiers;
    use uuid::Uuid;

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarding_multiple_documents_splits_without_source_or_draft_changes() {
        use presage::libsignal_service::sender::AttachmentSpec;
        let (mut app, mut events, _) = test_app().await;
        let source_channel = app.selected_channel_id().unwrap();
        let source = MessageId::new(source_channel, 0);
        let source_message = app.storage.message(source).unwrap();
        let mock = SignalManagerMock::new();
        let sent = mock.sent_messages.clone();
        let files = mock.sent_files.clone();
        app.signal_manager = Box::new(mock);
        let mut channel = app.channel(source_channel).unwrap().clone();
        channel.id = ChannelId::Group([71; 32]);
        let target = channel.id;
        app.store_channel(channel);
        for c in "unsent draft".chars() {
            app.input.put_char(c);
        }
        app.on_pgup();
        app.forward_dialog = Some(ForwardDialog {
            entries: vec![(target, "target".into())],
            selected: 0,
            error: None,
            preparing: true,
            source,
            message: source_message.clone(),
            generation: 7,
        });
        let payload = (0..3)
            .map(|n| {
                (
                    AttachmentSpec {
                        content_type: "application/pdf".into(),
                        file_name: Some(format!("doc{n}.pdf")),
                        length: 1,
                        ..Default::default()
                    },
                    vec![n],
                )
            })
            .collect();
        app.finish_forward(7, target, Ok(payload));
        for _ in 0..3 {
            app.handle_event(events.recv().await.unwrap()).unwrap();
        }
        assert!(app.forward_dialog.is_none());
        assert_eq!(app.input.data, "unsent draft");
        assert_eq!(app.selected_channel_id(), Some(source_channel));
        assert_eq!(app.positions[&source_channel].selected, Some(0));
        assert_eq!(
            files.borrow().iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1, 1]
        );
        let sent = sent.borrow();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|m| m.quote.is_none()));
        assert_eq!(sent[0].message, source_message.message);
        assert!(sent[1..].iter().all(|m| m.message.as_deref() == Some("")));
        assert_eq!(app.storage.messages(target).count(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarding_reuploads_files_without_source_quote_and_preserves_draft() {
        let (mut app, mut events, _) = test_app().await;
        let source = app.selected_channel_id().unwrap();
        let mock = SignalManagerMock::new();
        let sent = mock.sent_messages.clone();
        let files = mock.sent_files.clone();
        let targets = mock.sent_destinations.clone();
        app.signal_manager = Box::new(mock);
        let mut target = app.channel(source).unwrap().clone();
        target.id = ChannelId::Group([77; 32]);
        target.name = "Destination".into();
        let target_id = target.id;
        app.store_channel(target);
        let hidden = ChannelId::User(Uuid::new_v4());
        app.hidden_channels.insert(hidden);
        let mut other = app.channel(source).unwrap().clone();
        other.id = hidden;
        app.store_channel(other);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut message = app.storage.message(MessageId::new(source, 0)).unwrap();
        message.message = Some("body only".into());
        message.quote = Some(Box::new(message.clone()));
        message.attachments.push(Attachment {
            id: "original-remote-id".into(),
            content_type: "image/png".into(),
            filename: path,
            size: bytes.len() as u32,
        });
        app.store_message(source, message);
        app.on_pgup();
        for c in "unsent draft".chars() {
            app.input.put_char(c);
        }
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT))
            .await
            .unwrap();
        let dialog = app.forward_dialog.as_ref().unwrap();
        assert_eq!(dialog.entries.len(), 1);
        assert_eq!(dialog.entries[0].0, target_id);
        app.on_paste("do not modify draft");
        app.on_key(KeyCode::Enter.into()).await.unwrap();
        assert!(sent.borrow().is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while app.forward_dialog.is_some() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(targets.borrow().as_slice(), &[target_id]);
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message.as_deref(), Some("body only"));
        assert!(sent[0].quote.is_none());
        assert!(sent[0].body_ranges.is_empty());
        assert_eq!(files.borrow()[0][0].0, "image/png");
        assert_eq!(files.borrow()[0][0].2, bytes);
        assert_eq!(app.input.data, "unsent draft");
        assert_eq!(app.selected_channel_id(), Some(source));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_forward_file_and_cancel_never_send_partial_message() {
        let (mut app, mut events, sent) = test_app().await;
        let source = app.selected_channel_id().unwrap();
        let mut target = app.channel(source).unwrap().clone();
        target.id = ChannelId::User(Uuid::new_v4());
        app.store_channel(target);
        let mut message = app.storage.message(MessageId::new(source, 0)).unwrap();
        message.attachments.push(Attachment {
            id: "missing".into(),
            content_type: "text/plain".into(),
            filename: "/missing-gurk-forward-file".into(),
            size: 1,
        });
        app.store_message(source, message);
        app.on_pgup();
        app.open_forward_dialog();
        app.on_forward_key(KeyCode::Enter.into());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while app.forward_dialog.as_ref().unwrap().preparing {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert!(app.forward_dialog.as_ref().unwrap().error.is_some());
        assert!(sent.borrow().is_empty());
        app.on_forward_key(KeyCode::Esc.into());
        assert!(app.forward_dialog.is_none());
    }
}
