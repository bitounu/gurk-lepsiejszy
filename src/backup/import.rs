//! Additive merge of a fully authenticated staging database. No Signal transport calls.
use super::{cloud::CloudSession, media::restore_media, staging::Staging, types::MediaReference};
use crate::{
    signal::Attachment,
    storage::{MessageId, SqliteStorage, Storage},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, Default, serde::Serialize)]
pub struct ImportReport {
    pub channels_added: u64,
    pub messages_added: u64,
    pub duplicates_preserved: u64,
    pub timestamp_collisions: u64,
    pub media_restored: u64,
    pub media_failed: u64,
    pub messages_deferred: u64,
    pub long_text_deferred: u64,
    pub media_skipped: u64,
    pub expired_skipped: u64,
}
#[async_trait(?Send)]
pub trait MediaLoader {
    async fn restore(&self, reference: &MediaReference) -> Result<Attachment>;
}
pub struct CloudMedia<'a> {
    pub session: &'a CloudSession,
    pub media_root: [u8; 32],
    pub work_dir: &'a Path,
    pub files_dir: &'a Path,
}
#[async_trait(?Send)]
impl MediaLoader for CloudMedia<'_> {
    async fn restore(&self, reference: &MediaReference) -> Result<Attachment> {
        restore_media(
            self.session,
            &self.media_root,
            reference,
            self.work_dir,
            self.files_dir,
        )
        .await
    }
}

pub async fn merge(
    stage: &Staging,
    storage: &mut SqliteStorage,
    media: Option<&dyn MediaLoader>,
) -> Result<ImportReport> {
    anyhow::ensure!(
        stage.validated().await?,
        "refusing to import an unauthenticated staging database"
    );
    let mut report = ImportReport::default();
    for channel in stage.channels().await? {
        if storage.channel(channel.id).is_none() {
            storage.store_channel(&channel);
            anyhow::ensure!(
                storage.channel(channel.id).is_some(),
                "failed to store imported conversation"
            );
            report.channels_added += 1;
        }
    }
    let mut after = None;
    let mut order = Vec::new();
    let mut staged_ids = std::collections::BTreeSet::new();
    loop {
        let batch = stage.records(after, 1).await?;
        if batch.is_empty() {
            break;
        }
        for mut record in batch {
            after = Some(record.render_index);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis();
            if record
                .message
                .expires_at
                .is_some_and(|deadline| u128::from(deadline) <= now_ms)
            {
                report.expired_skipped += 1;
                continue;
            }
            if !staged_ids.insert((
                record.channel_id,
                record.message.from_id,
                record.message.arrived_at,
            )) {
                report.timestamp_collisions += 1;
                continue;
            }
            let existing = storage.message_by_protocol(
                record.channel_id,
                record.message.from_id,
                record.message.arrived_at,
            );
            if let Some(existing) = existing.as_ref()
                && existing.deleted
            {
                order.push(MessageId::new(record.channel_id, existing.storage_id()));
                report.duplicates_preserved += 1;
                continue;
            }
            let media_count = record.media.iter().filter(|r| !r.is_long_text).count();
            let mut complete = true;
            let mut long_text_complete = !record.media.iter().any(|r| r.is_long_text);
            if let Some(loader) = media {
                for (index, reference) in record.media.iter().enumerate() {
                    match loader.restore(reference).await {
                        Ok(attachment) => {
                            report.media_restored += 1;
                            if reference.is_long_text {
                                const MAX_LONG_TEXT: u64 = 16 * 1024 * 1024;
                                if std::fs::metadata(&attachment.filename)?.len() <= MAX_LONG_TEXT {
                                    record.message.message = Some(
                                        std::fs::read_to_string(&attachment.filename)
                                            .context("long-text attachment is not UTF-8")?,
                                    );
                                    long_text_complete = true;
                                } else {
                                    report.long_text_deferred += 1;
                                    complete = false;
                                }
                            } else {
                                record.message.attachments.push(attachment);
                            }
                        }
                        Err(error) => {
                            complete = false;
                            report.media_failed += 1;
                            stage
                                .media_error(record.render_index, index, &format!("{error:#}"))
                                .await?;
                        }
                    }
                }
            } else if !record.media.is_empty() {
                complete = false;
                report.media_skipped += record.media.len() as u64;
            }
            let stored_id;
            if let Some(mut existing) = existing {
                stored_id = existing.storage_id();
                // Preserve current text, edits, receipts and tombstones. Hydrate only missing files.
                if !existing.edited
                    && existing.message == record.message.message
                    && complete
                    && record.message.attachments.len() >= media_count
                    && (existing.attachments.len() < record.message.attachments.len()
                        || existing
                            .attachments
                            .iter()
                            .any(|file| !file.filename.is_file()))
                {
                    existing.attachments = record.message.attachments.clone();
                    storage.store_message(record.channel_id, &existing);
                    anyhow::ensure!(
                        storage
                            .message(MessageId::new(record.channel_id, stored_id))
                            .is_some_and(|saved| saved.attachments == existing.attachments),
                        "failed to store restored attachments"
                    );
                }
                // Older imports stored only a quote timestamp. Equal-time messages
                // could therefore resolve the quote to the wrong author. Restore the
                // immutable author/time from the authenticated source, preserving text edits.
                if existing.message == record.message.message
                    && record.message.quote.as_ref().is_some_and(|source| {
                        existing.quote.as_ref().is_none_or(|saved| {
                            saved.from_id != source.from_id || saved.arrived_at != source.arrived_at
                        })
                    })
                {
                    existing.quote = record.message.quote.clone();
                    storage.store_message(record.channel_id, &existing);
                    anyhow::ensure!(
                        storage
                            .message(MessageId::new(record.channel_id, stored_id))
                            .and_then(|saved| saved.quote)
                            .zip(existing.quote.as_ref())
                            .is_some_and(|(saved, source)| saved.from_id == source.from_id
                                && saved.arrived_at == source.arrived_at),
                        "failed to restore quote identity"
                    );
                }
                report.duplicates_preserved += 1;
            } else {
                if !long_text_complete {
                    report.messages_deferred += 1;
                    continue;
                }
                if record.message.message.as_deref().is_none_or(str::is_empty)
                    && record.message.attachments.is_empty()
                    && !record.message.deleted
                {
                    report.messages_deferred += 1;
                    continue;
                }
                storage.store_message(record.channel_id, &record.message);
                let saved = storage
                    .message_by_protocol(
                        record.channel_id,
                        record.message.from_id,
                        record.message.arrived_at,
                    )
                    .context("failed to store imported message")?;
                stored_id = saved.storage_id();
                report.messages_added += 1;
            }
            order.push(MessageId::new(record.channel_id, stored_id));
            // Keep staged source immutable so re-import cannot append duplicate attachments.
            if order.len() % 500 == 0 {
                eprintln!(
                    "Imported {} messages; preserved {} existing",
                    report.messages_added, report.duplicates_preserved
                );
            }
        }
    }
    storage.restore_received_order(&order).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backup::types::{ArchiveSink, ArchiveSummary, ImportedMessage},
        data::{Channel, ChannelId, Message, TypingSet},
        passphrase::Passphrase,
        storage::{AliasTarget, Draft},
    };
    use uuid::Uuid;

    #[tokio::test(flavor = "multi_thread")]
    async fn authenticated_merge_preserves_local_state_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let pass = Passphrase::new("test-only-secret").unwrap();
        let mut stage = Staging::open(&dir.path().join("stage.sqlite"), &pass)
            .await
            .unwrap();
        stage.reset_records().await.unwrap();
        let mut storage = SqliteStorage::open_unencrypted(
            &url::Url::from_file_path(dir.path().join("main.sqlite")).unwrap(),
        )
        .await
        .unwrap();
        let person = Uuid::new_v4();
        let channel = ChannelId::User(person);
        let c = Channel {
            id: channel,
            name: "Original".into(),
            group_data: None,
            unread_messages: 0,
            muted: false,
            typing: TypingSet::new(false),
            expire_timer: None,
        };
        stage.channel(c.clone()).unwrap();
        stage
            .message(ImportedMessage {
                channel_id: channel,
                message: Message::text(person, 200, "archive version".into()),
                render_index: 1,
                media: vec![],
            })
            .unwrap();
        stage
            .message(ImportedMessage {
                channel_id: channel,
                message: Message::text(person, 100, "second in received order".into()),
                render_index: 2,
                media: vec![],
            })
            .unwrap();
        assert!(merge(&stage, &mut storage, None).await.is_err());
        assert!(storage.channels().is_empty());
        storage.store_channel(&c);
        storage.store_message(
            channel,
            &Message::text(person, 200, "current edited version".into()),
        );
        storage
            .set_alias(AliasTarget::Person(person), Some("Local friend"))
            .unwrap();
        storage.set_channel_hidden(channel, true).unwrap();
        let draft = Draft {
            text: "unsent".into(),
            ..Default::default()
        };
        storage.store_draft(channel, &draft).unwrap();
        stage.finish(&ArchiveSummary::default()).await.unwrap();
        let result = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(result.messages_added, 1);
        assert_eq!(result.duplicates_preserved, 1);
        assert_eq!(
            storage
                .message(MessageId::new(channel, 200))
                .unwrap()
                .message
                .as_deref(),
            Some("current edited version")
        );
        assert_eq!(
            storage
                .aliases()
                .unwrap()
                .get(&AliasTarget::Person(person))
                .map(String::as_str),
            Some("Local friend")
        );
        assert!(storage.hidden_channels().unwrap().contains(&channel));
        assert_eq!(storage.drafts().unwrap().get(&channel), Some(&draft));
        storage.set_message_order(crate::config::MessageOrder::Received);
        assert_eq!(
            storage
                .messages_tail(channel, 10)
                .iter()
                .map(|m| m.arrived_at)
                .collect::<Vec<_>>(),
            vec![200, 100]
        );
        let result = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(result.messages_added, 0);
        assert_eq!(result.duplicates_preserved, 2);
        assert_eq!(storage.messages_tail(channel, 10).len(), 2);
    }

    struct TextLoader {
        path: std::path::PathBuf,
    }
    #[async_trait(?Send)]
    impl MediaLoader for TextLoader {
        async fn restore(&self, _: &MediaReference) -> Result<Attachment> {
            Ok(Attachment {
                id: "fixture".into(),
                content_type: "text/plain".into(),
                filename: self.path.clone(),
                size: 12,
            })
        }
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn long_text_is_deferred_until_complete_and_local_edits_survive() {
        let dir = tempfile::tempdir().unwrap();
        let mut stage = Staging::open(
            &dir.path().join("stage.sqlite"),
            &Passphrase::new("test-secret").unwrap(),
        )
        .await
        .unwrap();
        stage.reset_records().await.unwrap();
        let person = Uuid::new_v4();
        let channel = ChannelId::User(person);
        stage
            .channel(Channel {
                id: channel,
                name: "Test".into(),
                group_data: None,
                unread_messages: 0,
                muted: false,
                typing: TypingSet::new(false),
                expire_timer: None,
            })
            .unwrap();
        stage
            .message(ImportedMessage {
                channel_id: channel,
                message: Message::text(person, 1, "prefix".into()),
                render_index: 1,
                media: vec![MediaReference {
                    pointer_bytes: vec![],
                    content_type: "text/plain".into(),
                    file_name: None,
                    is_long_text: true,
                }],
            })
            .unwrap();
        stage.finish(&ArchiveSummary::default()).await.unwrap();
        let mut storage = SqliteStorage::open_unencrypted(
            &url::Url::from_file_path(dir.path().join("main.sqlite")).unwrap(),
        )
        .await
        .unwrap();
        let report = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(report.messages_deferred, 1);
        assert!(storage.message(MessageId::new(channel, 1)).is_none());
        let path = dir.path().join("text.txt");
        std::fs::write(&path, "complete body").unwrap();
        let loader = TextLoader { path };
        assert_eq!(
            merge(&stage, &mut storage, Some(&loader))
                .await
                .unwrap()
                .messages_added,
            1
        );
        let mut current = storage.message(MessageId::new(channel, 1)).unwrap();
        assert_eq!(current.message.as_deref(), Some("complete body"));
        current.message = Some("local edit".into());
        current.edited = true;
        storage.store_message(channel, &current);
        merge(&stage, &mut storage, Some(&loader)).await.unwrap();
        assert_eq!(
            storage
                .message(MessageId::new(channel, 1))
                .unwrap()
                .message
                .as_deref(),
            Some("local edit")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn equal_timestamps_from_different_senders_import_and_resume_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let pass = Passphrase::new("collision-import-test").unwrap();
        let mut stage = Staging::open(&dir.path().join("staging.sqlite"), &pass)
            .await
            .unwrap();
        stage.reset_records().await.unwrap();
        let mut storage = SqliteStorage::open_unencrypted(
            &url::Url::from_file_path(dir.path().join("main.sqlite")).unwrap(),
        )
        .await
        .unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let channel = ChannelId::Group([29; 32]);
        let c = Channel {
            id: channel,
            name: "Test group".into(),
            group_data: None,
            unread_messages: 0,
            muted: false,
            typing: TypingSet::new(true),
            expire_timer: None,
        };
        stage.channel(c.clone()).unwrap();
        storage.store_channel(&c);
        storage.store_message(channel, &Message::text(a, 500, "local text kept".into()));
        storage
            .set_alias(AliasTarget::Channel(channel), Some("Local group"))
            .unwrap();
        for (seq, who, text) in [(1, a, "archive first"), (2, b, "archive second")] {
            stage
                .message(ImportedMessage {
                    channel_id: channel,
                    message: Message::text(who, 500, text.into()),
                    render_index: seq,
                    media: vec![],
                })
                .unwrap();
        }
        stage.finish(&ArchiveSummary::default()).await.unwrap();
        let report = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(report.messages_added, 1);
        assert_eq!(report.duplicates_preserved, 1);
        assert_eq!(report.timestamp_collisions, 0);
        storage.set_message_order(crate::config::MessageOrder::Received);
        let rows = storage.messages_tail(channel, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().map(|m| m.arrived_at).collect::<Vec<_>>(),
            vec![500, 500]
        );
        assert_ne!(rows[0].storage_id(), rows[1].storage_id());
        assert_eq!(rows[0].from_id, a);
        assert_eq!(rows[1].from_id, b);
        assert_eq!(
            storage
                .message_by_protocol(channel, a, 500)
                .unwrap()
                .message
                .as_deref(),
            Some("local text kept")
        );
        assert_eq!(
            storage
                .message_by_protocol(channel, b, 500)
                .unwrap()
                .message
                .as_deref(),
            Some("archive second")
        );
        let report = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(report.messages_added, 0);
        assert_eq!(report.duplicates_preserved, 2);
        assert_eq!(report.timestamp_collisions, 0);
        assert_eq!(
            storage
                .aliases()
                .unwrap()
                .get(&AliasTarget::Channel(channel))
                .map(String::as_str),
            Some("Local group")
        );

        // Simulate an old timestamp-only quote join resolving to sender A.
        let mut legacy_reply = Message::text(a, 700, "reply".into());
        legacy_reply.quote = Some(Box::new(Message::text(a, 500, "wrong author".into())));
        storage.store_message(channel, &legacy_reply);
        let mut source_reply = Message::text(a, 700, "reply".into());
        source_reply.quote = Some(Box::new(Message::text(b, 500, "archive second".into())));
        stage
            .message(ImportedMessage {
                channel_id: channel,
                message: source_reply,
                render_index: 3,
                media: vec![],
            })
            .unwrap();
        let report = merge(&stage, &mut storage, None).await.unwrap();
        assert_eq!(report.messages_added, 0);
        let quote = storage
            .message_by_protocol(channel, a, 700)
            .unwrap()
            .quote
            .unwrap();
        assert_eq!(quote.from_id, b);
        assert_eq!(quote.arrived_at, 500);
        assert_eq!(
            quote.storage_id(),
            storage
                .message_by_protocol(channel, b, 500)
                .unwrap()
                .storage_id()
        );
    }
}
