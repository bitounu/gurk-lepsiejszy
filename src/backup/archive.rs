//! Bounded, authenticated streaming conversion of Signal backup frames.
use super::types::*;
use crate::data::{
    AssociatedValue, BodyRange, Channel, ChannelId, GroupData, Message, Style, TypingSet,
};
use crate::receipt::Receipt;
use anyhow::{Context, Result, bail};
use futures::{AsyncRead, AsyncReadExt};
use libsignal_message_backup::unknown::VisitUnknownFieldsExt as _;
use libsignal_message_backup::{
    backup::{CompletedBackup, PartialBackup, Purpose, ValidateOnly},
    frame::{FileReaderFactory, FramesReader, VerifyHmac},
    key::MessageBackupKey,
    proto::backup as pb,
};
use protobuf::Message as _;
use std::{collections::HashMap, path::Path};
use uuid::Uuid;

const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Unlike upstream's delimited reader, check the length before allocating.
async fn next_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut length = 0u64;
    for index in 0..10 {
        let mut byte = [0];
        if r.read(&mut byte).await? == 0 {
            if index == 0 {
                return Ok(None);
            }
            bail!("truncated frame length");
        }
        if index == 9 && byte[0] > 1 {
            bail!("overflowing frame length");
        }
        length |= u64::from(byte[0] & 127) << (index * 7);
        if byte[0] & 128 == 0 {
            anyhow::ensure!(
                length <= MAX_FRAME as u64,
                "backup frame exceeds 64 MiB limit"
            );
            let mut bytes = vec![0; length as usize];
            r.read_exact(&mut bytes)
                .await
                .context("truncated backup frame")?;
            return Ok(Some(bytes));
        }
    }
    bail!("invalid frame length")
}

#[derive(Clone)]
struct Recipient {
    person: Option<Uuid>,
    channel: Option<Channel>,
}

fn uuid(bytes: &[u8]) -> Option<Uuid> {
    Uuid::from_slice(bytes).ok()
}
fn channel(id: ChannelId, name: String, group_data: Option<GroupData>) -> Channel {
    Channel {
        id,
        name,
        group_data,
        unread_messages: 0,
        muted: false,
        typing: TypingSet::new(matches!(id, ChannelId::Group(_))),
        expire_timer: None,
    }
}
fn media(pointer: &pb::FilePointer, is_long_text: bool) -> Result<MediaReference> {
    Ok(MediaReference {
        pointer_bytes: pointer.write_to_bytes()?,
        content_type: pointer
            .contentType
            .clone()
            .unwrap_or_else(|| "application/octet-stream".into()),
        file_name: pointer.fileName.clone(),
        is_long_text,
    })
}

pub async fn read_archive(
    path: &Path,
    key: &MessageBackupKey,
    own_aci: Uuid,
    sink: &mut dyn ArchiveSink,
) -> Result<ArchiveSummary> {
    let mut reader = FramesReader::new(key, FileReaderFactory { path }).await?;
    let first = next_frame(&mut reader).await?.context("empty backup")?;
    let info = pb::BackupInfo::parse_from_bytes(&first)?;
    sink.metadata(BackupMetadata {
        media_root: info
            .mediaRootBackupKey
            .as_slice()
            .try_into()
            .context("invalid media root key")?,
        version: info.version,
        backup_time_ms: info.backupTimeMs,
    })?;
    let mut validator = PartialBackup::<ValidateOnly>::new_validator(info, Purpose::RemoteBackup)?;
    let mut recipients = HashMap::<u64, Recipient>::new();
    let mut chats = HashMap::<u64, ChannelId>::new();
    let mut summary = ArchiveSummary::default();
    let mut render_index = 0;
    while let Some(raw) = next_frame(&mut reader).await? {
        summary.frames += 1;
        let frame = pb::Frame::parse_from_bytes(&raw)?;
        let mut unsupported = if frame.collect_unknown_fields().is_empty() {
            None
        } else {
            Some("unknown_protobuf_fields")
        };
        match frame.item.as_ref() {
            Some(pb::frame::Item::Recipient(r)) => {
                let mapped = match r.destination.as_ref() {
                    Some(pb::recipient::Destination::Self_(_)) => Recipient {
                        person: Some(own_aci),
                        channel: Some(channel(own_aci.into(), "Note to Self".into(), None)),
                    },
                    Some(pb::recipient::Destination::Contact(c)) => {
                        let person = c.aci.as_deref().and_then(uuid);
                        let name = [c.systemGivenName.as_str(), c.systemFamilyName.as_str()]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let name = if name.is_empty() {
                            [
                                c.profileGivenName.as_deref().unwrap_or(""),
                                c.profileFamilyName.as_deref().unwrap_or(""),
                            ]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ")
                        } else {
                            name
                        };
                        if person.is_none() {
                            unsupported = Some("recipient_without_aci");
                        }
                        Recipient {
                            person,
                            channel: person.map(|id| {
                                channel(
                                    id.into(),
                                    if name.is_empty() {
                                        id.to_string()
                                    } else {
                                        name
                                    },
                                    None,
                                )
                            }),
                        }
                    }
                    Some(pb::recipient::Destination::Group(g)) => {
                        let id = ChannelId::from_master_key_bytes(&g.masterKey)?;
                        let snapshot = g.snapshot.as_ref();
                        let name = snapshot
                            .and_then(|s| s.title.as_ref())
                            .and_then(|t| match t.content.as_ref() {
                                Some(pb::group::group_attribute_blob::Content::Title(t)) => {
                                    Some(t.clone())
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| "Imported group".into());
                        let data = GroupData {
                            master_key_bytes: g
                                .masterKey
                                .as_slice()
                                .try_into()
                                .context("invalid group key")?,
                            revision: snapshot.map(|s| s.version).unwrap_or(0),
                            members: snapshot
                                .map(|s| s.members.iter().filter_map(|m| uuid(&m.userId)).collect())
                                .unwrap_or_default(),
                        };
                        Recipient {
                            person: None,
                            channel: Some(channel(id, name, Some(data))),
                        }
                    }
                    _ => {
                        unsupported = Some("unsupported_recipient");
                        Recipient {
                            person: None,
                            channel: None,
                        }
                    }
                };
                recipients.insert(r.id, mapped);
            }
            Some(pb::frame::Item::Chat(c)) => {
                if let Some(mut mapped) = recipients
                    .get(&c.recipientId)
                    .and_then(|r| r.channel.clone())
                {
                    mapped.expire_timer = c
                        .expirationTimerMs
                        .and_then(|ms| u32::try_from(ms / 1000).ok());
                    chats.insert(c.id, mapped.id);
                    sink.channel(mapped)?;
                    summary.channels += 1;
                } else {
                    unsupported = Some("chat_without_stable_identity");
                }
            }
            Some(pb::frame::Item::ChatItem(item)) => {
                render_index += 1;
                if let (Some(id), Some(author)) = (
                    chats.get(&item.chatId),
                    recipients.get(&item.authorId).and_then(|r| r.person),
                ) {
                    let (message, refs, reason) = convert_item(item, author, &recipients)?;
                    if let Some(reason) = reason {
                        unsupported = Some(reason);
                    }
                    sink.message(ImportedMessage {
                        channel_id: *id,
                        message,
                        render_index,
                        media: refs,
                    })?;
                    summary.messages += 1;
                } else {
                    unsupported = Some("message_without_stable_identity");
                }
            }
            Some(pb::frame::Item::Account(_)) => (),
            _ => unsupported = Some("unsupported_frame"),
        }
        if let Some(reason) = unsupported {
            *summary.unsupported.entry(reason.into()).or_default() += 1;
            sink.unsupported(UnsupportedRecord {
                reason: reason.into(),
                frame_index: summary.frames,
                frame_bytes: raw,
            })?;
        }
        validator
            .add_frame(frame)
            .with_context(|| format!("invalid backup frame {}", summary.frames))?;
    }
    reader.verify_hmac().await?;
    let _: CompletedBackup<ValidateOnly> = validator.try_into()?;
    Ok(summary)
}

fn convert_item(
    item: &pb::ChatItem,
    author: Uuid,
    recipients: &HashMap<u64, Recipient>,
) -> Result<(Message, Vec<MediaReference>, Option<&'static str>)> {
    let mut message = Message::text(author, item.dateSent, String::new());
    message.message = None;
    message.expire_timer = item
        .expiresInMs
        .and_then(|ms| u32::try_from(ms / 1000).ok());
    message.expires_at = item
        .expireStartDate
        .zip(item.expiresInMs)
        .and_then(|(a, b)| a.checked_add(b));
    if let Some(pb::chat_item::DirectionalDetails::Outgoing(outgoing)) =
        item.directionalDetails.as_ref()
    {
        message.receipt = outgoing
            .sendStatus
            .iter()
            .map(|status| match status.deliveryStatus.as_ref() {
                Some(pb::send_status::DeliveryStatus::Read(_))
                | Some(pb::send_status::DeliveryStatus::Viewed(_)) => Receipt::Read,
                Some(pb::send_status::DeliveryStatus::Delivered(_)) => Receipt::Delivered,
                Some(pb::send_status::DeliveryStatus::Sent(_)) => Receipt::Sent,
                _ => Receipt::Nothing,
            })
            .min()
            .unwrap_or(Receipt::Nothing);
    }
    if let Some(pb::chat_item::DirectionalDetails::Incoming(incoming)) =
        item.directionalDetails.as_ref()
    {
        message.receipt = if incoming.read {
            Receipt::Read
        } else {
            Receipt::Delivered
        };
    }
    let mut refs = Vec::new();
    let mut reason = if item.sms || item.pinDetails.is_some() {
        Some("message_extra_metadata")
    } else {
        None
    };
    match item.item.as_ref() {
        Some(pb::chat_item::Item::StandardMessage(standard)) => {
            message.message = standard.text.as_ref().map(|text| text.body.clone());
            if let Some(text) = standard.text.as_ref() {
                message.body_ranges = text
                    .bodyRanges
                    .iter()
                    .filter_map(|range| {
                        let start = u16::try_from(range.start).ok()?;
                        let end = u16::try_from(range.start.checked_add(range.length)?).ok()?;
                        let value = match range.associatedValue.as_ref()? {
                            pb::body_range::AssociatedValue::MentionAci(bytes) => {
                                AssociatedValue::MentionUuid(uuid(bytes)?)
                            }
                            pb::body_range::AssociatedValue::Style(style) => {
                                AssociatedValue::Style(match style.value() {
                                    1 => Style::Bold,
                                    2 => Style::Italic,
                                    3 => Style::Spoiler,
                                    4 => Style::Strikethrough,
                                    5 => Style::Monospace,
                                    _ => return None,
                                })
                            }
                            _ => return None,
                        };
                        Some(BodyRange { start, end, value })
                    })
                    .collect();
            }

            for a in &standard.attachments {
                if let Some(p) = a.pointer.as_ref() {
                    refs.push(media(p, false)?);
                }
            }
            if let Some(p) = standard.longText.as_ref() {
                refs.push(media(p, true)?);
            }
            message.reactions = standard
                .reactions
                .iter()
                .filter_map(|r| {
                    recipients
                        .get(&r.authorId)
                        .and_then(|p| p.person)
                        .map(|id| (id, r.emoji.clone()))
                })
                .collect();
            if let Some(q) = standard.quote.as_ref()
                && let (Some(timestamp), Some(author)) = (
                    q.targetSentTimestamp,
                    recipients.get(&q.authorId).and_then(|r| r.person),
                )
            {
                message.quote = Some(Box::new(Message::text(
                    author,
                    timestamp,
                    q.text.as_ref().map(|t| t.body.clone()).unwrap_or_default(),
                )));
            }
            // Retain original wire data for fields whose Gurk representation differs.
            if !item.revisions.is_empty()
                || standard
                    .text
                    .as_ref()
                    .is_some_and(|t| !t.bodyRanges.is_empty())
                || !standard.linkPreview.is_empty()
                || !standard.attachments.is_empty()
                || standard.quote.is_some()
                || message.reactions.len() != standard.reactions.len()
            {
                reason = Some("standard_message_extra_fields");
            }
        }
        Some(pb::chat_item::Item::RemoteDeletedMessage(_)) => {
            message.deleted = true;
        }
        _ => {
            message.message = Some(
                "[Imported Signal message: unsupported type; original record preserved]".into(),
            );
            reason = Some("unsupported_message_type");
        }
    }
    if !item.revisions.is_empty() {
        message.edited = true;
        reason = Some("message_revisions_preserved");
    }
    Ok((message, refs, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{executor::block_on, io::Cursor};

    #[test]
    fn delimited_reader_rejects_large_frames_before_body_read() {
        // 64 MiB + 1, with no body: rejection must be the resource limit.
        let mut input = Cursor::new([0x81, 0x80, 0x80, 0x20]);
        let error = block_on(next_frame(&mut input)).unwrap_err();
        assert!(error.to_string().contains("64 MiB"));
    }

    #[test]
    fn delimited_reader_distinguishes_eof_and_truncation() {
        assert!(
            block_on(next_frame(&mut Cursor::new([])))
                .unwrap()
                .is_none()
        );
        assert!(block_on(next_frame(&mut Cursor::new([0x80]))).is_err());
        assert!(block_on(next_frame(&mut Cursor::new([2, 42]))).is_err());
        assert_eq!(
            block_on(next_frame(&mut Cursor::new([2, 42, 43]))).unwrap(),
            Some(vec![42, 43])
        );
    }

    #[derive(Default)]
    struct Collect {
        messages: Vec<ImportedMessage>,
        unsupported: Vec<UnsupportedRecord>,
    }
    impl ArchiveSink for Collect {
        fn metadata(&mut self, _: BackupMetadata) -> Result<()> {
            Ok(())
        }
        fn channel(&mut self, _: Channel) -> Result<()> {
            Ok(())
        }
        fn message(&mut self, m: ImportedMessage) -> Result<()> {
            self.messages.push(m);
            Ok(())
        }
        fn unsupported(&mut self, r: UnsupportedRecord) -> Result<()> {
            self.unsupported.push(r);
            Ok(())
        }
    }

    fn encrypted_fixture() -> (Vec<u8>, MessageBackupKey) {
        // Synthetic fixture following upstream's valid/account-data.jsonproto.
        let frames = serde_json::json!([
            {"version": "1", "backupTimeMs": "1715636551000", "mediaRootBackupKey": "q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s="},
            {"account": {"profileKey": "YQKRq+3DQklInaOaMcmlzZnN0m/1hzLiaONX7gB12dg=", "givenName": "Test", "accountSettings": {"phoneNumberSharingMode": "NOBODY", "defaultSentMediaQuality": "STANDARD", "appTheme": "SYSTEM", "callsUseLessDataSetting": "MOBILE_DATA_ONLY"}}},
            {"recipient": {"id": "91", "self": {}}},
            {"recipient": {"id": "42", "contact": {"aci": "EREREREREREREREREREREQ==", "systemGivenName": "Contact", "registered": {}}}},
            {"chat": {"id": "888", "recipientId": "42"}},
            {"chatItem": {"chatId": "888", "authorId": "42", "dateSent": "1715636550002", "incoming": {"dateReceived": "1715636550100", "read": true}, "standardMessage": {"text": {"body": "first received"}}}},
            {"chatItem": {"chatId": "888", "authorId": "42", "dateSent": "1715636550001", "incoming": {"dateReceived": "1715636550200", "read": false}, "standardMessage": {"text": {"body": "second received"}}}}
        ]);
        let plain =
            libsignal_message_backup::backup::convert_from_json(frames.as_array().unwrap().clone())
                .unwrap();
        let key = MessageBackupKey {
            hmac_key: [7; 32],
            aes_key: [9; 32],
        };
        let iv = [3; 16];
        let mut encrypted = libsignal_message_backup::export::gzip_compress(Cursor::new(plain));
        libsignal_message_backup::export::aes_cbc_encrypt(&key.aes_key, &iv, &mut encrypted);
        let mac = libsignal_message_backup::export::hmac_checksum(&key.hmac_key, &iv, &encrypted);
        let mut bytes = iv.to_vec();
        bytes.extend(encrypted);
        bytes.extend(mac);
        (bytes, key)
    }

    #[test]
    fn encrypted_archive_maps_stable_ids_and_received_order() {
        let (bytes, key) = encrypted_fixture();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.backup");
        std::fs::write(&path, &bytes).unwrap();
        let mut sink = Collect::default();
        let summary = block_on(read_archive(&path, &key, Uuid::nil(), &mut sink)).unwrap();
        assert_eq!(summary.messages, 2);
        assert_eq!(sink.messages[0].message.receipt, Receipt::Read);
        assert_eq!(sink.messages[1].message.receipt, Receipt::Delivered);
        assert_eq!(
            sink.messages[0].channel_id,
            ChannelId::User(Uuid::from_bytes([0x11; 16]))
        );
        assert_eq!(sink.messages[0].render_index, 1);
        assert_eq!(sink.messages[1].render_index, 2);
        assert!(sink.messages[0].message.arrived_at > sink.messages[1].message.arrived_at);
        let wrong = MessageBackupKey {
            hmac_key: [8; 32],
            aes_key: [9; 32],
        };
        assert!(
            block_on(read_archive(
                &path,
                &wrong,
                Uuid::nil(),
                &mut Collect::default()
            ))
            .is_err()
        );
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(
            block_on(read_archive(
                &path,
                &key,
                Uuid::nil(),
                &mut Collect::default()
            ))
            .is_err()
        );
    }
}
