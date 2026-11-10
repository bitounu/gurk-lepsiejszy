//! Implementation of [`crate::signal::SignalManager`] via `presage`

use std::{path::PathBuf, pin::Pin};

use anyhow::Context;
use async_trait::async_trait;
use presage::libsignal_service::prelude::ProfileKey;
use presage::libsignal_service::protocol::ServiceId;
use presage::libsignal_service::sender::AttachmentSpec;
use presage::manager::Registered;
use presage::model::contacts::Contact;
use presage::model::groups::Group;
use presage::proto::data_message::{Delete, Quote, Reaction};
use presage::proto::{AttachmentPointer, DataMessage, EditMessage, GroupContextV2, SyncMessage};
use presage::store::ContentsStore;
use presage::{
    libsignal_service::content::{Content, ContentBody},
    model::messages::Received,
};
use presage_store_sqlite::SqliteStore;
use sha2::Digest;
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::data::{Channel, ChannelId, GroupData, Message};
use crate::receipt::Receipt;
use crate::util::utc_now_timestamp_msec;

use super::{
    Attachment, GroupMasterKeyBytes, LocalPool, ProfileKeyBytes, ResolvedGroup, SignalManager,
    attachment,
};

pub(super) struct PresageManager {
    manager: presage::Manager<SqliteStore, Registered>,
    data_dir: PathBuf,
    local_pool: LocalPool,
    send_order: super::send_order::SendOrder,
}

impl PresageManager {
    pub(super) fn new(
        manager: presage::Manager<SqliteStore, Registered>,
        data_dir: PathBuf,
        local_pool: LocalPool,
    ) -> Self {
        Self {
            manager,
            data_dir,
            local_pool,
            send_order: Default::default(),
        }
    }
}

/// Keep both the destination and payload construction together: contact ACIs are
/// metadata in the encrypted self-sync, never transport destinations.
fn private_read_sync(own: Uuid, read: Vec<(Uuid, u64)>) -> (ServiceId, ContentBody) {
    (
        ServiceId::Aci(own.into()),
        ContentBody::SynchronizeMessage(SyncMessage {
            read: read
                .into_iter()
                .map(|(sender, timestamp)| presage::proto::sync_message::Read {
                    sender_aci: Some(sender.to_string()),
                    sender_aci_binary: Some(sender.as_bytes().to_vec()),
                    timestamp: Some(timestamp),
                })
                .collect(),
            ..Default::default()
        }),
    )
}

#[async_trait(?Send)]
impl SignalManager for PresageManager {
    fn clone_boxed(&self) -> Box<dyn SignalManager> {
        Box::new(Self {
            manager: self.manager.clone(),
            data_dir: self.data_dir.clone(),
            local_pool: self.local_pool.clone(),
            send_order: self.send_order.clone(),
        })
    }

    fn user_id(&self) -> Uuid {
        self.manager.registration_data().service_ids.aci
    }

    async fn resolve_group(
        &mut self,
        master_key_bytes: GroupMasterKeyBytes,
    ) -> anyhow::Result<ResolvedGroup> {
        let decrypted_group = self
            .manager
            .store()
            .group(master_key_bytes)
            .await?
            .context("no group found")?;

        let mut members = Vec::with_capacity(decrypted_group.members.len());
        let mut profile_keys = Vec::with_capacity(decrypted_group.members.len());
        for member in decrypted_group.members {
            members.push(member.aci.into());
            profile_keys.push(member.profile_key.bytes);
        }

        let name = decrypted_group.title;
        let group_data = GroupData {
            master_key_bytes,
            members,
            revision: decrypted_group.revision,
        };

        Ok(ResolvedGroup {
            name,
            group_data,
            profile_keys,
        })
    }

    fn save_attachment(
        &self,
        attachment_pointer: AttachmentPointer,
    ) -> oneshot::Receiver<anyhow::Result<Attachment>> {
        let (tx, rx) = oneshot::channel();
        let manager = self.manager.clone();
        let data_dir = self.data_dir.clone();
        self.local_pool.spawn(move || async move {
            let attachment = match manager.get_attachment(&attachment_pointer).await {
                Ok(data) => attachment::save(&data_dir, attachment_pointer, &data),
                Err(error) => Err(error.into()),
            };
            let _ = tx.send(attachment);
        });
        rx
    }

    fn send_receipt(&self, _sender_uuid: Uuid, _timestamps: Vec<u64>, _receipt: Receipt) {
        // Privacy boundary: never publish delivery/read activity to contacts.
        // Private linked-device state uses send_read_sync with our own ACI.
    }

    fn send_read_sync(&self, read: Vec<(Uuid, u64)>) -> oneshot::Receiver<anyhow::Result<()>> {
        let (tx, rx) = oneshot::channel();
        let (destination, body) = private_read_sync(self.user_id(), read);
        let mut manager = self.manager.clone();
        self.local_pool.spawn(move || async move {
            let result = manager
                .send_message(destination, body, utc_now_timestamp_msec())
                .await
                .map(|_| ())
                .map_err(Into::into);
            let _ = tx.send(result);
        });
        rx
    }

    fn send_text(
        &self,
        channel: &Channel,
        text: crate::mentions::MessageText,
        quote_message: Option<&Message>,
        edit_message_timestamp: Option<u64>,
        attachments: Vec<(AttachmentSpec, Vec<u8>)>,
    ) -> (Message, oneshot::Receiver<anyhow::Result<()>>) {
        let (message, body_ranges) = text.encode();
        let has_attachments = !attachments.is_empty();

        let timestamp = crate::util::next_send_timestamp_msec();

        let quote = quote_message.map(|message| Quote {
            id: Some(message.arrived_at),
            author_aci: None,
            text: message.message.clone(),
            body_ranges: message.body_ranges.iter().map(From::from).collect(),
            author_aci_binary: Some(message.from_id.as_bytes().to_vec()),
            ..Default::default()
        });
        let quote_message = quote.clone().and_then(Message::from_quote).map(Box::new);

        let mut data_message = DataMessage {
            body: Some(message.clone()),
            body_ranges: body_ranges.iter().map(From::from).collect(),
            required_protocol_version: (!body_ranges.is_empty())
                .then_some(presage::proto::data_message::ProtocolVersion::Mentions as u32),
            quote,
            expire_timer: channel.expire_timer,
            ..Default::default()
        };

        let mut saved_attachments: Vec<Attachment> = Vec::new();
        if has_attachments {
            for (spec, data) in &attachments {
                let attachment_pointer = AttachmentPointer {
                    content_type: Some(spec.content_type.clone()),
                    size: Some(spec.length as u32),
                    digest: Some(sha2::Sha256::digest(data).to_vec()),
                    file_name: spec.file_name.clone(),
                    width: spec.width,
                    height: spec.height,
                    upload_timestamp: Some(utc_now_timestamp_msec()),
                    ..Default::default()
                };
                match attachment::save(&self.data_dir, attachment_pointer, data) {
                    Ok(attachment) => {
                        saved_attachments.push(attachment);
                    }
                    Err(error) => {
                        error!(%error, "failed to save attachment");
                    }
                }
            }
        }

        let (response_tx, response) = oneshot::channel();
        let turn = self.send_order.reserve(channel.id);
        match channel.id {
            ChannelId::User(uuid) => {
                let mut manager = self.manager.clone();
                self.local_pool.spawn(move || async move {
                    let _turn = turn.wait().await;
                    if let Err(error) =
                        upload_attachments(&manager, attachments, &mut data_message, timestamp)
                            .await
                    {
                        error!(%error, "failed to upload attachments");
                        let _ = response_tx.send(Err(error));
                        return;
                    }

                    let uploaded_count = data_message.attachments.len();
                    if uploaded_count > 0 {
                        info!(
                            message_timestamp = timestamp,
                            attachment_count = uploaded_count,
                            "attachment_message_submit_start"
                        );
                    }
                    let body = if let Some(target_sent_timestamp) = edit_message_timestamp {
                        ContentBody::EditMessage(EditMessage {
                            target_sent_timestamp: Some(target_sent_timestamp),
                            data_message: Some(data_message),
                        })
                    } else {
                        ContentBody::DataMessage(data_message)
                    };

                    if let Err(error) = manager
                        .send_message(ServiceId::Aci(uuid.into()), body, timestamp)
                        .await
                    {
                        error!(dest =% uuid, %error, "failed to send message");
                        let _ = response_tx.send(Err(error.into()));
                        return;
                    }
                    if uploaded_count > 0 {
                        info!(
                            message_timestamp = timestamp,
                            attachment_count = uploaded_count,
                            "attachment_message_submit_complete"
                        );
                    }
                    let _ = response_tx.send(Ok(()));
                });
            }
            ChannelId::Group(_) => {
                if let Some(group_data) = channel.group_data.as_ref() {
                    let mut manager = self.manager.clone();

                    let master_key_bytes = group_data.master_key_bytes.to_vec();
                    data_message.group_v2 = Some(GroupContextV2 {
                        master_key: Some(master_key_bytes.clone()),
                        revision: Some(group_data.revision),
                        ..Default::default()
                    });

                    self.local_pool.spawn(move || async move {
                        let _turn = turn.wait().await;
                        if let Err(error) =
                            upload_attachments(&manager, attachments, &mut data_message, timestamp)
                                .await
                        {
                            error!(%error, "failed to upload attachments");
                            let _ = response_tx.send(Err(error));
                            return;
                        }

                        let uploaded_count = data_message.attachments.len();
                        if uploaded_count > 0 {
                            info!(
                                message_timestamp = timestamp,
                                attachment_count = uploaded_count,
                                "attachment_message_submit_start"
                            );
                        }
                        let body = if let Some(target_sent_timestamp) = edit_message_timestamp {
                            ContentBody::EditMessage(EditMessage {
                                target_sent_timestamp: Some(target_sent_timestamp),
                                data_message: Some(data_message),
                            })
                        } else {
                            ContentBody::DataMessage(data_message)
                        };

                        if let Err(error) = manager
                            .send_message_to_group(&master_key_bytes, body, timestamp)
                            .await
                        {
                            error!(%error, "failed to send group message");
                            let _ = response_tx.send(Err(error.into()));
                            return;
                        }
                        if uploaded_count > 0 {
                            info!(
                                message_timestamp = timestamp,
                                attachment_count = uploaded_count,
                                "attachment_message_submit_complete"
                            );
                        }
                        let _ = response_tx.send(Ok(()));
                    });
                } else {
                    error!("cannot send to broken channel without group data");
                }
            }
        }

        let message = Message {
            from_id: self.user_id(),
            message: Some(message),
            arrived_at: timestamp,
            local_id: None,
            is_edit_snapshot: false,
            quote: quote_message,
            attachments: saved_attachments,
            reactions: Default::default(),
            receipt: Receipt::Sent,
            body_ranges,
            send_failed: Default::default(),
            attachment_downloads: Default::default(),
            edit: edit_message_timestamp,
            edited: edit_message_timestamp.is_some(),
            deleted: Default::default(),
            expire_timer: channel.expire_timer,
            expires_at: channel
                .expire_timer
                .filter(|&t| t > 0)
                .map(|t| timestamp + u64::from(t) * 1000),
        };
        (message, response)
    }

    fn send_reaction(&self, channel: &Channel, message: &Message, emoji: String, remove: bool) {
        let timestamp = utc_now_timestamp_msec();
        let target_author_uuid = message.from_id;
        let target_sent_timestamp = message.arrived_at;

        let mut data_message = DataMessage {
            reaction: Some(Reaction {
                emoji: Some(emoji.clone()),
                remove: Some(remove),
                target_author_aci_binary: Some(target_author_uuid.as_bytes().to_vec()),
                target_author_aci: None,
                target_sent_timestamp: Some(target_sent_timestamp),
            }),
            ..Default::default()
        };

        match (channel.id, channel.group_data.as_ref()) {
            (ChannelId::User(uuid), _) => {
                let mut manager = self.manager.clone();
                let body = ContentBody::DataMessage(data_message);
                self.local_pool.spawn(move || async move {
                    if let Err(e) = manager
                        .send_message(ServiceId::Aci(uuid.into()), body, timestamp)
                        .await
                    {
                        // TODO: Proper error handling
                        error!("failed to send reaction {} to {}: {}", &emoji, uuid, e);
                    }
                });
            }
            (ChannelId::Group(_), Some(group_data)) => {
                let mut manager = self.manager.clone();

                let master_key_bytes = group_data.master_key_bytes.to_vec();
                data_message.group_v2 = Some(GroupContextV2 {
                    master_key: Some(master_key_bytes.clone()),
                    revision: Some(group_data.revision),
                    ..Default::default()
                });

                self.local_pool.spawn(move || async move {
                    if let Err(e) = manager
                        .send_message_to_group(&master_key_bytes, data_message, timestamp)
                        .await
                    {
                        // TODO: Proper error handling
                        error!("failed to send group reaction {}: {}", &emoji, e);
                    }
                });
            }
            _ => {
                error!("cannot send to broken channel without group data");
            }
        }
    }

    fn send_delete(&self, channel: &Channel, target_sent_timestamp: u64) {
        let timestamp = utc_now_timestamp_msec();

        let mut data_message = DataMessage {
            delete: Some(Delete {
                target_sent_timestamp: Some(target_sent_timestamp),
            }),
            ..Default::default()
        };

        match (channel.id, channel.group_data.as_ref()) {
            (ChannelId::User(uuid), _) => {
                let mut manager = self.manager.clone();
                let body = ContentBody::DataMessage(data_message);
                self.local_pool.spawn(move || async move {
                    if let Err(e) = manager
                        .send_message(ServiceId::Aci(uuid.into()), body, timestamp)
                        .await
                    {
                        error!(%e, "failed to send delete to {uuid}");
                    }
                });
            }
            (ChannelId::Group(_), Some(group_data)) => {
                let mut manager = self.manager.clone();

                let master_key_bytes = group_data.master_key_bytes.to_vec();
                data_message.group_v2 = Some(GroupContextV2 {
                    master_key: Some(master_key_bytes.clone()),
                    revision: Some(group_data.revision),
                    ..Default::default()
                });

                self.local_pool.spawn(move || async move {
                    if let Err(e) = manager
                        .send_message_to_group(&master_key_bytes, data_message, timestamp)
                        .await
                    {
                        error!(%e, "failed to send group delete");
                    }
                });
            }
            _ => {
                error!("cannot send to broken channel without group data");
            }
        }
    }

    fn send_delete_for_me(&self, channel: &Channel, message: &Message) {
        let timestamp = utc_now_timestamp_msec();
        let user_id = self.user_id();

        let conversation = Some(channel_to_conversation_id(channel));
        let addressable = presage::proto::AddressableMessage {
            sent_timestamp: Some(message.arrived_at),
            author: Some(
                presage::proto::addressable_message::Author::AuthorServiceIdBinary(
                    ServiceId::Aci(message.from_id.into()).service_id_binary(),
                ),
            ),
        };
        let delete_for_me = presage::proto::sync_message::DeleteForMe {
            message_deletes: vec![
                presage::proto::sync_message::delete_for_me::MessageDeletes {
                    conversation,
                    messages: vec![addressable],
                },
            ],
            ..Default::default()
        };
        let sync_message = ContentBody::SynchronizeMessage(SyncMessage {
            content: Some(presage::proto::sync_message::Content::DeleteForMe(
                delete_for_me,
            )),
            ..Default::default()
        });

        let mut manager = self.manager.clone();
        self.local_pool.spawn(move || async move {
            if let Err(e) = manager
                .send_message(ServiceId::Aci(user_id.into()), sync_message, timestamp)
                .await
            {
                error!(%e, "failed to send delete-for-me");
            }
        });
    }

    fn resolve_profile_name(
        &self,
        id: Uuid,
        profile_key: ProfileKeyBytes,
    ) -> oneshot::Receiver<Option<String>> {
        let (tx, rx) = oneshot::channel();
        let mut manager = self.manager.clone();
        self.local_pool.spawn(move || async move {
            let name = match manager
                .retrieve_profile_by_uuid(id, ProfileKey::create(profile_key))
                .await
            {
                Ok(profile) => profile.name.map(|name| name.given_name),
                Err(error) => {
                    error!(%error, "failed to retrieve user profile");
                    None
                }
            };
            let _ = tx.send(name);
        });
        rx
    }

    async fn profile_name(&self, id: Uuid) -> Option<String> {
        let service_id = ServiceId::Aci(id.into());
        let profile_key = self.manager.store().profile_key(&service_id).await.ok()??;
        let profile = self.manager.store().profile(id, profile_key).await.ok()??;
        let given_name = profile.name?.given_name;
        if !given_name.is_empty() {
            Some(given_name)
        } else {
            None
        }
    }

    async fn contact(&self, id: Uuid) -> Option<Contact> {
        let id = ServiceId::Aci(id.into());
        self.manager.store().contact_by_id(&id).await.ok()?
    }

    async fn receive_messages(
        &mut self,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = Box<Content>>>>> {
        Ok(Box::pin(self.manager.receive_messages().await?.filter_map(
            |received| match received {
                Received::Content(content) => Some(content),
                Received::QueueEmpty => None,
                Received::Contacts => {
                    // TODO: <https://github.com/boxdot/gurk-rs/issues/349>
                    warn!("Received contacts, but not implemented yet");
                    None
                }
                Received::DecryptionError(service_id) => {
                    error!(?service_id, "Received decryption error");
                    None
                }
            },
        )))
    }

    async fn contacts(&self) -> Box<dyn Iterator<Item = Contact>> {
        Box::new(
            self.manager
                .store()
                .contacts()
                .await
                .into_iter()
                .flatten()
                .flatten(),
        )
    }

    async fn groups(&self) -> Box<dyn Iterator<Item = (GroupMasterKeyBytes, Group)>> {
        Box::new(
            self.manager
                .store()
                .groups()
                .await
                .into_iter()
                .flatten()
                .flatten(),
        )
    }
}

fn channel_to_conversation_id(channel: &Channel) -> presage::proto::ConversationIdentifier {
    use presage::proto::conversation_identifier::Identifier;
    let identifier = match channel.id {
        ChannelId::User(uuid) => {
            Identifier::ThreadServiceIdBinary(ServiceId::Aci(uuid.into()).service_id_binary())
        }
        ChannelId::Group(group_id) => Identifier::ThreadGroupId(group_id.to_vec()),
    };
    presage::proto::ConversationIdentifier {
        identifier: Some(identifier),
    }
}

async fn upload_attachments(
    manager: &presage::Manager<SqliteStore, Registered>,
    attachments: Vec<(AttachmentSpec, Vec<u8>)>,
    data_message: &mut DataMessage,
    message_timestamp: u64,
) -> anyhow::Result<()> {
    let expected_count = attachments.len();
    if expected_count == 0 {
        return Ok(());
    }
    info!(
        message_timestamp,
        expected_count,
        total_bytes = attachments
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>(),
        "attachment_batch_upload_start"
    );
    let attachment_pointers = manager.upload_attachments(attachments).await?;
    let actual_count = attachment_pointers.len();
    for (index, result) in attachment_pointers.iter().enumerate() {
        info!(
            message_timestamp,
            attachment_index = index + 1,
            expected_count,
            success = result.is_ok(),
            "attachment_upload_result"
        );
    }
    apply_uploaded_attachments(data_message, attachment_pointers, expected_count)?;
    info!(
        message_timestamp,
        expected_count, actual_count, "attachment_batch_upload_complete"
    );
    Ok(())
}

fn apply_uploaded_attachments(
    data_message: &mut DataMessage,
    attachment_pointers: Vec<
        Result<AttachmentPointer, presage::libsignal_service::sender::AttachmentUploadError>,
    >,
    expected_count: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        attachment_pointers.len() == expected_count,
        "Incomplete attachment upload: expected {} files, received {} results; message was not sent",
        expected_count,
        attachment_pointers.len()
    );
    // Presage reports an outer request result and one result per uploaded file.
    // Never send a reduced payload if any file failed (including image-only sends).
    let pointers = attachment_pointers
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.with_context(|| format!("Attachment {} upload failed", index + 1))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    data_message.attachments = pointers;
    Ok(())
}

#[cfg(test)]
mod attachment_upload_tests {
    use super::*;
    use presage::libsignal_service::sender::AttachmentUploadError;

    fn upload_failure() -> Result<AttachmentPointer, AttachmentUploadError> {
        Err(std::io::Error::other("fixture upload refused").into())
    }

    #[test]
    fn short_successful_upload_batch_cannot_drop_three_of_six_files() {
        let original = AttachmentPointer {
            file_name: Some("existing.txt".into()),
            ..Default::default()
        };
        let mut message = DataMessage {
            attachments: vec![original.clone()],
            ..Default::default()
        };
        let result = apply_uploaded_attachments(
            &mut message,
            (0..3).map(|_| Ok(AttachmentPointer::default())).collect(),
            6,
        );
        assert!(
            result.is_err(),
            "three returned pointers must not count as six successful uploads"
        );
        assert_eq!(message.attachments, vec![original]);
    }

    #[test]
    fn six_successful_uploads_keep_every_filename_and_order() {
        let pointers: Vec<_> = (0..6)
            .map(|index| AttachmentPointer {
                file_name: Some(format!("file{index}.jpg")),
                ..Default::default()
            })
            .collect();
        let mut message = DataMessage::default();
        apply_uploaded_attachments(&mut message, pointers.iter().cloned().map(Ok).collect(), 6)
            .unwrap();
        assert_eq!(message.attachments, pointers);
    }

    #[test]
    fn failed_image_upload_aborts_instead_of_sending_empty_message() {
        let mut message = DataMessage {
            body: Some(String::new()),
            ..Default::default()
        };
        let result = apply_uploaded_attachments(&mut message, vec![upload_failure()], 1);
        assert!(
            result.is_err(),
            "must abort sending when the only image failed to upload"
        );
        let reason = format!("{:#}", result.unwrap_err());
        assert!(reason.contains("Attachment 1 upload failed"));
        assert!(reason.contains("fixture upload refused"));
    }

    #[test]
    fn partial_upload_failure_preserves_payload_and_aborts_caption_send() {
        let existing = AttachmentPointer {
            file_name: Some("existing.txt".into()),
            ..Default::default()
        };
        let mut message = DataMessage {
            body: Some("caption".into()),
            attachments: vec![existing.clone()],
            ..Default::default()
        };
        let result = apply_uploaded_attachments(
            &mut message,
            vec![Ok(AttachmentPointer::default()), upload_failure()],
            2,
        );
        assert!(
            result.is_err(),
            "must not send a caption with a missing attachment"
        );
        assert_eq!(message.attachments, vec![existing]);
        assert_eq!(message.body.as_deref(), Some("caption"));
    }

    #[test]
    fn successful_upload_keeps_complete_server_pointers() {
        use presage::proto::attachment_pointer::AttachmentIdentifier;
        let pointer = AttachmentPointer {
            content_type: Some("image/png".into()),
            key: Some(vec![42; 64]),
            digest: Some(vec![9; 32]),
            cdn_number: Some(3),
            attachment_identifier: Some(AttachmentIdentifier::CdnKey("fixture-cdn-key".into())),
            size: Some(10),
            ..Default::default()
        };
        let mut message = DataMessage::default();
        apply_uploaded_attachments(&mut message, vec![Ok(pointer.clone())], 1).unwrap();
        assert_eq!(message.attachments, vec![pointer]);
        apply_uploaded_attachments(&mut message, vec![], 0).unwrap();
        assert!(message.attachments.is_empty());
    }
}

#[cfg(test)]
mod private_read_sync_tests {
    use super::*;

    #[test]
    fn read_sync_is_addressed_to_self_and_contains_only_reads() {
        let own = Uuid::new_v4();
        let sender = Uuid::new_v4();
        let (destination, body) = private_read_sync(own, vec![(sender, 42)]);
        assert_eq!(destination, ServiceId::Aci(own.into()));
        let ContentBody::SynchronizeMessage(sync) = body else {
            panic!("expected sync")
        };
        assert_eq!(sync.read.len(), 1);
        assert_eq!(
            sync.read[0].sender_aci.as_deref(),
            Some(sender.to_string().as_str())
        );
        assert_eq!(
            sync.read[0].sender_aci_binary.as_deref(),
            Some(sender.as_bytes().as_slice())
        );
        assert_eq!(sync.read[0].timestamp, Some(42));
        assert!(sync.viewed.is_empty());
        assert!(sync.sticker_pack_operation.is_empty());
        assert!(sync.content.is_none());
    }
}
